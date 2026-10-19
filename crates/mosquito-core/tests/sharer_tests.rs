//! Lifecycle integration tests. These use real sockets deliberately:
//! ports, binds, and shutdowns are the thing under test.

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mosquito_core::{FileStore, LanAddress, RootRunStatus, RunId, Sharer, SharerError};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Always reports the same IP. 192.0.2.x is TEST-NET-1, reserved for
/// documentation — it never routes anywhere, so it can't collide
/// with a real interface.
struct FixedLan(IpAddr);

impl LanAddress for FixedLan {
    fn local_ip(&self) -> std::io::Result<IpAddr> {
        Ok(self.0)
    }
}

/// Simulates "no network interface found".
struct BrokenLan;

impl LanAddress for BrokenLan {
    fn local_ip(&self) -> std::io::Result<IpAddr> {
        Err(std::io::Error::other("no network (mock)"))
    }
}

fn sharer_on(ip: &str) -> Sharer {
    let store = Arc::new(Mutex::new(FileStore::new()));
    Sharer::with_lan(store, Box::new(FixedLan(ip.parse().unwrap())))
}

#[tokio::test]
async fn start_reports_running_info() {
    let sharer = sharer_on("192.0.2.7");

    let info = sharer.start_on(0).unwrap();

    assert!(info.running);

    let port = info.port.unwrap();

    assert!(port > 0);
    assert_eq!(info.url, Some(format!("http://192.0.2.7:{port}")));
    assert_eq!(info.lan_ip, Some("192.0.2.7".parse().unwrap()));

    sharer.stop().await.unwrap();
}

#[tokio::test]
async fn start_when_already_running_is_error() {
    let sharer = sharer_on("192.0.2.7");
    let first = sharer.start_on(0).unwrap();

    let err = sharer.start_on(0).unwrap_err();

    assert!(matches!(err, SharerError::AlreadyRunning(_)));
    // The original server must be untouched.
    assert_eq!(sharer.status().port, first.port);

    sharer.stop().await.unwrap();
}

#[tokio::test]
async fn stop_returns_to_stopped_state() {
    let sharer = sharer_on("192.0.2.7");
    sharer.start_on(0).unwrap();

    assert!(sharer.status().running);

    sharer.stop().await.unwrap();

    let status = sharer.status();

    assert!(!status.running);
    assert!(status.url.is_none());
    assert!(status.port.is_none());
}

#[tokio::test]
async fn stop_without_start_is_error() {
    let sharer = sharer_on("192.0.2.7");
    assert!(matches!(sharer.stop().await, Err(SharerError::NotRunning)));
}

#[tokio::test]
async fn can_restart_after_stop() {
    let sharer = sharer_on("192.0.2.7");

    sharer.start_on(0).unwrap();
    sharer.stop().await.unwrap();

    let info = sharer.start_on(0).unwrap();

    assert!(info.running);

    sharer.stop().await.unwrap();
}

#[tokio::test]
async fn start_falls_back_when_preferred_port_taken() {
    // Hold a port ourselves, then ask for exactly that port.
    let blocker = std::net::TcpListener::bind(("0.0.0.0", 0)).unwrap();
    let taken = blocker.local_addr().unwrap().port();

    let sharer = sharer_on("192.0.2.7");
    let info = sharer.start_on(taken).unwrap();

    assert_ne!(info.port, Some(taken), "should have fallen back");
    assert!(info.running);

    sharer.stop().await.unwrap();
}

#[tokio::test]
async fn started_server_serves_real_http_requests() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("bite.txt");

    std::fs::write(&path, b"mosquito").unwrap();

    let store = Arc::new(Mutex::new(FileStore::new()));
    let sharer = Sharer::with_lan(store, Box::new(FixedLan("127.0.0.1".parse().unwrap())));

    sharer.add_file(&path).unwrap();

    let port = sharer.start_on(0).unwrap().port.unwrap();

    // Plain socket-level request: proves bind + serve + store + file
    // streaming work together, not just router-in-a-harness.
    let mut stream = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    .unwrap()
    .unwrap();

    stream
        .write_all(b"GET /download/0 HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut buf = Vec::new();

    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut buf))
        .await
        .unwrap()
        .unwrap();

    let text = String::from_utf8_lossy(&buf);

    assert!(text.starts_with("HTTP/1.1 200"), "got: {text}");
    assert!(text.ends_with("mosquito"), "got: {text}");

    sharer.stop().await.unwrap();
}

