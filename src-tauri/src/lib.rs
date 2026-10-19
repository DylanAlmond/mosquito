mod commands;
#[cfg(all(debug_assertions, feature = "pilot"))]
mod harness;
// Issue #18: the values the Explorer registration publishes live in
// `shell::registration`, compiled whenever tests can reach them so debug
// builds can pin them. The registration glue inside `shell` stays
// gated to release Windows builds, so a debug build never writes the
// registry (and non-Windows release builds compile none of it).
//
// Invariant: the module gate (`any(windows, test)` here) is the loosest
// of the three — inside `shell`, `registration` is gated
// `any(test, all(windows, not(debug_assertions)))` and the registry
// glue `all(windows, not(debug_assertions))` — so writing the registry
// is impossible outside a release Windows build, whatever shows up in
// `registration`.
#[cfg(any(windows, test))]
mod shell;

use crate::commands::*;
use mosquito_core::Sharer;
use std::sync::Mutex;
use tauri::{Emitter, Manager};

pub struct PendingShareFiles(Mutex<Vec<String>>);

impl PendingShareFiles {
    fn new(paths: Vec<String>) -> Self {
        Self(Mutex::new(paths))
    }

    fn append(&self, paths: Vec<String>) {
        self.0
            .lock()
            .expect("pending share files lock poisoned")
            .extend(paths);
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().expect("pending share files lock poisoned"))
    }
}

fn share_paths(args: impl Iterator<Item = String>) -> Vec<String> {
    let mut args = args.skip_while(|arg| arg != "--share");
    if args.next().is_none() {
        return Vec::new();
    }
    args.collect()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
        let paths = share_paths(argv.into_iter().skip(1));
        if paths.is_empty() {
            return;
        }

        app.state::<PendingShareFiles>().append(paths);
        let _ = app.emit("share-files-requested", ());
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.set_focus();
        }
    }));

    let builder = builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Sharer::new())
        .manage(PendingShareFiles::new(share_paths(
            std::env::args().skip(1),
        )));

    // The tauri-pilot socket server is debug tooling (issue #7; see
    // `harness`): the plugin dependency (optional `pilot` feature in
    // `Cargo.toml`) and its registration are both gated the same way, so
    // a release binary contains no pilot code at all. The registration
    // is the only way anything the `pilot:default` capability
    // (`capabilities/pilot.toml`, fed in by `build.rs` only when the
    // feature is on) grants becomes reachable — and it never happens in
    // release. This handler table carries the debug-only fixture
    // commands, so the two invoke_handler registrations must list the
    // same prod commands.
    #[cfg(all(debug_assertions, feature = "pilot"))]
    let builder =
        builder
            .plugin(tauri_plugin_pilot::init())
            .invoke_handler(tauri::generate_handler![
                list_top,
                list_page,
                add_files,
                remove_file,
                start_server,
                stop_server,
                get_server_info,
                get_qr,
                take_pending_share_paths,
                register_context_menu,
                import_paths,
                cancel_import,
                import_runs,
                list_roots,
                materialize_root,
                remove_root,
                remove_node,
                harness::harness_seed
            ]);

    #[cfg(any(not(debug_assertions), not(feature = "pilot")))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        list_top,
        list_page,
        add_files,
        remove_file,
        start_server,
        stop_server,
        get_server_info,
        get_qr,
        take_pending_share_paths,
        register_context_menu,
        import_paths,
        cancel_import,
        import_runs,
        list_roots,
        materialize_root,
        remove_root,
        remove_node
    ]);

    // Belt and braces: `pilot` is for dev tooling only. Release builds
    // resolve default features (which exclude it), so anything accidentally
    // enabling it in release — e.g. `cargo build --release -f pilot`,
    // `tauri build --features pilot` — fails loudly here instead of
    // shipping a drivable binary.
    #[cfg(all(not(debug_assertions), feature = "pilot"))]
    compile_error!("the `pilot` feature is debug tooling (harness/desktop/README.md); it must not be enabled in release builds");

    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::{share_paths, PendingShareFiles};

    #[test]
    fn share_paths_collects_every_argument_after_the_flag() {
        let args = ["--share", r"C:\Files\one.txt", r"C:\Files\two.txt"]
            .into_iter()
            .map(str::to_string);

        assert_eq!(
            share_paths(args),
            [r"C:\Files\one.txt", r"C:\Files\two.txt"]
        );
    }

    #[test]
    fn share_paths_ignores_regular_launches() {
        assert!(share_paths(["--normal"].into_iter().map(str::to_string)).is_empty());
    }

    #[test]
    fn pending_share_files_are_drained_once() {
        let pending = PendingShareFiles::new(vec!["first.txt".into()]);
        pending.append(vec!["second.txt".into()]);

        assert_eq!(pending.take(), ["first.txt", "second.txt"]);
        assert!(pending.take().is_empty());
    }
}
