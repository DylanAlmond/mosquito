//! Manual smoke test: serve files from the terminal, no GUI needed.
//!
//! cargo run -p mosquito-core --example serve -- ~/some-file.pdf
//! then open http://localhost:8472

use std::sync::{Arc, Mutex};

use mosquito_core::{AppState, FileStore, router};

#[tokio::main]
async fn main() {
    let mut store = FileStore::new();

    for path in std::env::args().skip(1) {
        match store.add(&path) {
            Ok(f) => println!("sharing {} ({} B) as /download/{}", f.name, f.size, f.id.0),
            Err(e) => eprintln!("skipping {path}: {e}"),
        }
    }

    if store.list().is_empty() {
        eprintln!("usage: cargo run -p mosquito-core --example serve -- <file> [<file>...]");
        std::process::exit(1);
    }

    let state = AppState::new(Arc::new(Mutex::new(store)));
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], 8472));
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

    println!("listening on http://{addr}");

    axum::serve(listener, router(state)).await.unwrap();
}
