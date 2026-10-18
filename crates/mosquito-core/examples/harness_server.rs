//! Deterministic fixture servers for the web UI harness (issue #7).
//!
//! ```sh
//! cargo run -p mosquito-core --example harness_server
//! ```
//!
//! Serves two listing pages from the production [`axum::Router`],
//! standing in for the states a Playwright run asserts on
//! (`harness/web/README.md`):
//!
//! - `http://127.0.0.1:8473` — loose files plus a folder group
//! - `http://127.0.0.1:8474` — the empty listing
//!
//! All listeners stay on loopback. Fixture files are written to
//! `<temp>/mosquito-harness/web` on every start — rebuilt from scratch so
//! runs are reproducible and stale content never leaks into a page.

use std::sync::{Arc, Mutex};

use mosquito_core::{AppState, FileStore, router};

/// Ports picked away from the app's default 8472 so a desktop instance
/// and the harness can run at the same time.
const FIXTURE_PORT: u16 = 8473;
const EMPTY_PORT: u16 = 8474;

fn fixture_dir() -> std::path::PathBuf {
    std::env::temp_dir().join("mosquito-harness").join("web")
}

/// Recreate the fixture tree. Loose files lead the list, the folder
/// group follows: the desktop layout the listing mirrors (ADR-0001).
fn write_fixtures(dir: &std::path::Path) -> std::io::Result<()> {
    let folder = dir.join("Photographs");
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(folder.join("notes"))?;

    std::fs::write(
        dir.join("recipes.txt"),
        "banana bread\n- 3 bananas\n- 250 g flour\n",
    )?;
    std::fs::write(
        dir.join("checklist.txt"),
        "plan a trip: pack, board, relax\n",
    )?;
    std::fs::write(
        folder.join("beach.jpg"),
        b"\xFF\xD8\xFF\xE0 stand-in JPEG header bytes for the harness fixture\0",
    )?;
    std::fs::write(
        folder.join("notes").join("todo.txt"),
        "sharpen the photos before printing\n",
    )?;

    Ok(())
}

async fn serve(port: u16, store: FileStore) -> std::io::Result<()> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("  http://{addr}");

    axum::serve(listener, router(AppState::new(Arc::new(Mutex::new(store))))).await
}

#[tokio::main]
async fn main() {
    let dir = fixture_dir();
    if let Err(e) = write_fixtures(&dir) {
        eprintln!("harness: writing fixtures failed: {e}");
        std::process::exit(1);
    }
    println!("harness fixtures in {}", dir.display());

    let mut seeded = FileStore::new();
    let outcome = seeded.add_paths(vec![
        dir.join("recipes.txt"),
        dir.join("checklist.txt"),
        dir.join("Photographs"),
    ]);
    if !outcome.failed.is_empty() {
        eprintln!("harness: fixture adds failed: {outcome:?}");
        std::process::exit(1);
    }

    println!("harness listing states:");

    let empty = tokio::spawn(async move {
        serve(EMPTY_PORT, FileStore::new())
            .await
            .expect("empty server");
    });
    let fixtures = tokio::spawn(async move {
        serve(FIXTURE_PORT, seeded).await.expect("fixture server");
    });

    // Ctrl+C never reaches the spawned tasks directly; process exit is
    // the teardown for a throwaway harness.
    let _ = tokio::join!(empty, fixtures);
}
