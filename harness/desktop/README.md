# Desktop UI harness (tauri-pilot)

Makes the desktop UI inspectable by an agent for debugging and UI work
(issue #7). The [tauri-pilot](https://github.com/mpiton/tauri-pilot)
plugin answers a local Named Pipe in the dev build; its CLI (`tauri-pilot`)
drives the real window — snapshot, click, assert, screenshot, logs. The
release binary never starts the plugin, so only debug builds can be
driven.

## Prerequisites (once)

- Rust ≥ 1.95 (`rustup update stable`)
- `cargo install tauri-pilot-cli`
- yarn (already the package manager here)

## One command: `yarn harness:desktop`

Builds and launches the dev window, seeds fixtures through the test IPC
surface, runs `harness/desktop/scenarios/desktop-sweep.toml` against the
real app, and tears it down. Exits 0 only when every scenario step
passes.

Fixture tree (recreated per run, `$TEMP\mosquito-harness\desktop`):

```text
beach-notes.txt     loose
release-plan.md     loose
Photographs/        folder root → tree "Photographs" (starts expanded)
  sunrise.jpg       member (direct)
  travel/beach.jpg  nested member, hidden until `travel` expands
```

Screenshots land in gitignored `test-results/desktop-harness/`.

## Driving it by hand (the agent workflow)

Run in terminal 1 (keep open):

```sh
yarn tauri dev      # debug build; registers tauri-plugin-pilot
```

Then inspect / drive the window from terminal 2:

```sh
tauri-pilot ping
tauri-pilot ipc harness_seed        # stage the fixture tree through the
                                    # pending-share flow (loose files +
                                    # folder); the UI adds them and
                                    # starts the server like a drop
tauri-pilot snapshot -i             # refs (@e1, @e2…) — refs reset per snapshot
tauri-pilot click @e12              # toggle the files list, select a row…
tauri-pilot assert text .folder-name "Photographs"
tauri-pilot assert count .file-row 4
tauri-pilot screenshot /tmp/desktop-now.png
tauri-pilot logs --level error      # console errors + unhandled rejections
```

Targets may be snapshot refs, CSS selectors, or `x,y`. For selectors,
the useful stable handles on the list are:

| Selector | Meaning |
| --- | --- |
| `.file-table` | the whole shared-files table |
| `.file-row` / `.file-row.is-selected` | file rows, highlighted rows |
| `.folder-row` | an expandable folder row (root or subfolder) |
| `.folder-name` | a folder row's display name |
| `[aria-label="Collapse <name>"]` / `[aria-label="Expand <name>"]` | the chevron only — the folder row itself is never a toggle |
| `[aria-label="…"][aria-expanded="true"]` | a folder's expansion state |
| `[aria-label="Select or deselect all files in <name>"]` | folder subtree checkbox (acts on folded members too) |
| `[aria-label="Select all shared files"]` | select-all header checkbox |
| `[aria-label="Delete selected files"]` | bulk-removal action |

The folder-tree selectors are the stable test contract for this UI
(issue #8): scenarios target attributes, never layout or snapshot
refs, so the DOM can change without harness breakage.

Run the scenario again while the window is open:

```sh
tauri-pilot run harness/desktop/scenarios/desktop-sweep.toml --screenshots-dir test-results/desktop-harness/failures
```

## Scope of the test IPC surface

`harness_seed` (debug builds only, `src-tauri/src/harness.rs`) creates
the fixture files and queues their paths in the production
`PendingShareFiles` buffer, then emits `share-files-requested` — the
frontend drains it through the same `add_paths` flow as a real
drop/pick, including server start. Scenario coverage: the folder tree
(expansion defaults, chevron toggling, subtree selection counting
folded members), selection (row + folder + select-all), bulk removal,
and the return to the empty dropzone.

## Release safety

`tauri_plugin_pilot::init()` and the whole `harness` module sit behind
`#[cfg(debug_assertions)]` (`src-tauri/src/lib.rs`): the crate compiles,
but the socket server and the command never register in release, so a
released binary cannot be driven from outside.

## Troubleshooting

- `eval timed out after 10s` — a build is missing the `pilot:default`
  capability, or a release build (plugin unplugged) is being driven.
- `No tauri-pilot socket found` — the app isn't running yet, or a
  running instance predates the capability. Restart `yarn tauri dev`.
- CLI picks the wrong instance: pin one with
  `TAURI_PILOT_SOCKET` to the newest manifest under
  `%LOCALAPPDATA%\tauri-pilot\instances\`.
- `harness_seed` twice in one session stacks duplicate rows — seed
  against a fresh app (relaunch terminal 1), or stop the server and drop
  everything manually first.
