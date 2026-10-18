use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tokio::fs::File;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::io::ReaderStream;
use zip::CompressionMethod;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::archive_planner::{ArchiveEntry, ArchivePlan, plan_archive};
use crate::file_store::{FileId, FileStore};
use crate::listing::{device_name, render_listing};

/// State shared by every handler. Cheap to clone (it's an Arc).
#[derive(Clone)]
pub struct AppState {
    store: Arc<Mutex<FileStore>>,
}

impl AppState {
    pub fn new(store: Arc<Mutex<FileStore>>) -> Self {
        Self { store }
    }
}

/// Build the app's routes. Tests and `main` both go through this.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(listing))
        .route("/download/zip", get(download_zip))
        .route("/download/{id}", get(download))
        .route("/assets/dynapuff-400.woff2", get(font_regular))
        .route("/assets/dynapuff-600.woff2", get(font_semibold))
        .route("/assets/favicon.ico", get(favicon))
        .with_state(state)
}

async fn listing(State(state): State<AppState>) -> impl IntoResponse {
    // The pattern used everywhere in this app: lock, copy what we need,
    // drop the lock — never hold a std Mutex guard across an `.await`.
    let files = {
        let store = state.store.lock().expect("file store not poisoned");
        store.list().to_vec()
    };

    let html = render_listing(&files, &device_name());

    ([(CONTENT_TYPE, "text/html; charset=utf-8")], html)
}

async fn font_regular() -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "font/woff2")],
        &include_bytes!("assets/dynapuff-latin-400-normal.woff2")[..],
    )
}

async fn font_semibold() -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "font/woff2")],
        &include_bytes!("assets/dynapuff-latin-600-normal.woff2")[..],
    )
}

async fn favicon() -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "image/x-icon")],
        &include_bytes!("assets/favicon.ico")[..],
    )
}

async fn download(State(state): State<AppState>, Path(id): Path<u64>) -> Response {
    let file = {
        let store = state.store.lock().expect("file store not poisoned");
        store.get(FileId(id)).cloned()
    };

    let Some(file) = file else {
        return not_found();
    };

    // Open now and serve from this handle: the bytes are committed even if
    // the file is unregistered (or moved on disk) mid-download.
    let Ok(handle) = File::open(&file.path).await else {
        return not_found();
    };

    // Use the *current* length, not the add-time snapshot, so
    // Content-Length always matches the bytes we actually send.
    let len = match handle.metadata().await {
        Ok(meta) => meta.len(),
        Err(_) => return internal_error(),
    };

    let mut response = Response::new(Body::from_stream(ReaderStream::with_capacity(
        handle,
        64 * 1024,
    )));

    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );

    headers.insert(CONTENT_LENGTH, HeaderValue::from(len));
    headers.insert(CONTENT_DISPOSITION, content_disposition(&file.name));

    response
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found\n").into_response()
}

/// `GET /download/zip?ids=0,2` — the selected files as one streamed
/// archive; no ids means everything currently shared. Bytes go out
/// entry-by-entry, so multi-gigabyte selections never sit in memory:
/// each member is read and compressed only when the client reaches it.
async fn download_zip(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let requested = match query.get("ids") {
        None => None,
        Some(raw) => match parse_ids(raw) {
            Some(ids) => Some(ids),
            None => return not_found(),
        },
    };

    let selected = {
        let store = state.store.lock().expect("file store not poisoned");
        match requested {
            None => store.list().to_vec(),
            Some(ids) => {
                let mut selected = Vec::with_capacity(ids.len());
                for id in ids {
                    match store.get(FileId(id)) {
                        Some(file) => selected.push(file.clone()),
                        None => return not_found(),
                    }
                }
                selected
            }
        }
    };

    // The archive can only succeed if every member is still readable:
    // a dangling member (content removed after the add) is a legit
    // state, but must be visible as a failure (ADR-0001) — a hole-free
    // partial archive would hide the loss instead. Answering 404 here
    // mirrors the single-file download's unreadable → 404.
    let all_present = selected.iter().all(|file| {
        std::fs::metadata(&file.path)
            .map(|meta| meta.is_file())
            .unwrap_or(false)
    });
    if !all_present {
        return not_found();
    }
    let ArchivePlan { name, entries } = plan_archive(&selected);

    let (tx, rx) = mpsc::channel::<io::Result<Vec<u8>>>(8);
    tokio::task::spawn_blocking(move || {
        if let Err(err) = write_zip_archive(&entries, &tx) {
            // Headers are already sent; surfacing the failure as a
            // body error at least aborts the transfer instead of
            // leaving the client with a silently truncated archive.
            let _ = tx.blocking_send(Err(err));
        }
    });

    let mut response = Response::new(Body::from_stream(ReceiverStream::new(rx)));

    // No Content-Length — a streamed, compressed archive can't know its
    // length up front, so the transfer stays chunked end-to-end.
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/zip"));
    headers.insert(CONTENT_DISPOSITION, content_disposition(&name));

    response
}

