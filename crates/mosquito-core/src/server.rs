use std::collections::{HashMap, HashSet};
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

use crate::file_store::{FileId, FileStore, FolderProvenance, SharedFile};

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

/// Generic archive name whenever the single-root rule doesn't apply.
const GENERIC_ARCHIVE_NAME: &str = "mosquito-files.zip";

/// One selected file and the path it gets inside the archive.
struct ArchiveEntry {
    file: SharedFile,
    entry_path: String,
}

/// The complete archive layout: the name for Content-Disposition plus
/// entry paths in write order. Pure, so the layout rules are
/// unit-testable without touching disk or HTTP.
struct ArchivePlan {
    name: String,
    entries: Vec<ArchiveEntry>,
}

/// Derive the archive layout at write time from the stored frozen
/// snapshots (ADR-0001): folder members keep their root's name as a path
/// prefix, loose files sit at the archive root, and colliding paths get
/// numeric suffixes so nothing is dropped. Selection order = write order.
fn plan_archive(files: &[SharedFile]) -> ArchivePlan {
    let mut used = HashSet::new();
    let entries = files
        .iter()
        .map(|file| {
            let entry_path = unique_entry_path(&archive_entry_path(file), &mut used);
            ArchiveEntry {
                file: file.clone(),
                entry_path,
            }
        })
        .collect();

    ArchivePlan {
        name: archive_name(files),
        entries,
    }
}

/// Path inside the archive: `Photos/vacation/beach.jpg` for a member of
/// root `<...>/Photos`, the display name for a loose file. Forward
/// slashes are already guaranteed by the stored relative path, and a
/// directory name can't contain one.
fn archive_entry_path(file: &SharedFile) -> String {
    let Some(provenance) = &file.provenance else {
        return file.name.clone();
    };

    let root = provenance
        .folder_root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    // A root without a file name (a drive letter, say) has nothing to
    // prefix; members land at the archive root and collisions are
    // resolved like any other.
    if root.is_empty() {
        provenance.relative_path.clone()
    } else {
        format!("{root}/{}", provenance.relative_path)
    }
}

/// The first unused entry path, renaming collisions ` (2)`, ` (3)`, ...
/// before the extension (`beach.jpg` → `beach (2).jpg`). Same file added
/// twice and same-named roots both land here; nothing is dropped.
fn unique_entry_path(requested: &str, used: &mut HashSet<String>) -> String {
    let mut candidate = requested.to_string();
    let mut n = 2;
    while !used.insert(candidate.clone()) {
        candidate = with_numeric_suffix(requested, n);
        n += 1;
    }
    candidate
}

/// `dir/name.ext` → `dir/name (2).ext`. A leading dot is part of a
/// dotfile's name, not an extension (` .hidden` → `.hidden (2)`).
fn with_numeric_suffix(path: &str, n: usize) -> String {
    let (dir, name) = match path.rfind('/') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    };

    match name.rfind('.').filter(|&dot| dot > 0) {
        Some(dot) => format!("{dir}{} ({n}){}", &name[..dot], &name[dot..]),
        None => format!("{dir}{name} ({n})"),
    }
}

/// `<root-name>.zip` when every selected file belongs to exactly one
/// folder root; the generic name otherwise (loose files in the mix,
/// several roots, or an empty selection).
fn archive_name(files: &[SharedFile]) -> String {
    let mut root: Option<&std::path::Path> = None;
    for file in files {
        let Some(provenance) = &file.provenance else {
            return GENERIC_ARCHIVE_NAME.to_string();
        };
        match root {
            Some(seen) if *seen != provenance.folder_root => {
                return GENERIC_ARCHIVE_NAME.to_string();
            }
            _ => root = Some(&provenance.folder_root),
        }
    }

    match root.and_then(|root| root.file_name()) {
        Some(name) => format!("{}.zip", name.to_string_lossy()),
        None => GENERIC_ARCHIVE_NAME.to_string(),
    }
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

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;") // must be first, or we'd double-escape the rest
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Human-readable size, 1024-based (Windows Explorer style).
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    let (value, unit) = if bytes >= TB {
        (bytes as f64 / TB as f64, "TB")
    } else if bytes >= GB {
        (bytes as f64 / GB as f64, "GB")
    } else if bytes >= MB {
        (bytes as f64 / MB as f64, "MB")
    } else if bytes >= KB {
        (bytes as f64 / KB as f64, "KB")
    } else {
        return format!("{bytes} B");
    };

    format!("{value:.1} {unit}")
}

