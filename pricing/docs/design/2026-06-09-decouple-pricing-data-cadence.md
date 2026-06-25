# Design Document: Decouple Pricing-Data Changes from the Shared Library

**Author:** Scott Idler
**Date:** 2026-06-09
**Status:** Implemented
**Review Passes Completed:** 5/5 (rewritten after Architect + Staff Engineer design review, 2026-06-09; both endorsed in a second round and the open questions below are resolved)

## Summary

The friction we want to remove is real but small: Anthropic changes pricing
often, and a data refresh should never drag the shared Rust library or its
consumers (`ccu`, `cr`) through a build/release. The original proposal was to
split `claude-pricing` into a data-only feed repo plus a separate logic crate.
Two independent design reviews (Gemini Architect and Codex Staff Engineer) both
recommended **against** the split: the functional benefit already exists via the
runtime feed, the split would break the shipped `ccu --offline` behavior, and it
is a 4-repo migration for a structural-only gain. This doc adopts their landing
point: **keep one repo, and remove the friction with a process rule plus a CI
path-filter** - no split, no API break.

## Problem Statement

### Background

`claude-pricing` is one Rust crate (`claude_pricing` v0.2.0) that owns both the
pricing **data** (`data/pricing.json`, refreshed daily from Anthropic by
`bin/update`, published to GitHub Pages by `pages.yml`) and the shared **logic**
(JSONL parsing, cost math, `normalize_model_id`, the `Pricing` fetch client).
The data is also compiled into the crate via `include_str!` as the
`Pricing::embedded()` baseline. `ccu` and `cr` depend on the crate by git tag
with `features = ["fetch"]` and call `Pricing::auto` at runtime.

### Problem

It *feels* like every pricing change requires re-pinning and rebuilding the
consumers. Review verified this is mostly perception, not mechanism:

- The runtime fetch path already delivers pricing updates with no rebuild
  (`DEFAULT_FEED_URL`, `src/feed.rs:13`; consumers call `Pricing::auto` -
  `ccu src/main.rs:660`, `cr src/lib.rs:32`).
- The README already states the intended rule (`README.md`): "To bump pricing
  in either tool, do nothing... To bump library code, cut a tag here."
- The last release train was **not** data-only. The `v0.2.0` bump commit
  (`5c223c1`) touched only `Cargo.toml`/`Cargo.lock`; the substantive change it
  followed (`5fa654c`) was a mixed code/data change (`bin/update`,
  `data/pricing.json`, `src/pricing.rs`, tests). So it is not evidence that
  data-only changes force rebuilds.

What remains are three concrete, small irritants:

1. The Rust CI (`ci.yml`) runs on every PR, including pure data refreshes, so a
   one-line `pricing.json` change burns a full cargo build + clippy + test.
2. `release.yml` builds a `claude-pricing` **binary that does not exist** (the
   crate is `[lib]`-only; consumers use git + tag, not release artifacts). It is
   vestigial and confusing.
3. The no-re-pin rule lives only in the README prose; nothing enforces or
   reinforces it, so the habit of bumping the tag on data PRs persists.

### Goals

- A data-only PR triggers **no Rust build** and requires **no consumer action**.
- Pricing updates keep flowing to `ccu`/`cr` at runtime (unchanged).
- `ccu --offline` keeps working (the embedded baseline stays).
- Minimal change - no repo split, no public API break, no migration.

### Non-Goals

- Splitting `claude-pricing` into two repos (rejected - see Alternatives).
- Dropping the `Pricing::embedded()` baseline (rejected - breaks `--offline`).
- Reimplementing parsing/cost-math in consumers.
- Changing the `bin/update` refresh pipeline or the feed schema.

## Proposed Solution

### Overview

Decouple by **process + CI**, not by repository topology:

1. **Path-filter the Rust CI** so data-only PRs skip the cargo job entirely.
2. **Delete the vestigial `release.yml`.**
3. **Codify the no-re-pin rule** where contributors will see it.
4. **Leave the library, the embedded baseline, and the fetch chain unchanged.**

### Architecture

The repo topology does not change. The only change is which workflows fire on
which paths.

