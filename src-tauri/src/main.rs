//! The looptrace desktop app. The backend reads Claude Code transcripts
//! (read-only) and hands trace events to the web UI through two commands.

// Hide the extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod sessions;

use std::path::PathBuf;

use tauri::Manager;

use sessions::{LoadedSession, SessionError, SessionSummary};

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

/// Loads one session and returns its trace events and skipped lines.
/// `project` and `session_id` must be values returned by `list_sessions`.
#[tauri::command(rename_all = "snake_case")]
async fn load_session(
    paths: tauri::State<'_, AppPaths>,
    project: String,
    session_id: String,
) -> Result<LoadedSession, String> {
    let root = paths.root().map_err(|e| e.to_string())?;
    let loaded = run_blocking(move || sessions::load_session(&root, &project, &session_id)).await?;
    tracing::info!(
        events = loaded.events.len(),
        skipped = loaded.skipped.len(),
        "loaded session"
    );
    Ok(loaded)
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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![list_sessions, load_session])
        .run(tauri::generate_context!())?;
    Ok(())
}