fn device_name() -> String {
    ["COMPUTERNAME", "HOSTNAME"]
        .into_iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .filter(|name| !name.trim().is_empty())
        })
        .unwrap_or_else(|| "Unknown device".to_string())
}

/// The page a phone sees after scanning the QR code.
///
/// Rendering mirrors the desktop list (`FileList.vue`): file checkboxes
/// plus a select-all use the app's checkbox styling, folder groups are
/// slim header rows with member rows below them, and the action button
/// where the app has "Delete selected" is the icon-only download button.
fn render_listing(files: &[SharedFile], device_name: &str) -> String {
    // Grouping is derived at render time (ADR-0001): loose files lead,
    // then one group per distinct folder root in first-appearance
    // order — the desktop list's exact layout.
    //
    /// One folder group's rendered header + member rows.
    struct GroupHtml {
        html: String,
    }

    let mut loose = String::new();
    let mut groups: Vec<GroupHtml> = Vec::new();
    // Keys are the root's lossy string form. Two distinct non-UTF-8
    // roots could collapse into one group; the desktop's string-keyed
    // `folderRoot` grouping is shaped the same, so parity keeps the two
    // pages aligned.
    let mut group_index: HashMap<String, usize> = HashMap::new();

    for file in files {
        match &file.provenance {
            None => loose.push_str(&file_row_html(file, None)),
            Some(provenance) => {
                let root = provenance.folder_root.to_string_lossy().into_owned();

                let index = match group_index.get(&root) {
                    Some(&index) => index,
                    None => {
                        let display_name = provenance
                            .folder_root
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let index = groups.len();
                        groups.push(GroupHtml {
                            html: format!(
                                "<li class=\"group-header\" role=\"row\" data-group-index=\"{index}\">\
<button type=\"button\" role=\"checkbox\" aria-checked=\"false\" aria-label=\"Select all files in {name}\" data-select-group=\"{index}\" hidden>\
{unchecked_svg}{checked_svg}\
</button>\
<span class=\"group-name\">{name}</span></li>",
                                name = escape_html(&display_name),
                                unchecked_svg = UNCHECKED_BOX_SVG,
                                checked_svg = CHECKED_BOX_SVG,
                            ),
                        });
                        group_index.insert(root, index);
                        index
                    }
                };

                groups[index].html.push_str(&file_row_html(
                    file,
                    Some((provenance, index)),
                ));
            }
        }
    }

    let mut items = loose;
    for group in groups {
        items.push_str(&group.html);
    }

    let (header, list_html) = if files.is_empty() {
        (
            "<p class=\"file-count\">Shared files</p>".to_string(),
            "<p class=\"empty-state\">Nothing is being shared right now.</p>".to_string(),
        )
    } else {
        (
            format!(
                "<div class=\"files-header\" role=\"row\">\
<button id=\"select-all\" type=\"button\" role=\"checkbox\" aria-checked=\"false\" aria-label=\"Select all shared files\" hidden>\
{unchecked}{checked}\
</button>\
<p class=\"file-count\">{count}</p>\
<a id=\"download-selected\" class=\"download-selected\" href=\"/download/zip\" aria-label=\"Download selected files\">\
{download_icon}\
</a>\
</div>",
                count = format!(
                    "{} file{} available",
                    files.len(),
                    if files.len() == 1 { "" } else { "s" }
                ),
                unchecked = UNCHECKED_BOX_SVG,
                checked = CHECKED_BOX_SVG,
                download_icon = DOWNLOAD_ICON_SVG,
            ),
            format!("<ul class=\"file-list\" role=\"rowgroup\">{items}</ul>"),
        )
    };

    include_str!("listing.html")
        .replace("{{LIST_HEADER}}", &header)
        .replace("{{DEVICE_NAME}}", &escape_html(device_name))
        .replace("{{FILE_CONTENT}}", &list_html)
}

