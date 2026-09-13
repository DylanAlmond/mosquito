//! Manual end-to-end check, no GUI:
//!   cargo run -p mosquito-core --example share -- ~/some-file.pdf
//! Open the printed URL from another device on the same Wi-Fi.
//! Ctrl+C performs a graceful stop.

use mosquito_core::Sharer;

#[tokio::main]
async fn main() {
    let sharer = Sharer::new();

    for path in std::env::args().skip(1) {
        match sharer.add_file(path) {
            Ok(f) => println!("sharing {} ({} B)", f.name, f.size),
            Err(e) => eprintln!("skipping: {e}"),
        }
    }

    let info = sharer.start().expect("failed to start server");

    println!("\n  sharing at: {}", info.url.unwrap());
    println!("  ctrl+C to stop\n");

    tokio::signal::ctrl_c().await.unwrap();
    sharer.stop().await.unwrap();
    println!("server stopped cleanly");
}
