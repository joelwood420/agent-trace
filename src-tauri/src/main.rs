//! The Snitchcraft desktop app. The backend reads Claude Code transcripts
//! (read-only), builds the diagram model in Rust, and hands it to the web UI
//! through four commands. A watcher thread keeps the open session live and
//! pushes updates to the UI over Tauri channels.

// Hide the extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod live;
mod sessions;
mod watch;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use tauri::Manager;
use tauri::ipc::Channel;
use trace_view::NodeDetail;

use live::{LiveMessage, LiveSession, LiveStatus};
use sessions::{SessionError, SessionSummary, SessionView};
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

/// Lists session transcripts under the projects folder, newest first.
#[tauri::command(rename_all = "snake_case")]
async fn list_sessions(paths: tauri::State<'_, AppPaths>) -> Result<Vec<SessionSummary>, String> {
    let root = paths.root().map_err(|e| e.to_string())?;
    let sessions = run_blocking(move || sessions::list_sessions(&root)).await?;
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
    project: String,
    session_id: String,
    on_update: Channel<LiveMessage>,
) -> Result<SessionView, String> {
    let root = paths.root().map_err(|e| e.to_string())?;
    let shared = Arc::clone(&shared);
    // Taken before any work, so a load that started later always wins, even
    // if it finishes first.
    let ticket = shared.begin_load();
    let view = run_blocking(move || {
        let session = LiveSession::open(&root, &project, &session_id)?;
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
/// changed, at most every two seconds. The signal carries no data; the UI
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
            app.manage(AppPaths { projects_root });
            app.manage(shared);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            load_session,
            node_detail,
            watch_sessions
        ])
        .run(tauri::generate_context!())?;
    Ok(())
}
