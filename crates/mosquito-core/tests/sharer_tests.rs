//! Lifecycle integration tests. These use real sockets deliberately:
//! ports, binds, and shutdowns are the thing under test.

use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mosquito_core::{FileStore, LanAddress, Sharer, SharerError};
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
