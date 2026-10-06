//! Connects the capture proxy to the app: saves each captured call in the
//! app's own captures folder, keeps the open session's capture markers up to
//! date, and serves the capture views to the commands.
//!
//! Lock rule: the open session's lock (`Shared::active`) is never held while
//! the store is read or written, records are summarised, or a capture view
//! is built. Records are loaded first and only then applied under the lock;
//! views copy what they need from the trace under the lock and are built
//! after it is released. Sequences that change the store and then apply the
//! result to the open session also hold `RecordCache::lock_updates`, so they
//! reach the session in order. The order is: `lock_updates`, then the record
//! cache, then the store's write lock; and `lock_updates`, then `active`.
//! The cache and store locks are never taken while `active` is held.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use adapter_claude_code::{SESSION_HEADER, session_key};
use capture::{CaptureSink, CaptureStore, PROXY_PORT, StoreError, UNKNOWN_SESSION};
use capture_core::{CaptureRecord, RequestSummary};
use serde::Serialize;

use crate::captures::{self, CaptureDetail, CaptureOverview, SessionRecords, TraceFacts};
use crate::live::LiveSession;
use crate::sessions::{SessionError, session_path};
use crate::watch::Shared;

/// The longest capture id a command accepts. Record ids are short; this
/// only stops absurd input.
const MAX_CAPTURE_ID_LEN: usize = 256;

/// Locks `mutex`, recovering the data if a thread panicked while holding it.
/// Every value guarded here stays valid after a panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The decoded records of the last session read from the store, with the
/// summaries built from them so far and the reasons for stored data that
/// could not be read. A new record for that session is added to them;
/// deleting the session's captures clears them.
#[derive(Default)]
pub struct RecordCache {
    /// The session key and its records.
    last: Mutex<Option<(String, SessionRecords)>>,
    /// Held while a change to the store is applied to the open session.
    updates: Mutex<()>,
    /// How many times the store was read, so tests can see cache hits.
    #[cfg(test)]
    loads: std::sync::atomic::AtomicUsize,
}

impl RecordCache {
    /// The cached records of session `key`, read from the store unless they
    /// are cached. The cache lock is held while reading, so an `invalidate`
    /// that follows a write always clears what was read before the write.
    fn get_or_load_all(
        &self,
        store: &CaptureStore,
        key: &str,
    ) -> Result<SessionRecords, StoreError> {
        let mut last = lock(&self.last);
        if let Some((cached_key, cached)) = last.as_ref() {
            if cached_key == key {
                return Ok(cached.clone());
            }
        }
        #[cfg(test)]
        self.loads.fetch_add(1, Ordering::SeqCst);
        let loaded = store.load(key)?;
        for reason in &loaded.skipped {
            tracing::debug!(reason, "skipped stored capture data");
        }
        let cached = SessionRecords::new(loaded.records, loaded.skipped);
        *last = Some((key.to_string(), cached.clone()));
        Ok(cached)
    }

    /// The records of session `key`, read from the store unless they are
    /// cached. Builds no summaries.
    pub fn get_or_load(
        &self,
        store: &CaptureStore,
        key: &str,
    ) -> Result<Arc<Vec<CaptureRecord>>, StoreError> {
        Ok(self.get_or_load_all(store, key)?.records)
    }

    /// The records of session `key` with every record summarised. Only
    /// records not summarised before are summarised, outside the cache lock,
    /// and their summaries are kept for next time.
    pub fn get_summarised(
        &self,
        store: &CaptureStore,
        key: &str,
    ) -> Result<SessionRecords, StoreError> {
        let mut captures = self.get_or_load_all(store, key)?;
        let known = captures.summaries.len();
        captures.summarise_missing();
        if captures.summaries.len() != known {
            let mut last = lock(&self.last);
            if let Some((cached_key, cached)) = last.as_mut() {
                if cached_key == key {
                    let ids: HashSet<&str> = cached.records.iter().map(|r| r.id.as_str()).collect();
                    let new: Vec<(String, Option<RequestSummary>)> = captures
                        .summaries
                        .iter()
                        .filter(|(id, _)| {
                            ids.contains(id.as_str()) && !cached.summaries.contains_key(*id)
                        })
                        .map(|(id, summary)| (id.clone(), summary.clone()))
                        .collect();
                    if !new.is_empty() {
                        Arc::make_mut(&mut cached.summaries).extend(new);
                    }
                }
            }
        }
        Ok(captures)
    }

