//! The live watcher. One background thread owns a recursive `notify`
//! watcher on the projects folder. It refreshes the open session when its
//! files change (and polls it once a second as a safety net), and tells the
//! UI when the session list may have changed. Updates reach the UI through
//! Tauri channels that the UI passes to `load_session` and `watch_sessions`.
//!
//! The watcher only reads. It never writes to, moves or deletes anything.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};
use serde::Serialize;

use crate::live::{LiveMessage, LiveSession};

/// Somewhere to send messages for the UI. Returns `false` when the message
/// could not be delivered, for example because the receiver is gone.
pub trait Sink<T>: Send {
    /// Sends one message. Must not block for long, since the caller may hold
    /// a lock on the shared state.
    fn send(&self, message: T) -> bool;
}

/// A Tauri channel is a sink. Sending only queues a script for the webview,
/// so it does not block.
impl<T: tauri::ipc::IpcResponse + Send> Sink<T> for tauri::ipc::Channel<T> {
    fn send(&self, message: T) -> bool {
        tauri::ipc::Channel::send(self, message).is_ok()
    }
}

/// The signal that the session list may have changed. It carries no data:
/// the UI calls `list_sessions` again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SessionsChanged {}

/// The open session and where to send its updates.
pub struct Active {
    /// The session the UI shows.
    pub session: LiveSession,
    /// The channel the UI passed to `load_session`.
    pub sink: Box<dyn Sink<LiveMessage>>,
}

/// State shared by the commands and the watcher thread.
///
/// No code holds both locks at the same time, so there is no lock order to
/// get wrong.
#[derive(Default)]
pub struct Shared {
    /// The open session, if any.
    pub active: Mutex<Option<Active>>,
    /// The channel the UI passed to `watch_sessions`, if any.
    pub list_sink: Mutex<Option<Box<dyn Sink<SessionsChanged>>>>,
    /// Whether the file watcher started. If not, the open session is still
    /// polled, but the session list is not watched.
    pub watcher_ok: AtomicBool,
}

/// How long the worker waits for more file events before acting on them.
pub const BATCH: Duration = Duration::from_millis(250);
/// How often the open session is refreshed even without a file event.
pub const POLL: Duration = Duration::from_secs(1);
/// The shortest time between two session list change signals.
pub const LIST_GAP: Duration = Duration::from_secs(2);

/// Decides when to refresh the open session and when to signal a session
/// list change. Pure logic with the clock passed in, so it can be tested.
pub struct Scheduler {
    session_dirty: bool,
    list_dirty: bool,
    last_poll: Instant,
    last_list: Instant,
}

/// What the worker should do now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Due {
    /// Refresh the open session.
    pub refresh_session: bool,
    /// Send a session list change signal.
    pub list_changed: bool,
}

impl Scheduler {
    /// A scheduler with nothing pending, as if it had just polled and
    /// signalled at `now`.
    pub fn new(now: Instant) -> Self {
        Self {
            session_dirty: false,
            list_dirty: false,
            last_poll: now,
            last_list: now,
        }
    }

    /// Notes that a file under the projects folder changed.
    /// `touches_session` says whether it belongs to the open session.
    pub fn file_changed(&mut self, touches_session: bool) {
        self.list_dirty = true;
        if touches_session {
            self.session_dirty = true;
        }
    }

    /// What is due at `now`. Calling this marks the returned work as done.
    pub fn due(&mut self, now: Instant) -> Due {
        let refresh_session =
            self.session_dirty || now.saturating_duration_since(self.last_poll) >= POLL;
        if refresh_session {
            self.session_dirty = false;
            self.last_poll = now;
        }
        let list_changed =
            self.list_dirty && now.saturating_duration_since(self.last_list) >= LIST_GAP;
        if list_changed {
            self.list_dirty = false;
            self.last_list = now;
        }
        Due {
            refresh_session,
            list_changed,
        }
    }
}

