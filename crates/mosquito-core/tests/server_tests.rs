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

/// A sync helper so a store assertion never holds the mutex across a
/// test's awaits: the guard lives inside the non-async call only.
fn assert_registered_members(state: &AppState, expected: usize, message: &str) {
    let store = state.store().lock().expect("file store not poisoned");
    assert_eq!(store.list().len(), expected, "{message}");
}

/// Zero or exactly-one assertions read in words, not raw counts: the
/// store never registers one disk file twice.
fn member_is_registered(state: &AppState, name: &str, expected: usize) {
    let store = state.store().lock().expect("file store not poisoned");
    let count = store.list().iter().filter(|file| file.name == name).count();
    assert_eq!(count, expected, "{name}");
}

/// Deny read access to a directory, so unreadable-member behavior can
/// be exercised over HTTP. Returns None where the environment won't
/// cooperate, so the test can skip; access is restored on drop.
/// (Integration tests can't reach the crate-internal fixtures.)
enum SealedDir {
    #[cfg(windows)]
    Windows { path: std::path::PathBuf },
    #[cfg(unix)]
    Unix { path: std::path::PathBuf },
}

fn seal_directory(path: &std::path::Path) -> Option<SealedDir> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let deny = std::process::Command::new("icacls")
            .arg(path)
            .args(["/deny", "*S-1-1-0:(OI)(CI)R"])
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .ok()?
            .success();
        deny.then(|| SealedDir::Windows {
            path: path.to_path_buf(),
        })
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0)).ok()?;
        // Privileges may retain access (e.g. running as root): the try
        // must be verified, not assumed.
        if std::fs::read_dir(path).is_ok() {
            restore_unix(path);
            return None;
        }
        Some(SealedDir::Unix {
            path: path.to_path_buf(),
        })
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = path;
        None
    }
}

#[cfg(unix)]
fn restore_unix(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
}

#[cfg(unix)]
impl Drop for SealedDir {
    fn drop(&mut self) {
        match self {
            SealedDir::Unix { path } => restore_unix(path),
        }
    }
}

#[cfg(windows)]
impl Drop for SealedDir {
    fn drop(&mut self) {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let path = match self {
            SealedDir::Windows { path } => path,
        };
        let _ = std::process::Command::new("icacls")
            .arg(path)
            .args(["/reset", "/t", "/q"])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }
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

/// A store seeded the way imports are now: loose files added
/// directly, then a folder root as a count-only record — its members
/// stay lazily-known until a listing shows them.
fn state_with_root_record() -> (TempDir, AppState) {
    let dir = TempDir::new().unwrap();
    let folder = dir.path().join("Photographs");
    std::fs::create_dir_all(folder.join("notes")).unwrap();

    std::fs::write(dir.path().join("recipes.txt"), b"r").unwrap();
    std::fs::write(dir.path().join("checklist.txt"), b"c").unwrap();
    std::fs::write(folder.join("beach.jpg"), b"beach").unwrap();
    std::fs::write(folder.join("notes").join("todo.txt"), b"todo").unwrap();

    let mut store = FileStore::new();
    store.add(dir.path().join("recipes.txt")).unwrap();
    store.add(dir.path().join("checklist.txt")).unwrap();
    store.commit_root_record(&folder, 2);

    (dir, AppState::new(Arc::new(Mutex::new(store))))
}

#[tokio::test]
async fn root_page_renders_root_records_without_any_member_rows() {
    let (_dir, state) = state_with_root_record();
    let app = router(state);

    let response = get(app, "/").await;

    assert_eq!(response.status(), StatusCode::OK);
    let html = body_text(response).await;

    // Loose files first with their download links.
    assert!(html.contains("href=\"/download/0\""));
    assert!(html.contains("href=\"/download/1\""));

    // One row per root record: name plus import-time count, no
    // sub-page rendering, no path leak (the materialized member ids
    // 0–1 are loose; nothing beneath the root row renders).
    assert!(html.contains("data-folder-key=\"Photographs\""));
    assert!(html.contains("class=\"folder-count\">2 files</small>"));
    assert!(!html.contains("href=\"/download/2\""));
    assert!(!html.contains("beach.jpg"));
    assert!(!html.contains(std::env::temp_dir().to_string_lossy().as_ref()));

    // Bounded: a whole store pass would have strung every member;
    // here the page holds exactly three rows' worth of markup.
    assert_eq!(html.matches("class=\"file-row\"").count(), 2);
    assert_eq!(html.matches("class=\"folder-row\"").count(), 1);
}

#[tokio::test]
async fn root_page_stamps_the_five_children_default_expansion() {
    let (dir, _state_before_replace) = state_with_root_record();
    // Recompose the state directly: the fixture above holds 2 direct
    // children (expanded); a 6-entry folder would fold.
    let mut store = FileStore::new();
    let big = dir.path().join("Big");
    std::fs::create_dir(&big).unwrap();
    for name in ["a", "b", "c", "d", "e", "f"] {
        std::fs::write(big.join(format!("{name}.txt")), b"x").unwrap();
    }
    store.commit_root_record(&big, 6);

    let app = router(AppState::new(Arc::new(Mutex::new(store))));
    let html = body_text(get(app, "/").await).await;

    // Over the budget: stamped collapsed. Nothing about its members
    // renders either.
    assert!(html.contains("aria-expanded=\"false\""));
    assert!(!html.contains("class=\"file-row\""));
}

#[tokio::test]
async fn node_fragment_serves_a_page_and_materializes_its_children() {
    let (_dir, state) = state_with_root_record();
    let app = router(state);

    let response = get(app, "/node/0").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "text/html; charset=utf-8"
    );

