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

A path the user tried to share that could not be added (unreadable, special file, …). Reported back to the desktop UI without rejecting the rest of a batch. A **folder add** reports failed entries the same way, per entry.

## Folder root

A directory the user chose to add. A folder root is not itself a shareable entity: the files found beneath it are added as individual **shared files**. Folder roots are remembered so their members can be grouped, selected, and downloaded together. Two folder roots with the same name may coexist independently.

## Frozen snapshot

The membership model for folder adds: which files belong to a folder root is decided once, at add time, by walking the tree then. Files inside keep being streamed from disk live, so content changes on disk are visible, but membership and membership-derived structure (grouping, zip layout) are not re-scanned. Consistent with how file sizes are captured at add time while contents stream live.

## Loose file

A shared file added directly rather than under a folder root. Loose files are ungrouped in lists and sit at the root of any zip archive containing them.

## Relative path

A member file's path below its folder root (e.g. `vacation/beach.jpg` under root `Photos`). Used for grouping in the UIs and for entry paths inside zip archives. Only defined for members of a folder root; loose files don't have one.

## Shared file (SharedFile)

A single regular file registered with the sharer: an immutable **file id**, a display **name**, an absolute on-disk **path**, and a **size** captured at add time. The only shared entity; there is no "folder" entity in the store.
