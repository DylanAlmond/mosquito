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
| loose files first | `127.0.0.1:8473` | loose rows before any group, JS-driven checkboxes |
| folder groups | `127.0.0.1:8473` | `Photographs` header, member rows by relative path |
| selection | `127.0.0.1:8473` | select-all → all highlighted + zip link; ungroup → mixed state |

Screenshots land in a deterministic, gitignored directory —
`test-artifacts/web-harness/` (`empty-list`, `loose-files`,
`folder-groups`, `selections` — same names every run).

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
`Photographs` with `beach.jpg` and `notes/todo.txt` beneath it — four
rows, ids 0–3, matching the zip-link ids asserted in the selection test.
