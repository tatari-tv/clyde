# Design Document: Move Pricing-Feed Publishing from `claude-pricing` into clyde

**Author:** Scott Idler
**Date:** 2026-06-29
**Status:** Draft
**Review Passes Completed:** 5/5 (+ cross-model review panel - Architect/Gemini + Staff Engineer/Codex - incorporated 2026-07-03)

## Summary

clyde's vendored `pricing/` crate fetches its live pricing feed at runtime from
`https://tatari-tv.github.io/claude-pricing/pricing.json`, but clyde does not publish that feed -
the GitHub Pages deploy and the daily refresh run only in the standalone `tatari-tv/claude-pricing`
repo. This doc moves the feed-publishing machinery (the `pages.yml` deploy and the
`refresh-pricing.yml` daily cron) into the clyde repo, path-scoped to the `pricing/` crate, enables
GitHub Pages on `tatari-tv/clyde`, and repoints `DEFAULT_FEED_URL` to
`https://tatari-tv.github.io/clyde/pricing.json`. After this, clyde owns its own pricing feed
end-to-end.

## Problem Statement

### Background

clyde absorbed the `claude-pricing` library as the in-workspace `pricing/` crate. The library code,
the data files (`pricing/data/{pricing.json,normalization.json,pricing-page.sha256}`), and the
refresh scripts (`pricing/bin/{update,update.sh,update.py}`) all came along with the vendoring and
are byte-identical to the standalone repo. What did **not** come along in a working state is the
publishing layer:

- The standalone repo's root `.github/workflows/` runs the live `pages.yml` (deploys
  `pricing.json` to GitHub Pages) and `refresh-pricing.yml` (daily cron that re-scrapes Anthropic
  and opens a data-refresh PR).
- clyde's root `.github/workflows/` has only `ci.yml` and `release.yml`. The copies of `pages.yml`
  and `refresh-pricing.yml` that rode along inside `clyde/pricing/.github/workflows/` are
  **dormant**: GitHub only executes workflows located at the repository root, not inside a
  subdirectory.

Verified live state: `gh api repos/tatari-tv/clyde/pages` returns 404 (no Pages site);
`gh api repos/tatari-tv/claude-pricing/pages` is live with `build_type: workflow`. This is the exact
gap recorded in `docs/shakedown-v0.2.0.md:122-131`.

### Problem

clyde depends on an external repo (`claude-pricing`) to publish the feed clyde reads at runtime and
to keep that feed fresh. clyde cannot own its pricing story until publishing lives in clyde.

### Goals

- Publish `pricing.json` to GitHub Pages from the clyde repo at
  `https://tatari-tv.github.io/clyde/pricing.json`.
- Run the daily pricing-refresh cron from the clyde repo, scoped to `pricing/data/`.
- Repoint the crate's `DEFAULT_FEED_URL` to clyde's Pages URL.
- Remove the dormant duplicate workflow files under `clyde/pricing/.github/`.
- Cut over without breaking live consumers (sequence the URL flip after the new feed is verified).
- Disable the standalone `claude-pricing` daily refresh cron once clyde's cron is live, so exactly
  one repo refreshes the feed (no split-brain / competing refresh PRs).

### Non-Goals

- **Anything involving `ccu` (claude-cost-usage) or `cr` (claude-report).** Out of scope; handled
  separately.
- **Archiving or retiring the standalone `claude-pricing` repo.** Out of scope; handled separately.
  Its Pages site stays live (still serving the old URL) during and after this work, which is what
  makes the cutover zero-downtime. Its daily `refresh-pricing.yml` cron IS disabled as part of this
  work (Phase 6) to stop two repos opening competing refresh PRs - disabling one workflow is not
  retiring the repo.
- Changing the pricing schema, the `schema_version`/`min_library_version` contract, the parser
  scripts, or the data shape. This is a hosting move, not a schema or library change.
- Changing the cache path. The runtime cache lives at `<cache_dir>/clyde/pricing/pricing.json`
  (`pricing/src/fetch.rs:32-35`; e.g. `~/.cache/clyde/pricing/pricing.json` on Linux) and is
  unchanged by this move - only the remote URL moves. (The crate is still *named* `claude-pricing`,
  but the on-disk cache already moved under the unified `clyde` home.)