    /// Adds a record that was just saved for session `key` to the cached
    /// records, if they are this session's, so the next read needs no
    /// decoding. A record already there (read from the store after it was
    /// saved) is not added twice. The record is summarised only when a view
    /// needs it.
    pub fn add_record(&self, key: &str, record: &CaptureRecord) {
        let mut last = lock(&self.last);
        let Some((cached_key, cached)) = last.as_mut() else {
            return;
        };
        if cached_key != key || cached.records.iter().any(|r| r.id == record.id) {
            return;
        }
        cached.push(record.clone());
    }

    /// Forgets the cached records if they belong to session `key`.
    pub fn invalidate(&self, key: &str) {
        let mut last = lock(&self.last);
        if last.as_ref().is_some_and(|(cached, _)| cached == key) {
            *last = None;
        }
    }

    /// Takes the lock that keeps store changes and their effect on the open
    /// session in order. Taken before `Shared::active`, never after it.
    pub fn lock_updates(&self) -> MutexGuard<'_, ()> {
        lock(&self.updates)
    }
}

/// The capture proxy's state, for the status line in the UI.
#[derive(Default)]
pub struct ProxyState {
    /// True once the proxy is listening.
    pub listening: AtomicBool,
    /// Why the proxy could not start, if it could not.
    pub error: Mutex<Option<String>>,
    /// The last error while saving a capture, if any.
    pub last_save_error: Mutex<Option<String>>,
}

impl ProxyState {
    /// Records why the proxy could not start.
    pub fn set_error(&self, message: String) {
        *lock(&self.error) = Some(message);
    }
}

/// The PowerShell command that starts Claude Code with capture on.
pub const CAPTURE_COMMAND: &str = "$env:ANTHROPIC_BASE_URL='http://127.0.0.1:47821'; claude";

/// What `capture_status` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CaptureStatus {
    /// True if the proxy is listening.
    pub listening: bool,
    /// The port the proxy listens on (or tried to).
    pub port: u16,
    /// The command to copy into PowerShell to capture a session.
    pub command: String,
    /// Why the proxy could not start, if it could not.
    pub error: Option<String>,
    /// The last error while saving a capture, if any.
    pub last_save_error: Option<String>,
}

/// The current capture status.
pub fn capture_status(state: &ProxyState) -> CaptureStatus {
    CaptureStatus {
        listening: state.listening.load(Ordering::SeqCst),
        port: PROXY_PORT,
        command: CAPTURE_COMMAND.to_string(),
        error: lock(&state.error).clone(),
        last_save_error: lock(&state.last_save_error).clone(),
    }
}

/// Receives the proxy's finished calls.
pub struct AppSink {
    store: Arc<CaptureStore>,
    shared: Arc<Shared>,
    cache: Arc<RecordCache>,
    state: Arc<ProxyState>,
}

impl AppSink {
    /// A sink that saves into `store` and updates the open session in `shared`.
    pub fn new(
        store: Arc<CaptureStore>,
        shared: Arc<Shared>,
        cache: Arc<RecordCache>,
        state: Arc<ProxyState>,
    ) -> Self {
        Self {
            store,
            shared,
            cache,
            state,
        }
    }
}

impl CaptureSink for AppSink {
    fn record(&self, record: CaptureRecord) {
        let key = session_key(record.header(SESSION_HEADER))
            .unwrap_or_else(|| UNKNOWN_SESSION.to_string());
        let _updates = self.cache.lock_updates();
        if let Err(err) = self.store.append(&key, &record) {
            tracing::warn!(error = %err, "could not save a capture");
            *lock(&self.state.last_save_error) = Some(err.to_string());
            return;
        }
        *lock(&self.state.last_save_error) = None;
        self.cache.add_record(&key, &record);
        let is_open =
            self.shared.active().as_ref().is_some_and(|active| {
                active.session.capture_key().as_deref() == Some(key.as_str())
            });
        if is_open {
            update_open_session(&self.store, &self.cache, &self.shared, &key);
        }
    }
}

/// Gets the records of session `key` through the cache (outside the lock)
/// and, if it is still the open session, applies them and sends an update
/// when a model call gained or lost a capture. The caller
/// holds `RecordCache::lock_updates`.
fn update_open_session(store: &CaptureStore, cache: &RecordCache, shared: &Shared, key: &str) {
    let records = match cache.get_or_load(store, key) {
        Ok(records) => records,
        Err(err) => {
            tracing::warn!(error = %err, "could not read the open session's captures");
            return;
        }
    };
    let mut guard = shared.active();
    let Some(active) = guard
        .as_mut()
        .filter(|a| a.session.capture_key().as_deref() == Some(key))
    else {
        return;
    };
    if let Some(message) = active.session.refresh_captures(records) {
        if !active.sink.send(message) {
            tracing::debug!("could not send a capture update; the UI may have reloaded");
        }
    }
}

