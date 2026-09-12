use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tokio::fs::File;
use tokio_util::io::ReaderStream;

use crate::file_store::{FileId, FileStore, SharedFile};

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
        .route("/download/{id}", get(download))
        .with_state(state)
}

async fn listing(State(state): State<AppState>) -> impl IntoResponse {
    // The pattern used everywhere in this app: lock, copy what we need,
    // drop the lock — never hold a std Mutex guard across an `.await`.
    let files = {
        let store = state.store.lock().expect("file store not poisoned");
        store.list().to_vec()
    };

    let html = render_listing(&files);

    ([(CONTENT_TYPE, "text/html; charset=utf-8")], html)
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

/// The page a phone sees after scanning the QR code.
/// Hand-rolled HTML — one page doesn't justify a templating crate.
fn render_listing(files: &[SharedFile]) -> String {
    let items: String = files
        .iter()
        .map(|f| {
            format!(
                "<li><a href=\"/download/{id}\">{name}</a> <small>({size})</small></li>",
                id = f.id.0,
                name = escape_html(&f.name),
                size = format_size(f.size),
            )
        })
        .collect();

    let body = if files.is_empty() {
        "<p>Nothing is being shared right now.</p>".to_string()
    } else {
        format!(
            "<p>{count} file{s} available:</p><ul>{items}</ul>",
            count = files.len(),
            s = if files.len() == 1 { "" } else { "s" },
        )
    };

    format!(
        "<!DOCTYPE html>\
         <html lang=\"en\">\
         <head>\
           <meta charset=\"utf-8\">\
           <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
           <title>Mosquito</title>\
         </head>\
         <body>\
           <h1>Mosquito</h1>\
           {body}\
         </body>\
         </html>"
    )
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
        let html = render_listing(&[]);

        assert!(html.contains("Nothing is being shared"));
        assert!(!html.contains("<li>"));
    }

    #[test]
    fn listing_escapes_names_and_links_ids() {
        let files = vec![SharedFile {
            id: FileId(0),
            name: "<script>.txt".into(),
            path: "unused".into(),
            size: 42,
        }];

        let html = render_listing(&files);

        assert!(html.contains("&lt;script&gt;.txt"));
        assert!(html.contains("href=\"/download/0\""));
        assert!(html.contains("42 B"));
        assert!(!html.contains("<script>"));
    }
}
