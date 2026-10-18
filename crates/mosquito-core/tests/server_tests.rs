//! HTTP server integration tests.
//!
//! `tower::ServiceExt::oneshot` invokes the router directly, in-process:
//! no listener, no port, no network. Requests go in, responses come out.

use std::io::Cursor;
use std::io::Read;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_LENGTH};
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

/// Collects a /download/zip response body and parses it as an archive,
/// so assertions speak in entries and contents rather than zip bytes.
async fn parse_zip(response: axum::response::Response) -> zip::ZipArchive<Cursor<Vec<u8>>> {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    zip::ZipArchive::new(Cursor::new(bytes.to_vec())).unwrap()
}

fn entry_names(archive: &mut zip::ZipArchive<Cursor<Vec<u8>>>) -> Vec<String> {
    (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().unwrap().into_owned())
        .collect()
}

fn entry_content(archive: &mut zip::ZipArchive<Cursor<Vec<u8>>>, name: &str) -> String {
    let mut content = String::new();
    archive
        .by_name(name)
        .unwrap()
        .read_to_string(&mut content)
        .unwrap();
    content
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
    assert!(html.contains("font-family: 'DynaPuff'"));
    assert!(html.contains("border-radius: 0.5rem"));
    assert!(html.contains("class=\"file-row\""));
    assert!(html.contains("Shared from "));
    assert!(html.contains("https://github.com/DylanAlmond/mosquito"));
}

#[tokio::test]
async fn hosted_page_serves_the_app_fonts_locally() {
    let app = router(AppState::new(Arc::new(Mutex::new(FileStore::new()))));

    for uri in ["/assets/dynapuff-400.woff2", "/assets/dynapuff-600.woff2"] {
        let response = get(app.clone(), uri).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "font/woff2");

        let font = response.into_body().collect().await.unwrap().to_bytes();
        assert!(font.starts_with(b"wOF2"));
    }
}

#[tokio::test]
async fn hosted_page_serves_the_favicon_locally() {
    let app = router(AppState::new(Arc::new(Mutex::new(FileStore::new()))));

    let response = get(app, "/assets/favicon.ico").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/x-icon");

    let icon = response.into_body().collect().await.unwrap().to_bytes();
    assert!(icon.starts_with(&[0x00, 0x00, 0x01, 0x00]));
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

/// Store over loose files created in a fresh temp dir.
fn store_with(dir: &TempDir, files: &[(&str, &[u8])]) -> FileStore {
    let mut store = FileStore::new();
    for (name, contents) in files {
        std::fs::write(dir.path().join(name), contents).unwrap();
        store.add(dir.path().join(name)).unwrap();
    }
    store
}

#[tokio::test]
async fn zip_download_returns_valid_archive_of_selected_loose_files() {
    let dir = TempDir::new().unwrap();
    let state = AppState::new(Arc::new(Mutex::new(store_with(
        &dir,
        &[
            ("a.txt", b"alpha".as_slice()),
            ("b.txt", b"beta".as_slice()),
        ],
    ))));
    let app = router(state);

    let response = get(app, "/download/zip?ids=0,1").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/zip");

    let mut archive = parse_zip(response).await;
    assert_eq!(entry_names(&mut archive), ["a.txt", "b.txt"]);
    assert_eq!(entry_content(&mut archive, "a.txt"), "alpha");
    assert_eq!(entry_content(&mut archive, "b.txt"), "beta");
}

#[tokio::test]
async fn zip_download_groups_folder_members_under_the_root_name() {
    let photos = TempDir::new().unwrap();
    std::fs::create_dir_all(photos.path().join("Photos").join("sub")).unwrap();
    std::fs::write(photos.path().join("Photos").join("a.txt"), b"alpha").unwrap();
    std::fs::write(
        photos.path().join("Photos").join("sub").join("b.txt"),
        b"beta",
    )
    .unwrap();

    let loose = TempDir::new().unwrap();

    let mut store = FileStore::new();
    let outcome = store.add_directory(photos.path().join("Photos"));
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    std::fs::write(loose.path().join("c.txt"), b"gamma").unwrap();
    store.add(loose.path().join("c.txt")).unwrap();

    let app = router(AppState::new(Arc::new(Mutex::new(store))));

    // No ids: everything currently shared joins the archive.
    let response = get(app, "/download/zip").await;

    assert_eq!(response.status(), StatusCode::OK);

    let mut archive = parse_zip(response).await;
    assert_eq!(
        entry_names(&mut archive),
        ["Photos/a.txt", "Photos/sub/b.txt", "c.txt"]
    );
    assert_eq!(entry_content(&mut archive, "Photos/sub/b.txt"), "beta");
}

#[tokio::test]
async fn zip_download_suffixes_colliding_paths_but_drops_nothing() {
    let dir = TempDir::new().unwrap();
    let mut store = FileStore::new();
    std::fs::write(dir.path().join("bite.txt"), b"same bytes").unwrap();
    // The same file added twice keeps two ids — collisions are
    // resolved at archive-write time only (ADR-0001).
    store.add(dir.path().join("bite.txt")).unwrap();
    store.add(dir.path().join("bite.txt")).unwrap();

    let app = router(AppState::new(Arc::new(Mutex::new(store))));

    let response = get(app, "/download/zip?ids=0,1").await;

    let mut archive = parse_zip(response).await;
    let names = entry_names(&mut archive);
    assert_eq!(names, ["bite.txt", "bite (2).txt"]);
    for name in names {
        assert_eq!(entry_content(&mut archive, &name), "same bytes");
    }
}

#[tokio::test]
async fn zip_download_with_two_folder_roots_prefixes_each_by_its_own_root() {
    let temp = TempDir::new().unwrap();
    let mut store = FileStore::new();
    for root_name in ["Photos", "Docs"] {
        let root = temp.path().join(root_name);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), root_name.as_bytes()).unwrap();
        let outcome = store.add_directory(&root);
        assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    }

    let app = router(AppState::new(Arc::new(Mutex::new(store))));

    let response = get(app, "/download/zip?ids=0,1").await;

    assert_eq!(response.status(), StatusCode::OK);
    let disposition = response.headers()[CONTENT_DISPOSITION].to_str().unwrap();
    assert!(
        disposition.starts_with("attachment; filename=\"mosquito-files.zip\""),
        "{disposition}"
    );
    let mut archive = parse_zip(response).await;
    assert_eq!(entry_names(&mut archive), ["Photos/a.txt", "Docs/a.txt"]);
}

