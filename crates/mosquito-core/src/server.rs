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
use crate::archive_subtree::collect_subtree;
use crate::error::RemoveError;
use crate::file_store::{FileId, FileStore, RootId, SharedFile};
use crate::listing::{default_expansions, device_name, render_node_fragment, render_top_listing};
use crate::listing_pages::LISTING_PAGE_ROWS;

/// State shared by every handler. Cheap to clone (it's an Arc).
#[derive(Clone)]
pub struct AppState {
    store: Arc<Mutex<FileStore>>,
}

impl AppState {
    pub fn new(store: Arc<Mutex<FileStore>>) -> Self {
        Self { store }
    }

    /// The shared store, for callers that must inspect or hold the
    /// underlying store directly (harnesses, tests).
    pub fn store(&self) -> &Arc<Mutex<FileStore>> {
        &self.store
    }
}

/// Build the app's routes. Tests and `main` both go through this.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(listing))
        .route("/node/{root_id}", get(node_page))
        .route("/download/zip", get(download_zip))
        .route("/download/zip/node/{root_id}", get(download_node_zip))
        .route("/download/{id}", get(download))
        .route("/assets/dynapuff-400.woff2", get(font_regular))
        .route("/assets/dynapuff-600.woff2", get(font_semibold))
        .route("/assets/favicon.ico", get(favicon))
        .with_state(state)
}

async fn listing(State(state): State<AppState>) -> impl IntoResponse {
    // One brief lock to copy the whole derivation input: the top
    // listing plus the root records (the budget rule needs their
    // paths, which `RootRow` deliberately doesn't carry). The lock
    // drops before any scan — a request never holds it across a
    // listing-wide pass (ADR-0006).
    let (top, records) = {
        let store = state.store.lock().expect("file store not poisoned");
        (store.list_top(), store.roots().to_vec())
    };
    debug_assert_eq!(
        top.roots.len(),
        records.len(),
        "the top listing derives from the same root records"
    );

    // The five-children stamps scan the disks' directories count-only,
    // no lock held: displaying never registers content (issue #24).
    let expansions = default_expansions(&records);

    let html = render_top_listing(&top, &expansions, &device_name());

    ([(CONTENT_TYPE, "text/html; charset=utf-8")], html)
}

/// `GET /node/{root_id}?sub=<path>&offset=<n>` — one node's page as an
/// HTML fragment (ADR-0006): the request itself materializes the
/// node's direct children (ADR-0005: the listing showing a folder is
/// the trigger) and derives up to the page budget's rows, so repeat
/// fetches never duplicate registrations. `limit` exists so tests and
/// tooling can window the page; production fetches take the default.
///
/// The fragment wraps its rows in an inert `<template>` carrying the
/// window's paging state. An unknown root id, or a sub path that
/// escapes the root record, answers 404 like any unknown address —
/// listing never widens a caller's reach.
async fn node_page(
    State(state): State<AppState>,
    Path(root_id): Path<u64>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let sub_path = query.get("sub").cloned().unwrap_or_default();
    let offset = query
        .get("offset")
        .and_then(|offset| offset.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = query
        .get("limit")
        .and_then(|limit| limit.parse::<usize>().ok())
        .unwrap_or(LISTING_PAGE_ROWS)
        .min(LISTING_PAGE_ROWS);

    // One lock for one node's materialization and derivation plus the
    // key lookup — the page-budget window's worth of work, and the
    // lock goes down with the guard (no `.await` inside).
    let response = {
        let mut store = state.store.lock().expect("file store not poisoned");
        match store.list_page(RootId(root_id), &sub_path, offset, limit) {
            Ok(page) => {
                // The fragment stamps the node's `list_top` key, so the
                // page's rows key identically wherever they render.
                let root_key = store
                    .list_top()
                    .roots
                    .into_iter()
                    .find(|root| root.id == RootId(root_id))
                    .map(|root| root.key)
                    .unwrap_or_default();
                let html = render_node_fragment(&page, &root_key);
                ([(CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response()
            }
            Err(RemoveError::RootNotFound(_) | RemoveError::SubPathEscapesRoot(_)) => not_found(),
            // `list_page` only errors per root-id/sub-path; the file-id
            // error arm is unreachable here.
            Err(err @ RemoveError::NotFound(_)) => {
                unreachable!("list_page never answers with a file id error: {err}")
            }
        }
    };

    response
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

    streamed_zip_response(&selected)
}

/// `GET /download/zip/node/{root_id}?sub=<path>` — the node-keyed zip
/// route (ADR-0005): the addressed node's whole subtree, materialized
/// through the store's idempotent key and streamed through the same
/// planner and writer the id-keyed route uses. `sub` is absent for the
/// root record's own node. Malformed or unknown node keys, and sub
/// paths escaping the root record, answer not-found like any unknown
/// address — the route never widens a caller's reach.
///
/// The walk runs on the blocking pool: whole-drive requests may take
/// a long while, and no listing should stand behind them (each step
/// takes the store lock for one directory scan at most, so the walk
/// itself doesn't hold the UI back either — [`crate::archive_subtree`]).
async fn download_node_zip(
    State(state): State<AppState>,
    Path(root_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    // Hand-parsed, unlike the sibling node page's extracted `u64`: the
    // ticket pins malformed node keys to not-found, and axum's failed
    // extractor answers 400. `Path<String>` keeps that in the handler.
    let Ok(root_id) = root_id.parse::<u64>().map(RootId) else {
        return not_found();
    };
    let sub_path = query.get("sub").cloned().unwrap_or_default();

    let selected = match tokio::task::spawn_blocking({
        let store = Arc::clone(&state.store);
        move || collect_subtree(&store, root_id, &sub_path)
    })
    .await
    {
        Ok(selected) => selected,
        // The walk itself ran off the async reactor; a panic there is a
        // server bug, reported like any other reading failure.
        Err(join) => {
            eprintln!("subtree walk failed: {join}");
            return internal_error();
        }
    };

    match selected {
        Ok(selected) => streamed_zip_response(&selected),
        Err(RemoveError::RootNotFound(_) | RemoveError::SubPathEscapesRoot(_)) => not_found(),
        Err(err @ RemoveError::NotFound(_)) => {
            unreachable!("collect_subtree never answers with a file id error: {err}")
        }
    }
}

/// The archive response both zip routes end in: vanished members are
/// screened up front (a dangling member is visible as a failure, not
/// a hole in a partial archive, per ADR-0001), the layout is planned
/// — entry paths, collision suffixes, single-root archive naming —
/// and the bytes stream out entry-by-entry, so multi-gigabyte
/// selections never sit in memory: each member is read and
/// compressed only when the client reaches it.
fn streamed_zip_response(selected: &[SharedFile]) -> Response {
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
    let ArchivePlan { name, entries } = plan_archive(selected);

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