/// Whether `path` is the open session's transcript or inside the folder that
/// holds its subagents. Names are compared exactly, as `list_sessions`
/// returned them.
pub fn touches_session(root: &Path, path: &Path, project: &str, session_id: &str) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let mut names = relative.components().map(|c| match c {
        Component::Normal(name) => name.to_str(),
        _ => None,
    });
    let (Some(Some(first)), Some(Some(second))) = (names.next(), names.next()) else {
        return false;
    };
    first == project
        && (second == session_id
            || second
                .strip_suffix(".jsonl")
                .is_some_and(|stem| stem == session_id))
}

/// Starts the watcher thread. The file watcher is created first: if it
/// fails (for example because `root` does not exist yet) this is logged,
/// `watcher_ok` stays false, and the thread still polls the open session.
pub fn spawn(root: PathBuf, shared: Arc<Shared>) -> std::io::Result<JoinHandle<()>> {
    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let watcher = match start_watcher(&root, tx) {
        Ok(watcher) => {
            shared.watcher_ok.store(true, Ordering::SeqCst);
            Some(watcher)
        }
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not watch the projects folder; polling the open session only"
            );
            None
        }
    };
    std::thread::Builder::new()
        .name("snitchcraft-watch".into())
        .spawn(move || {
            // Keep the watcher alive for as long as the thread runs.
            let _watcher = watcher;
            run(&root, &shared, &rx);
        })
}

/// Creates the file watcher and starts watching `root` recursively. On
/// failure the watcher, and with it `tx`, is dropped.
fn start_watcher(
    root: &Path,
    tx: mpsc::Sender<notify::Result<notify::Event>>,
) -> notify::Result<notify::RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })?;
    watcher.watch(root, RecursiveMode::Recursive)?;
    Ok(watcher)
}

/// The worker loop. Runs until the process ends.
fn run(root: &Path, shared: &Shared, rx: &mpsc::Receiver<notify::Result<notify::Event>>) {
    let mut scheduler = Scheduler::new(Instant::now());
    loop {
        match rx.recv_timeout(BATCH) {
            Ok(event) => handle_event(root, shared, &mut scheduler, event),
            Err(RecvTimeoutError::Timeout) => {}
            // No watcher: wait as if for events, then poll.
            Err(RecvTimeoutError::Disconnected) => std::thread::sleep(BATCH),
        }
        while let Ok(event) = rx.try_recv() {
            handle_event(root, shared, &mut scheduler, event);
        }
        let due = scheduler.due(Instant::now());
        if due.refresh_session {
            refresh_active(shared);
        }
        if due.list_changed {
            signal_list(shared);
        }
    }
}

/// Notes one file event in the scheduler, marking the session dirty if any
/// of its paths belong to the open session.
fn handle_event(
    root: &Path,
    shared: &Shared,
    scheduler: &mut Scheduler,
    event: notify::Result<notify::Event>,
) {
    let event = match event {
        Ok(event) => event,
        Err(err) => {
            tracing::debug!(error = %err, "file watcher error");
            return;
        }
    };
    if matches!(event.kind, EventKind::Access(_)) {
        return;
    }
    let touches = match shared.active.lock() {
        Ok(guard) => guard.as_ref().is_some_and(|active| {
            event.paths.iter().any(|path| {
                touches_session(
                    root,
                    path,
                    active.session.project(),
                    active.session.session_id(),
                )
            })
        }),
        Err(_) => {
            tracing::warn!("live session state is unavailable");
            false
        }
    };
    scheduler.file_changed(touches);
}

/// Refreshes the open session and sends any update. The send happens under
/// the lock so updates for one session keep their order; a channel send only
/// queues work for the webview, so it does not block.
fn refresh_active(shared: &Shared) {
    let mut guard = match shared.active.lock() {
        Ok(guard) => guard,
        Err(_) => {
            tracing::warn!("live session state is unavailable");
            return;
        }
    };
    let Some(active) = guard.as_mut() else {
        return;
    };
    match active.session.refresh() {
        Ok(Some(message)) => {
            if !active.sink.send(message) {
                tracing::debug!("could not send a live update; the UI may have reloaded");
            }
        }
        Ok(None) => {}
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not refresh the open session; keeping the last view"
            );
        }
    }
}