#[tokio::test]
async fn zip_download_with_two_roots_sharing_a_name_resolves_all_collisions() {
    let temp = TempDir::new().unwrap();
    let mut store = FileStore::new();
    for (n, root_name) in ["Photos", "Photos"].iter().enumerate() {
        let root = temp.path().join(format!("root{n}")).join(root_name);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), n.to_string().as_bytes()).unwrap();
        let outcome = store.add_directory(&root);
        assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    }

    let app = router(AppState::new(Arc::new(Mutex::new(store))));

    let response = get(app, "/download/zip?ids=0,1").await;

    assert_eq!(response.status(), StatusCode::OK);
    let mut archive = parse_zip(response).await;
    // Every member collides under the shared root name; each keeps its
    // content under the first unused entry path.
    assert_eq!(
        entry_names(&mut archive),
        ["Photos/a.txt", "Photos/a (2).txt"]
    );
    assert_eq!(entry_content(&mut archive, "Photos/a.txt"), "0");
    assert_eq!(entry_content(&mut archive, "Photos/a (2).txt"), "1");
}

#[tokio::test]
async fn zip_download_names_the_archive_after_a_single_root() {
    let photos = TempDir::new().unwrap();
    std::fs::create_dir_all(photos.path().join("Photos")).unwrap();
    std::fs::write(photos.path().join("Photos").join("a.txt"), b"alpha").unwrap();

    let mut store = FileStore::new();
    let outcome = store.add_directory(photos.path().join("Photos"));
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);

    let app = router(AppState::new(Arc::new(Mutex::new(store))));

    let response = get(app, "/download/zip?ids=0").await;

    let disposition = response.headers()[CONTENT_DISPOSITION].to_str().unwrap();
    assert!(
        disposition.starts_with("attachment; filename=\"Photos.zip\""),
        "{disposition}"
    );

    // Drain the body like a client reading to EOF, so the streaming
    // writer finishes cleanly instead of logging a broken pipe.
    let _ = response.into_body().collect().await;
}

