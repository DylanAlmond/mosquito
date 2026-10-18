# Glossary

Terms are listed alphabetically. Keep this file free of implementation details — it defines words, not designs.

## Add / Adding

Registering a file that already exists on the user's disk with the sharer, so it becomes available for download over the local network. Nothing is copied, transferred, or "uploaded" — the server streams the file's bytes from its on-disk location when a device downloads it. The canonical verb is **add**; "upload" is not a term in this domain.

## Archive entry path

A shared file's path inside a zip archive, derived at download time: a folder member keeps its **folder root**'s name as a prefix over its **relative path** (`Photos/vacation/beach.jpg`); a **loose file** sits at the archive root under its display name. Colliding entry paths get numeric suffixes (` (2)`) so nothing is dropped. Distinct from the on-disk path, which stays absolute and never enters the archive.

## Content-Disposition encoding

The header treatment that gives downloads a safe filename, including RFC 5987 UTF-8 encoding for non-ASCII names. Reused by any new download surface (including zip archives).

## Download

Transferring a shared file's bytes to a device over the local network via the HTTP server. Distinct from **add**, which happens on the desktop side only.

## Failed add

A path the user tried to share that could not be added (unreadable, special file, …). Reported back to the desktop UI without rejecting the rest of a batch. Scoped to imports: loose-file adds and **folder roots** that could not be counted. Errors reading individual folder members later are **materialization failures**, not failed adds.

## Folder root

A directory the user chose to share wholesale rather than registering individual files. On import a folder root is held by the store as a **root record**; its members are not registered up front. Two folder roots with the same name may coexist independently.

## Folder tree

The derived structure the UIs and zip layout present: the **folder roots** as top-level folders, and beneath each, into arbitrarily deep nesting, the files and **subfolders** implied by materialized members' **relative paths** and — inside a still-**live subtree** — by the directory scan at view time. Derived at render time and never stored; a folder in the tree is never itself shareable or mutable — only its contents are.

## Frozen snapshot

The membership model for materialized content: which files belong where is decided at add or **materialize** time and is never re-scanned for registered **shared files** — grouping, the **folder tree**, and zip layout derive only from those registrations, and sizes are captured at registration time. The files inside keep streaming from disk live, so content changes on disk remain visible. Contents of a **root record** that are not yet materialized follow the **live subtree** contract instead.

## Import

Bringing content from the disk into the share. Loose files are registered immediately; a folder is imported by recording a **root record** and counting the files beneath it — nothing under the root is registered at import time. Import happens on the desktop side only; its counterpart on devices is **download**. Distinct from **add**, which is the per-file registration act. _Avoid_: upload, ingest.

## Import run

A single user-initiated import, from the moment it starts until every folder root and loose file in it is counted, added, fails, or is cancelled. Progress is reported per folder root. A cancelled folder import leaves no root record — the record committed at import start is removed when the walk is cancelled or fails, so a half-done count never stands as a record. _Avoid_: batch, job, discovery.

## Live subtree

A folder root's contents that have not yet been materialized: listings show them by scanning the disk when viewed, and downloads reflect the disk when they happen — including files added or deleted after the import. Once a folder inside is materialized, its membership is frozen (see **frozen snapshot**). _Avoid_: virtual folder.

## Live count

The count a **root record** holds while its **import run's** counting walk is still in flight: committed at 0 when the folder is dropped, updated by the walk's progress checkpoints, display-only on both surfaces. Distinct from a final count, which is simply the last number the walk left. _Avoid_: discovery count, provisional count.

## Loose file

A shared file added directly rather than under a folder root. Loose files are ungrouped in lists and sit at the root of any zip archive containing them.

## Materialization failure

A folder member that could not be registered when its parent folder was materialized (unreadable, special file, …). Reported inline on that folder's listing rather than as part of an import outcome, and it never blocks the rest of the folder. _Avoid_: failed add, skip.

## Materialize

Registering the lazily-known members of a **root record** as **shared files** at first need: when a listing shows that folder, or when a download or archive requires them. One materialization covers a folder's direct children; deeper levels materialize on their own turn. _Avoid_: hydrate, expand, lazy load.

## Page

A request-scoped window of entries in a listing — the unit the UI fetches next when the user asks for more within a folder. Distinct from a whole-folder listing request. _Avoid_: chunk, batch.

## Relative path

A member file's path below its folder root (e.g. `vacation/beach.jpg` under root `Photos`). Used for grouping in the UIs, for identifying lazily-known members inside a **live subtree**, and for entry paths inside zip archives. Only defined for members of a folder root; loose files don't have one.

## Root record

The store's record of an imported folder root: its absolute path and a file count. The record commits when the folder is dropped (ADR-0008), with the count going **live** until the import run's counting walk ends. A root record is not itself shareable or downloadable; its members become **shared files** only as they are materialized.

## Shared file (SharedFile)

A single regular file registered with the sharer: an immutable **file id**, a display **name**, an absolute on-disk **path**, and a **size** captured at add time. Shared files are the only downloadable entities; a lazily-known file under a root record does not exist as a shared file until it is materialized.

## Subfolder

A folder nested below a **folder root** in the **folder tree**, named by a segment of its members' **relative paths** (e.g. `vacation` under root `Photos`, containing `beach.jpg`). A subfolder is a part of the derived tree, not an entity: it cannot be renamed, moved, removed, or shared — only its contents, through the shared files it derives from.