/// `ids=0,2` → [0, 2]. Any malformed piece (empty, non-numeric) makes
/// the whole query invalid, answered with 404 like an unknown id.
fn parse_ids(raw: &str) -> Option<Vec<u64>> {
    raw.split(',')
        .map(|part| part.trim().parse::<u64>().ok())
        .collect()
}

/// Bridges the sync zip writer to the async response body: every write
/// is one chunk handed to the stream. The bounded channel is the
/// backpressure — a slow reader throttles reads from disk.
struct ChannelWriter {
    tx: mpsc::Sender<io::Result<Vec<u8>>>,
}

impl io::Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Fails once the response is dropped (client disconnected);
        // that error is what makes the zip writer stop.
        self.tx
            .blocking_send(Ok(buf.to_vec()))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "zip receiver dropped"))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Stream one entry per selected file: open it, start the zip entry,
/// copy the bytes. The zip crate's stream writer defers sizes into data
/// descriptors, so nothing is seeked back over an already-flushed
/// channel; the whole archive never lives in memory.
///
/// Members were pre-screened for readability, so an open failure here
/// is a disk-level race between the check and this open. Aborting the
/// stream (an io error the caller turns into a body error) keeps the
/// loss visible instead of emitting a hole nobody can see.
fn write_zip_archive(
    entries: &[ArchiveEntry],
    tx: &mpsc::Sender<io::Result<Vec<u8>>>,
) -> io::Result<()> {
    let mut zip = ZipWriter::new_stream(ChannelWriter { tx: tx.clone() });

    // Zip64 so multi-gigabyte members don't corrupt the archive.
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .large_file(true);

    for entry in entries {
        // Open before starting the zip entry: a failure then aborts
        // without leaving a half-written entry behind.
        let mut file = std::fs::File::open(&entry.file.path)?;

        // Planning guarantees unique entry paths; the only errors left
        // come from the channel, which map onto io like any other sink.
        zip.start_file(&entry.entry_path, options)
            .map_err(|err| io::Error::other(err.to_string()))?;
        io::copy(&mut file, &mut zip)?;
    }

    // Writes the central directory: tiny, proportional to entry count.
    zip.finish()
        .map_err(|err| io::Error::other(err.to_string()))?;

    Ok(())
}

fn internal_error() -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, "could not read file\n").into_response()
}

/// `Content-Disposition` for a download. The plain `filename=` is the
/// percent-encoded name (fallback for old clients); `filename*=` is the
/// RFC 5987 form that lets modern browsers display the real Unicode name.
fn content_disposition(name: &str) -> HeaderValue {
    let encoded = percent_encode(name);

    HeaderValue::from_str(&format!(
        "attachment; filename=\"{encoded}\"; filename*=UTF-8''{encoded}"
    ))
    .expect("percent-encoded output is pure ASCII, always a valid header value")
}

/// Minimal percent-encoding: keep RFC 3986 unreserved characters,
/// encode every other byte as %XX. Output is always ASCII.
fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());

    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- percent_encode ---

    #[test]
    fn percent_encode_keeps_unreserved_ascii() {
        assert_eq!(percent_encode("report-2_final.txt"), "report-2_final.txt");
    }

    #[test]
    fn percent_encode_encodes_spaces_and_symbols() {
        assert_eq!(
            percent_encode("my file (v2).txt"),
            "my%20file%20%28v2%29.txt"
        );
    }

    #[test]
    fn percent_encode_encodes_utf8_as_bytes() {
        // 'é' is two bytes (C3 A9) in UTF-8
        assert_eq!(percent_encode("café.txt"), "caf%C3%A9.txt");
    }
}
