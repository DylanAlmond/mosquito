use std::path::PathBuf;

use mosquito_core::{qr_svg, AddFilesOutcome, FileId, ServerInfo, SharedFile, Sharer};
use tauri::State;

#[tauri::command]
pub async fn list_files(sharer: State<'_, Sharer>) -> Result<Vec<SharedFile>, String> {
    Ok(sharer.files())
}

#[tauri::command]
pub async fn add_files(
    sharer: State<'_, Sharer>,
    paths: Vec<String>,
) -> Result<AddFilesOutcome, String> {
    let paths = paths.into_iter().map(PathBuf::from).collect();
    Ok(sharer.add_files(paths))
}

#[tauri::command]
pub async fn remove_file(sharer: State<'_, Sharer>, id: u64) -> Result<SharedFile, String> {
    sharer.remove_file(FileId(id)).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn start_server(sharer: State<'_, Sharer>) -> Result<ServerInfo, String> {
    sharer.start().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn stop_server(sharer: State<'_, Sharer>) -> Result<(), String> {
    sharer.stop().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_server_info(sharer: State<'_, Sharer>) -> Result<ServerInfo, String> {
    Ok(sharer.status())
}

/// The QR lives on the local screen only, so it crosses IPC as an SVG
/// string instead of being an HTTP endpoint (which would hand the URL
/// to everyone on the LAN).
#[tauri::command]
pub async fn get_qr(sharer: State<'_, Sharer>) -> Result<String, String> {
    let url = sharer
        .status()
        .url
        .ok_or_else(|| "server is not running".to_string())?;
    qr_svg(&url).map_err(|e| e.to_string())
}
