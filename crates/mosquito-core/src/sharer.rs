use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use thiserror::Error;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::file_store::{FileId, FileStore, SharedFile};
use crate::lan::{LanAddress, SystemLanAddress};
use crate::server::{AppState, router};
use crate::{AddFileError, RemoveError};

/// Preferred port. If it's taken, we fall back to an ephemeral one and
/// the UI shows whatever actually got bound.
pub const DEFAULT_PORT: u16 = 8472;

#[derive(Debug, Error)]
pub enum SharerError {
    #[error("server is already running at {0}")]
    AlreadyRunning(String),
    #[error("server is not running")]
    NotRunning,
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("server task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}

/// Snapshot of server status, shaped for the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerInfo {
    pub running: bool,
    /// `http://<lan-ip>:<port>` — what the QR code encodes.
    pub url: Option<String>,
    pub port: Option<u16>,
    pub lan_ip: Option<IpAddr>,
}

impl ServerInfo {
    fn stopped() -> Self {
        Self {
            running: false,
            url: None,
            port: None,
            lan_ip: None,
        }
    }
}

/// The handles only exist while running — you can't stop a server
/// you don't have, and a stopped server can't be contacted.
enum ServerState {
    Stopped,
    Running {
        info: ServerInfo,
        shutdown: CancellationToken,
        task: JoinHandle<std::io::Result<()>>,
    },
}

/// Owns everything: the shared files, the HTTP server, the LAN address.
pub struct Sharer {
    store: Arc<Mutex<FileStore>>,
    lan: Box<dyn LanAddress>,
    state: Mutex<ServerState>,
}

impl Sharer {
    pub fn new() -> Self {
        Self::with_lan(
            Arc::new(Mutex::new(FileStore::new())),
            Box::new(SystemLanAddress),
        )
    }

    pub fn with_lan(store: Arc<Mutex<FileStore>>, lan: Box<dyn LanAddress>) -> Self {
        Self {
            store,
            lan,
            state: Mutex::new(ServerState::Stopped),
        }
    }

    // -- store delegates: keeps the Tauri layer a thin shell -----------

    pub fn add_file(&self, path: impl Into<PathBuf>) -> Result<SharedFile, AddFileError> {
        self.store
            .lock()
            .expect("file store not poisoned")
            .add(path)
    }

    pub fn remove_file(&self, id: FileId) -> Result<SharedFile, RemoveError> {
        self.store
            .lock()
            .expect("file store not poisoned")
            .remove(id)
    }

    pub fn files(&self) -> Vec<SharedFile> {
        self.store
            .lock()
            .expect("file store not poisoned")
            .list()
            .to_vec()
    }

    // -- lifecycle ------------------------------------------------------

    /// Start sharing on the default port (with fallback if taken).
    pub fn start(&self) -> Result<ServerInfo, SharerError> {
        self.start_on(DEFAULT_PORT)
    }

    /// Start sharing on `preferred_port` (0 = let the OS pick).
    pub fn start_on(&self, preferred_port: u16) -> Result<ServerInfo, SharerError> {
        let mut state = self.state.lock().expect("server state not poisoned");

        if let ServerState::Running { info, .. } = &*state {
            return Err(SharerError::AlreadyRunning(
                info.url.clone().unwrap_or_else(|| "unknown url".into()),
            ));
        }

        let (std_listener, port) = bind_listener(preferred_port)?;

        // tokio requires the caller to flip the listener to non-blocking
        // before from_std — forgetting this is a classic hang.
        std_listener.set_nonblocking(true)?;
        let listener = tokio::net::TcpListener::from_std(std_listener)?;

        // No detectable LAN address? Degrade to localhost rather than
        // failing the whole start.
        let lan_ip = self.lan.local_ip().unwrap_or(IpAddr::from([127, 0, 0, 1]));

        // SocketAddr's Display adds brackets for IPv6 ([::1]:8472) —
        // formatting the IP by hand would break on IPv6.
        let url = format!("http://{}", SocketAddr::new(lan_ip, port));
        let info = ServerInfo {
            running: true,
            url: Some(url),
            port: Some(port),
            lan_ip: Some(lan_ip),
        };

        let shutdown = CancellationToken::new();

        let app = router(AppState::new(Arc::clone(&self.store)));

        let token = shutdown.clone();

        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move { token.cancelled().await })
                .await
        });

        *state = ServerState::Running {
            info: info.clone(),
            shutdown,
            task,
        };

        Ok(info)
    }

    /// Stop sharing. Waits for the server task to finish, so when this
    /// returns, the port is genuinely released.
    pub async fn stop(&self) -> Result<(), SharerError> {
        // Take the handles OUT of the lock, leaving Stopped behind.
        // The lock is dropped before any .await — this extract-then-await
        // pattern is how std::sync::Mutex and async code coexist safely.
        let (shutdown, task) = {
            let mut state = self.state.lock().expect("server state not poisoned");
            match std::mem::replace(&mut *state, ServerState::Stopped) {
                ServerState::Running { shutdown, task, .. } => (shutdown, task),
                ServerState::Stopped => return Err(SharerError::NotRunning),
            }
        };

        shutdown.cancel();
        task.await
            .map_err(SharerError::Join)?
            .map_err(SharerError::Io)?;

        Ok(())
    }

    /// Pure read of the current state.
    pub fn status(&self) -> ServerInfo {
        let state = self.state.lock().expect("server state not poisoned");

        match &*state {
            ServerState::Running { info, .. } => info.clone(),
            ServerState::Stopped => ServerInfo::stopped(),
        }
    }
}

impl Drop for Sharer {
    fn drop(&mut self) {
        // Best-effort: signal shutdown (can't await inside drop).
        // The real guarantee for the app is process exit.
        let state = self.state.lock().expect("server state not poisoned");

        if let ServerState::Running { shutdown, .. } = &*state {
            shutdown.cancel();
        }
    }
}

/// Bind `0.0.0.0:preferred` — all interfaces, so other devices can
/// reach us. If that port is taken, fall back to an ephemeral port:
/// predictable URL when possible, working server always.
fn bind_listener(preferred: u16) -> std::io::Result<(std::net::TcpListener, u16)> {
    match std::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, preferred)) {
        Ok(listener) => {
            let port = listener.local_addr()?.port();
            Ok((listener, port))
        }
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && preferred != 0 => {
            let listener = std::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))?;
            let port = listener.local_addr()?.port();
            Ok((listener, port))
        }
        Err(e) => Err(e),
    }
}