## Proposed Solution

### Overview

Graft the two publishing workflows onto clyde's root `.github/workflows/`, rewritten so every path
that was repo-root-relative in `claude-pricing` (where `data/` was at the root) is prefixed with
`pricing/` in clyde (where the crate lives at `pricing/` and its data at `pricing/data/`). Enable
Pages on the clyde repo with the source set to GitHub Actions. Once the new feed is verified live,
repoint `DEFAULT_FEED_URL`.

The refresh scripts and data files need **no changes**: `bin/update` computes its repo root as
`$(dirname "$0")/..`, so when invoked as `pricing/bin/update` it resolves to `clyde/pricing/` and
its internal `data/` paths already point at `pricing/data/`. Only the workflow steps that hardcode
`data/pricing.json` relative to the repo root need the `pricing/` prefix.

### Architecture

Two independent workflows, two distinct trigger surfaces, no collision with clyde's existing CI or
release:

```text
clyde/.github/workflows/
  ci.yml            (existing) push to **, runs otto ci in builder-base container
  release.yml       (existing) push v* tag, cross-compiles + publishes GitHub Release
  pages.yml         (NEW)      push to main on pricing/data/pricing.json -> deploy Pages
  refresh-pricing.yml (NEW)    cron '17 6 * * *' -> bin/update -> open data-refresh PR
```

**Publish path (`pages.yml`):** on a push to `main` that touches `pricing/data/pricing.json`,
stage that file into `_site/pricing.json` with a `.nojekyll` marker, then run the standard
Actions-based Pages deploy (`actions/configure-pages` -> `actions/upload-pages-artifact` ->
`actions/deploy-pages`). Requires `pages: write` + `id-token: write`. This is the modern
Actions-based deploy, not a `gh-pages` branch and not peaceiris, which is why the clyde Pages source
must be set to "GitHub Actions".

**Refresh path (`refresh-pricing.yml`):** daily cron runs `pricing/bin/update`, which fetches
Anthropic's pricing markdown, runs the dual awk/python parsers, cross-checks them, carries forward
delisted models, runs the regression guards, and rewrites `pricing/data/pricing.json` +
`pricing/data/pricing-page.sha256` only when content changed. If the data changed, it opens a PR via
`peter-evans/create-pull-request` (it does not push to `main` directly). On failure it files an
issue. When that PR merges to `main`, the path filter on `pages.yml` fires the deploy. So the two
workflows chain: refresh opens a PR -> human merges -> push-to-main deploys the new feed.

**Token and merge mechanics (verified against live `main` protection):** the refresh workflow uses
the default `GITHUB_TOKEN`. A PR (and its branch push) authored by `GITHUB_TOKEN` does **not**
trigger clyde's `ci.yml` - that workflow triggers on `push` to `**` and `workflow_dispatch`, never
`pull_request`, and GitHub deliberately suppresses workflow runs on `GITHUB_TOKEN`-authored pushes
to prevent recursion. That suppression is documented GitHub behavior, not an assumption: "When you
use the repository's `GITHUB_TOKEN` to perform tasks, events triggered by the `GITHUB_TOKEN` [...]
will not create a new workflow run" (GitHub Actions docs, *Triggering a workflow* ->
*Triggering a workflow from a workflow*), and `peter-evans/create-pull-request` restates the same
limitation in its "Triggering further workflow runs" FAQ. This is not a problem here: `main` has
**no required status checks**
(`gh api repos/tatari-tv/clyde/branches/main/protection` returns `required_pull_request_reviews`
with `required_approving_review_count: 1` and `require_code_owner_reviews: true`, but **no**
`required_status_checks` block, and `enforce_admins: false`). The only merge gate is **one
code-owner approval**, so each daily bot PR needs a human code-owner to approve and merge it - there
is no auto-merge and no required-check deadlock. clyde CI still runs post-merge, because the merge
to `main` is a human-authored push that does fire `ci.yml`. (The org-level ruleset on `main` adds
`deletion`/`non_fast_forward` protection and a `workflows` rule requiring the org
`.github/workflows/security.yaml`; none of these block a code-owner-approved data PR.)

