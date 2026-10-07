//! The Snitchcraft desktop app. The backend reads Claude Code transcripts
//! (read-only), builds the diagram model in Rust, and hands it to the web UI
//! through commands. A watcher thread keeps the open session live and
//! pushes updates to the UI over Tauri channels. A local proxy captures the
//! API calls of sessions started with capture on and saves them in the app's
//! own captures folder.

// Hide the extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture_sink;
mod captures;
mod context;
mod live;
mod sessions;
mod watch;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use capture::{CaptureStore, Proxy};
use tauri::Manager;
use tauri::ipc::Channel;
use trace_view::NodeDetail;

use capture_sink::{AppSink, CaptureStatus, ProxyState, RecordCache};
use captures::{CaptureDetail, CaptureOverview};
use context::SessionContext;
use insights::ContextBreakdown;
use live::{LiveMessage, LiveSession, LiveStatus};
use sessions::{SessionError, SessionSummary, SessionView, TitleCache};
use watch::{Active, SessionsChanged, Shared, Sink};

/// Where the Claude Code projects folder is, resolved once at startup.
struct AppPaths {
    /// `None` if the home folder could not be found.
    projects_root: Option<PathBuf>,
}

impl AppPaths {
    fn root(&self) -> Result<PathBuf, SessionError> {
        self.projects_root.clone().ok_or(SessionError::NoHome)
    }
}

/// The capture store and what goes with it, set up once at startup.
struct Captures {
    /// `None` if the app data folder could not be found; capture is off.
    store: Option<Arc<CaptureStore>>,
    /// The last session's decoded records.
    cache: Arc<RecordCache>,
    /// The proxy's state.
    proxy: Arc<ProxyState>,
}

impl Captures {
    fn store(&self) -> Result<Arc<CaptureStore>, SessionError> {
        self.store.clone().ok_or(SessionError::CapturesOff)
    }
}

/// Lists session transcripts under the projects folder, newest first.
#[tauri::command(rename_all = "snake_case")]
async fn list_sessions(
    paths: tauri::State<'_, AppPaths>,
    titles: tauri::State<'_, Arc<TitleCache>>,
) -> Result<Vec<SessionSummary>, String> {
    let root = paths.root().map_err(|e| e.to_string())?;
    let titles = Arc::clone(&titles);
    let sessions = run_blocking(move || sessions::list_sessions_cached(&root, &titles)).await?;
    tracing::info!(count = sessions.len(), "listed sessions");
    Ok(sessions)
}

/// Loads one session, keeps it as the open session, and returns its
/// diagram model and skipped lines. `project` and `session_id` must be values
/// returned by `list_sessions`. The watch status, and later updates of the
/// session, are sent through `on_update`. Loading another session replaces
/// this one.
#[tauri::command(rename_all = "snake_case")]
async fn load_session(
    paths: tauri::State<'_, AppPaths>,
    shared: tauri::State<'_, Arc<Shared>>,
    captures: tauri::State<'_, Captures>,
    project: String,
    session_id: String,
    on_update: Channel<LiveMessage>,
) -> Result<SessionView, String> {
    let root = paths.root().map_err(|e| e.to_string())?;
    let shared = Arc::clone(&shared);
    let store = captures.store.clone();
    let cache = Arc::clone(&captures.cache);
    // Taken before any work, so a load that started later always wins, even
    // if it finishes first.
    let ticket = shared.begin_load();
    let view = run_blocking(move || {
        let mut session = LiveSession::open(&root, &project, &session_id)?;
        // Held until the session is installed, so a capture saved meanwhile
        // is either in these records or applied after the install.
        let _updates = cache.lock_updates();
        if let Some(store) = &store {
            capture_sink::attach_captures(store, &cache, &mut session);
        }
        let view = session.view();
        let status = if shared.watcher_ok.load(Ordering::SeqCst) {
            LiveStatus::Watching
        } else {
            LiveStatus::NoWatcher
        };
        let sink: Box<dyn Sink<LiveMessage>> = Box::new(on_update);
        let message = LiveMessage::Status {
            project,
            session_id,
            status,
        };
        shared.install(ticket, Active { session, sink }, message);
        Ok(view)
    })
    .await?;
    tracing::info!(
        prompts = view.diagram.prompts.len(),
        skipped = view.skipped.len(),
        "loaded session"
    );
    Ok(view)
}

