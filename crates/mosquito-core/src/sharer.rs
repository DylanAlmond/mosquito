use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::error::SharerError;
use crate::file_store::{
    AddFilesOutcome, FailedAdd, FileId, FileStore, MaterializeOutcome, NodeRemoval, RootId,
    RootRecord, RootRemoval, SharedFile,
};
use crate::import_run::{
    CountFolderError, ImportRun, ImportRunSnapshot, ImportRuns, PROGRESS_EVERY_FILES,
    RootRunStatus, count_folder,
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

    /// Remove every materialized member under one node address (sub
    /// path relative to the record). The record survives, so the share
    /// can never empty through this — no auto-stop check: the empty-sub
    /// path delegates to `remove_root`, which carries the rule itself.
    pub async fn remove_node(
        &self,
        id: RootId,
        sub_path: &str,
    ) -> Result<NodeRemoval, RemoveError> {
        self.store
            .lock()
            .expect("file store not poisoned")
            .remove_node(id, sub_path)
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
    /// and synchronously; folder roots commit their root record at drop
    /// — synchronously, one brief lock, count 0 — and the server
    /// auto-starts in the same synchronous path, so drop → server
    /// listening is milliseconds, never gated by the walk (ADR-0008).
    /// The walk then runs in the background, keeping the record's count
    /// live at its checkpoints; progress and cancellation are observable
    /// per run through [`Self::import_run_status`] and
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

                // Committed at drop (ADR-0008): one brief lock, count 0,
                // before the walk starts. The walk's outcome decides
                // whether the record stays — see [`live_count_walk`].
                let root_id = {
                    let context = ImportContext {
                        store: Arc::clone(&self.store),
                        state: Arc::clone(&self.state),
                    };
                    context
                        .store
                        .lock()
                        .expect("file store not poisoned")
                        .commit_root_record(&path, 0)
                        .id
                };

                // The count is content — "dropping files IS the start
                // button" — and the server comes up in the same
                // synchronous path as the commit, ahead of the walk.
                let running = matches!(
                    &*self.state.lock().expect("server state not poisoned"),
                    ServerState::Running { .. }
                );
                if !running {
                    // A taken port falls back inside start_core; any
                    // other failure surfaces through the server UI, as
                    // when the user starts it themselves.
                    let _ = self.start();
                }

                // One background count walk per root. The walk holds no
                // lock across enumeration; each checkpoint takes the
                // store mutex briefly.
                tokio::spawn(live_count_walk(
                    Arc::clone(&import),
                    root_index,
                    path,
                    root_id,
                    ImportContext {
                        store: Arc::clone(&self.store),
                        state: Arc::clone(&self.state),
                    },
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

    /// Cancel one run. The outcome contract is removal (ADR-0008):
    /// counting roots stop early and their records — committed at drop —
    /// are removed, everything materialized under them with them; roots
    /// already committed stay. `false` = unknown run id.
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
        shutdown(&self.state).await
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

/// Stop a server on bare state — the extract-then-await pattern the
/// Sharer::stop path and the background removal path share: the lock
/// never spans an `.await`, which is how std::sync::Mutex and async
/// code coexist safely. `Sharer::stop` keeps its "stop without start is
/// an error" contract; the background removal caller pre-checks
/// `running` and swallows the impossible-by-timing `NotRunning`.
async fn shutdown(state: &Arc<Mutex<ServerState>>) -> Result<(), SharerError> {
    let (shutdown, task) = {
        let mut state = state.lock().expect("server state not poisoned");
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

/// Everything a background count walk needs to keep its root record's
/// count live and to act on the walk's outcome (ADR-0008) — owned
/// outright, since it outlives the `Sharer` reference it was spawned
/// from. The LAN address lives with the `Sharer`: the server auto-start
/// happens in the drop's own synchronous path, not in the walk.
#[derive(Clone)]
struct ImportContext {
    store: Arc<Mutex<FileStore>>,
    state: Arc<Mutex<ServerState>>,
}

impl ImportContext {
    /// The walk's checkpoint heartbeat (ADR-0008's live count): the run
    /// board's per-root copy moves first, then the record's count under
    /// one brief lock per checkpoint. Display only — no registration and
    /// no materialization follows from it; a record that was already
    /// removed (cancel, failure, the user's own removal) takes a late
    /// checkpoint as a no-op.
    fn checkpoint(&self, import: &ImportRun, root_index: usize, root_id: RootId, files: u64) {
        import.set_files(root_index, files);
        self.store
            .lock()
            .expect("file store not poisoned")
            .update_root_count(root_id, files);
    }

    /// When a removal — cancellation or failure — empties the share (no
    /// registered files and no root records left), the server stops, the
    /// same rule a user's own removal of the last content triggers.
    async fn stop_when_empty(&self) {
        let empty = self
            .store
            .lock()
            .expect("file store not poisoned")
            .is_empty();
        if empty {
            let running = matches!(
                &*self.state.lock().expect("server state not poisoned"),
                ServerState::Running { .. }
            );
            if running {
                let _ = shutdown(&self.state).await;
            }
        }
    }

    /// The walk-outcome removal (ADR-0008): the record plus everything
    /// materialized under it goes in one sweep — the store's own
    /// `remove_root`, the same action the user's delete uses — so the
    /// cancelled/failed half-count leaves nothing behind even though the
    /// drop committed the record. An already-removed record reports its
    /// outcome to the run board all the same.
    async fn remove_record(
        &self,
        import: &ImportRun,
        root_index: usize,
        id: RootId,
        status: crate::import_run::RootRunStatus,
    ) {
        let _ = self
            .store
            .lock()
            .expect("file store not poisoned")
            .remove_root(id);
        import.set_status(root_index, status);

        self.stop_when_empty().await;
    }
}

/// One root's background life (ADR-0008): record and server already
/// exist from the synchronous drop path. The walk keeps the record's
/// count live at its checkpoints without holding anything; its outcome
/// decides whether the record stays — completion leaves the final
/// number, cancellation and failure remove the record again.
async fn live_count_walk(
    import: Arc<ImportRun>,
    root_index: usize,
    path: PathBuf,
    root_id: RootId,
    context: ImportContext,
) {
    let counted = tokio::task::spawn_blocking({
        let import = Arc::clone(&import);
        let context = context.clone();
        let count_path = path.clone();
        move || {
            count_folder(
                &count_path,
                &|| import.is_cancelled(),
                PROGRESS_EVERY_FILES,
                &mut |files| context.checkpoint(&import, root_index, root_id, files),
            )
        }
    })
    .await;

    match counted {
        Ok(Ok(_total)) => {
            // The completion heartbeat already left the final count on
            // the record. A cancellation may still land between the
            // walk's last poll and here: re-check before keeping the
            // record — a cancelled root's outcome is removal, even when
            // the count itself finished.
            if import.is_cancelled() {
                context
                    .remove_record(&import, root_index, root_id, RootRunStatus::Cancelled)
                    .await;
                return;
            }
            import.set_status(root_index, RootRunStatus::Committed);
        }
        Ok(Err(CountFolderError::Cancelled)) => {
            context
                .remove_record(&import, root_index, root_id, RootRunStatus::Cancelled)
                .await
        }
        Ok(Err(CountFolderError::Unreadable(error))) => {
            // Failed-add semantics keep exactly their old meaning — no
            // record stands, the failure surfaces once through the run
            // board's banner flow — reached now by removal.
            context
                .remove_record(&import, root_index, root_id, RootRunStatus::Failed(error))
                .await
        }
        // The walk task itself died — that is a bug, but leaving a stuck
        // zero-count record with no walk would live-count forever.
        Err(join) => {
            context
                .remove_record(
                    &import,
                    root_index,
                    root_id,
                    RootRunStatus::Failed(join.to_string()),
                )
                .await
        }
    }
}