### Data Model

The published artifact is unchanged: a single `pricing.json` file at the Pages site root, carrying
the v2 envelope (`schema_version: 2`, `data_version`, `min_library_version: "2.0.0"`, `aliases`,
`family_rules`, `pricing`). No index page. The crate's feed gate in `pricing/src/feed.rs` (rejects
`schema_version` above `CURRENT_SCHEMA_VERSION`, falls back to embedded when `min_library_version`
exceeds the crate version) is untouched. Moving where the JSON is hosted changes neither
`schema_version` nor `min_library_version`, so the crate-major-locked-to-schema rule in
`pricing/CLAUDE.md` is unaffected.

**Deploy trigger is `pricing.json`-only, and that is load-bearing.** `pages.yml` fires only on a
change to `pricing/data/pricing.json`. But `aliases`/`family_rules` are human-authored policy in
`pricing/data/normalization.json`, which `bin/update` **splices into** `pricing.json` at build time
(`pricing/bin/update:253`). Consequence: a normalization-only edit does NOT deploy on its own - it
reaches the live feed only once `bin/update` regenerates and commits `pricing.json`. The invariant
to enforce: **never hand-edit `normalization.json` and expect it to publish; always regenerate
`pricing.json` (run `pricing/bin/update`) and commit that in the same PR.** Relatedly, a refresh PR
that changes only `pricing/data/pricing-page.sha256` (content unchanged, `pricing/bin/update:295,312`)
will not deploy - correct and expected, since the served envelope is identical.

### API Design

The single load-bearing code change:

```rust
// pricing/src/feed.rs:15
pub const DEFAULT_FEED_URL: &str = "https://tatari-tv.github.io/clyde/pricing.json";
```

The `CLAUDE_PRICING_FEED_URL` environment override (`pricing/src/fetch.rs:37`) is unchanged; it
already falls back to `DEFAULT_FEED_URL`. This override is the mechanism used to verify the clyde
feed before the default flips (point a build at the new URL without editing the constant).

### Implementation Plan

#### Phase 1: Add root `pages.yml`
**Model:** sonnet
- Copy `claude-pricing/.github/workflows/pages.yml` to `clyde/.github/workflows/pages.yml`.
- Rewrite the trigger `paths:` from `data/pricing.json` to `pricing/data/pricing.json` (and the
  workflow's own path to `.github/workflows/pages.yml`).
- Rewrite the stage step `cp data/pricing.json _site/pricing.json` to
  `cp pricing/data/pricing.json _site/pricing.json`.
- Keep the Actions-based deploy steps, permissions (`pages: write`, `id-token: write`), and
  concurrency group verbatim.
- **Grafting invariant:** every path that was repo-root-relative in `claude-pricing` (where `data/`
  sat at the root) MUST gain the `pricing/` prefix here. A missed prefix does not error - the
  workflow simply never fires (a silent no-op), so double-check the `paths:` filter and every `cp`
  against `pricing/data/...` before merging.
- Independently committable; inert until Pages is enabled (Phase 4).

#### Phase 2: Add root `refresh-pricing.yml`
**Model:** sonnet
- Copy `claude-pricing/.github/workflows/refresh-pricing.yml` to clyde root.
- Rewrite the run step `bin/update` to `pricing/bin/update`.
- Rewrite the change-detect `git diff --quiet -- data/pricing.json data/pricing-page.sha256` to
  prefix both with `pricing/data/`.
- Rewrite `add-paths:` to `pricing/data/pricing.json` and `pricing/data/pricing-page.sha256`.
- Keep it on bare `ubuntu-latest` (NOT the builder-base container); the script needs
  `curl`/`jq`/`python3`/`awk`, all present on `ubuntu-latest`, and does not need Rust.
- Update the PR-body text to describe clyde's runtime feed pickup; drop the stale consumer-naming
  copy carried over from the standalone repo. (Copy cleanup only; no consumer-repo work.)
- Verify via `workflow_dispatch` that it runs end-to-end and can open a PR (or files the failure
  issue if Actions-created PRs are disabled org-wide).

#### Phase 3: Remove dormant duplicates
**Model:** sonnet
- Delete `clyde/pricing/.github/workflows/{pages,refresh-pricing,ci}.yml` via `rkvr rmrf` (they
  never run at that location and are now duplicated at root).
- **Do NOT add `paths-ignore: pricing/data/**` to `ci.yml`.** It was considered and rejected: the
  refresh PR's branch push is `GITHUB_TOKEN`-authored, so it does not trigger `ci.yml` at all (see
  "Token and merge mechanics"), and there are no required status checks - so there is neither a
  wasted-run problem to suppress nor a pending-check deadlock to avoid. Adding the filter would be
  dead config.