/// One selectable file row: a plain download link (works without
/// JavaScript) wrapped around the name and size, with the selection
/// checkbox beside it — revealed only when JavaScript runs. Members of
/// a folder group pass their provenance plus the group's render-order
/// index, which links the row to its group's checkbox.
fn file_row_html(file: &SharedFile, group: Option<(&FolderProvenance, usize)>) -> String {
    // Inside a group the row shows where it hangs under the folder
    // root; loose files keep their display name (desktop
    // `memberTitle()`).
    let title = match group {
        Some((provenance, _)) => provenance.relative_path.as_str(),
        None => file.name.as_str(),
    };

    let group_attr = group
        .map(|(_, index)| format!(" data-group-index=\"{index}\""))
        .unwrap_or_default();

    format!(
        "<li class=\"file-row\" role=\"row\"{group_attr}>\
<button type=\"button\" role=\"checkbox\" aria-checked=\"false\" aria-label=\"Select {title}\" data-select=\"{id}\" hidden>\
{unchecked_svg}{checked_svg}\
</button>\
<a href=\"/download/{id}\"><span>{title}</span><small>{size}</small></a>\
</li>",
        id = file.id.0,
        title = escape_html(title),
        size = format_size(file.size),
        unchecked_svg = UNCHECKED_BOX_SVG,
        checked_svg = CHECKED_BOX_SVG,
    )
}

/// The app's checkbox artwork (desktop `square-rounded.svg`), inlined so
/// the listing stays a single self-contained page.
const UNCHECKED_BOX_SVG: &str = "<svg class=\"icon-unchecked\" xmlns=\"http://www.w3.org/2000/svg\" width=\"24\" height=\"24\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path stroke=\"none\" d=\"M0 0h24v24H0z\" fill=\"none\" /><path d=\"M12 3c7.2 0 9 1.8 9 9c0 7.2 -1.8 9 -9 9c-7.2 0 -9 -1.8 -9 -9c0 -7.2 1.8 -9 9 -9\" /></svg>";

const CHECKED_BOX_SVG: &str = "<svg class=\"icon-checked\" xmlns=\"http://www.w3.org/2000/svg\" width=\"24\" height=\"24\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path stroke=\"none\" d=\"M0 0h24v24H0z\" fill=\"none\" /><path d=\"M9 12l2 2l4 -4\" /><path d=\"M12 3c7.2 0 9 1.8 9 9c0 7.2 -1.8 9 -9 9c-7.2 0 -9 -1.8 -9 -9c0 -7.2 1.8 -9 9 -9\" /></svg>";

