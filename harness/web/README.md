# Web UI harness (Playwright)

Drives the hosted web listing — the page a phones see after scanning
the QR code — in a real browser with DOM assertions and screenshots
(issue #7). The listing is plain server-rendered HTML plus a small
selection script, so a plain Chromium run needs no Tauri.

## One command: `yarn harness:web`

Starts the axum fixture servers (`cargo run -p mosquito-core --example
harness_server`, loopback only) and runs `harness/web/listing.spec.ts`:

| Test | Server | Asserts |
| --- | --- | --- |
| empty listing state | `127.0.0.1:8474` | empty-state text, no rows, no select-all |
| loose files first | `127.0.0.1:8473` | the top page's loose rows before any folder, JS-driven checkboxes |
| fragment-loaded root | `127.0.0.1:8473` | one root row carrying its import-time count, its stamped expansion auto-fetching the fragment (`beach.jpg` at depth 1, `notes` collapsed), nested member **absent** until expanded, depth-shading and depth attributes |
| chevron toggles | `127.0.0.1:8473` | chevron-only expand/collapse, region re-shading, row/name clicks are no-ops, collapse prunes rows (re-expand refetches) |
| selection | `127.0.0.1:8473` | folder rows carry node checkboxes (issue #27): a node pick links the node route alone or merges with file ids (`/download/zip?ids=…&nodes=…`); checked/mixed/unchecked per the desktop group rule; unchecking drops the subtree's member picks and nested node selections; select-all spans rendered files + every root's node selection; collapse prunes file-id picks but a folder selection survives |

Screenshots land in a deterministic, gitignored directory —
`test-artifacts/web-harness/` (`empty-list`, `loose-files`,
`folder-groups`, `folder-toggles`, `selections`,
`surviving-selections` — same names every run).

## Stable selectors (the test contract)

The listing is server-rendered in fragments (ADR-0006): the initial
page carries the top level only, and each expanded node's rows arrive
from its own page through the enhancement script's fetches — same
code path as the initial render. The visual language mirrors the
desktop list's — a deliberate mirror-by-duplicated-tokens, not shared
code (ADR-0003) — so the web suite drives the same stable handles the
desktop contract documents (`harness/desktop/README.md`). Specs target
these selectors and the `data-*` attributes, never layout or generated
markup, so the served page can change without harness breakage:

| Selector | Meaning |
| --- | --- |
| `.empty-state` | the empty listing's message "Nothing is being shared right now." |
| `.file-row` / `.file-row.is-selected` | file rows (loose and tree members alike), highlighted rows |
| `.file-row[data-depth="n"]` | a tree member's nesting depth (loose files carry none) |
| `.folder-row` | an expandable folder row (root or subfolder) |
| `.folder-row[data-folder-key="Photographs"]` / `.folder-row[data-folder-key="Photographs/notes"]` | a folder row by its tree key: the folder root's display name plus its subfolder path; the first root to carry a name keeps it bare, each later same-named root appends `-2`, `-3`, … Keys are appearance-ranked: they hold across re-renders and added roots, and only renumber when the first-appearance order of same-named roots changes (e.g. removing the earlier same-named root) |
| `.folder-row[data-node-root="<id>"][data-node-sub="<sub>"]` | a folder row's node address for its fragment: the root record id plus the sub path relative to it — never an absolute filesystem path, and no pre-registration token (the root record id is stable, unlike the old mirror's path payload) |
| `.folder-name` | a folder row's display name |
| `.folder-count` | a root row's import-time file count (display only — showing it materializes nothing, issue #24) |
| `[aria-label="Collapse <name>"]` / `[aria-label="Expand <name>"]` | the chevron only — the folder row, its name, and its body never toggle |
| `[aria-expanded="true"]` / `[aria-expanded="false"]` | a folder's expansion state, seated by the server's five-children stamp and re-derived by the fragment's `aria-expanded` rows |
| `[data-shade]` / `[data-shade="1"]` / `[data-shade="0"]` | the grey-region shade attribute, alternating per nesting level; `1` = shaded grey, `0` = plain surface |
| `.materialize-marker` | a node's inline materialization-failure marker (ADR-0005): a count only, no failing paths |
| `button[data-select="<id>"]` | a per-file checkbox (`aria-checked` `false`/`true`; hidden until the script runs; **disabled** while an ancestor folder's node selection covers it — see below) |
| `button[data-node-select]` | a folder row's node checkbox (hidden until the script runs) — selects the node's whole subtree by address, never member file ids; aria state is `false`/`true`/`mixed`, mirroring the desktop root checkbox: checked only while the node itself is selected, mixed when some rendered member (picked file row or picked subfolder) participates |
| `#select-all` | select-all header checkbox (`aria-checked` `false`/`true`/`mixed`) — covers exactly the rendered file rows plus every root's node selection, so an expansion makes it drop to `mixed` until re-clicked |
| `#download-selected` / `#download-selected.is-visible` | icon-only download button, invisible until something is selected; `href` is `/download/<id>` for one file, `/download/zip/node/<id>?sub=<sub>` for one node, `/download/zip?ids=<ids>&nodes=<nodes>` for anything else — node addresses arrive as `nodes=<root_id>:<sub>,…` with the sub percent-encoded per address |

Each fragment answers as one inert `<template data-node-key …
data-offset data-window-rows data-total data-has-more>` wrapping flat
`<li>` rows; the enhancement script swaps them into the flat list and
advances the node's fetch cursor by the stamps. File rows select by
individual file id (`data-select`); folder rows select by node address
(`data-node-root` + `data-node-sub`, issue #27) — checking a folder
never materializes anything, a folder selection survives its row being
collapsed by a parent, and a mixed selection (ids + node addresses)
downloads as one merged archive.

And the group's tick reaches the rows it covers: while an ancestor
folder's node selection covers a rendered row, that row shows the
selection and its own checkbox goes **disabled** — node selections
have no member exclusions, so the covering folder is the way to change
the scope. The covered state derives from the flat row ancestry
(preceding rows of strictly-shallower depth) and mirrors the desktop's
`FileList.vue`.

Rows a fragment's fetch adds render only when their ancestors are
expanded (collapse **removes** them from the DOM); tests assert that
as presence (`toHaveCount`, `:visible`) rather than any hidden
attribute.

Same tree, same contract wording where the two UIs correspond — but
the implementations really do differ in places, so the READMEs differ
too:

- Shading is a `data-shade` **attribute** on the web where the app uses
  a `shade-1` **class**; both mean the same alternating grey region
  (ADR-0002), with the 16px-per-depth indent (name text, chevron, and
  member catch-up) mirroring too (ADR-0004).
- Folder rows: the app's root rows carry a subtree checkbox and its
  subfolder rows a spacer, where the web gives **every** folder row
  its node checkbox (issue #27) — selection resolves through the node
  on both surfaces, the node-level payload form riding the archive
  route (ADR-0006, ADR-0005's archive trigger).
- Web row bodies are download links (per-file download works with
  JavaScript off), where app rows select on click; the web suite
  therefore always selects through checkboxes.

## Servers by hand

```sh
yarn harness:web:server          # fixture tree in $TEMP\mosquito-harness\web
# then open:
#   http://127.0.0.1:8473  — loose files + folder group
#   http://127.0.0.1:8474  — empty listing
```

Playwright reuses an already-listening server (`reuseExistingServer`),
so pointing it at your own Terminal run is enough:

```sh
yarn playwright test --config harness/web/playwright.config.ts
```

## Debug when a test fails

- `yarn playwright test --config harness/web/playwright.config.ts --headed` to
  watch the browser.
- Page snapshots on failure are kept under Playwright's `test-results/`
  (gitignored too).
- Fixture sources sit in `crates/mosquito-core/examples/harness_server.rs`;
  the ports are fixed in `harness/web/ports.ts` (8473 fixture, 8474
  empty — away from the app's 8472) so a desktop dev build can run at
  the same time.

## What the fixture data looks like

`recipes.txt` + `checklist.txt` loose, then the folder group
`Photographs` seeded as a count-only root record (the import shape,
ADR-0006): 2 direct children fits the five-children budget, so the
root renders stamped-expanded and its fragment auto-fetches on load —
`beach.jpg` directly inside (materialized as id 2 when the page that
shows it fetches) and the `notes` subfolder (collapsed by default)
holding `todo.txt` (id 3, materialized by its own node's page). The
ids the selection test therefore sees: 0–1 loose, 2–3 shown through
expansions.