#### Phase 4: Enable Pages and verify the live feed
**Model:** opus
- **Owner:** Scott Idler drives this phase end-to-end (the manual Pages toggle has no CI surface and
  is the one human-in-the-loop step).
- One-time manual repo setting on `tatari-tv/clyde`: set Pages source to "GitHub Actions". There is
  no idempotent CI mechanism to flip this; `gh api repos/tatari-tv/clyde/pages` returning 404
  confirms it is currently off. Do not script it; do not narrate the GitHub settings UI from memory
  (verify against current GitHub docs or drive it one step at a time).
- `tatari-tv/clyde` is a private repo, and the feed must be fetchable by users' machines without
  auth, so Pages visibility must be **public** (a setting distinct from the source). `claude-pricing`
  is also private and already serves a public feed, so the org tier supports private-repo public
  Pages; confirm the same toggle on clyde.
- Trigger `pages.yml` via `workflow_dispatch`.
- Verify: `curl https://tatari-tv.github.io/clyde/pricing.json | jq .schema_version` returns `2` and
  the full envelope is present.
- **Gate:** Phase 5 (the `DEFAULT_FEED_URL` flip) MUST NOT begin until this `curl ... | jq` returns
  the full schema-2 envelope over plain HTTP with no auth. If Pages is not verifiably live, stop
  here - a premature flip 404s every consumer (they then silently fall back to cache/embedded,
  `pricing/src/fetch.rs:83`).
- Opus-tagged because the cutover ordering is the one place a wrong sequence silently breaks live
  consumers; the feed must be confirmed live here before Phase 5 ships.

#### Phase 5: Repoint `DEFAULT_FEED_URL`
**Model:** opus
- Change `pricing/src/feed.rs:15` to the clyde Pages URL.
- Update the vendored `pricing/README.md` line about being "the publishing point for the JSON
  pricing feed" so it is now accurate, and repoint any doc URL references for accuracy.
- This is the contract-bearing change; it must land only after Phase 4 verification. Until a clyde
  release carrying this constant is built and installed, consumers keep reading the old
  `claude-pricing` feed (still live, out of scope to retire), so there is no downtime.
- **Rollback:** if the clyde feed misbehaves after the flip, revert `pricing/src/feed.rs:15` to the
  old `https://tatari-tv.github.io/claude-pricing/pricing.json` and cut a new clyde patch release;
  the old feed is kept hot precisely so this revert is always available. Do not disable the legacy
  cron (Phase 6) until at least one clyde release carrying the new URL has run clean for a full
  refresh cycle (>24h), so the old feed stays fresh as a fallback until then.

#### Phase 6: Disable the legacy `claude-pricing` refresh cron
**Model:** sonnet
- Once a clyde release carrying the new `DEFAULT_FEED_URL` has run clean for >24h (Phase 5 rollback
  window closed), disable the daily refresh in `tatari-tv/claude-pricing` so only clyde refreshes
  the feed. Remove the `schedule:` trigger from that repo's `.github/workflows/refresh-pricing.yml`
  (leave `workflow_dispatch` for manual use) via a PR.
