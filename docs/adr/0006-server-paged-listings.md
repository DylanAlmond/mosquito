# ADR-0006: Server-paged listings; the folder-tree mirror dissolves into the server

Date: 2026-10-18
Status: Accepted (revises ADR-0002, extends ADR-0004)

## Context

ADR-0002 made the folder tree a derived view computed independently by both UIs —
server-rendered HTML in `listing.rs` and the desktop's `folderTree.ts` — kept equal by
mirroring tests. Both surfaces render the *entire* store: `render_listing` strings the
whole tree into one HTML document, and the desktop pulls one complete `list_files` for
its Vue tree. Expansion on both surfaces is client-side toggling of pre-rendered rows;
nothing is ever fetched dynamically.

Under ADR-0005's lazy membership that shape stops working: most of the tree has no
registered rows to pre-render, and "render everything" is exactly the DOM and payload
explosion to avoid once shares hold thousands of entries per folder. Two transport
directions were weighed:

1. Add JSON endpoints and rewrite the receiver's listing as a client-rendered app.
2. Keep server rendering: add per-node fragment routes served from the same rendering
   code that builds the initial page, and have the desktop consume the same store code
   path via Tauri commands.

Option 1 was rejected: it converts the receiver's page into a JS application, orphans
the existing inline-script architecture, and buys nothing here.

## Decision

- Folde-tree derivation happens **only on the server, per request**. The desktop no
  longer derives the tree; `folderTree.ts` and its mirroring tests are dissolved.
  The server-side derivation is the single source of tree truth, and the desktop
  fetches pages of rows through a Tauri command that sits on the identical store code
  path the web fragment route uses — so both surfaces are equal by construction
  rather than by mirrored implementations.
- The listing pages **per folder node**: each request returns one request-scoped page
  (about 200 entries) of that node's materialized rows plus its lazily-known
  subfolder rows. The client fetches the next page by **infinite scroll** (fetch-ahead
  near the end of rendered entries), identical on desktop and web; there is no
  "load more" affordance.
- The receiver's page is still server-rendered HTML: `GET /` renders top-level rows
  (loose files plus one collapsed row per root record), and a fragment route serves
  per-node pages for expansion. The inline script grows from toggle-only to fragment
  swap plus scroll-fetch.
- ADR-0004's five-children rule (roots with few direct children default to expanded)
  survives, now expressed once on the server; the cross-language pin disappears
  together with the mirror.
- Folder-checkbox subtree payloads (today `data-subtree` id lists) are dropped for
  lazily-known content: subtree selection resolves through the node, registered
  members keep id-based selection.

## Consequences

- No surface ever sends, holds, or renders all entries at once; payload, DOM, and
  frontend memory are bounded per page.
- Equality between desktop and web listing is now architectural, not tested-by-twin —
  the mirror tests are deleted rather than duplicated further.
- Tree state (expanded/collapsed per node) stays view-level and unsaved (ADR-0002's
  rule unchanged), now kept beside the paging state that drives fetch-ahead.
- Mid-list ordering guarantees only hold within a page as fetched; a live subtree may
  present differently on a later page request than an earlier one (live-subtree
  contract from ADR-0005).