```
data-only PR (data/pricing.json, *.sha256)
   -> Rust CI: SKIPPED (path-filtered)
   -> merge (code-owner review only; CI is not a required check)
   -> push to main -> pages.yml -> GitHub Pages feed
   -> ccu/cr pick up new rates at runtime (<=24h TTL). No re-pin.

code PR (src/**, Cargo.toml, build.rs)
   -> Rust CI: RUNS (test + clippy + fmt)
   -> merge -> bump -> tag -> re-pin ccu/cr to the new tag -> rebuild
```

### Why path-filtering is safe here

`main`'s branch protection requires PR review + code-owner review
(`@tatari-tv/sre`, per `.github/CODEOWNERS`) but has **no required status
checks** (verified this session via `gh api
repos/tatari-tv/claude-pricing/branches/main/protection` -
`required_status_checks` is empty). So skipping the Rust CI on a data-only PR
does **not** leave a required check pending - the PR can still merge on review
alone. This removes the classic path-filter trap (required
check never reports -> PR wedged forever); it simply does not apply here.

### Implementation Plan

#### Phase 1: CI path-filter, workflow cleanup, and codified rule
**Model:** sonnet
- `ci.yml`: add `paths-ignore` for `data/**` and `**/*.md` on the
  `pull_request` and `push` triggers so data-only and docs-only changes skip the
  cargo job. Leave Rust paths fully covered.
- Delete `release.yml` (builds a non-existent binary; consumers consume the
  crate by git + tag). If a release artifact is ever wanted, that is a separate,
  deliberate decision.
- Reinforce the no-re-pin rule at the highest-leverage point: add it directly to
  the automated PR body in `refresh-pricing.yml` (the existing review-guidance
  block) - "data-only refreshes must not bump the crate version or re-pin
  consumers." Keep the README statement; a `CONTRIBUTING.md` is optional and
  weaker than the PR-body note.
- Verify: open a throwaway data-only PR and confirm the Rust CI does not run and
  `pages.yml` still deploys on merge; open a throwaway `src/**` PR and confirm
  the Rust CI does run.

No code (`src/**`) changes. No consumer changes.

## Alternatives Considered

### Alternative 1: Split into two repos and drop the embedded baseline (original proposal)
- **Description:** `claude-pricing` becomes a data-only Pages publisher; logic
  moves to a new crate (`claude-cost-core`); drop `Pricing::embedded()`.
- **Pros:** Cleanest conceptual separation; Rust-free data repo; independent
  release cadence and history.
- **Cons (verified by both reviewers):**
  - **Breaks `ccu --offline`.** The offline path calls
    `Pricing::with_user_override("ccu")` (`ccu src/main.rs:661`), which falls
    back to `embedded()` (`src/feed.rs:60`). Removing embedded turns a fresh
    machine with no cache/override into a hard error - a regression of a
    documented, shipped flag (`ccu README.md`), not an internal edge case.
  - **Weakens the `min_library_version` contract.** Keep-and-warn is unsafe:
    crate-side logic like `LONG_CONTEXT_THRESHOLD` (`src/pricing.rs:28`, used at
    `:73`) can change while `schema_version` stays `1` and `min_library_version`
    bumps. An old crate would apply new rates at the old threshold and silently
    miscalculate. The embedded fallback exists precisely to lock data to the
    logic that understands it.
  - **API contradiction in the original doc:** the free-standing
    `pricing::calculate_usd` (`src/pricing.rs:116`) calls `default_pricing()`,
    so it cannot be "unchanged" while `default_pricing()` is removed. (Neither
    consumer uses the free function - both use the `Pricing::calculate_usd`
    method - so it is removable, but the doc was internally inconsistent.)
  - 4-repo migration for a benefit that is structural only; the functional
    zero-rebuild outcome already exists.
- **Why not chosen:** Both reviewers independently recommended against it. The
  cost and the `--offline` regression outweigh conceptual cleanliness.

### Alternative 2: Status quo (do nothing)
- **Description:** Keep one repo and rely on the existing README rule.
- **Pros:** Zero work.
- **Cons:** Leaves the three irritants (Rust CI on data PRs, vestigial
  `release.yml`, unenforced rule). The friction that motivated this doc stays.
- **Why not chosen:** The proposed cleanup is cheap and removes the friction
  without the split's downsides.

### Alternative 3: Frozen last-resort baseline instead of `include_str!`
- **Description:** Replace the compiled-from-`data/pricing.json` baseline with a
  small hand-frozen snapshot.