    let html = body_text(response).await;

    // The page's rows: the materialized direct member and the
    // lazily-known subfolder, wrapped in the fragment's state.
    assert!(html.contains("<template data-node-key=\"Photographs\""));
    assert!(html.contains("data-window-rows=\"2\""));
    assert!(html.contains("data-has-more=\"false\""));
    assert!(html.contains("href=\"/download/2\""));
    assert!(html.contains("data-node-sub=\"notes\""));
    // Materialization happened server-side: the fragment's file
    // carries its id; the lazily-known row carries no id, no link.
    let notes = html.find("data-folder-key=\"Photographs/notes\"").unwrap();
    let notes_end = notes + html[notes..].find("</li>").unwrap();
    assert!(!html[notes..notes_end].contains("href="));
    // No absolute path leaks into the fragment either.
    assert!(!html.contains(std::env::temp_dir().to_string_lossy().as_ref()));
}

#[tokio::test]
async fn node_fragment_repeat_fetches_never_duplicate_registration() {
    let (_dir, state) = state_with_root_record();
    let app = router(state.clone());

    let first = body_text(get(app.clone(), "/node/0").await).await;
    let second = body_text(get(app, "/node/0").await).await;

    // Idempotent materialization: both pages carry the same ids, and
    // the store holds the two loose files plus one materialized member.
    assert_eq!(first, second);
    let store = state.store().lock().expect("file store not poisoned");
    assert_eq!(store.list().len(), 3, "two loose before, one materialized");
    let beach_count = store
        .list()
        .iter()
        .filter(|file| file.name == "beach.jpg")
        .count();
    assert_eq!(beach_count, 1);
}

#[tokio::test]
async fn node_fragment_windows_paging_state_and_materialize_deep_nodes() {
    let (_dir, state) = state_with_root_record();
    let app = router(state.clone());

    // A two-row window at offset 2 pages past the node's rows; and a
    // sub-node page materializes *its* children on the way.
    let paged = body_text(get(app.clone(), "/node/0?limit=2&offset=2").await).await;
    assert!(paged.contains("data-offset=\"2\""));
    assert!(paged.contains("data-window-rows=\"0\""));
    assert!(paged.contains("data-has-more=\"false\""));

    let sub = body_text(get(app, "/node/0?sub=notes").await).await;
    assert!(sub.contains("<template data-node-key=\"Photographs/notes\""));
    assert!(
        sub.contains("href=\"/download/3\""),
        "todo.txt materialized"
    );
    assert!(sub.contains("data-depth=\"2\""), "inside a depth-1 node");
}

