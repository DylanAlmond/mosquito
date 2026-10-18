use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::error::SharerError;
use crate::file_store::{
    AddFilesOutcome, FailedAdd, FileId, FileStore, MaterializeOutcome, RootId, RootRecord,
    RootRemoval, SharedFile,
};
use crate::import_run::{
    CountFolderError, ImportRun, ImportRunSnapshot, ImportRuns, PROGRESS_EVERY_FILES, count_folder,
};
use crate::lan::{LanAddress, SystemLanAddress};
use crate::server::{AppState, router};
use crate::{AddFileError, RemoveError};

/// Preferred port. If it's taken, we fall back to an ephemeral one and
/// the UI shows whatever actually got bound.
pub const DEFAULT_PORT: u16 = 8472;

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

/// What a mixed drop produced at import time. Folder roots do *not*
/// finish here — they run in the background; `run_id` identifies their
/// progress (None when the drop held no folder at all).
#[derive(Debug, Serialize)]
pub struct ImportStartOutcome {
    pub run_id: Option<crate::import_run::RunId>,
    pub immediate: AddFilesOutcome,
}

/// Owns everything: the shared files, the HTTP server, the LAN address.
pub struct Sharer {
    store: Arc<Mutex<FileStore>>,
    lan: Arc<dyn LanAddress>,
    state: Arc<Mutex<ServerState>>,
    runs: Mutex<ImportRuns>,
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
            lan: Arc::from(lan),
            state: Arc::new(Mutex::new(ServerState::Stopped)),
            runs: Mutex::new(ImportRuns::new()),
        }
    }

    // -- store delegates: keeps the Tauri layer a thin shell -----------

    pub fn add_file(&self, path: impl Into<PathBuf>) -> Result<SharedFile, AddFileError> {
        self.store
            .lock()
            .expect("file store not poisoned")
            .add(path)
    }

    /// Remove a shared file. If that empties the share — no registered
    /// files and no root records — the server auto-stops, mirroring the
    /// old "last file" rule now that roots count as content (ADR-0005).
    pub async fn remove_file(&self, id: FileId) -> Result<SharedFile, RemoveError> {
        let removed = self
            .store
            .lock()
            .expect("file store not poisoned")
            .remove(id)?;
        self.auto_stop_when_empty().await;
        Ok(removed)
    }

    /// Remove a root record and everything materialized under it; same
    /// auto-stop rule as file removal.
    pub async fn remove_root(&self, id: RootId) -> Result<RootRemoval, RemoveError> {
        let removal = self
            .store
            .lock()
            .expect("file store not poisoned")
            .remove_root(id)?;
        self.auto_stop_when_empty().await;
        Ok(removal)
    }

    /// The imported root records, insertion-ordered.
    pub fn roots(&self) -> Vec<RootRecord> {
        self.store
            .lock()
            .expect("file store not poisoned")
            .roots()
            .to_vec()
    }

    /// Register the lazily-known direct children of an imported root's
    /// folder (ADR-0005). Idempotent, keyed by (folder root, relative
    /// path) — safe to call repeatedly or concurrently. A brief lock with
    /// bounded local work: only ever one folder deep.
    pub fn materialize_root(&self, id: RootId) -> Result<MaterializeOutcome, RemoveError> {
        self.store
            .lock()
            .expect("file store not poisoned")
            .materialize(id)
    }

    /// The listing derivation, shared command-shape: the top of the
    /// list (loose files plus one row per root record). Pure derivation,
    /// brief lock, no disk I/O.
    pub fn list_top(&self) -> crate::listing_pages::TopListing {
        self.store
            .lock()
            .expect("file store not poisoned")
            .list_top()
    }

    /// One paged window of a node's rows. The request itself
    /// materializes the node's direct children (ADR-0005 trigger), so
    /// the lock brief covers one directory's scan at most.
    pub fn list_page(
        &self,
        root_id: RootId,
        sub_path: &str,
        offset: usize,
        limit: usize,
    ) -> Result<crate::listing_pages::NodePage, RemoveError> {
        self.store
            .lock()
            .expect("file store not poisoned")
            .list_page(root_id, sub_path, offset, limit)
    }

    pub fn files(&self) -> Vec<SharedFile> {
        self.store
            .lock()
            .expect("file store not poisoned")
            .list()
            .to_vec()
    }

    /// Register many paths at once (the drag-and-drop case). Never fails
    /// wholesale: each path succeeds or fails independently. Directories
    /// walk into frozen snapshots of their regular files; files add as
    /// one. The routing lives in the store — this is the lock boundary.
    pub fn add_files(&self, paths: Vec<PathBuf>) -> AddFilesOutcome {
        self.store
            .lock()
            .expect("file store not poisoned")
            .add_paths(paths)
    }

    // -- import runs (ADR-0007) ------------------------------------------

    /// Begin an import of many paths: loose files register immediately
    /// and synchronously; folder roots start background count walks that
    /// commit root records when they finish. Progress and cancellation
    /// are observable per run through [`Self::import_run_status`] and
    /// [`Self::cancel_import`].
    pub async fn import_paths(&self, paths: Vec<PathBuf>) -> ImportStartOutcome {
        let mut immediate = AddFilesOutcome {
            added: Vec::new(),
            failed: Vec::new(),
        };
        let mut run: Option<Arc<ImportRun>> = None;

        for path in paths {
            // Same rule add_paths applies to dropped links: never follow.
            if crate::file_store::is_dropped_link(&path) {
                immediate.failed.push(FailedAdd {
                    path,
                    error: "symlinks can't be shared".into(),
                });
                continue;
            }

            if path.is_dir() {
                let import = match &run {
                    Some(import) => Arc::clone(import),
                    None => {
                        let begun = self.runs.lock().expect("import runs not poisoned").begin();
                        run = Some(Arc::clone(&begun));
                        begun
                    }
                };
                let root_index = import.push_root(path.clone());

                // One background count walk per root. The walk holds no
                // lock; its commit later takes the store mutex briefly.
                let context = ImportContext {
                    store: Arc::clone(&self.store),
                    state: Arc::clone(&self.state),
                    lan: Arc::clone(&self.lan),
                };
                tokio::spawn(count_and_commit(
                    Arc::clone(&import),
                    root_index,
                    path,
                    context,
                ));
            } else {
                match self
                    .store
                    .lock()
                    .expect("file store not poisoned")
                    .add(&path)
                {
                    Ok(file) => immediate.added.push(file),
                    Err(e) => immediate.failed.push(FailedAdd {
                        path,
                        error: e.to_string(),
                    }),
                }
            }
        }

        ImportStartOutcome {
            run_id: run.as_ref().map(|run| run.id),
            immediate,
        }
    }

    /// Cancel one run. Roots already committed stay committed; counting
    /// roots stop early and leave no record. `false` = unknown run id.
    pub fn cancel_import(&self, run_id: crate::import_run::RunId) -> bool {
        self.runs
            .lock()
            .expect("import runs not poisoned")
            .cancel(run_id)
    }

    /// Snapshot of every recorded run, oldest first (finished runs stay
    /// listed so their terminal state is observable).
    pub fn import_run_status(&self) -> Vec<ImportRunSnapshot> {
        self.runs
            .lock()
            .expect("import runs not poisoned")
            .snapshot()
    }

    /// Server auto-stop: after removals, an empty share stops serving.
    async fn auto_stop_when_empty(&self) {
        let empty = self
            .store
            .lock()
            .expect("file store not poisoned")
            .is_empty();
        if empty && self.status().running {
            let _ = self.stop().await;
        }
    }

    // -- lifecycle ------------------------------------------------------

    /// Start sharing on the default port (with fallback if taken).
    pub fn start(&self) -> Result<ServerInfo, SharerError> {
        self.start_on(DEFAULT_PORT)
    }

    /// Start sharing on `preferred_port` (0 = let the OS pick).
    pub fn start_on(&self, preferred_port: u16) -> Result<ServerInfo, SharerError> {
        start_core(&self.store, &self.state, &self.lan, preferred_port)
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

impl Default for Sharer {
    fn default() -> Self {
        Self::new()
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

/// The start logic, parameterized so a background import task can bring
/// the share up on its own — the frontend can't be relied on to notice a
/// root record committing later than its drop. Callable from async
/// contexts only (it spawns the serve task).
fn start_core(
    store: &Arc<Mutex<FileStore>>,
    state: &Arc<Mutex<ServerState>>,
    lan: &Arc<dyn LanAddress>,
    preferred_port: u16,
) -> Result<ServerInfo, SharerError> {
    let mut state = state.lock().expect("server state not poisoned");

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
    let lan_ip = lan.local_ip().unwrap_or(IpAddr::from([127, 0, 0, 1]));

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

    let app = router(AppState::new(Arc::clone(store)));

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

/// Everything a background count walk needs in order to commit its root
/// record and bring the share up — owned outright, since it outlives the
/// `Sharer` reference it was spawned from.
struct ImportContext {
    store: Arc<Mutex<FileStore>>,
    state: Arc<Mutex<ServerState>>,
    lan: Arc<dyn LanAddress>,
}

/// One root's background life: count without any lock, then commit the
/// root record under a brief lock, then bring the share up if nothing
/// is serving yet. A cancelled walk leaves no record; an unreadable
/// root fails its status per the per-root tolerance rule.
async fn count_and_commit(
    import: Arc<ImportRun>,
    root_index: usize,
    path: PathBuf,
    context: ImportContext,
) {
    let counted = tokio::task::spawn_blocking({
        let import = Arc::clone(&import);
        let count_path = path.clone();
        move || {
            count_folder(
                &count_path,
                &|| import.is_cancelled(),
                PROGRESS_EVERY_FILES,
                &mut |files| import.set_files(root_index, files),
            )
        }
    })
    .await;

    match counted {
        Ok(Ok(total)) => {
            // A cancellation may land between the walk's last poll and
            // here: re-check before committing, so a cancelled root
            // leaves no record even when the count itself finished.
            if import.is_cancelled() {
                import.set_status(root_index, crate::import_run::RootRunStatus::Cancelled);
                return;
            }
            // Brief lock: the store accepts one record and is dropped
            // before anything else happens (the no-await-across-lock rule).
            context
                .store
                .lock()
                .expect("file store not poisoned")
                .commit_root_record(&path, total);
            import.set_status(root_index, crate::import_run::RootRunStatus::Committed);

            // The count is content — "dropping files IS the start button",
            // even when the commit lands long after the drop.
            let running = matches!(
                &*context.state.lock().expect("server state not poisoned"),
                ServerState::Running { .. }
            );
            if !running {
                let _ = start_core(&context.store, &context.state, &context.lan, DEFAULT_PORT);
            }
        }
        Ok(Err(CountFolderError::Cancelled)) => {
            import.set_status(root_index, crate::import_run::RootRunStatus::Cancelled)
        }
        Ok(Err(CountFolderError::Unreadable(error))) => {
            import.set_status(root_index, crate::import_run::RootRunStatus::Failed(error))
        }
        Err(join) => import.set_status(
            root_index,
            crate::import_run::RootRunStatus::Failed(join.to_string()),
        ),
    }
}
