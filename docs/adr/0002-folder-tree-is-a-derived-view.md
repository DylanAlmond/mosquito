# ADR-0002: The folder tree is a derived view; subfolders are not entities

Date: 2026-10-17
Status: Accepted

## Context

ADR-0001 made folder grouping a purely derived view: the store holds flat
`SharedFile`s with `folder_root` + `relative_path` provenance, and no `Folder`
entity. The first UI drew each folder root as a flat, non-expandable group
header with its members listed as rows whose titles carried the relative path
(`travel/beach.jpg`). Subfolders were therefore text, not structure: they could
not be expanded, collapsed, or selected as a subtree.

The listing now needs to present real structure — expandable folders, nested
subfolders, subtree selection — in both UIs. This triggers ADR-0001's own
caveat: "if folder needs grow, this model will need revisiting; that would be
a new ADR".

Two options:

1. Introduce a stored `Folder` entity (or store a parsed path tree), files
   referencing folders; the UIs render the store directly.
2. Keep the store flat and derive the tree at render time from relative paths,
   deepening the existing one-per-language derivation that both UIs already
   mirror.

## Decision

Option 2: the folder tree remains fully derived (ADR-0001 undisturbed).

- The flat derivation that produced one group per folder root deepens into a
  tree: each folder node holds directly-contained files plus subfolder
  children, recursively; ordering is insertion order throughout (files before
  subfolders).
- Subfolders stay non-entities: they cannot be renamed, moved, removed, or
  shared. Ownership and mutation remain at the shared-file level; subtree
  selection and bulk removal keep resolving to file ids.
- Both UIs compute the tree independently but identically — the existing
  convention that the server-rendered listing mirrors the desktop list
  (`FileList.vue`) extends to the tree.
- Web assumes JavaScript is present; the server renders the tree in its
  default (top-level expanded) state and the enhanced script adds toggling.
  _(Narrowed by ADR-0004 in 2026-10: roots with more than five direct
  children now start collapsed on both surfaces; the mirror stays
  desktop-first.)_

## Consequences

- The store, ids, selection, and archive entry paths are unchanged; zip
  layout already expressed subfolders via entry paths.
- Two mirrored implementations of the derivation must be kept equivalent —
  tests on both sides and in both harnesses enforce the mirroring.
- Subfolder state (expanded/collapsed) is view-level and lives with the UI,
  never the store; it survives re-reads within a session but is not persisted.
- Renaming or moving a subtree would require revisiting this decision again —
  there is still no stored structure to shuffle.
