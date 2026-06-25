# Implementation Notes: Decouple Pricing-Data Changes from the Shared Library

Running record of how the implementation interprets or diverges from
`2026-06-09-decouple-pricing-data-cadence.md`.

## Phase 1: CI path-filter, workflow cleanup, and codified rule

### Design decisions
- `ci.yml` - applied identical `paths-ignore` blocks (`data/**`, `**/*.md`) to
  both the `push` and `pull_request` triggers. The doc specified both triggers;
  the glob list is duplicated rather than anchored because GitHub Actions has no
  YAML-anchor support across separate trigger keys in a way that survives the
  workflow parser cleanly, and duplication is the idiomatic form.
- Both design docs were committed together - the active decouple doc and the
  superseded `2026-06-09-split-pricing-data-from-logic.md`, which the decouple
  doc references as "retained for history."

### Deviations
- The doc's Phase 1 text says "add it directly to the automated PR body in
  `refresh-pricing.yml` ... Keep the README statement; a `CONTRIBUTING.md` is
  optional." I added the rule to the PR body and kept the README line; I did not
  add a `CONTRIBUTING.md` (explicitly optional). No deviation in substance.
- While editing the `refresh-pricing.yml` PR body I replaced a pre-existing em
  dash in the adjacent "awk parser" sentence with a regular dash, to comply with
  the no-em-dash formatting rule for external-facing text. This is a one-character
  cleanup outside the strict scope of the rule-text addition.

### Tradeoffs
- `git rm` vs. archival delete for `release.yml` - used `git rm` because the file
  is git-tracked and fully recoverable from history; an archival tool adds no
  recoverability over git here.
- `**/*.md` ignore glob is repo-wide, so a future code change that ships only a
  doc edit alongside no `src/**` change would skip Rust CI. Accepted: that matches
  the doc's intent (docs-only changes need no cargo build), and any real code
  change touches `src/**`/`Cargo.toml`, which are not ignored.

### Open questions
- None. The doc's two open questions were already resolved in round-2 review
  (the `fetch_and_cache` write-before-validate bug is tracked separately and out
  of scope; the split revisit-trigger is documented). The behavioral
  verification step (throwaway data-only PR vs. code PR) is a post-merge live-CI
  check that cannot be exercised pre-merge from the working tree.