#[tokio::test]
async fn lan_failure_falls_back_to_localhost() {
    let store = Arc::new(Mutex::new(FileStore::new()));
    let sharer = Sharer::with_lan(store, Box::new(BrokenLan));

    let info = sharer.start_on(0).unwrap();

    let port = info.port.unwrap();

    assert_eq!(info.lan_ip, Some("127.0.0.1".parse().unwrap()));
    assert_eq!(info.url, Some(format!("http://127.0.0.1:{port}")));

    sharer.stop().await.unwrap();
}

#[tokio::test]
async fn stop_releases_the_port() {
    let sharer = sharer_on("192.0.2.7");
    let port = sharer.start_on(0).unwrap().port.unwrap();

    sharer.stop().await.unwrap();

    // Only possible if stop() truly waited for the listener to close.
    assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_ok());
}

#[test]
fn add_files_splits_successes_and_failures() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("good.txt"), b"x").unwrap();
    std::fs::create_dir(dir.path().join("emptydir")).unwrap();

    let sharer = sharer_on("192.0.2.7");
    let outcome = sharer.add_files(vec![
        dir.path().join("good.txt"),
        dir.path().join("missing.txt"),
        // An empty directory contributes neither files nor failures.
        dir.path().join("emptydir"),
    ]);

    assert_eq!(outcome.added.len(), 1);
    assert_eq!(outcome.added[0].name, "good.txt");
    assert_eq!(outcome.failed.len(), 1);
    assert!(outcome.failed[0].error.contains("does not exist"));
}

