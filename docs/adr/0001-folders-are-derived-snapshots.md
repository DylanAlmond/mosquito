# ADR-0001: Folders are derived snapshots, not entities

Date: 2026-10-17
Status: Accepted

## Context

The file store holds a flat, insertion-ordered list of shared files identified by
sequential u64 ids. Adding a directory has so far been an error. Folder support needs
the store, the desktop list UI, and zip download to agree on what "adding a folder"
means.

Two plausible models:

1. A hierarchical `Folder` entity tree in the store, files referencing parents.
2. A flat, **derived** tree: folders are remembered only as *folder roots*, each file
   added beneath one carries its path relative to that root, and folder structure is
   presented by grouping on those relative paths.

## Decision

Adopt the derived model (option 2):

- No `Folder` entity exists in the store. A folder add recursively walks the tree at
  add time and registers one shared file per readable regular file found — a **frozen
  snapshot** of membership. Files added directly remain, and are treated as, loose at
  the tree's edge.
- Membership never changes after the add; file contents are streamed from disk live,
  which is already the app's behaviour for sizes and contents.
- Empty directories are not represented anywhere; they are invisible in the UIs and
  absent from archives.
- A dropped path that is itself a symlink is rejected as a failed add, the same rule
  that skips links encountered inside the walk: links never get followed.
- Entry paths inside zip archives, folder grouping in both UIs, and "remove a whole
  folder" are all *derived* from the stored relative paths and root names at use time.

## Consequences

- The store stays flat; ids, selection, and bulk removal keep working unchanged.
- Selection and removal operate on file ids even when the user interacts via a folder
  group; a folder "group" is a view, not a thing that can be stored or removed itself.
- Duplicates stay legal (the same path may be added twice, two roots may share a name);
  collisions are resolved at presentation/archive-write time, not in the store.
- Deleting a file from disk after adding it now makes one member of a group dangling,
  visible as a failure only when it is downloaded or zipped.
- If folder needs grow (renaming a folder, moving a subtree), this model will need
  revisiting; that would be a new ADR, not a silent change.
