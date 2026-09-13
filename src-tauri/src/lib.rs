mod commands;

use crate::commands::*;
use mosquito_core::{Sharer};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Sharer::new())
        .invoke_handler(tauri::generate_handler![
            list_files,
            add_files,
            remove_file,
            start_server,
            stop_server,
            get_server_info,
            get_qr
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