#[tokio::test]
async fn list_page_materializes_through_the_sharers_lock() {
    use mosquito_core::{LISTING_PAGE_ROWS, PageRow};

    let dir = TempDir::new().unwrap();
    let folder = dir.path().join("Photos");
    std::fs::create_dir_all(folder.join("sub")).unwrap();
    std::fs::write(folder.join("a.txt"), b"a").unwrap();

    let sharer = sharer_on("192.0.2.7");
    let outcome = sharer.import_paths(vec![folder.clone()]).await;

    // Import runs commit in the background; wait for the record.
    let run_id = outcome.run_id.expect("a folder import is a run");
    let _ = wait_terminated(&sharer, run_id).await;

    let root_id = sharer
        .list_top()
        .roots
        .first()
        .expect("root record committed")
        .id;

    let page = sharer.list_page(root_id, "", 0, LISTING_PAGE_ROWS).unwrap();

    let names: Vec<&str> = page
        .rows
        .iter()
        .filter_map(|row| match row {
            PageRow::File { file } => Some(file.name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(names, ["a.txt"]);
    // Subfolders ride the scan as lazily-known rows.
    assert!(
        page.rows
            .iter()
            .any(|row| matches!(row, PageRow::LazyFolder { name } if name == "sub"))
    );
}

#[test]
fn add_files_walks_directories_into_their_files() {
    let dir = TempDir::new().unwrap();
    let folder = dir.path().join("folder");
    std::fs::create_dir_all(folder.join("sub")).unwrap();
    std::fs::write(folder.join("a.txt"), b"a").unwrap();
    std::fs::write(folder.join("sub").join("b.txt"), b"b").unwrap();
    let loose = dir.path().join("loose.txt");
    std::fs::write(&loose, b"l").unwrap();

    let sharer = sharer_on("192.0.2.7");
    let outcome = sharer.add_files(vec![folder.clone(), loose.clone()]);

    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);

    // Loose files keep their old shape: no folder provenance.
    let loose_file = outcome
        .added
        .iter()
        .find(|f| f.name == "loose.txt")
        .unwrap();
    assert!(loose_file.provenance.is_none());

    // Folder members carry both parts of their provenance.
    let a = outcome.added.iter().find(|f| f.name == "a.txt").unwrap();
    let a_provenance = a.provenance.as_ref().unwrap();
    assert_eq!(a_provenance.relative_path, "a.txt");
    assert_eq!(a_provenance.folder_root, folder);

    let b = outcome.added.iter().find(|f| f.name == "b.txt").unwrap();
    let b_provenance = b.provenance.as_ref().unwrap();
    assert_eq!(b_provenance.relative_path, "sub/b.txt");
    assert_eq!(b_provenance.folder_root, folder);
}

#[test]
fn add_files_allows_the_same_folder_twice() {
    let dir = TempDir::new().unwrap();
    let folder = dir.path().join("folder");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("a.txt"), b"a").unwrap();

    let sharer = sharer_on("192.0.2.7");
    let first = sharer.add_files(vec![folder.clone()]);
    let second = sharer.add_files(vec![folder]);

    assert_eq!(first.added.len(), 1);
    assert_eq!(second.added.len(), 1);

    // Same path can be shared twice, as for loose files: distinct ids,
    // identical provenance — resolving that collision is a job for
    // consumers (e.g. zip writing), not the store.
    assert_ne!(first.added[0].id, second.added[0].id);
    assert_eq!(first.added[0].provenance, second.added[0].provenance);
    assert_eq!(sharer.files().len(), 2);
}

#[test]
fn add_files_reports_the_original_path_on_failure() {
    let sharer = sharer_on("192.0.2.7");
    let outcome = sharer.add_files(vec![r"C:\Users\nope.txt".into()]);
    assert_eq!(outcome.failed[0].path, PathBuf::from(r"C:\Users\nope.txt"));
}

#[test]
fn add_files_with_no_paths_is_clean() {
    let sharer = sharer_on("192.0.2.7");
    let outcome = sharer.add_files(vec![]);
    assert!(outcome.added.is_empty());
    assert!(outcome.failed.is_empty());
}

// -- import runs (ADR-0007): background count-only folder imports ------

/// Wait until every root in a run left the Counting state, then hand
/// back each root's terminal status. Fails after 5 s instead of hanging.
async fn wait_terminated(sharer: &Sharer, run_id: RunId) -> Vec<RootRunStatus> {
    let start = std::time::Instant::now();
    loop {
        let snapshot = sharer
            .import_run_status()
            .into_iter()
            .find(|snapshot| snapshot.run_id == run_id)
            .expect("the run is recorded in the registry");
        let counting = snapshot
            .roots
            .iter()
            .filter(|root| root.status == RootRunStatus::Counting)
            .count();
        if counting == 0 {
            return snapshot.roots.into_iter().map(|root| root.status).collect();
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "import never finished: {snapshot:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Nested tree with exactly two files (a.txt at the root, sub/b.txt).
fn make_nested_tree(dir: &std::path::Path) -> PathBuf {
    let root = dir.join("photos");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("a.txt"), b"a").unwrap();
    std::fs::write(root.join("sub").join("b.txt"), b"b").unwrap();
    root
}

/// A flat folder with exactly `total` tiny files — big enough that the
/// count walk takes many checkpoints to walk it, so cancellation lands
/// mid-walk deterministically-seen-from-the-caller's side.
fn make_flat_tree(dir: &std::path::Path, total: usize) -> PathBuf {
    let root = dir.join("big");
    std::fs::create_dir(&root).unwrap();
    for i in 0..total {
        std::fs::write(root.join(format!("f{i}.txt")), b"x").unwrap();
    }
    root
}

#[tokio::test]
async fn a_folder_drop_commits_its_root_record_at_drop() {
    let dir = TempDir::new().unwrap();
    let root = make_nested_tree(dir.path());

    let sharer = sharer_on("192.0.2.7");
    let started = sharer.import_paths(vec![root.clone()]).await;

    // Nothing registers inline for a folder drop — the record is the product.
    assert!(started.immediate.added.is_empty());
    assert!(started.immediate.failed.is_empty());
    let run_id = started.run_id.expect("a folder drop starts a run");

    // Committed at drop (ADR-0008), before the walk's outcome is known:
    // the record already exists with a count at or below the total
    // (0, or whatever checkpoint the walk reached in the meantime).
    let roots = sharer.roots();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].path, root);
    assert!(
        roots[0].file_count <= 2,
        "live count is event-driven: got {}",
        roots[0].file_count
    );
    // The server came up in the same synchronous path as the commit —
    // drop → listening, never gated by the walk.
    assert!(sharer.status().running);

    let statuses = wait_terminated(&sharer, run_id).await;
    assert_eq!(statuses, vec![RootRunStatus::Committed]);

    // Count-only, exactly as before: no shared file was registered
    // under the root, and the record's count converged to the total.
    assert!(sharer.files().is_empty());
    let roots = sharer.roots();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].path, root);
    assert_eq!(roots[0].file_count, 2);

    sharer.stop().await.unwrap();
}

