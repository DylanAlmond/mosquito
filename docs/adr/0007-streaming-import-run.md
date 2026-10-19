# ADR-0007: Streaming import run — lock-free counting walk with progress and cancellation

Date: 2026-10-18
Status: Accepted

## Context

Importing content was one synchronous IPC call into a store whose mutex was held for
the entire walk ("add_files is the lock boundary"): one giant outcome returned at the
end, no progress, no cancellation, UI blocked. ADR-0005 redefined what an import
records (count-only root records), which removes the walk's memory spike, but the
*time* — counting a whole drive can take minutes on spinning disks — and the ownership
of the store lock still needed deciding.

## Decision

- An import is a **run**: a background task that processes the dropped set, reporting
  progress per folder root, cancellable independently per run. The desktop keeps full
  UI availability while a run is in flight — further drops become further runs,
  and adds, removals, and server controls never block.
- The counting walk **never holds the store mutex**. Each root's record is committed
  under a brief lock when it completes counting; materialized inserts (later, from
  listings/archive) hold the mutex only for their own bounded inserts. The
  lock-held-for-the-walk boundary is dead.
- Walk memory is O(depth): directory enumeration plus a per-root counter, no
  accumulation. The walk skips symlinks/junctions without following them, per
  ADR-0001's rule, and counts only regular files, so the reported count and a later
  materialization agree in kind.
- **Cancelling discards the root entirely** — a half-done count is worthless as a
  record; re-dropping is one click. Loose files register per-entry regardless, so a
  cancelled run still keeps its loose adds.
- Progress is throttled (coarse, per-root: "counting <root> — N entries"). The
  core exposes per-root progress through queryable run snapshots; the Tauri
  layer turns these into desktop events when the import-run UI lands (the
  event emission is a UI-boundary concern, not core machinery). There is no
  percentage because that would require counting twice.
- The server auto-starts when the first committed content exists — a root record
  counts as content (ADR-0005) — and auto-stop applies when no root records and no
  shared files remain, replacing the old "last file" rule.

## Consequences

- Import failures reduce to two kinds: loose-file and root-level failures (reported
  as failed adds, as before) and later per-node materialization failures (ADR-0005),
  never global batch failures.
- Concurrency between a running count walk, materializations, and user removals is
  safe by construction from the lock discipline: walks mutate nothing until commit,
  and idempotent keyed inserts absorb overlapping registrations.
- Report-out tests must treat counts as event-driven and toleranced, not exact
  mid-run; the progress throttle makes intermediate numbers approximate by design.