#[tokio::test]
async fn node_fragment_answers_404_for_unknown_roots_and_escaping_sub_paths() {
    let (_dir, state) = state_with_root_record();
    let app = router(state);

    for uri in [
        "/node/99",
        "/node/0?sub=..%2F..",
        "/node/0?sub=..",
        "/node/0?sub=.%2F",
    ] {
        let response = get(app.clone(), uri).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn node_fragment_lists_unreadable_members_as_inline_failures() {
    let dir = TempDir::new().unwrap();
    let folder = dir.path().join("Sealed");
    std::fs::create_dir_all(folder.join("sealed")).unwrap();
    std::fs::write(folder.join("visible.txt"), b"v").unwrap();

    let Some(_guard) = seal_directory(&folder.join("sealed")) else {
        return; // the environment can't deny reads; skip
    };

    let mut store = FileStore::new();
    store.commit_root_record(&folder, 2);

    let app = router(AppState::new(Arc::new(Mutex::new(store))));

    // The readable sibling materializes and links; the sealed subfolder
    // stays a lazily-known row on its own node...
    let root = body_text(get(app.clone(), "/node/0").await).await;
    assert!(root.contains("href=\"/download/"), "visible link");
    assert!(root.contains("data-node-sub=\"sealed\""));

    // ...whose page is where its unreadable members surface: the count
    // inline, and never a path in markup.
    let sealed = body_text(get(app, "/node/0?sub=sealed").await).await;
    assert!(sealed.contains("1 item of this folder couldn't be read"));
    assert!(!sealed.contains(std::env::temp_dir().to_string_lossy().as_ref()));
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
async fn node_zip_archives_a_whole_lazy_root_without_prior_expansion() {
    let (_dir, state) = state_with_root_record();
    let app = router(state);

    // Nothing was expanded or registered first: the request itself
    // materializes the whole subtree (ADR-0005's archive trigger).
    let response = get(app, "/download/zip/node/0").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/zip");
    let disposition = response.headers()[CONTENT_DISPOSITION].to_str().unwrap();
    assert!(
        disposition.starts_with("attachment; filename=\"Photographs.zip\""),
        "{disposition}"
    );

    // Every member under the node, at every depth, under the root
    // name — the entry paths the existing planner derives.
    let mut archive = parse_zip(response).await;
    assert_eq!(
        entry_names(&mut archive),
        ["Photographs/beach.jpg", "Photographs/notes/todo.txt"]
    );
    assert_eq!(
        entry_content(&mut archive, "Photographs/beach.jpg"),
        "beach"
    );
    assert_eq!(
        entry_content(&mut archive, "Photographs/notes/todo.txt"),
        "todo"
    );
}

#[tokio::test]
async fn node_zip_of_a_nested_subfolder_scopes_to_that_node() {
    let (_dir, state) = state_with_root_record();
    let app = router(state);

    // `sub` addresses the node: only what hangs below `notes`.
    let response = get(app, "/download/zip/node/0?sub=notes").await;

    assert_eq!(response.status(), StatusCode::OK);
    let mut archive = parse_zip(response).await;
    assert_eq!(entry_names(&mut archive), ["Photographs/notes/todo.txt"]);
    assert_eq!(
        entry_content(&mut archive, "Photographs/notes/todo.txt"),
        "todo"
    );
}

#[tokio::test]
async fn node_zip_repeat_requests_never_duplicate_entries() {
    let (_dir, state) = state_with_root_record();
    let app = router(state.clone());

    let first = parse_zip(get(app.clone(), "/download/zip/node/0").await).await;
    let second = parse_zip(get(app, "/download/zip/node/0").await).await;

    // Idempotent materialization: the same members on both, and no
    // duplicated registrations the second time around.
    let mut first = first;
    let mut second = second;
    assert_eq!(entry_names(&mut first), entry_names(&mut second));
    member_is_registered(&state, "beach.jpg", 1);
    member_is_registered(&state, "todo.txt", 1);
    assert_registered_members(&state, 4, "nested materialization converged too");
}

#[tokio::test]
async fn node_zip_covers_mixed_materialized_and_unmaterialized_content() {
    let (_dir, state) = state_with_root_record();
    let app = router(state.clone());

    // Expand `notes` first: its direct children are now registered
    // shared files with ids (a receiver browsed there earlier).
    let _ = body_text(get(app.clone(), "/node/0?sub=notes").await).await;
    assert_registered_members(&state, 3, "two loose plus the materialized todo.txt");

    // The whole-root zip picks the materialized member and its
    // unmaterialized siblings up as one subtree, with no duplicate.
    let response = get(app, "/download/zip/node/0").await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut archive = parse_zip(response).await;
    assert_eq!(
        entry_names(&mut archive),
        ["Photographs/beach.jpg", "Photographs/notes/todo.txt"]
    );
}

#[tokio::test]
async fn node_zip_answers_404_for_unknown_or_malformed_node_keys() {
    let (_dir, state) = state_with_root_record();
    let app = router(state);

    for uri in [
        "/download/zip/node/99",
        "/download/zip/node/abc",
        "/download/zip/node/",
        "/download/zip/node/0?sub=..",
        "/download/zip/node/0?sub=..%2F..",
    ] {
        let response = get(app.clone(), uri).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn node_zip_with_a_vanished_member_is_404_like_the_id_keyed_route() {
    let dir = TempDir::new().unwrap();
    let folder = dir.path().join("Photographs");
    std::fs::create_dir_all(folder.join("notes")).unwrap();
    std::fs::write(folder.join("beach.jpg"), b"beach").unwrap();
    std::fs::write(folder.join("notes").join("todo.txt"), b"todo").unwrap();

    let state = AppState::new(Arc::new(Mutex::new(FileStore::new())));
    {
        let mut store = state.store().lock().expect("file store not poisoned");
        store.commit_root_record(&folder, 2);
        // `notes` was browsed earlier: todo.txt registered, then its
        // content vanished from disk (frozen snapshot keeps it listed).
        let _ = store
            .list_page(mosquito_core::RootId(0), "notes", 0, 10)
            .unwrap();
    }
    std::fs::remove_file(folder.join("notes").join("todo.txt")).unwrap();

    let app = router(state);

    // Existing archive rules: a vanished member is visible as a
    // failure up front — an archive with a silent hole is never sent.
    let response = get(app, "/download/zip/node/0").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn node_zip_skips_unreadable_members_like_the_listing_does() {
    let dir = TempDir::new().unwrap();
    let folder = dir.path().join("Photographs");
    std::fs::create_dir_all(folder.join("sealed").join("inner")).unwrap();
    std::fs::write(folder.join("beach.jpg"), b"beach").unwrap();
    std::fs::write(folder.join("sealed").join("inner").join("gone.txt"), b"g").unwrap();

    let Some(_guard) = seal_directory(&folder.join("sealed")) else {
        return; // the environment can't deny reads; skip
    };

    let mut store = FileStore::new();
    store.commit_root_record(&folder, 2);

    let app = router(AppState::new(Arc::new(Mutex::new(store))));

    // Materialization failures are per-node markers on the listing,
    // never blockers (ADR-0005): the sealed branch never registered,
    // so the archive holds exactly what materialized and succeeds.
    let response = get(app, "/download/zip/node/0").await;
    assert_eq!(response.status(), StatusCode::OK);

    let mut archive = parse_zip(response).await;
    assert_eq!(entry_names(&mut archive), ["Photographs/beach.jpg"]);
}

/// Two folder roots plus one loose file, all untouched (lazily known):
/// enough structure for multi-node, merged, and 404 payload tests.
/// Root ids come from the commit order — Photographs is 0, Docs is 1.
fn state_with_two_root_records() -> (TempDir, AppState) {
    let dir = TempDir::new().unwrap();
    let mut store = FileStore::new();

    std::fs::write(dir.path().join("recipes.txt"), b"recipes").unwrap();
    store.add(dir.path().join("recipes.txt")).unwrap();

    let photographs = dir.path().join("Photographs");
    std::fs::create_dir_all(photographs.join("notes")).unwrap();
    std::fs::write(photographs.join("lead.txt"), b"lead").unwrap();
    std::fs::write(photographs.join("notes").join("todo.txt"), b"todo").unwrap();
    store.commit_root_record(&photographs, 2);

    let docs = dir.path().join("Docs");
    std::fs::create_dir_all(docs.join("drafts")).unwrap();
    std::fs::write(docs.join("lead.txt"), b"lead").unwrap();
    std::fs::write(docs.join("drafts").join("wip.txt"), b"wip").unwrap();
    store.commit_root_record(&docs, 2);

    (dir, AppState::new(Arc::new(Mutex::new(store))))
}

#[tokio::test]
async fn zip_download_archives_each_node_address_of_the_payload() {
    let (_dir, state) = state_with_two_root_records();
    let app = router(state);

    // Two lazily-known nodes across two roots: each materialized
    // through the same idempotent node-keyed walk #26's route uses,
    // nothing registered or expanded beforehand.
    let response = get(app, "/download/zip?nodes=0:notes,1:drafts").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/zip");

    let mut archive = parse_zip(response).await;
    // Node addresses archive in payload order — the planner's layout
    // preserves the selection's order (as the two-,roots test pins).
    assert_eq!(
        entry_names(&mut archive),
        ["Photographs/notes/todo.txt", "Docs/drafts/wip.txt"]
    );
}

#[tokio::test]
async fn zip_download_merges_ids_and_node_addresses_into_one_archive() {
    let (_dir, state) = state_with_two_root_records();
    let app = router(state);

    // A rendered loose id plus a node address: one archive spans both
    // forms, the id-keyed files resolving as always.
    let response = get(app, "/download/zip?ids=0&nodes=0:notes").await;

    assert_eq!(response.status(), StatusCode::OK);
    let mut archive = parse_zip(response).await;
    // Id files lead (as the payload orders them), then the node's.
    // Layout preserves the selection's order.
    assert_eq!(
        entry_names(&mut archive),
        ["recipes.txt", "Photographs/notes/todo.txt"]
    );
    assert_eq!(entry_content(&mut archive, "Photographs/notes/todo.txt"), "todo");
    assert_eq!(entry_content(&mut archive, "recipes.txt"), "recipes");
}

#[tokio::test]
async fn zip_download_of_a_root_node_address_uses_the_empty_sub() {
    let (_dir, state) = state_with_two_root_records();
    let app = router(state);

    // A root's own node address (id 0): the form a single-node
    // selection links directly. `notes` below it is never expanded —
    // the walk covers the whole subtree.
    let response = get(app, "/download/zip?nodes=0:").await;

    assert_eq!(response.status(), StatusCode::OK);
    let disposition = response.headers()[CONTENT_DISPOSITION].to_str().unwrap();
    assert!(
        disposition.starts_with("attachment; filename=\"Photographs.zip\""),
        "{disposition}"
    );
    let mut archive = parse_zip(response).await;
    assert_eq!(
        entry_names(&mut archive),
        ["Photographs/lead.txt", "Photographs/notes/todo.txt"]
    );
}

#[tokio::test]
async fn zip_download_with_unknown_or_malformed_node_addresses_is_404() {
    let (_dir, state) = state_with_two_root_records();
    let app = router(state);

    // No address (`0` without its sub), an empty payload, a
    // non-numeric root, an unknown root, and a sub escaping the root
    // record — every malformed piece of the payload invalidates the
    // whole request, the way the ids form answers.
    for uri in [
        "/download/zip?nodes=0",
        "/download/zip?nodes=",
        "/download/zip?nodes=abc:notes",
        "/download/zip?nodes=99:notes",
        "/download/zip?nodes=0:..",
        "/download/zip?nodes=0:..%2F..",
        "/download/zip?ids=0&nodes=99:notes",
    ] {
        let response = get(app.clone(), uri).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn zip_download_decodes_encoded_names_in_the_nodes_payload() {
    let dir = TempDir::new().unwrap();
    // A folder name carrying the payload's own delimiters: the comma
    // splits addresses and the space is a form-encoding hazard — the
    // request encoding (%XX, never `+` for spaces) must cover both.
    let folder = dir.path().join("Photographs");
    let named = folder.join("trip notes, 2024");
    std::fs::create_dir_all(&named).unwrap();
    std::fs::write(named.join("pool.jpg"), b"pool").unwrap();

    let mut store = FileStore::new();
    store.commit_root_record(&folder, 1);

    let app = router(AppState::new(Arc::new(Mutex::new(store))));

    // The raw payload splits on the encoded comma; the `%20`s decode
    // per piece afterward, so the name arrives whole.
    let response = get(app, "/download/zip?nodes=0:trip%20notes%2C%202024").await;

    assert_eq!(response.status(), StatusCode::OK);
    let mut archive = parse_zip(response).await;
    assert_eq!(
        entry_names(&mut archive),
        ["Photographs/trip notes, 2024/pool.jpg"]
    );
}

#[tokio::test]
async fn zip_download_node_selection_materializes_without_any_expansion() {
    let (_dir, state) = state_with_two_root_records();
    let app = router(state.clone());

    let response = get(app, "/download/zip?nodes=0:notes").await;
    assert_eq!(response.status(), StatusCode::OK);

    // The walk's materialization is the same idempotent trigger every
    // node route carries: todo.txt registers once, the untouched
    // branches never materialize.
    member_is_registered(&state, "todo.txt", 1);
    assert_registered_members(&state, 2, "one loose plus the notes member");
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
