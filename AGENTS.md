# AGENTS.md

Guidance for AI agents working in this repository.

## Agent skills

### Issue tracker

Issues live as GitHub issues on `DylanAlmond/mosquito`, managed with the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Domain docs

Single-context: `GLOSSARY.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.

### UI harnesses

For UI debugging and UI work, drive the real UIs instead of guessing: desktop via
tauri-pilot (`yarn harness:desktop`, workflow in `harness/desktop/README.md`), web
listing via Playwright (`yarn harness:web`, `harness/web/README.md`). Both are
debug tooling — never shipped, never referenced by app code paths.
