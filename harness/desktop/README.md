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
Photographs/        folder → counted by a background import run
  sunrise.jpg       (ADR-0007); commits a root record — it produces no
  travel/beach.jpg  file rows on the desktop list until #24 browses
                    lazily-known content
```

Screenshots land in gitignored `test-results/desktop-harness/`.

## Driving it by hand (the agent workflow)

Run in terminal 1 (keep open):

```sh
yarn tauri dev --features pilot
    # debug build; registers tauri-plugin-pilot (opt-in, see below)
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
tauri-pilot assert count .file-row 2
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
| `.drop-title` | the idle dropzone's instruction text "Drop files or folders here" |
| `[aria-label="Choose file"]` / `[aria-label="Choose folder"]` | the two _Choose File_ / _Choose Folder_ affordances beneath the dropzone and the active card |
| `.dropzone` / `.dropzone--active` | the idle pick surface (opens the file dialog) and the sharing-details card |

The folder-tree selectors are the stable test contract for this UI
(issue #8): scenarios target attributes, never layout or snapshot
refs, so the DOM can change without harness breakage. From #24 on,
the record's group row materializes through its own expansion, so the
sweep drives the loose-row parts of the table first, then the group
row: #29 closes the sweep with the node pass — the `travel`
subfolder row's own node checkbox, its covered member standing down,
and a one-action remove_node sweep that leaves the record alive. The
web
harness (`harness/web/README.md`) carries the corresponding contract
for the served listing — same tree, same wording where the selectors
match the app's, honest differences where the two implementations
don't (attribute vs class shading, checkbox label wording, download
links instead of click-to-select rows).

Run the scenario again while the window is open:

```sh
tauri-pilot run harness/desktop/scenarios/desktop-sweep.toml --screenshots-dir test-results/desktop-harness/failures
```

## Scope of the test IPC surface

`harness_seed` (debug builds only, `src-tauri/src/harness.rs`) creates
the fixture files and queues their paths in the production
`PendingShareFiles` buffer, then emits `share-files-requested` — the
frontend drains it through the same `import_paths` flow as a real
drop/pick: the loose files register immediately, and `Photographs`
starts an import run that reports per-root progress on the board
(`[aria-label="Import progress"]`, "counting <path> — N entries",
with `[aria-label^="Cancel import of"]` to cancel) until the root
record commits and the server auto-starts. The run is far too quick
to observe for a two-file fixture, so the progress board itself is
covered by the SharePanel unit tests; the sweep covers the loose rows
(selection, bulk removal) and the share staying up because the
imported root record still counts as content, then the record's own
cover. Scenario coverage: loose-file rendering and selection,
select-all, bulk removal (with the record's group deselected first,
per ADR-0006's node-resolved selection), the group row's expansion,
the `travel` node checkbox with its covered member inert, and the
one-action node removal that leaves the record and its updated live
count in place (#29).

## Release safety

The `tauri-plugin-pilot` dependency itself never links into release: it
is an optional dependency behind the `pilot` feature (`src-tauri/
Cargo.toml`), and `pilot` is deliberately not in `default` — Cargo has
no profile-gated dependencies, so default-features resolution is what
keeps release builds (CI `tauri-action`, plain `cargo build --release`)
pilot-free. `tauri_plugin_pilot::init()` and the whole `harness` module
sit behind `#[cfg(all(debug_assertions, feature = "pilot"))]`
(`src-tauri/src/lib.rs`), with a `compile_error!` guard that fails any
release build that enables the feature anyway. The `pilot:default`
capability lives in `capabilities/pilot.toml` — TOML because that's
where its comment lives — declaring a grant that is inert in release:
nothing the permission allows is reachable when the plugin never
registers (and `build.rs` only feeds the file to tauri-build when the
feature is on).

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