- **Pros:** Would be the right floor *if* the data ever left the repo.
- **Why not chosen:** Moot while staying single-repo - the embedded baseline is
  already the same file as the published feed, so it is auto-fresh on every data
  PR with zero extra maintenance. Only relevant if a split is revisited later.

## Technical Considerations

### Dependencies
- None added or changed. No `src/**`, `Cargo.toml`, or consumer edits.

### Performance / Security
- No runtime change. CI minutes drop on data-only PRs. No new surface.

### Testing Strategy
- The existing crate test suite is untouched (the embedded-baseline tests in
  `src/feed/tests.rs`, `src/fetch/tests.rs`, `src/pricing/tests.rs` stay valid).
- Validate the CI path-filter behaviorally with one data-only and one code PR
  as described in Phase 1.

### Rollout Plan
- Single small PR to `claude-pricing`. No coordination with consumers required.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Path-filter wedges a data PR on a required check | Low | High | Verified `main` has no required status checks; filter is safe |
| `paths-ignore` accidentally skips CI on a code PR | Low | Med | Scope ignore to `data/**` and `**/*.md` only; verify with a code PR |
| Contributor re-pins consumers out of habit | Med | Low | Codify the rule in README + CONTRIBUTING; reinforce in refresh PR body |
| Deleting `release.yml` removes a wanted artifact | Low | Low | It builds a non-existent binary today; re-add deliberately if ever needed |

## Open Questions (resolved in round-2 review)

- [x] **`fetch_and_cache` write-before-validate (`src/fetch.rs:192`).**
      **Resolved: real bug, out of scope here, track as a separate issue.** Both
      reviewers confirmed the downloaded bytes are cached before validation, so
      an incompatible feed poisons the cache and causes a wasteful
      fetch-write-fail cycle on every subsequent run. **Important subtlety
      (Codex):** the naive "parse first, write only if `Ok`" fix is
      insufficient, because `from_bytes` returns `Ok(Pricing::embedded())` - not
      `Err` - on a too-high `min_library_version` (`src/feed.rs:99-104`). The
      correct fix caches only when the parse result is genuinely fetched:

      ```rust
      let pricing = Pricing::from_bytes(&bytes, cfg.url.clone(),
          Source::Fetched { url: cfg.url.clone(), fetched_at })?;
      if !matches!(pricing.source(), Source::Fetched { .. }) {
          return Err(PricingError::Fetch { url: cfg.url.clone(),
              message: "fetched feed is incompatible with this library".into() });
      }
      write_cache_atomic(&cfg.cache_path(), &bytes)?;
      ```

      It changes `src/**` runtime behavior, needs cache-non-poisoning tests
      (extend `schema_mismatch_falls_back`, `malformed_response_...` in
      `src/fetch/tests.rs`), and warrants its own crate tag - so it does **not**
      belong in this no-code-changes PR. File it separately.

- [x] **Revisit trigger for the split.** **Resolved: "no, unless X."** Split the
      repo only when the feed needs ownership/governance independent of the Rust
      crate - concretely, when **both** hold: (1) the pricing feed becomes a
      shared cross-language product with multiple active non-Rust/external
      consumers that need data/schema changes, review ownership, or publishing
      permissions independent of the crate maintainers; **and** (2) the crate has
      first moved to a frozen last-known-compatible embedded baseline instead of
      `include_str!`-ing the live `data/pricing.json`. Adjacent single triggers
      the Architect flagged: a non-Rust consumer that cannot depend on the crate;
      automated pricing PRs so frequent they saturate `src/` git history and
      break `git bisect`; or consumers carrying their own build-time baseline
      (removing the need for `Pricing::embedded()` entirely). Until one of these
      lands, path-filtering delivers the operational benefit without the
      migration.

## References

- Reviews: Architect (Gemini) and Staff Engineer (Codex) design reviews,
  2026-06-09 - both recommended against the split.
- This repo: `src/feed.rs` (`DEFAULT_FEED_URL`, embedded fallback, gate),
  `src/fetch.rs` (fetch/cache chain), `src/pricing.rs` (`LONG_CONTEXT_THRESHOLD`,
  `calculate_usd`), `ci.yml`, `pages.yml`, `release.yml` (vestigial),
  `README.md` (no-re-pin rule).
- Consumers: `tatari-tv/claude-cost-usage` (`--offline` path), `tatari-tv/claude-report`.
- Superseded proposal: `2026-06-09-split-pricing-data-from-logic.md` (the split;
  retained for history).