/// Full content of one trace node of a session, or `null` if the session has
/// no node with that id. Uses the open session when it is the same one, so
/// clicking boxes does not re-read the file; otherwise reads the session
/// without keeping it.
#[tauri::command(rename_all = "snake_case")]
async fn node_detail(
    paths: tauri::State<'_, AppPaths>,
    shared: tauri::State<'_, Arc<Shared>>,
    project: String,
    session_id: String,
    trace_id: String,
) -> Result<Option<NodeDetail>, String> {
    let root = paths.root().map_err(|e| e.to_string())?;
    let shared = Arc::clone(&shared);
    let detail = run_blocking(move || {
        sessions::check_trace_id(&trace_id)?;
        {
            let guard = shared.active();
            if let Some(active) = guard
                .as_ref()
                .filter(|a| a.session.is(&project, &session_id))
            {
                return Ok(active.session.node_detail(&trace_id));
            }
        }
        let live = LiveSession::open(&root, &project, &session_id)?;
        Ok(live.node_detail(&trace_id))
    })
    .await?;
    tracing::info!(found = detail.is_some(), "loaded node detail");
    Ok(detail)
}

/// Sends a signal through `on_change` whenever the session list may have
/// changed, at most every two seconds and at least once a minute (so live
/// markers expire). The signal carries no data; the UI
/// calls `list_sessions` again. A later call replaces the channel.
#[tauri::command(rename_all = "snake_case")]
fn watch_sessions(
    shared: tauri::State<'_, Arc<Shared>>,
    on_change: Channel<SessionsChanged>,
) -> Result<(), String> {
    let mut guard = shared.list_sink();
    *guard = Some(Box::new(on_change));
    tracing::info!("watching the session list");
    Ok(())
}

/// Whether the capture proxy is listening, the command that starts a
/// captured Claude Code session, and any proxy or save error.
#[tauri::command(rename_all = "snake_case")]
fn capture_status(captures: tauri::State<'_, Captures>) -> CaptureStatus {
    capture_sink::capture_status(&captures.proxy)
}

/// The overview of one session's captured API calls. `project` and
/// `session_id` must be values returned by `list_sessions`.
#[tauri::command(rename_all = "snake_case")]
async fn session_captures(
    paths: tauri::State<'_, AppPaths>,
    shared: tauri::State<'_, Arc<Shared>>,
    captures: tauri::State<'_, Captures>,
    project: String,
    session_id: String,
) -> Result<CaptureOverview, String> {
    let root = paths.root().map_err(|e| e.to_string())?;
    let store = captures.store().map_err(|e| e.to_string())?;
    let cache = Arc::clone(&captures.cache);
    let shared = Arc::clone(&shared);
    let view = run_blocking(move || {
        capture_sink::session_overview(&root, &store, &cache, &shared, &project, &session_id)
    })
    .await?;
    tracing::info!(calls = view.calls.len(), "loaded capture overview");
    Ok(view)
}

/// What fills the context of every model call of one session, as one bar
/// per call, with the main agent's latest call in full. Calls with a
/// captured request use it; the others are estimated from the transcript.
/// Works when capture is off.
#[tauri::command(rename_all = "snake_case")]
async fn session_context(
    paths: tauri::State<'_, AppPaths>,
    shared: tauri::State<'_, Arc<Shared>>,
    captures: tauri::State<'_, Captures>,
    project: String,
    session_id: String,
) -> Result<SessionContext, String> {
    let root = paths.root().map_err(|e| e.to_string())?;
    let store = captures.store.clone();
    let cache = Arc::clone(&captures.cache);
    let shared = Arc::clone(&shared);
    let view = run_blocking(move || {
        capture_sink::session_context_view(
            &root,
            store.as_deref(),
            &cache,
            &shared,
            &project,
            &session_id,
        )
    })
    .await?;
    tracing::info!(
        bars = view.bars.len(),
        latest = view.latest.is_some(),
        "loaded session context"
    );
    Ok(view)
}

/// What fills the context of one model call in full, with advice, or `null`
/// if the session has no such model call or the call has no data yet.
#[tauri::command(rename_all = "snake_case")]
async fn call_context(
    paths: tauri::State<'_, AppPaths>,
    shared: tauri::State<'_, Arc<Shared>>,
    captures: tauri::State<'_, Captures>,
    project: String,
    session_id: String,
    trace_id: String,
) -> Result<Option<ContextBreakdown>, String> {
    let root = paths.root().map_err(|e| e.to_string())?;
    let store = captures.store.clone();
    let cache = Arc::clone(&captures.cache);
    let shared = Arc::clone(&shared);
    let view = run_blocking(move || {
        capture_sink::call_context_view(
            &root,
            store.as_deref(),
            &cache,
            &shared,
            &project,
            &session_id,
            &trace_id,
        )
    })
    .await?;
    tracing::info!(found = view.is_some(), "loaded call context");
    Ok(view)
}

