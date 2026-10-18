# ADR-0008: Root records commit at drop; counts go live

Date: 2026-10-18
Status: Accepted (revises ADR-0007's commit timing, extends ADR-0005)

## Context

ADR-0007 gave the import a background count walk, but the root record still
committed only when the walk finished: a whole-drive drop left the list empty
and the server stopped for the walk's full duration — minutes on an entire
drive — with only the run board to look at. The user's ask is the opposite
order: drop, and the share is up and the list is populating while discovery
(the count walk) proceeds. Committing before the count is known inverts a
documented contract: "a cancelled folder import leaves no root record, a
half-done count is never recorded" (ADR-0007 / GLOSSARY).

## Decision

- A folder drop **commits its root record at drop time**, synchronously under
  one brief lock, before the walk starts. The server auto-starts in the same
  synchronous path — drop → server listening is milliseconds; the walk is no
  longer on any user-visible path.
- The record's count is a **live count**: the walk updates `file_count` in the
  record at its existing progress checkpoints (every 500 files) under one
  brief lock per checkpoint. The count is display-only on both surfaces
  (desktop list, receiver page) — the live sub­tree contract already accepts
  drift, and a climbing count is drift by design. Nothing materializes
  because of it; the tree stays lazily known.
- With the record committed before the walk's outcome is known, both
  walk-outcome contracts change from *non-commit* to *removal*:
  - **Cancel** removes the root record plus everything materialized under it
    (the store's existing `remove_root`) — the same outcome the old contract
    promised, now reached by removal.
  - **Unreadable/vanished root** (count walk fails) auto-removes its own
    record, so "failed add" semantics keep exactly their old meaning, with
    the failure still surfacing through the run board's banner flow.
- The run board keeps its shape — cancel affordance, per-root failure
  reporting, settled-run registry. The list row gains only the live count.

## Considered options

- **Keep late commit, start the server earlier** — rejected: the list would
  still be empty for the whole walk, which is the complaint.
- **Placeholder rows from run snapshots without committing records** —
  rejected: two sources of truth for the top of the list; the store-side
  derivation (ADR-0006) must stay the single one.

## Consequences

- The "count captured at import" reading of a root record is replaced by a
  **live count**: the number the walk has reached so far. Materialization and
  listing behavior are unchanged.
- A cancelled or failed walk leaves no record *because the store removes it*,
  not because it never committed. A record's provenance includes content
  materialized during its walk — removal sweeps that too.
- Report-out tests treat the recorded count as event-driven and toleranced
  mid-run (ADR-0007's consequence carries over, now against the record's own
  number).
