/// Shared constants for the web UI harness (`harness/web/README.md`).
/// Ports sit away from the app's default 8472 so a desktop dev build and
/// the harness servers can run side by side (`harness_server.rs`).

export const FIXTURE_PORT = 8473;
export const EMPTY_PORT = 8474;

export const FIXTURE_URL = `http://127.0.0.1:${FIXTURE_PORT}`;
export const EMPTY_URL = `http://127.0.0.1:${EMPTY_PORT}`;

/// Deterministic, gitignored screenshot output — same file names every
/// run (`test-artifacts/` in .gitignore). Paths are resolved from the
/// repo root: every entry point that sets cwd is a yarn script.
export const SCREENSHOT_DIR = 'test-artifacts/web-harness';
