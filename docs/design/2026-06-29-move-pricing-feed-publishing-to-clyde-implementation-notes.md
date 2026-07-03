# Implementation Notes: Move Pricing-Feed Publishing into clyde

Companion to `docs/design/2026-06-29-move-pricing-feed-publishing-to-clyde.md`.
Append-only. One section per phase.

## Phase 1: Add root `pages.yml`

### Design decisions
- `.github/workflows/pages.yml` — ported the `claude-pricing` workflow verbatim except for the
  `pricing/` path prefixes: `paths: pricing/data/pricing.json` and
  `cp pricing/data/pricing.json _site/pricing.json`. The self-path trigger
  `.github/workflows/pages.yml` stays root-relative (it already is, in clyde).

### Deviations
- None.

### Tradeoffs
- Kept the Actions-based deploy (configure-pages → upload-pages-artifact → deploy-pages) verbatim
  rather than a gh-pages branch, per the doc's Alternative 1 rejection.

### Open questions
- None. (Inert until Pages is enabled in Phase 4.)

## Phase 2: Add root `refresh-pricing.yml`

### Design decisions
- `.github/workflows/refresh-pricing.yml` — `pricing/` prefixes applied to all repo-root-relative
  paths: `run: pricing/bin/update`, the `git diff --quiet` change-detect on
  `pricing/data/{pricing.json,pricing-page.sha256}`, and the `add-paths:` list.
- Kept the job on bare `ubuntu-latest` (not the builder-base container) — the script needs
  `curl`/`jq`/`python3`/`awk`, all present on `ubuntu-latest`, and does not need Rust.
- PR-body copy rewritten: references `pricing/data/pricing.json`, describes the merge→Pages-deploy→
  clyde-runtime-pickup chain, and drops the stale `ccu`/`cr` consumer naming carried over from the
  standalone repo. `pricing/src/**` replaces `src/**` in the "cut a tag only for library code" line.

### Deviations
- None.

### Tradeoffs
- Left the `Open issue on failure` step and its static body verbatim (only GitHub-controlled env
  vars, no attacker-controllable event payload — no injection surface), rather than restructuring it.

### Open questions
- None. End-to-end `workflow_dispatch` verification is deferred to post-merge (the workflow must be
  on `main` to dispatch); tracked by the design doc's Phase 2 verification step.

## Phase 3: Remove dormant duplicates

### Design decisions
- Archive-deleted `pricing/.github/workflows/{pages,refresh-pricing,ci}.yml` via `rkvr rmrf`
  (archived to `/var/tmp/rmrf/2026-07-03-005727-000/`).
- Did NOT add `paths-ignore: pricing/data/**` to `ci.yml` — per the design doc's resolved decision,
  it would be dead config (GITHUB_TOKEN-authored refresh pushes don't trigger `ci.yml`, and `main`
  has no required status checks).

### Deviations
- Left `pricing/.github/CODEOWNERS` in place — the doc scoped Phase 3 to the three workflow files
  only; CODEOWNERS is harmless and out of scope.

### Tradeoffs
- None.

### Open questions
- None.

## Phases 4-6: NOT implemented this session (hard gates)

Phases 4, 5, and 6 are intentionally deferred; they cannot run from a code-editing session:
- **Phase 4 (Enable Pages + verify)** is a manual GitHub repo settings toggle owned by Scott, plus a
  remote `curl` verification. It is the hard gate: Phase 5 must not begin until the live feed returns
  the schema-2 envelope over unauthenticated HTTP.
- **Phase 5 (Repoint `DEFAULT_FEED_URL`)** must land only after Phase 4 verification — flipping the
  constant before Pages is live would 404 every consumer.
- **Phase 6 (Disable legacy `claude-pricing` cron)** is a change in a different repo, gated on a
  >24h bake after a clyde release carrying the new URL ships.

Phases 1-3 are inert until Phase 4/5 happen (Pages off + `DEFAULT_FEED_URL` still points at the old
feed), so landing them now is safe and does not change runtime behavior.
