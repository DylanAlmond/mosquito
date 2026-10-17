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
