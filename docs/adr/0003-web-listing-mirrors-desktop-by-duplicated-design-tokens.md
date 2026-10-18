# ADR-0003: The web listing mirrors the desktop by duplicating design tokens

Date: 2026-10-18
Status: Accepted

## Context

Restyling the served listing (`listing.html` + the markup helpers in
`listing.rs`) gave it the desktop list's (`src/components/FileList.vue`)
visual language: the same ink-on-white palette, the DynaPuff faces,
the 2px ink borders, the shared grid with the 32px checkbox column,
the top border between every row, the inverted selection rows, the
chevron-only expansion, and the grey `--folder-shade` region
(ADR-0002's depth-based shading).

The two renderers live in different worlds. The desktop list styles
itself with the app's design tokens, defined once in `src/style.css`
(`--ink`, `--surface`, `--muted`, `--radius`, `--folder-shade`). The
listing is a self-contained HTML document embedded in the Rust crate: no
bundler, no build step, styled by its own inline `<style>` element.
Something must decide how the design vocabulary reaches it.

Three options:

1. **Share the stylesheet**: the page links a stylesheet emitted by the
   frontend build, or the crate embeds copies of the app's CSS (or the
   app swallows the crate's static assets).
2. **Detect divergence with visual-regression tooling**: screenshot both
   UIs routinely and diff them, flagging drift.
3. **Duplicate the design tokens** into the listing's own CSS, on
   purpose, and keep the mirror honest through review and tests.

## Decision

Option 3: mirror by duplicated tokens. The listing's stylesheet declares
its own copy of the relevant tokens — same names, same values —
annotated as mirroring the desktop's. This is the same shape as the
convention ADR-0002 already established for the tree: each renderer
computes and paints the same thing from its own code, with tests
enforcing that the mirrors agree. The visual vocabulary is mirrored,
not shared, exactly like the logic underneath it.

- The listing stays a single self-contained page: one inline `<style>`,
  inline SVG artwork. It never gains a dependency on the frontend
  build, and the crate never embeds app assets.
- The mirror is one-directional: the desktop list is the reference;
  changes are made on `FileList.vue` first and echoed into `listing.html`.
- The duplication is curated, not wholesale: only the tokens and rules
  the listing actually reproduces get copied, and where the surfaces
  genuinely differ (a download button where the app has delete) the
  listing does its own arithmetic — the tokens are duplicated, the rest
  is per-surface. _(Revised by ADR-0004 in 2026-10: rows and headers are
  one 38px strip on both surfaces now, so the old "3rem rows vs 38px
  tree rows" split is gone.)_

**Alternatives rejected:**

- *Shared CSS import (1)* — couples the Rust-served static page to the
  frontend build (the page gains a build artefact to live), or couples
  the app to the crate (the crate embeds UI assets that are really the
  app's). Unifying two consumers that share exactly one file's worth of
  paint is not worth making either build depend on the other.
- *Screenshot/visual-regression divergence detection (2)* — heavier
  tooling (image infrastructure, cross-platform font and pixel
  tolerance) for marginal gain over what already exists: the harnesses
  assert selectors and behaviour in both UIs, and the listing's unit
  tests assert markup.

## Consequences

- Drift can be silent: a token change on the desktop side does not
  propagate automatically, and nothing screams when the copies diverge.
  The mitigation is that this mirror, like the derivation one, is
  asserted rather than trusted: `listing.rs` tests assert the markup,
  the two harnesses assert selectors and behaviour, and a restyle
  touches both stylesheets in one change or risks being half a mirror.
- Token changes are two edits by hand; there is no pipeline to update,
  but also nothing to forget to run — the page rebuilds with `cargo`.
- **Scope: the visual language only.** The convention covers how the
  page looks, not how any other layer of the two UIs is built. The tree
  derivation is a separate (already-mirrored) unity — each renderer
  computes it independently, per ADR-0002 — and the web page's
  JavaScript assumption ("progressive enhancement only, the enhancement
  script does the toggling") stays documented there (ADR-0002), not
  here.
- Two harness READMEs now carry corresponding *selector contracts* for
  this same tree — wording matched where the selectors correspond,
  honestly different where the implementations do.
- If the visual language ever stops being affordable to maintain as a
  curated copy (a palette refactor spanning many tokens, a dark scheme),
  this decision should be revisited — it is duplication, traded
  deliberately for decoupling, and still the cheaper half of the
  bargain.