/// Sends the session list change signal, and forgets the sink if it is gone.
fn signal_list(shared: &Shared) {
    let mut guard = match shared.list_sink.lock() {
        Ok(guard) => guard,
        Err(_) => {
            tracing::warn!("session list watcher state is unavailable");
            return;
        }
    };
    if let Some(sink) = guard.as_ref() {
        if !sink.send(SessionsChanged {}) {
            tracing::debug!("could not send a session list change; dropping the channel");
            *guard = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;

    impl<T: Send> Sink<T> for mpsc::Sender<T> {
        fn send(&self, message: T) -> bool {
            mpsc::Sender::send(self, message).is_ok()
        }
    }

    #[test]
    fn touches_session_matches_relative_names() {
        let root = Path::new("projects");
        let s = "00000000-0000-4000-8000-000000000002";
        assert!(touches_session(
            root,
            &root.join("basic").join(format!("{s}.jsonl")),
            "basic",
            s
        ));
        assert!(touches_session(
            root,
            &root
                .join("basic")
                .join(s)
                .join("subagents")
                .join("agent-1.jsonl"),
            "basic",
            s
        ));
        assert!(!touches_session(
            root,
            &root.join("other").join(format!("{s}.jsonl")),
            "basic",
            s
        ));
        assert!(!touches_session(
            root,
            &root.join("basic").join("another.jsonl"),
            "basic",
            s
        ));
        assert!(!touches_session(
            root,
            Path::new("elsewhere").join("basic").as_path(),
            "basic",
            s
        ));
    }

    #[test]
    fn scheduler_batches_and_polls() {
        let start = Instant::now();
        let mut s = Scheduler::new(start);
        let none = s.due(start);
        assert!(!none.refresh_session && !none.list_changed);

        s.file_changed(true);
        let due = s.due(start + Duration::from_millis(10));
        assert!(
            due.refresh_session,
            "a change to the session refreshes it at once"
        );
        assert!(!due.list_changed, "list changes wait for the gap");

        let due = s.due(start + LIST_GAP);
        assert!(due.list_changed);
        assert!(due.refresh_session, "polled again after a second");

        let due = s.due(start + LIST_GAP + Duration::from_millis(10));
        assert!(!due.refresh_session && !due.list_changed);
    }

    #[test]
    fn other_files_change_the_list_but_do_not_force_a_refresh() {
        let start = Instant::now();
        let mut s = Scheduler::new(start);
        s.file_changed(false);
        let due = s.due(start + Duration::from_millis(10));
        assert!(!due.refresh_session);
        assert!(s.due(start + LIST_GAP).list_changed);
    }

    #[test]
    fn worker_sends_updates_when_the_open_file_grows() {
        let root = std::env::temp_dir().join(format!("snitchcraft-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("basic")).expect("mkdir");
        let session = "00000000-0000-4000-8000-000000000002";
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("fixtures")
            .join("claude-code")
            .join("basic")
            .join(format!("{session}.jsonl"));
        let text = std::fs::read_to_string(fixture).expect("fixture");
        let lines: Vec<&str> = text.lines().collect();
        let main = root.join("basic").join(format!("{session}.jsonl"));
        let half = lines.len() / 2;
        std::fs::write(
            &main,
            lines[..half]
                .iter()
                .map(|l| format!("{l}\n"))
                .collect::<String>(),
        )
        .expect("write");

        let shared = Arc::new(Shared::default());
        let (tx, rx) = mpsc::channel::<LiveMessage>();
        let (list_tx, list_rx) = mpsc::channel::<SessionsChanged>();
        {
            let live = LiveSession::open(&root, "basic", session).expect("open");
            *shared.active.lock().expect("lock") = Some(Active {
                session: live,
                sink: Box::new(tx),
            });
            *shared.list_sink.lock().expect("lock") = Some(Box::new(list_tx));
        }
        let _worker = spawn(root.clone(), Arc::clone(&shared)).expect("spawn");
        assert!(
            shared.watcher_ok.load(Ordering::SeqCst),
            "watcher starts on a temp folder"
        );

        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&main)
            .expect("open");
        for line in &lines[half..] {
            writeln!(f, "{line}").expect("write");
        }
        drop(f);

        let message = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("an update within 5 seconds");
        assert!(matches!(message, LiveMessage::Updated { .. }));
        list_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("a list change within 5 seconds");
    }
}