/// One captured API call in full, with the changes since the call before
/// it, or `null` if the session has no capture with that id.
#[tauri::command(rename_all = "snake_case")]
async fn capture_detail(
    paths: tauri::State<'_, AppPaths>,
    shared: tauri::State<'_, Arc<Shared>>,
    captures: tauri::State<'_, Captures>,
    project: String,
    session_id: String,
    capture_id: String,
) -> Result<Option<CaptureDetail>, String> {
    let root = paths.root().map_err(|e| e.to_string())?;
    let store = captures.store().map_err(|e| e.to_string())?;
    let cache = Arc::clone(&captures.cache);
    let shared = Arc::clone(&shared);
    let detail = run_blocking(move || {
        capture_sink::session_capture_detail(
            &root,
            &store,
            &cache,
            &shared,
            &project,
            &session_id,
            &capture_id,
        )
    })
    .await?;
    tracing::info!(found = detail.is_some(), "loaded capture detail");
    Ok(detail)
}

/// Deletes every saved capture of one session from the app's captures
/// folder. Nothing under the Claude Code folders is touched.
#[tauri::command(rename_all = "snake_case")]
async fn delete_captures(
    paths: tauri::State<'_, AppPaths>,
    shared: tauri::State<'_, Arc<Shared>>,
    captures: tauri::State<'_, Captures>,
    project: String,
    session_id: String,
) -> Result<(), String> {
    let root = paths.root().map_err(|e| e.to_string())?;
    let store = captures.store().map_err(|e| e.to_string())?;
    let cache = Arc::clone(&captures.cache);
    let shared = Arc::clone(&shared);
    run_blocking(move || {
        capture_sink::delete_session_captures(&root, &store, &cache, &shared, &project, &session_id)
    })
    .await?;
    tracing::info!("deleted a session's captures");
    Ok(())
}

/// Starts the capture proxy on Tauri's async runtime. A failure to listen
/// (for example, the port is in use) is logged and kept for
/// `capture_status`; the rest of the app works without the proxy.
fn start_proxy(sink: AppSink, state: Arc<ProxyState>) {
    tauri::async_runtime::spawn(async move {
        match Proxy::bind(Arc::new(sink)).await {
            Ok(proxy) => {
                state.listening.store(true, Ordering::SeqCst);
                proxy.serve().await;
                state.listening.store(false, Ordering::SeqCst);
            }
            Err(err) => {
                tracing::error!(error = %err, "could not start the capture proxy");
                state.set_error(err.to_string());
            }
        }
    });
}

/// Runs file work on a blocking thread so the UI stays responsive, and turns
/// every error into a message for the UI.
async fn run_blocking<T, F>(work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, SessionError> + Send + 'static,
{
    match tauri::async_runtime::spawn_blocking(work).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => {
            tracing::warn!(error = %err, "command failed");
            Err(err.to_string())
        }
        Err(err) => {
            tracing::error!(error = %err, "background task failed");
            Err("internal error".to_string())
        }
    }
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    tauri::Builder::default()
        .setup(|app| {
            let projects_root = match app.path().home_dir() {
                Ok(home) => Some(sessions::projects_root(&home)),
                Err(err) => {
                    tracing::error!(error = %err, "could not find the home folder");
                    None
                }
            };
            let shared = Arc::new(Shared::default());
            if let Some(root) = projects_root.clone() {
                // The app still works without the thread; sessions are just
                // not live.
                if let Err(err) = watch::spawn(root, Arc::clone(&shared)) {
                    tracing::error!(error = %err, "could not start the watcher thread");
                }
            }
            let store = match app.path().app_data_dir() {
                Ok(dir) => Some(Arc::new(CaptureStore::new(dir.join("captures")))),
                Err(err) => {
                    tracing::error!(error = %err, "could not find the app data folder; capture is off");
                    None
                }
            };
            let cache = Arc::new(RecordCache::default());
            let proxy = Arc::new(ProxyState::default());
            match &store {
                Some(store) => start_proxy(
                    AppSink::new(
                        Arc::clone(store),
                        Arc::clone(&shared),
                        Arc::clone(&cache),
                        Arc::clone(&proxy),
                    ),
                    Arc::clone(&proxy),
                ),
                None => proxy.set_error(
                    "capture is off: the app data folder could not be found".to_string(),
                ),
            }
            app.manage(AppPaths { projects_root });
            app.manage(shared);
            app.manage(Captures {
                store,
                cache,
                proxy,
            });
            app.manage(Arc::new(TitleCache::default()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            load_session,
            node_detail,
            watch_sessions,
            capture_status,
            session_captures,
            session_context,
            call_context,
            capture_detail,
            delete_captures
        ])
        .run(tauri::generate_context!())?;
    Ok(())
}