#[tokio::test]
async fn mixed_drops_register_loose_files_immediately() {
    let dir = TempDir::new().unwrap();
    let root = make_nested_tree(dir.path());
    let loose = dir.path().join("loose.txt");
    std::fs::write(&loose, b"l").unwrap();

    let sharer = sharer_on("192.0.2.7");
    let started = sharer.import_paths(vec![root.clone(), loose.clone()]).await;

    assert_eq!(started.immediate.added.len(), 1);
    assert_eq!(started.immediate.added[0].name, "loose.txt");
    assert!(started.immediate.added[0].provenance.is_none());

    let run_id = started
        .run_id
        .expect("the folder part of a drop starts a run");
    let statuses = wait_terminated(&sharer, run_id).await;
    assert_eq!(statuses, vec![RootRunStatus::Committed]);

    // Only the loose file is registered; the counted folder is not.
    assert_eq!(sharer.files().len(), 1);
    assert_eq!(sharer.roots().len(), 1);
}

#[tokio::test]
async fn a_folder_drop_starts_the_server_at_drop() {
    let dir = TempDir::new().unwrap();
    let empty = dir.path().join("empty");
    std::fs::create_dir(&empty).unwrap();

    let sharer = sharer_on("192.0.2.7");
    let started = sharer.import_paths(vec![empty]).await;
    let run_id = started.run_id.expect("a folder drop starts a run");

    // The server is listening already — in the drop's synchronous path,
    // milliseconds after the commit, never gated by the walk (ADR-0008).
    assert!(sharer.status().running);

    let statuses = wait_terminated(&sharer, run_id).await;
    assert_eq!(statuses, vec![RootRunStatus::Committed]);
    assert!(sharer.status().running);

    sharer.stop().await.unwrap();
}

