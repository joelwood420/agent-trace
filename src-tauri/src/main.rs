//! The Snitchcraft desktop app. The backend reads Claude Code transcripts
//! (read-only), builds the diagram model in Rust, and hands it to the web UI
//! through three commands.

// Hide the extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// The live watcher that calls `LiveSession::refresh` and sends `LiveMessage`
// to the UI is not wired up yet, so parts of this module are unused for now.
#[allow(dead_code)]
mod live;
mod sessions;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tauri::Manager;
use trace_view::NodeDetail;

use live::LiveSession;
use sessions::{SessionError, SessionSummary, SessionView};

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

/// The open session, shared by the commands. Clicking boxes in the diagram
/// asks this session for details instead of re-reading the transcript.
#[derive(Default)]
struct Open(Arc<Mutex<Option<LiveSession>>>);

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
/// returned by `list_sessions`.
#[tauri::command(rename_all = "snake_case")]
async fn load_session(
    paths: tauri::State<'_, AppPaths>,
    open: tauri::State<'_, Open>,
    project: String,
    session_id: String,
) -> Result<SessionView, String> {
    let root = paths.root().map_err(|e| e.to_string())?;
    let open = Arc::clone(&open.0);
    let view = run_blocking(move || {
        let live = LiveSession::open(&root, &project, &session_id)?;
        let view = live.view();
        let mut guard = open.lock().map_err(|_| SessionError::CachePoisoned)?;
        *guard = Some(live);
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
    open: tauri::State<'_, Open>,
    project: String,
    session_id: String,
    trace_id: String,
) -> Result<Option<NodeDetail>, String> {
    let root = paths.root().map_err(|e| e.to_string())?;
    let open = Arc::clone(&open.0);
    let detail = run_blocking(move || {
        sessions::check_trace_id(&trace_id)?;
        {
            let guard = open.lock().map_err(|_| SessionError::CachePoisoned)?;
            if let Some(live) = guard.as_ref().filter(|l| l.is(&project, &session_id)) {
                return Ok(live.node_detail(&trace_id));
            }
        }
        let live = LiveSession::open(&root, &project, &session_id)?;
        Ok(live.node_detail(&trace_id))
    })
    .await?;
    tracing::info!(found = detail.is_some(), "loaded node detail");
    Ok(detail)
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
            app.manage(AppPaths { projects_root });
            app.manage(Open::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            load_session,
            node_detail
        ])
        .run(tauri::generate_context!())?;
    Ok(())
}