- This prevents split-brain: two repos scraping Anthropic and opening competing refresh PRs against
  two different feeds. Leaving `claude-pricing`'s Pages site serving the frozen old URL is fine and
  intended (it is the rollback target); only the *cron* is retired here.
- Retiring the `claude-pricing` repo entirely remains out of scope (separate effort).

## Alternatives Considered

### Alternative 1: Publish from a `gh-pages` branch instead of Actions deploy
- **Description:** Commit `pricing.json` to a `gh-pages` branch and let classic branch-based Pages
  serve it.
- **Pros:** No `pages:write`/`id-token` permissions; conceptually simple.
- **Cons:** Diverges from the proven `claude-pricing` mechanism; adds a second publish branch to a
  PR-protected repo; loses the clean "push to main path-filter triggers deploy" chain.
- **Why not chosen:** The Actions-based deploy already works in `claude-pricing` and ports almost
  verbatim. Re-architecting the publish mechanism during a move adds risk for no benefit.

### Alternative 2: Keep publishing in `claude-pricing`, never move it
- **Description:** Leave the feed hosted at the standalone repo's Pages indefinitely; clyde
  continues to point at it.
- **Pros:** Zero work.
- **Cons:** clyde stays permanently dependent on an external repo it intends to retire; the daily
  refresh and the live feed are outside clyde's control.
- **Why not chosen:** Defeats the purpose of the clyde consolidation and blocks the (separate)
  retirement of `claude-pricing`.

### Alternative 3: Publish to a separate data-only repo (`claude-pricing-data`)
- **Description:** Split the feed into its own dedicated repo with its own Pages.
- **Pros:** Decouples data cadence from code entirely.
- **Cons:** Introduces a third repo and a third Pages site; another external dependency for clyde;
  contradicts the "one repo owns it" goal.
- **Why not chosen:** The whole point is to bring publishing *into* clyde, not to spawn another
  satellite. (This option was already weighed and rejected in
  `pricing/docs/design/2026-06-09-split-pricing-data-from-logic.md`.)

## Technical Considerations

### Dependencies

- `refresh-pricing.yml` needs `curl`, `jq`, `python3`, `awk`, `sha256sum`, `git` on the runner.
  Bare `ubuntu-latest` ships all of them; this is why the job must NOT run in clyde's builder-base
  container (which is a Rust image and may lack `jq`/`python3`).
- `pages.yml` uses `actions/configure-pages@v6`, `actions/upload-pages-artifact@v5`,
  `actions/deploy-pages@v5` (same pins as `claude-pricing`).
- `refresh-pricing.yml` uses `peter-evans/create-pull-request@v7` and the default `GITHUB_TOKEN`.

### Performance

Not a factor. The cron runs once daily; the Pages deploy runs only on a `pricing/data/pricing.json`
change. Neither is on a hot path.

### Security

- `pages.yml` requires `pages: write` + `id-token: write` (OIDC for the Pages deploy). `id-token`
  is already granted in clyde's `ci.yml`, so the org permits it.
- `refresh-pricing.yml` requires `contents: write` + `pull-requests: write` + `issues: write`. It
  opens a PR rather than pushing to `main`, which is compatible with clyde's protected-`main`,
  admin-merge flow. The `data/` PR still goes through review and merge like any other.
- The published artifact is public pricing data; nothing sensitive is exposed.

### Testing Strategy

- The parser machinery is fixture-tested in the crate already (`pricing/src/*/tests.rs`); unchanged
  by this move.
- `workflow_dispatch` on `refresh-pricing.yml` exercises the full fetch/parse/cross-check/PR path on
  demand.
- `workflow_dispatch` on `pages.yml` plus a `curl ... | jq` of the live URL is the end-to-end check
  that the feed is actually served (Phase 4).
- A pre-flip smoke test: set `CLAUDE_PRICING_FEED_URL=https://tatari-tv.github.io/clyde/pricing.json`
  and run a `clyde cost` command to confirm the crate fetches and parses the clyde-hosted feed
  before the default constant is changed.

### Rollout Plan

Strict ordering to keep live consumers reading a valid feed at all times:

1. Land Phases 1-3 (workflows in clyde root, dormant duplicates removed). The new workflows are
   inert because Pages is still off and `DEFAULT_FEED_URL` still points at the old feed.
2. Enable Pages on clyde and verify the live URL (Phase 4). This is the hard gate: do not proceed
   until an unauthenticated `curl` of the new URL returns the schema-2 envelope.
3. Repoint `DEFAULT_FEED_URL` and ship a clyde release (Phase 5). Consumers pick up the new URL only
   when they install the new clyde build. Keep the old feed hot as the rollback target.
4. After that release runs clean for a full refresh cycle (>24h), disable the legacy
   `claude-pricing` refresh cron (Phase 6) so only clyde refreshes the feed.

The old `claude-pricing` feed stays live throughout (its retirement is a separate effort), so at no
point is there a window where consumers have no feed to read.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Pages source not set to "GitHub Actions" -> deploy fails | Med | Med | Explicit one-time settings step in Phase 4; verify with `gh api .../pages` before relying on it. |
| Pages visibility left private on the private clyde repo -> feed 404s for unauthenticated consumers | Med | Med | Phase 4 sets visibility to public; verify with an unauthenticated `curl` of the live URL. |
| Org disables Actions-created PRs -> refresh PR step fails | Med | Low | Workflow already has an `issues: write` failure fallback that files an issue; data can be refreshed by running `pricing/bin/update` locally and opening the PR by hand. |
| `DEFAULT_FEED_URL` repointed before clyde Pages is live -> consumers 404 then fall back to stale embedded baseline | Low | Med | Phase ordering forbids this: Phase 5 (URL flip) lands only after Phase 4 (live-feed verification). Phase 5 also documents an immediate revert-the-constant rollback. |
| Both repos' crons run -> split-brain / competing refresh PRs against two feeds | Med | Med | Phase 6 disables the legacy `claude-pricing` cron once clyde's is live and baked >24h; only one repo refreshes thereafter. |
| Normalization-only edit never reaches the live feed (pages.yml watches `pricing.json`, not `normalization.json`) | Med | Med | Invariant in Data Model: always regenerate + commit `pricing.json` (`pricing/bin/update`) in the same PR as any `normalization.json` change. |
| Anthropic page format drifts and a parser breaks | Low | Med | Pre-existing dual-parser cross-check + regression guards in `bin/update` refuse to ship a bad parse and the workflow files an issue; unchanged by this move. |

## Resolved Decisions

- **Actions-created PRs:** `refresh-pricing.yml` is verified end-to-end via `workflow_dispatch` in
  Phase 2. If the org disables Actions-created PRs, the workflow's `issues: write` fallback files an
  issue and the refresh can be run locally (`pricing/bin/update`) with a hand-opened PR. No blocker.
- **`paths-ignore` on `ci.yml`:** decided NOT to add it (Phase 3). The refresh PR's branch push is
  `GITHUB_TOKEN`-authored and does not trigger `ci.yml`, and `main` has no required status checks -
  so there is nothing to suppress. Adding it would be dead config.
- **Legacy cron:** decided to disable the `claude-pricing` refresh cron (Phase 6) after clyde's
  release bakes for >24h, keeping the old Pages feed hot as the rollback target until then.

## References

- `docs/shakedown-v0.2.0.md:122-131` - original flag of this publishing gap, including the target URL.
- `claude-pricing/.github/workflows/pages.yml` - source of the publish workflow.
- `claude-pricing/.github/workflows/refresh-pricing.yml` - source of the refresh cron.
- `pricing/bin/update` - the dual-parser refresh orchestrator (already vendored in clyde).
- `pricing/src/feed.rs:15` - `DEFAULT_FEED_URL`, the one load-bearing code change.
- `pricing/build.rs:16-26` - embeds `pricing-page.sha256` for diagnostics (not a publish gate).
- `pricing/CLAUDE.md`, `pricing/Cargo.toml:1-5` - crate-major-locked-to-`schema_version` contract.
- `pricing/docs/design/2026-06-09-split-pricing-data-from-logic.md` - prior art weighing a separate
  data repo.
