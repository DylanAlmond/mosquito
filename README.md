<p>
  <img src="app-icon.png" alt="App Icon" width="96">
</p>

# Mosquito

![Status](https://img.shields.io/badge/Status-WIP-yellow)
![GitHub Release](https://img.shields.io/github/v/release/DylanAlmond/mosquito)

🦟 **Bite-sized file sharing.**

A tiny desktop app for sharing files between devices on the same local network.

## How it works

1. Drag one or more files into the window
2. Mosquito starts a temporary HTTP server on your LAN
3. A URL and QR code appear
4. Another device scans the code and downloads the files
5. **Stop Sharing**, removing the last file, or closing the app shuts the server down

## Tech stack

- Tauri: desktop shell
- Vue 3 + TypeScript: frontend
- Rust with `axum` + `tokio`: HTTP server
- `qrcode`: QR generation (SVG)
- `local-ip-address`: LAN address detection

## Project layout

A Cargo workspace. All app logic lives in `mosquito-core`, a pure-Rust crate
with no Tauri dependency which keeps it fully testable with plain
`cargo test`, no window or webview required. The Tauri layer is thin glue.

```
mosquito/
├── src/                     # Vue frontend
│   ├── components/          #   Header, FileList, SharePanel
│   ├── composables/         #   useSharer (state), useDragDrop (native drop)
│   └── types.ts             #   mirrors of the Rust IPC structs
├── src-tauri/               # Tauri shell: thin commands over the core
└── crates/
    └── mosquito-core/       # the app, minus the GUI
        ├── src/
        │   ├── file_store.rs    # in-memory registry of shared files
        │   ├── server.rs        # axum routes, streaming downloads, listing page
        │   ├── sharer.rs        # server lifecycle state machine, batch adds
        │   ├── lan.rs           # LAN IP detection (trait, mockable in tests)
        │   └── qr.rs            # QR-as-SVG generation
        └── tests/               # HTTP + lifecycle integration tests
```

## Interfaces

Tauri commands (frontend → Rust):

| Command           | Purpose                                              |
| ----------------- | ---------------------------------------------------- |
| `add_files`       | register dropped paths, get per-file success/failure |
| `remove_file`     | stop sharing one file                                |
| `list_files`      | current shared files (backend is source of truth)    |
| `start_server`    | start the HTTP server                                |
| `stop_server`     | graceful stop (drains in-flight downloads)           |
| `get_server_info` | running state, URL, port, LAN IP                     |
| `get_qr`          | QR code for the share URL, as an SVG string          |

HTTP endpoints (other devices → Rust):

| Endpoint             | Purpose                          |
| -------------------- | -------------------------------- |
| `GET /`              | file listing / download page     |
| `GET /download/{id}` | stream one file as an attachment |

The server binds `0.0.0.0` on port **8472**, falling back to an
OS-assigned port if that one is taken — the URL shown in the app is
always the one that works.

## Development

### Prerequisites

- [Node.js](https://nodejs.org/) (v16+)
- [Rust](https://www.rust-lang.org/tools/install) (Latest stable)
- [Tauri CLI](https://tauri.app/v1/guides/getting-started/prerequisites)

### Build from Source

```bash
# Clone the repository
git clone https://github.com/DylanAlmond/mosquito
cd mosquito

# Install frontend dependencies
yarn install

# Run in development mode
yarn tauri dev
```

On Windows, the first launch triggers a Windows Firewall prompt. Click
**Allow** so other devices can reach the server.

### Test

```bash
cargo test --workspace
```

Unit tests live next to the code in `mosquito-core`; integration tests
(router harness and real-socket lifecycle tests) are in
`crates/mosquito-core/tests/`. One test (real LAN-IP detection) is
`#[ignore]`d by default since it needs a live network interface —
run everything with:

```bash
cargo test --workspace -- --include-ignored
```

### Backend smoke test (no GUI)

```bash
cargo run -p mosquito-core --example share -- path/to/file.pdf
```

Starts the real server and prints a scannable URL — handy for testing
the flow from a phone without building the frontend.

### Building for Production

```bash
# Build the optimized release binary
yarn tauri build
```

_The compiled installer/binary will be located in `src-tauri/target/release/bundle/`._

## General Notes

- The firewall prompt appears **once per binary**. The dev build and the
  release build are different programs, so expect it again after packaging.
- The Wi-Fi network's profile must be **Private** for inbound LAN connections.
- Docker/WSL virtual adapters can confuse LAN-IP detection. If the URL
  shows a `172.x` address, that's why.

## Contributing

Pull requests are welcome. For major changes, please open an issue first to discuss what you would like to change.

## License

Copyright (c) 2026 Dylan Almond All rights reserved.