#[tokio::test]
async fn the_walk_keeps_the_record_count_live_at_its_checkpoints() {
    let dir = TempDir::new().unwrap();
    // Two checkpoints' worth of files: the walk must pass 500 and 1000
    // before finishing, whatever the disk's speed.
    let root = make_flat_tree(dir.path(), 1100);

    let sharer = sharer_on("192.0.2.7");
    let started = sharer.import_paths(vec![root]).await;
    let run_id = started.run_id.expect("a folder drop starts a run");

    // Read the record's number while the walk runs: every observation is
    // one of the walk's checkpoint values — event-driven (each one was
    // some real reached-total), monotonic, never past the walk's total.
    let mut seen: Vec<u64> = Vec::new();
    let start = std::time::Instant::now();
    loop {
        let snapshot = sharer
            .import_run_status()
            .into_iter()
            .find(|snapshot| snapshot.run_id == run_id)
            .expect("the run is recorded in the registry");
        let counting = snapshot.roots[0].status == RootRunStatus::Counting;

        seen.push(sharer.roots()[0].file_count);

        if !counting {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "import never finished: {snapshot:?}"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    for pair in seen.windows(2) {
        assert!(
            pair[1] >= pair[0],
            "the recorded count only climbs: {seen:?}"
        );
        assert!(pair[1] <= 1100, "checkpoint overflow: {seen:?}");
    }
    // The final count is the walk's total, and nothing registered
    // because of any of it — display only.
    assert_eq!(sharer.roots()[0].file_count, 1100);
    assert!(sharer.files().is_empty());

    sharer.stop().await.unwrap();
}

#[tokio::test]
async fn cancelling_mid_walk_removes_the_record_and_the_walks_materialization() {
    let dir = TempDir::new().unwrap();
    // Several checkpoints' worth of files: the walk cannot finish before
    // the cancel flag lands (it is set microseconds after import_paths
    // returns), so the removal path is the outcome under test.
    let root = make_flat_tree(dir.path(), 1500);

    let sharer = sharer_on("192.0.2.7");
    let started = sharer.import_paths(vec![root]).await;
    let run_id = started.run_id.expect("a folder drop starts a run");

    // The record exists from the drop, whatever the walk has reached.
    assert_eq!(sharer.roots().len(), 1);

    assert!(sharer.cancel_import(run_id));
    let statuses = wait_terminated(&sharer, run_id).await;
    assert_eq!(statuses, vec![RootRunStatus::Cancelled]);

    // The contract's outcome survives, reached by removal (ADR-0008):
    // no half-done count stands as a record anywhere — neither in the
    // store nor on the top listing — and anything materialized under the
    // record went with the same remove_root sweep a user's delete uses.
    assert!(sharer.roots().is_empty());
    assert!(sharer.list_top().roots.is_empty());
    assert!(sharer.files().is_empty());

    // The drop started the server; the removal of the last content
    // stops it, exactly like a user's own removal does.
    assert!(!sharer.status().running);
}

#[tokio::test]
async fn cancelling_after_a_root_committed_keeps_it() {
    let dir = TempDir::new().unwrap();
    let empty = dir.path().join("empty");
    std::fs::create_dir(&empty).unwrap();

    let sharer = sharer_on("192.0.2.7");
    let started = sharer.import_paths(vec![empty]).await;
    let run_id = started.run_id.unwrap();
    wait_terminated(&sharer, run_id).await;

    // The run settled before the cancel arrived: the committed record
    // is not retroactively cancelled.
    assert!(sharer.cancel_import(run_id));
    assert_eq!(sharer.roots().len(), 1);
    assert_eq!(
        wait_terminated(&sharer, run_id).await,
        vec![RootRunStatus::Committed]
    );
    assert!(sharer.status().running);

    sharer.stop().await.unwrap();
}

#[tokio::test]
async fn removing_the_last_root_record_auto_stops_the_server() {
    let dir = TempDir::new().unwrap();
    let empty = dir.path().join("empty");
    std::fs::create_dir(&empty).unwrap();

    let sharer = sharer_on("192.0.2.7");
    let started = sharer.import_paths(vec![empty]).await;
    let run_id = started.run_id.unwrap();
    wait_terminated(&sharer, run_id).await;
    assert!(sharer.status().running);

    let root_id = sharer.roots()[0].id;
    sharer.remove_root(root_id).await.unwrap();

    assert!(!sharer.status().running);
}

#[tokio::test]
async fn removing_the_last_file_auto_stops_the_server() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("lonely.txt");
    std::fs::write(&path, b"only").unwrap();

    let sharer = sharer_on("192.0.2.7");
    let added = sharer.add_file(&path).unwrap();
    sharer.start_on(0).unwrap();

    sharer.remove_file(added.id).await.unwrap();

    assert!(!sharer.status().running);
}

#[tokio::test]
async fn removing_a_node_sweeps_its_registered_members_and_keeps_the_share_up() {
    let dir = TempDir::new().unwrap();
    let root = make_nested_tree(dir.path()); // a.txt, sub/b.txt

    let sharer = sharer_on("192.0.2.7");
    let started = sharer.import_paths(vec![root.clone()]).await;
    let run_id = started.run_id.expect("a folder drop starts a run");
    wait_terminated(&sharer, run_id).await;
    let root_id = sharer.roots()[0].id;
    let counted = sharer.list_top().roots[0].file_count; // the walk's 2
    assert_eq!(counted, 2);

    // Views materialize (ADR-0005): the root page registers a.txt, the
    // sub page registers b.txt on its own turn.
    sharer.list_page(root_id, "", 0, 200).unwrap();
    sharer.list_page(root_id, "sub", 0, 200).unwrap();
    assert_eq!(sharer.files().len(), 2);

    let removal = sharer.remove_node(root_id, "sub").await.unwrap();
    assert_eq!(removal.files.len(), 1);

    // The record survives, so the share stays up — the sweep took only
    // the sub's registered member, the record's live count updated by it.
    assert!(sharer.status().running);
    assert_eq!(sharer.files().len(), 1);
    assert_eq!(sharer.list_top().roots[0].file_count, counted - 1);
    assert_eq!(sharer.list_top().roots.len(), 1);

    sharer.stop().await.unwrap();
}

#[tokio::test]
async fn duplicate_imports_of_the_same_folder_do_not_duplicate_registrations() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("flat");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("only.txt"), b"o").unwrap();

    let sharer = sharer_on("192.0.2.7");
    let first = sharer.import_paths(vec![root.clone()]).await;
    let second = sharer.import_paths(vec![root.clone()]).await;

    let first_status = wait_terminated(&sharer, first.run_id.unwrap()).await;
    let second_status = wait_terminated(&sharer, second.run_id.unwrap()).await;
    assert_eq!(first_status, vec![RootRunStatus::Committed]);
    assert_eq!(second_status, vec![RootRunStatus::Committed]);

    assert_eq!(sharer.roots().len(), 2, "duplicates stay legal");

    // Materializing both records converges on one registration.
    let via_first = sharer.materialize_root(sharer.roots()[0].id).unwrap();
    assert_eq!(via_first.added.len(), 1);

    let via_second = sharer.materialize_root(sharer.roots()[1].id).unwrap();
    assert_eq!(via_second.added.len(), 0);
    assert_eq!(via_second.skipped, 1);
    assert_eq!(sharer.files().len(), 1);
}

