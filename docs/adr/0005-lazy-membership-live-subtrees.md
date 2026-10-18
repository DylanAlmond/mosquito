# ADR-0005: Lazy membership — count-only imports and materialization on demand

Date: 2026-10-18
Status: Accepted (revises ADR-0001)

## Context

Folder imports faithfully followed ADR-0001: registering a folder walked the entire
tree at add time and built one shared file per regular file found, all inside a single
locking `add_files` call that returned one `AddFilesOutcome`. For the scale this must
now handle — entire drives dropped into the app, in the millions of files — that design
has three failures:

1. The store itself grows to hold every path in the drive immediately, whether or not
   anyone ever looks at or downloads it (hundreds of MB at a million entries).
2. The store's mutex is held for the whole walk, so the UI is blocked for the duration.
3. The single IPC call gives the desktop zero feedback until the walk ends.

Two directions were weighed:

1. Keep eager registration and fix the walk (streaming commits, progress, cancellation).
   The walk spike and the UI freeze go away, but the store still swallows the entire
   drive — the dominant memory cost.
2. Register nothing at import time: import records the folder root and a file count
   only; individual files are registered when something actually needs them.

Option 1 was rejected because it defends the wrong asset: the share list itself, not a
transient buffer, is what grows. Option 2 keeps the steady-state cost proportional to
what users actually interact with, at the price of a changed membership contract —
previously a folder add was a **frozen snapshot** taken at add time (ADR-0001); a
counted-but-unregistered root no longer promises that.

## Decision

Imports record lazily; use time registers for real:

- Importing a folder records a **root record** — the absolute path plus a file count
  from a count-mostly walk (directory enumeration, no per-file stat). Loose files in
  the same drop register immediately, exactly as before.
- Nothing under a root record is registered at import time. Until registered, its
  contents are a **live subtree**: listings scan the directory when viewed, downloads
  read the disk when they happen, so files added or deleted after import show up.
- The triggers that register are exactly: a listing showing that folder (desktop
  expand or receiver expand — the listing request itself materializes), and an archive
  download requiring the subtree. One materialization registers the folder's **direct
  children only**; deeper levels materialize on their own turn, keeping memory
  proportional to what is actually viewed.
- Materialization is **idempotent, keyed by (folder root, relative path)**: concurrent
  materializations converge and never double-register. Re-dropping the same folder
  still creates an independent root record whose tree resolves to the same shared
  files — duplicates remain legal, resolved at presentation time as in ADR-0001.
- Materialized entries follow the frozen-snapshot contract: registration fixes
  identity, name, and size as with any add. The two-tier contract is the glossary's
  *live subtree* vs *frozen snapshot*, which ADR-0001's original single reading of
  "membership never re-scans" is superseded by.
- Removing a folder group removes the root record and every shared file materialized
  under it. A store holding only root records (with counts) is a perfectly shareable
  state: a receiver opens the URL, sees the tree, and expanding materializes.
- Receiver-facing URLs never carry disk-absolute paths or pre-registration tokens:
  only materialized entries have ids and download links; lazy nodes are expandable
  rows, not clickable links. Materialization rides the listing expand, so no token
  URL form exists.

## Consequences

- Steady-state memory scales with what is registered (viewed/downloaded), not with
  what was imported. Importing a whole drive costs a count walk's time, not a
  million-entry store.
- ADR-0001's per-entry tolerance carries over: unreadable entries at materialization
  are per-folder inline markers in the listing ("materialization failures"), not global
  failures and not import failures.
- Membership shown in counts recorded at import can drift from the disk by the time a
  subtree is viewed — that is the live-subtree contract, accepted deliberately.
- Archive downloads of lazily-selected subtrees materialize the full subtree first
  (kept out of the shared "browse" path), so a whole-drive zip still pays large-scale
  planning once — deliberately, because the receiver explicitly asked for it.