/// Loads the captures of `session` through the cache and marks them in it.
/// Used by `load_session` before the first view. A store error is logged and
/// leaves the session without markers.
pub fn attach_captures(store: &CaptureStore, cache: &RecordCache, session: &mut LiveSession) {
    let Some(key) = session.capture_key() else {
        return;
    };
    match cache.get_or_load(store, &key) {
        Ok(records) => session.set_captures(records),
        Err(err) => tracing::warn!(error = %err, "could not read the session's captures"),
    }
}

/// Checks a capture id from the UI: not empty and not absurdly long.
fn check_capture_id(capture_id: &str) -> Result<(), SessionError> {
    if capture_id.is_empty() || capture_id.len() > MAX_CAPTURE_ID_LEN {
        return Err(SessionError::InvalidCaptureId);
    }
    Ok(())
}

/// The facts the capture views need from the trace of the session, using
/// the open session when it is this one and otherwise reading the session
/// without keeping it. The open session's lock is held only while the facts
/// are copied. The names must already be validated.
fn trace_facts(
    root: &Path,
    shared: &Shared,
    project: &str,
    session_id: &str,
) -> Result<TraceFacts, SessionError> {
    {
        let guard = shared.active();
        if let Some(active) = guard.as_ref().filter(|a| a.session.is(project, session_id)) {
            return Ok(TraceFacts::of(active.session.trace()));
        }
    }
    let live = LiveSession::open(root, project, session_id)?;
    Ok(TraceFacts::of(live.trace()))
}

/// The overview of one session's captures, from the record cache (which
/// also keeps the reasons for stored data that could not be read).
pub fn session_overview(
    root: &Path,
    store: &CaptureStore,
    cache: &RecordCache,
    shared: &Shared,
    project: &str,
    session_id: &str,
) -> Result<CaptureOverview, SessionError> {
    session_path(root, project, session_id)?;
    let key = session_key(Some(session_id));
    let (records, bytes) = match key.as_deref() {
        Some(key) => (cache.get_summarised(store, key)?, store.size(key)?),
        None => (SessionRecords::default(), 0),
    };
    let facts = trace_facts(root, shared, project, session_id)?;
    Ok(captures::overview(&facts, &records, key.as_deref(), bytes))
}

/// One captured call of a session in full, or `None` if there is none with
/// that id.
pub fn session_capture_detail(
    root: &Path,
    store: &CaptureStore,
    cache: &RecordCache,
    shared: &Shared,
    project: &str,
    session_id: &str,
    capture_id: &str,
) -> Result<Option<CaptureDetail>, SessionError> {
    session_path(root, project, session_id)?;
    check_capture_id(capture_id)?;
    let Some(key) = session_key(Some(session_id)) else {
        return Ok(None);
    };
    let records = cache.get_summarised(store, &key)?;
    let facts = trace_facts(root, shared, project, session_id)?;
    Ok(captures::detail(&facts, &records, capture_id))
}