#[tokio::test]
async fn cancel_import_reports_unknown_runs() {
    let sharer = sharer_on("192.0.2.7");
    assert!(!sharer.cancel_import(77));
}

#[test]
#[cfg(windows)]
fn unreadable_roots_auto_remove_their_record_and_fail_the_banner_report() {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let dir = TempDir::new().unwrap();
    let root = dir.path().join("sealed");
    std::fs::create_dir(&root).unwrap();

    let deny = std::process::Command::new("icacls")
        .arg(&root)
        .args(["/deny", "*S-1-1-0:(OI)(CI)R"])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .unwrap();
    assert!(deny.success(), "icacls deny failed");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = runtime.block_on(async {
        let sharer = sharer_on("192.0.2.7");
        let started = sharer.import_paths(vec![root.clone()]).await;
        let run_id = started.run_id.expect("a folder drop starts a run");
        // The record committed at drop — servable until the walk's
        // outcome is known — and the share came up with it.
        let committed_at_drop = sharer.roots().len() == 1;
        let was_running = sharer.status().running;
        let statuses = wait_terminated(&sharer, run_id).await;
        (
            statuses,
            committed_at_drop,
            was_running,
            sharer.status().running,
            sharer.roots().len(),
        )
    });
    let (statuses, committed_at_drop, was_running, running, roots) = result;

    // Lift the denial before the temp dir is cleaned up.
    let reset = std::process::Command::new("icacls")
        .arg(&root)
        .args(["/reset", "/t", "/q"])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .unwrap();
    assert!(reset.success(), "icacls reset failed");

    match &statuses[0] {
        RootRunStatus::Failed(_) => {}
        other => panic!("expected Failed, got {other:?}"),
    }
    // The drop committed the record and started the server; the failed
    // walk auto-removed its own record (failed-add semantics keep their
    // old meaning — nothing stands, reachable by removal now), and the
    // emptied share stopped serving like any removal of the last content.
    assert!(committed_at_drop);
    assert!(was_running);
    assert!(!running);
    assert_eq!(roots, 0);
}
