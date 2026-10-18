use std::path::PathBuf;

use mosquito_core::{
    qr_svg, AddFilesOutcome, FileId, ImportRunSnapshot, ImportStartOutcome, MaterializeOutcome,
    NodePage, RootId, RootRecord, RootRemoval, ServerInfo, SharedFile, Sharer, TopListing,
    LISTING_PAGE_ROWS,
};
use tauri::State;

use crate::PendingShareFiles;

// -- paged listing derivation (ADR-0006, issue #24) ----------------------

#[tauri::command]
pub async fn list_top(sharer: State<'_, Sharer>) -> Result<TopListing, String> {
    Ok(sharer.list_top())
}

/// One page of one node's rows: `sub_path` — empty for the root record
/// itself — and the window's `offset`. The default limit is the
/// listing's page budget; the UI fetches ahead within it.
#[tauri::command]
pub async fn list_page(
    sharer: State<'_, Sharer>,
    root_id: u64,
    sub_path: String,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<NodePage, String> {
    sharer
        .list_page(
            RootId(root_id),
            &sub_path,
            offset.unwrap_or(0),
            limit.unwrap_or(LISTING_PAGE_ROWS),
        )
        .map_err(|e| e.to_string())
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
    sharer
        .remove_file(FileId(id))
        .await
        .map_err(|e| e.to_string())
}

// -- import runs: count-only folder imports (ADR-0005/0007) ------------

#[tauri::command]
pub async fn import_paths(
    sharer: State<'_, Sharer>,
    paths: Vec<String>,
) -> Result<ImportStartOutcome, String> {
    let paths = paths.into_iter().map(PathBuf::from).collect();
    Ok(sharer.import_paths(paths).await)
}

#[tauri::command]
pub async fn cancel_import(sharer: State<'_, Sharer>, run_id: u64) -> Result<bool, String> {
    Ok(sharer.cancel_import(run_id))
}

#[tauri::command]
pub async fn import_runs(sharer: State<'_, Sharer>) -> Result<Vec<ImportRunSnapshot>, String> {
    Ok(sharer.import_run_status())
}

#[tauri::command]
pub async fn list_roots(sharer: State<'_, Sharer>) -> Result<Vec<RootRecord>, String> {
    Ok(sharer.roots())
}

#[tauri::command]
pub async fn materialize_root(
    sharer: State<'_, Sharer>,
    root_id: u64,
) -> Result<MaterializeOutcome, String> {
    sharer
        .materialize_root(RootId(root_id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn remove_root(sharer: State<'_, Sharer>, root_id: u64) -> Result<RootRemoval, String> {
    sharer
        .remove_root(RootId(root_id))
        .await
        .map_err(|e| e.to_string())
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

#[tauri::command]
pub async fn take_pending_share_paths(
    pending: State<'_, PendingShareFiles>,
) -> Result<Vec<String>, String> {
    Ok(pending.take())
}

#[tauri::command]
pub async fn register_context_menu() -> Result<(), String> {
    #[cfg(all(windows, not(debug_assertions)))]
    crate::shell::register_context_menu().map_err(|error| error.to_string())?;

    Ok(())
}