/// Deletes every capture of a session. If it is the open session, its
/// markers are cleared and an update is sent.
pub fn delete_session_captures(
    root: &Path,
    store: &CaptureStore,
    cache: &RecordCache,
    shared: &Shared,
    project: &str,
    session_id: &str,
) -> Result<(), SessionError> {
    session_path(root, project, session_id)?;
    let Some(key) = session_key(Some(session_id)) else {
        return Ok(());
    };
    let _updates = cache.lock_updates();
    store.delete(&key)?;
    cache.invalidate(&key);
    let mut guard = shared.active();
    if let Some(active) = guard.as_mut().filter(|a| a.session.is(project, session_id)) {
        if let Some(message) = active.session.refresh_captures(Arc::new(Vec::new())) {
            if !active.sink.send(message) {
                tracing::debug!("could not send a capture update; the UI may have reloaded");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::captures::fixture_records;
    use crate::live::LiveMessage;
    use crate::watch::Active;
    use std::path::PathBuf;
    use std::sync::mpsc;

    const SESSION: &str = "00000000-0000-4000-8000-000000000002";
    const OTHER: &str = "00000000-0000-4000-8000-000000000099";

    /// A temp folder holding a projects root with the sanitised `basic`
    /// fixture, and an empty captures folder. Returns (projects root, store).
    fn setup(name: &str) -> (PathBuf, Arc<CaptureStore>) {
        let base =
            std::env::temp_dir().join(format!("snitchcraft-sink-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("projects");
        std::fs::create_dir_all(root.join("basic")).expect("mkdir");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("fixtures")
            .join("claude-code")
            .join("basic")
            .join(format!("{SESSION}.jsonl"));
        std::fs::copy(fixture, root.join("basic").join(format!("{SESSION}.jsonl"))).expect("copy");
        let store = Arc::new(CaptureStore::new(base.join("captures")));
        (root, store)
    }

    /// Shared state with the fixture session open, and the receiving end of
    /// its sink.
    fn open_session(root: &Path) -> (Arc<Shared>, mpsc::Receiver<LiveMessage>) {
        let shared = Arc::new(Shared::default());
        let (tx, rx) = mpsc::channel::<LiveMessage>();
        *shared.active() = Some(Active {
            session: LiveSession::open(root, "basic", SESSION).expect("open"),
            sink: Box::new(tx),
        });
        (shared, rx)
    }

    fn sink(store: &Arc<CaptureStore>, shared: &Arc<Shared>) -> (AppSink, Arc<ProxyState>) {
        let state = Arc::new(ProxyState::default());
        let sink = AppSink::new(
            Arc::clone(store),
            Arc::clone(shared),
            Arc::new(RecordCache::default()),
            Arc::clone(&state),
        );
        (sink, state)
    }

    /// The first fixture capture that maps to a model call, and that call's id.
    fn mapped_capture() -> (CaptureRecord, String) {
        let record = fixture_records()
            .into_iter()
            .find(|r| {
                r.message_id
                    .as_deref()
                    .is_some_and(|m| m != "msg_title_test")
            })
            .expect("a mapped capture");
        let id = adapter_claude_code::model_call_id(record.message_id.as_deref().expect("id"));
        (record, id)
    }

    /// `record` with its session header set to `value`, or removed.
    fn with_session(mut record: CaptureRecord, value: Option<&str>) -> CaptureRecord {
        record
            .request
            .headers
            .retain(|h| !h.name.eq_ignore_ascii_case(SESSION_HEADER));
        if let Some(value) = value {
            let mut header = fixture_records()[0].request.headers[0].clone();
            header.name = SESSION_HEADER.to_string();
            header.value = value.to_string();
            record.request.headers.push(header);
        }
        record
    }

    #[test]
    fn sink_saves_and_updates_the_open_session() {
        let (root, store) = setup("saves");
        let (shared, rx) = open_session(&root);
        let (sink, state) = sink(&store, &shared);
        let (record, call) = mapped_capture();
        sink.record(with_session(record, Some(SESSION)));

        assert_eq!(store.load(SESSION).expect("load").records.len(), 1);
        let Ok(LiveMessage::Updated {
            view,
            changed_trace_ids,
            ..
        }) = rx.try_recv()
        else {
            panic!("expected an update");
        };
        assert!(changed_trace_ids.contains(&call));
        assert_eq!(view.captured_trace_ids, vec![call]);
        assert!(lock(&state.last_save_error).is_none());
    }

    #[test]
    fn sink_for_another_session_only_saves() {
        let (root, store) = setup("other");
        let (shared, rx) = open_session(&root);
        let (sink, _) = sink(&store, &shared);
        let (record, _) = mapped_capture();
        sink.record(with_session(record, Some(OTHER)));
        assert_eq!(store.load(OTHER).expect("load").records.len(), 1);
        assert!(store.load(SESSION).expect("load").records.is_empty());
        assert!(rx.try_recv().is_err(), "no message sent");
    }

    #[test]
    fn unknown_session_goes_to_the_unknown_folder() {
        let (root, store) = setup("unknown");
        let (shared, rx) = open_session(&root);
        let (sink, _) = sink(&store, &shared);
        let (record, _) = mapped_capture();
        sink.record(with_session(record.clone(), None));
        sink.record(with_session(record, Some("../not-plain")));
        assert_eq!(store.load(UNKNOWN_SESSION).expect("load").records.len(), 2);
        assert!(rx.try_recv().is_err(), "no message sent");
    }

    #[test]
    fn a_failed_save_is_reported_in_the_status() {
        let (root, _) = setup("save-error");
        // A store whose root is a file cannot create session folders.
        let blocker = root.join("blocker");
        std::fs::write(&blocker, b"x").expect("write");
        let store = Arc::new(CaptureStore::new(blocker));
        let (shared, rx) = open_session(&root);
        let (sink, state) = sink(&store, &shared);
        let (record, _) = mapped_capture();
        sink.record(with_session(record, Some(SESSION)));
        assert!(capture_status(&state).last_save_error.is_some());
        assert!(rx.try_recv().is_err(), "no message sent");
    }

    #[test]
    fn a_successful_save_clears_the_last_save_error() {
        let (root, good_store) = setup("save-error-cleared");
        let blocker = root.join("blocker");
        std::fs::write(&blocker, b"x").expect("write");
        let bad_store = Arc::new(CaptureStore::new(blocker));
        let (shared, _rx) = open_session(&root);
        let state = Arc::new(ProxyState::default());
        let cache = Arc::new(RecordCache::default());
        let failing = AppSink::new(
            bad_store,
            Arc::clone(&shared),
            Arc::clone(&cache),
            Arc::clone(&state),
        );
        let working = AppSink::new(good_store, shared, cache, Arc::clone(&state));
        let (record, _) = mapped_capture();
        failing.record(with_session(record.clone(), Some(SESSION)));
        assert!(capture_status(&state).last_save_error.is_some());
        working.record(with_session(record, Some(SESSION)));
        assert_eq!(capture_status(&state).last_save_error, None);
    }

    #[test]
    fn new_records_are_added_to_the_cache_without_a_reload() {
        let (root, store) = setup("cache-grows");
        let (shared, rx) = open_session(&root);
        let cache = Arc::new(RecordCache::default());
        let state = Arc::new(ProxyState::default());
        let sink = AppSink::new(
            Arc::clone(&store),
            Arc::clone(&shared),
            Arc::clone(&cache),
            state,
        );
        let records: Vec<CaptureRecord> = fixture_records()
            .into_iter()
            .filter(|r| {
                r.message_id
                    .as_deref()
                    .is_some_and(|m| m != "msg_title_test")
            })
            .take(2)
            .collect();
        assert_eq!(records.len(), 2);
        for record in &records {
            sink.record(with_session(record.clone(), Some(SESSION)));
        }
        assert_eq!(
            cache.loads.load(Ordering::SeqCst),
            1,
            "only the first record read the store"
        );
        let cached = cache.get_or_load(&store, SESSION).expect("cached");
        assert_eq!(
            cache.loads.load(Ordering::SeqCst),
            1,
            "served from the cache"
        );
        assert_eq!(*cached, store.load(SESSION).expect("load").records);
        assert_eq!(cached.len(), 2);
        assert_eq!(rx.try_iter().count(), 2, "one update per newly marked call");

        // A record already read from the store is not added twice.
        cache.add_record(SESSION, &cached[1]);
        assert_eq!(cache.get_or_load(&store, SESSION).expect("cached").len(), 2);
    }

    #[test]
    fn a_capture_that_marks_no_new_call_sends_nothing() {
        let (root, store) = setup("no-change");
        let (shared, rx) = open_session(&root);
        let (sink, _) = sink(&store, &shared);
        let title = fixture_records()
            .into_iter()
            .find(|r| r.message_id.as_deref() == Some("msg_title_test"))
            .expect("title call");
        sink.record(with_session(title, Some(SESSION)));
        let (record, _) = mapped_capture();
        sink.record(with_session(record.clone(), Some(SESSION)));
        let mut repeat = record;
        repeat.id = "repeat-001".into();
        sink.record(with_session(repeat, Some(SESSION)));
        assert_eq!(store.load(SESSION).expect("load").records.len(), 3);
        assert_eq!(rx.try_iter().count(), 1, "only the first mapped capture");
    }

    #[test]
    fn delete_clears_the_open_sessions_markers() {
        let (root, store) = setup("delete");
        let (shared, rx) = open_session(&root);
        let cache = RecordCache::default();
        for record in fixture_records() {
            store.append(SESSION, &record).expect("append");
        }
        {
            let mut guard = shared.active();
            let active = guard.as_mut().expect("open");
            attach_captures(&store, &cache, &mut active.session);
            assert!(!active.session.view().captured_trace_ids.is_empty());
        }

        delete_session_captures(&root, &store, &cache, &shared, "basic", SESSION).expect("delete");
        assert!(store.load(SESSION).expect("load").records.is_empty());
        let Ok(LiveMessage::Updated {
            view,
            changed_trace_ids,
            ..
        }) = rx.try_recv()
        else {
            panic!("expected an update");
        };
        assert!(view.captured_trace_ids.is_empty());
        assert!(!changed_trace_ids.is_empty());
        assert!(
            cache.get_or_load(&store, SESSION).expect("load").is_empty(),
            "the cache was cleared"
        );
    }

    #[test]
    fn overview_and_detail_read_the_store() {
        let (root, store) = setup("views");
        let shared = Arc::new(Shared::default());
        let cache = RecordCache::default();
        let records = fixture_records();
        for record in &records {
            store.append(SESSION, record).expect("append");
        }
        let view =
            session_overview(&root, &store, &cache, &shared, "basic", SESSION).expect("overview");
        assert_eq!(view.calls.len(), records.len());
        assert!(view.total_bytes > 0);

        let (_, call) = mapped_capture();
        let id = view
            .calls
            .iter()
            .find(|c| c.trace_id.as_deref() == Some(call.as_str()))
            .map(|c| c.capture_id.clone())
            .expect("call in overview");
        let detail = session_capture_detail(&root, &store, &cache, &shared, "basic", SESSION, &id)
            .expect("detail")
            .expect("found");
        assert_eq!(detail.record.id, id);
        assert!(
            session_capture_detail(&root, &store, &cache, &shared, "basic", SESSION, "nope")
                .expect("detail")
                .is_none()
        );
        assert!(matches!(
            session_capture_detail(&root, &store, &cache, &shared, "basic", SESSION, ""),
            Err(SessionError::InvalidCaptureId)
        ));
        assert!(matches!(
            session_overview(&root, &store, &cache, &shared, "..", SESSION),
            Err(SessionError::InvalidName { .. })
        ));
    }

    #[test]
    fn views_summarise_each_record_once_and_keep_skipped_reasons() {
        let (root, store) = setup("views-once");
        let shared = Arc::new(Shared::default());
        let cache = RecordCache::default();
        let records = crate::captures::synthetic_records(40);
        for record in &records {
            store.append(SESSION, record).expect("append");
        }
        // A damaged tail, so the store reports a skipped reason.
        let calls = store.root().join(SESSION).join("calls.jsonl.gz");
        let mut bytes = std::fs::read(&calls).expect("read");
        bytes.extend_from_slice(&[0x1f, 0x8b, 0x08, 0x00]);
        std::fs::write(&calls, bytes).expect("write");

        let before = crate::captures::summarise_calls();
        let first =
            session_overview(&root, &store, &cache, &shared, "basic", SESSION).expect("overview");
        assert_eq!(first.calls.len(), 40);
        assert_eq!(first.skipped.len(), 1, "{:?}", first.skipped);
        assert_eq!(crate::captures::summarise_calls() - before, 40);

        let again =
            session_overview(&root, &store, &cache, &shared, "basic", SESSION).expect("overview");
        let id = &records[20].id;
        let detail = session_capture_detail(&root, &store, &cache, &shared, "basic", SESSION, id)
            .expect("detail")
            .expect("found");
        assert_eq!(again, first);
        assert_eq!(detail.record.id, *id);
        assert_eq!(
            crate::captures::summarise_calls() - before,
            40,
            "nothing summarised twice"
        );
        assert_eq!(
            cache.loads.load(Ordering::SeqCst),
            1,
            "the store was read once"
        );

        let mut extra = records[0].clone();
        extra.id = "extra-0001".into();
        extra.started_at_ms += 10_000_000;
        store.append(SESSION, &extra).expect("append");
        cache.add_record(SESSION, &extra);
        let grown =
            session_overview(&root, &store, &cache, &shared, "basic", SESSION).expect("overview");
        assert_eq!(grown.calls.len(), 41);
        assert_eq!(
            crate::captures::summarise_calls() - before,
            41,
            "only the new one"
        );
    }

    #[test]
    fn capture_status_has_the_copyable_command() {
        let state = ProxyState::default();
        let status = capture_status(&state);
        assert_eq!(
            status.command,
            "$env:ANTHROPIC_BASE_URL='http://127.0.0.1:47821'; claude"
        );
        assert_eq!(status.port, 47821);
        assert!(!status.listening);
        assert_eq!(status.error, None);

        state.listening.store(true, Ordering::SeqCst);
        *lock(&state.error) = Some("port in use".into());
        let status = capture_status(&state);
        assert!(status.listening);
        assert_eq!(status.error.as_deref(), Some("port in use"));
    }
}