/// The project's download icon (desktop `download.svg`), for the
/// icon-only download button where the app has "Delete selected".
const DOWNLOAD_ICON_SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"24\" height=\"24\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path stroke=\"none\" d=\"M0 0h24v24H0z\" fill=\"none\" /><path d=\"M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2 -2v-2\" /><path d=\"M7 11l5 5l5 -5\" /><path d=\"M12 4l0 12\" /></svg>";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::file_store::FolderProvenance;

    fn shared_file(id: u64, name: &str, provenance: Option<FolderProvenance>) -> SharedFile {
        SharedFile {
            id: FileId(id),
            name: name.into(),
            path: PathBuf::from(format!("C:\\nowhere\\{name}")),
            size: 1,
            provenance,
        }
    }

    fn member(id: u64, name: &str, root: &str, relative_path: &str) -> SharedFile {
        shared_file(
            id,
            name,
            Some(FolderProvenance {
                folder_root: PathBuf::from(format!("C:\\nowhere\\{root}")),
                relative_path: relative_path.into(),
            }),
        )
    }

    // --- plan_archive: entry paths ---

    #[test]
    fn plan_puts_folder_members_under_the_root_name_and_loose_files_at_the_archive_root() {
        let files = vec![
            member(0, "a.txt", "Photos", "a.txt"),
            member(1, "b.txt", "Photos", "sub/b.txt"),
            shared_file(2, "c.txt", None),
        ];

        let plan = plan_archive(&files);

        let paths: Vec<_> = plan.entries.iter().map(|e| e.entry_path.as_str()).collect();
        assert_eq!(paths, ["Photos/a.txt", "Photos/sub/b.txt", "c.txt"]);
    }

    // --- plan_archive: collisions ---

    #[test]
    fn colliding_entry_paths_get_numeric_suffixes_before_the_extension() {
        let files = vec![
            shared_file(0, "a.txt", None),
            shared_file(1, "a.txt", None),
            shared_file(2, "a.txt", None),
        ];

        let plan = plan_archive(&files);

        let paths: Vec<_> = plan.entries.iter().map(|e| e.entry_path.as_str()).collect();
        assert_eq!(paths, ["a.txt", "a (2).txt", "a (3).txt"]);
    }

    #[test]
    fn collisions_suffix_the_file_name_and_keep_multi_part_extensions() {
        let files = vec![
            member(0, "a.jpg", "Photos", "vacation/beach.jpg"),
            member(1, "b.jpg", "photos", "trip/beach.jpg"),
            shared_file(2, "archive.tar.gz", None),
            shared_file(3, "archive.tar.gz", None),
            shared_file(4, ".hidden", None),
            shared_file(5, ".hidden", None),
        ];

        let plan = plan_archive(&files);

        let paths: Vec<_> = plan.entries.iter().map(|e| e.entry_path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "Photos/vacation/beach.jpg",
                "photos/trip/beach.jpg",
                "archive.tar.gz",
                "archive.tar (2).gz",
                ".hidden",
                ".hidden (2)",
            ]
        );
    }

    // --- plan_archive: archive name ---

    #[test]
    fn single_root_selection_names_the_archive_after_the_root() {
        let files = vec![
            member(0, "a.txt", "Photos", "a.txt"),
            member(1, "b.txt", "Photos", "sub/b.txt"),
        ];

        let plan = plan_archive(&files);

        assert_eq!(plan.name, "Photos.zip");
    }

    #[test]
    fn mixed_selections_fall_back_to_the_generic_archive_name() {
        let loose_plus_one_root = vec![
            member(0, "a.txt", "Photos", "a.txt"),
            shared_file(1, "c.txt", None),
        ];
        assert_eq!(
            plan_archive(&loose_plus_one_root).name,
            "mosquito-files.zip"
        );

        let two_roots = vec![
            member(0, "a.txt", "Photos", "a.txt"),
            member(1, "b.txt", "Docs", "b.txt"),
        ];
        assert_eq!(plan_archive(&two_roots).name, "mosquito-files.zip");

        let all_loose = vec![shared_file(0, "a.txt", None)];
        assert_eq!(plan_archive(&all_loose).name, "mosquito-files.zip");
    }

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

    // --- escape_html ---

    #[test]
    fn escape_html_escapes_all_specials() {
        assert_eq!(escape_html(r#"&<>"'"#), "&amp;&lt;&gt;&quot;&#39;");
    }

    #[test]
    fn escape_html_ampersand_first_never_double_escapes() {
        assert_eq!(escape_html("&lt;"), "&amp;lt;");
    }

    // --- format_size ---

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
    }

    #[test]
    fn format_size_kilobytes() {
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
    }

    #[test]
    fn format_size_megabytes_and_gigabytes() {
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    // --- render_listing ---

    #[test]
    fn listing_shows_empty_message_without_links() {
        let html = render_listing(&[], "test-device");

        assert!(html.contains("Nothing is being shared"));
        assert!(!html.contains("<li>"));
        assert!(html.contains("Shared from test-device"));
        assert!(html.contains("https://github.com/DylanAlmond/mosquito"));
        assert!(!html.contains("{{"));
    }

    #[test]
    fn listing_loose_files_come_before_folder_groups() {
        let files = vec![
            member(0, "b.txt", "Photos", "sub/b.txt"),
            shared_file(1, "a.txt", None),
            member(2, "a.txt", "Photos", "a.txt"),
        ];

        let html = render_listing(&files, "test-device");

        // Loose files lead, ungrouped; each folder group appears at its
        // first member's position (first-appearance order).
        let loose = html.find("href=\"/download/1\"").unwrap();
        let group = html.find("class=\"group-header\"").unwrap();
        let member = html.find("sub/b.txt").unwrap();
        assert!(loose < group, "loose files first");
        assert!(group < member, "grouped members under their header");
    }

    #[test]
    fn listing_escapes_folder_root_names_and_shows_their_members_flush_left() {
        let files = vec![
            member(0, "a.txt", "<script>", "vacation/beach.jpg"),
            shared_file(1, "b.txt", None),
            member(2, "b.txt", "<script>", "trip/notes.txt"),
        ];

        let html = render_listing(&files, "test-device");

        // The display name is the root's own name, escaped like any text.
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<span class=\"group-name\"><script>"));
        // Both members belong to the same root, so exactly one header
        // row carries both; member rows are keyed for the group toggle.
        assert_eq!(
            html.matches("data-select-group=\"").count(),
            1,
            "one header per distinct folder root"
        );
        // 1 header + 2 members: members reference their group.
        assert_eq!(html.matches("data-group-index=\"0\"").count(), 3);
    }

    #[test]
    fn listing_renders_desktop_style_selection_ui() {
        let files = vec![shared_file(0, "a.txt", None)];

        let html = render_listing(&files, "test-device");

        // Select-all checkbox in the header, per-file checkboxes in rows,
        // using the app's own square-rounded checkbox artwork.
        assert!(html.contains("aria-label=\"Select all shared files\""));
        assert!(html.contains("data-select=\"0\" hidden"));
        assert!(html.contains("icon-checked"));
        // The download button starts hidden: selection is empty and the
        // inline script only reveals it once something is selected.
        assert!(html.contains("id=\"download-selected\" class=\"download-selected\""));
        assert!(!html.contains("download-selected is-visible"));
        // Selection is wired by an inline vanilla script: no framework
        // and no build step anywhere in the page.
        assert!(html.contains("<script>"));
        assert!(html.contains("addEventListener"));
        // Plain per-file download links remain, so downloads work with
        // JavaScript disabled.
        assert!(html.contains("href=\"/download/0\""));
    }

    #[test]
    fn listing_escapes_names_and_links_ids() {
        let files = vec![SharedFile {
            id: FileId(0),
            name: "<script>.txt".into(),
            path: "unused".into(),
            size: 42,
            provenance: None,
        }];

        let html = render_listing(&files, "test-device");

        assert!(html.contains("&lt;script&gt;.txt"));
        assert!(html.contains("href=\"/download/0\""));
        assert!(html.contains("42 B"));
        // Narrowed to the raw name: the page now legitimately contains
        // its own enhancement <script>, so the whole-document check the
        // old assertion made no longer applies.
        assert!(!html.contains("<script>.txt"));
        assert!(
            html.find("class=\"file-count\"").unwrap() < html.find("class=\"file-list\"").unwrap()
        );
        assert!(html.contains("overflow-y: auto"));
    }

    #[test]
    fn listing_escapes_the_device_name() {
        let html = render_listing(&[], "<script>host</script>");

        assert!(html.contains("Shared from &lt;script&gt;host&lt;/script&gt;"));
        assert!(!html.contains("<script>host</script>"));
    }
}
