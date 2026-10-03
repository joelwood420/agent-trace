fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Listing the app's commands here makes Tauri generate `allow-<command>`
    // permissions for them and deny any command that a capability in
    // `capabilities/` does not grant. Without this list, every registered
    // command would be callable from every window.
    // See https://v2.tauri.app/security/capabilities/
    let manifest = tauri_build::AppManifest::new().commands(&["list_sessions", "load_session"]);
    let attributes = tauri_build::Attributes::new().app_manifest(manifest);
    tauri_build::try_build(attributes)?;
    Ok(())
}
