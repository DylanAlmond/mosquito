//! HTTP server integration tests.
//!
//! `tower::ServiceExt::oneshot` invokes the router directly, in-process:
//! no listener, no port, no network. Requests go in, responses come out.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::header::CONTENT_DISPOSITION;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mosquito_core::{AppState, FileId, FileStore, router};
use tempfile::TempDir;
use tower::ServiceExt;

/// Adds one file to a fresh store. Returns the temp dir (keep it alive
/// for the whole test — it self-deletes when dropped) plus the state.
fn state_with_file(name: &str, contents: &[u8]) -> (TempDir, AppState) {
    let dir = TempDir::new().unwrap();

    std::fs::write(dir.path().join(name), contents).unwrap();

    let mut store = FileStore::new();
    store.add(dir.path().join(name)).unwrap();

    (dir, AppState::new(Arc::new(Mutex::new(store))))
}

async fn get(app: axum::Router, uri: &str) -> axum::response::Response {
    let request = Request::builder().uri(uri).body(Body::empty()).unwrap();
    app.oneshot(request).await.unwrap()
}

async fn body_text(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn root_page_lists_files_with_links() {
    let (_dir, state) = state_with_file("hello.txt", b"hi");
    let app = router(state);

    let response = get(app, "/").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "text/html; charset=utf-8"
    );

    let html = body_text(response).await;

    assert!(html.contains("hello.txt"));
    assert!(html.contains("href=\"/download/0\""));
    assert!(html.contains("2 B"));
}

#[tokio::test]
async fn root_page_with_no_files_shows_empty_message() {
    let app = router(AppState::new(Arc::new(Mutex::new(FileStore::new()))));

    let response = get(app, "/").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        body_text(response)
            .await
            .contains("Nothing is being shared")
    );
}

#[tokio::test]
async fn root_page_lists_all_files_in_order() {
    let dir = TempDir::new().unwrap();

    std::fs::write(dir.path().join("a.txt"), b"a").unwrap();
    std::fs::write(dir.path().join("b.txt"), b"bb").unwrap();

    let mut store = FileStore::new();
    store.add(dir.path().join("a.txt")).unwrap();
    store.add(dir.path().join("b.txt")).unwrap();

    let app = router(AppState::new(Arc::new(Mutex::new(store))));

    let html = body_text(get(app, "/").await).await;

    let a = html.find("a.txt").unwrap();
    let b = html.find("b.txt").unwrap();

    assert!(a < b);
}

#[tokio::test]
async fn download_returns_exact_bytes() {
    let contents = b"mosquito bites are tiny but mighty";
    let (_dir, state) = state_with_file("bite.txt", contents);
    let app = router(state);

    let response = get(app, "/download/0").await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();

    assert_eq!(body.as_ref(), contents);
}

#[tokio::test]
async fn download_streams_binary_content_intact() {
    let contents = vec![0xABu8; 4096];
    let (_dir, state) = state_with_file("blob.bin", &contents);
    let app = router(state);

    let response = get(app, "/download/0").await;

    assert_eq!(response.status(), StatusCode::OK);

    let headers = response.headers();

    assert_eq!(headers["content-type"], "application/octet-stream");
    assert_eq!(headers["content-length"], "4096");

    let disposition = headers[CONTENT_DISPOSITION].to_str().unwrap();

    assert!(disposition.starts_with("attachment; filename=\"blob.bin\""));

    let body = response.into_body().collect().await.unwrap().to_bytes();

    assert_eq!(body.len(), 4096);
    assert!(body.iter().all(|&b| b == 0xAB));
}

#[tokio::test]
async fn download_with_unicode_name_encodes_header() {
    let (_dir, state) = state_with_file("café.txt", b"x");
    let app = router(state);

    let response = get(app, "/download/0").await;

    let disposition = response.headers()[CONTENT_DISPOSITION].to_str().unwrap();
    assert!(disposition.contains("filename*=UTF-8''caf%C3%A9.txt"));
    // Header values must stay ASCII — the raw é must not leak through.
    assert!(!disposition.contains('\u{e9}'));
}

#[tokio::test]
async fn download_empty_file() {
    let (_dir, state) = state_with_file("empty.txt", b"");
    let app = router(state);

    let response = get(app, "/download/0").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-length"], "0");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert!(body.is_empty());
}

#[tokio::test]
async fn download_unknown_id_is_404() {
    let (_dir, state) = state_with_file("hello.txt", b"hi");
    let app = router(state);

    let response = get(app, "/download/999").await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn download_removed_file_is_404() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("gone.txt"), b"bye").unwrap();
    let store = Arc::new(Mutex::new(FileStore::new()));
    store
        .lock()
        .unwrap()
        .add(dir.path().join("gone.txt"))
        .unwrap();
    let state = AppState::new(Arc::clone(&store));

    store.lock().unwrap().remove(FileId(0)).unwrap();

    let response = get(router(state), "/download/0").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
