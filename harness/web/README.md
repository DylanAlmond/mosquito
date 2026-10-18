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
| loose files first | `127.0.0.1:8473` | loose rows before any folder, JS-driven checkboxes |
| folder tree | `127.0.0.1:8473` | `Photographs` root expanded at depth 0, `notes` subfolder collapsed with its member hidden, depth-shading and depth attributes |
| chevron toggles | `127.0.0.1:8473` | chevron-only expand/collapse, region re-shading, row/name clicks are no-ops |
| subtree selection | `127.0.0.1:8473` | select-all → all highlighted + zip link; folder selection counts folded members → mixed on partial; batch-download link includes hidden ids |

Screenshots land in a deterministic, gitignored directory —
`test-artifacts/web-harness/` (`empty-list`, `loose-files`,
`folder-groups`, `folder-toggles`, `selections` — same names every
run).

## Stable selectors (the test contract)

The listing mirrors the desktop list's tree (ADR-0002) and — a
deliberate mirror-by-duplicated-tokens, not shared code (ADR-0003) —
its visual language too, so the web suite drives the same stable
handles the desktop contract documents (`harness/desktop/README.md`).
Specs target these selectors and the `data-*` attributes, never layout
or generated markup, so the served page can change without harness
breakage:

| Selector | Meaning |
| --- | --- |
| `.empty-state` | the empty listing's message "Nothing is being shared right now." |
| `.file-row` / `.file-row.is-selected` | file rows (loose and tree members alike), highlighted rows |
| `.file-row[data-depth="n"]` | a tree member's nesting depth (loose files carry none) |
| `.folder-row` | an expandable folder row (root or subfolder) |
| `.folder-row[data-folder-key="Photographs"]` / `.folder-row[data-folder-key="Photographs/notes"]` | a folder row by its tree key: the folder root's display name plus its subfolder path; the first root to carry a name keeps it bare, each later same-named root appends `-2`, `-3`, … Keys are appearance-ranked: they hold across re-renders and added roots, and only renumber when the first-appearance order of same-named roots changes (e.g. removing the earlier same-named root) |
| `.folder-name` | a folder row's display name |
| `[aria-label="Collapse <name>"]` / `[aria-label="Expand <name>"]` | the chevron only — the folder row, its name, and its body never toggle |
| `[aria-expanded="true"]` / `[aria-expanded="false"]` | a folder's expansion state |
| `[data-shade]` / `[data-shade="1"]` / `[data-shade="0"]` | the grey-region shade attribute, alternating per nesting level; `1` = shaded grey, `0` = plain surface |
| `button[data-select="<id>"]` | a per-file checkbox (`aria-checked` `false`/`true`; hidden until the script runs) |
| `button[data-subtree="<ids>"]` | a folder subtree checkbox listing every member id, folded members included (`aria-checked` `false`/`true`/`mixed`) |
| `#select-all` | select-all header checkbox (`aria-checked` `false`/`true`/`mixed`) |
| `#download-selected` / `#download-selected.is-visible` | icon-only download button, invisible until something is selected; `href` is `/download/<id>` for one file, `/download/zip?ids=<ids>` for several |

Rows the default expansion keeps folded render with `hidden` (the
enhancement script reveals them); tests assert that as visibility
(`:visible`, `toBeHidden`) rather than the attribute itself.

Same tree, same contract wording where the two UIs correspond — but
the implementations really do differ in places, so the READMEs differ
too:

- Shading is a `data-shade` **attribute** on the web where the app uses
  a `shade-1` **class**; both mean the same alternating, non-indented
  grey region (ADR-0002).
- Folder-checkbox labels differ: the web says "Select all files in
  `<name>`" where the app says "Select or deselect all files in
  `<name>`" — the tests key on `[data-select]`/`[data-subtree]` and the
  chevron labels above, not on those checkbox texts.
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

`recipes.txt` + `checklist.txt` loose, then the folder tree
`Photographs` (expanded by default) with `beach.jpg` directly inside
and the `notes` subfolder (collapsed by default) holding `todo.txt` —
four files, ids 0–3, matching the zip-link ids asserted in the
selection test.