#[tokio::test]
async fn zip_download_of_a_unicode_root_encodes_the_archive_name() {
    let photos = TempDir::new().unwrap();
    std::fs::create_dir_all(photos.path().join("caf\u{e9}")).unwrap();
    std::fs::write(photos.path().join("caf\u{e9}").join("a.txt"), b"alpha").unwrap();

    let mut store = FileStore::new();
    let outcome = store.add_directory(photos.path().join("caf\u{e9}"));
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);

    let app = router(AppState::new(Arc::new(Mutex::new(store))));

    let response = get(app, "/download/zip?ids=0").await;

    let disposition = response.headers()[CONTENT_DISPOSITION].to_str().unwrap();
    assert!(
        disposition.contains("filename*=UTF-8''caf%C3%A9.zip"),
        "{disposition}"
    );
    assert!(!disposition.contains('\u{e9}'));

    // Drain like a real client, so the streaming writer ends cleanly.
    let _ = response.into_body().collect().await;
}

#[tokio::test]
async fn zip_download_with_unknown_id_is_404() {
    let dir = TempDir::new().unwrap();
    let state = AppState::new(Arc::new(Mutex::new(store_with(&dir, &[("a.txt", b"a")]))));
    let app = router(state);

    let response = get(app, "/download/zip?ids=0,999").await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn zip_download_with_malformed_ids_is_404() {
    let dir = TempDir::new().unwrap();
    let state = AppState::new(Arc::new(Mutex::new(store_with(&dir, &[("a.txt", b"a")]))));
    let app = router(state);

    for uri in [
        "/download/zip?ids=abc",
        "/download/zip?ids=0,",
        "/download/zip?ids=",
    ] {
        let response = get(app.clone(), uri).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn zip_download_streams_large_content_intact_with_no_length_header() {
    use std::io::Read;
    // Incompressible data: the archive ends up as large as the source,
    // so a full 4 MiB really does flow through the streamed body —
    // contents surviving intact is the streaming evidence.
    let mut contents = Vec::with_capacity(4 * 1024 * 1024);
    let mut state = 0x2545_F491_4F6C_DD1Du64; // xorshift64 bit mixer
    for _ in 0..4 * 1024 * 1024 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        contents.push(state as u8);
    }

    let temp = TempDir::new().unwrap();
    std::fs::write(temp.path().join("big.bin"), &contents).unwrap();
    let mut store = FileStore::new();
    store.add(temp.path().join("big.bin")).unwrap();

    let app = router(AppState::new(Arc::new(Mutex::new(store))));

    let response = get(app, "/download/zip?ids=0").await;

    assert_eq!(response.status(), StatusCode::OK);
    // A streamed archive can't know its length up front: no
    // Content-Length, so the connection stays chunked end-to-end.
    assert!(response.headers().get(CONTENT_LENGTH).is_none());

    let mut archive = parse_zip(response).await;
    assert_eq!(archive.len(), 1);
    let mut extracted = Vec::new();
    archive
        .by_name("big.bin")
        .unwrap()
        .read_to_end(&mut extracted)
        .unwrap();
    assert_eq!(extracted, contents);
}

#[tokio::test]
async fn zip_download_with_nothing_shared_is_a_valid_empty_archive() {
    let app = router(AppState::new(Arc::new(Mutex::new(FileStore::new()))));

    let response = get(app, "/download/zip").await;

    assert_eq!(response.status(), StatusCode::OK);
    let archive = parse_zip(response).await;
    assert_eq!(archive.len(), 0);
}

#[tokio::test]
async fn zip_download_with_a_vanished_member_is_404() {
    let dir = TempDir::new().unwrap();
    let state = AppState::new(Arc::new(Mutex::new(store_with(
        &dir,
        &[
            ("a.txt", b"alpha".as_slice()),
            ("b.txt", b"beta".as_slice()),
        ],
    ))));

    // A member can dangle: the frozen snapshot keeps it in the store
    // while its content is gone from disk (ADR-0001).
    std::fs::remove_file(dir.path().join("b.txt")).unwrap();

    let app = router(state);

    let response = get(app, "/download/zip").await;

    // Visible as a failure, not an archive with a silent hole.
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
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
