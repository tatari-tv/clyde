# Design Document: never serve a feed older than embedded, and give the Pages root something to serve

**Author:** Scott Idler
**Date:** 2026-07-25
**Status:** Implemented (all three phases shipped; AC-P4 and AC-P5 closed live on 2026-07-25 against `https://tatari-tv.github.io/clyde/`, so every acceptance criterion is now checked)
**Review Passes Completed:** 5/5
**Funnel position:** five passes, then a two-seat review panel over two rounds (Architect on Gemini, Staff Engineer on Codex, both completed both rounds). Sixteen findings, every one dispositioned in "Review Panel Dispositions". F1 restructured the doc from two phases to three; F15 added the structural CI guard without which Phase 1 would delete its own safety net. Open Questions empty. Ready to build, starting at Phase 1. **(State at AUTHORING time. All three phases have since shipped -- see Status above.)**
**Sibling:** `docs/design/2026-07-25-render-output-ceilings-config.md`, separate work, different crate and branch, no shared code

## Summary

Two pre-existing gaps in the pricing feed surface, both found while shipping `v0.13.3` (PR https://github.com/tatari-tv/clyde/pull/59) and both deliberately left out of scope then. First: the staleness guard runs only on a live fetch, so a cached feed older than the crate's embedded baseline is served for up to 24h with no banner and silently low cost totals. Second: `https://tatari-tv.github.io/clyde/` 404s because `pages.yml` stages exactly one file and no `index.html` exists in the repo.

Separate work from the render-ceilings design. Different crate, different branch, own phases.

## Problem Statement

### Background

- The pricing feed resolution state machine is documented in `pricing/src/fetch.rs:1-34`. Its stated invariant: there is exactly one cache-write point (`write_cache_atomic` inside `fetch_and_cache`) and "every rejection path is arranged so a bad or stale feed can never reach it."
- That invariant is true, and it is about **writes**. Nothing checks a feed that is **already on disk**.
- The governing design is `docs/design/2026-06-29-move-pricing-feed-publishing-to-clyde.md`. Its stale-feed requirements are cited inline in the code as D2/D3/F1/F2 (the labels live in the code comments, not as greppable headings in the doc).

### Problem 1: a cache hit skips the embedded-baseline comparison

Two read sites return a cached feed with no version comparison:

| site | code |
|---|---|
| `fetch.rs:124-135`, the within-TTL fast path | `Ok(p) => return Ok(p.with_stale_feed(read_stale_marker(cfg)))` |
| `fetch.rs:192-199`, `fallback_chain` | `if cache.exists() && let Ok(p) = load_from_cache(...) { return Ok(p) }` |

`load_from_cache` (`fetch.rs:341`) only deserializes; it never reads `data_version`. The staleness guard (`fetched_feed_is_stale`, `fetch.rs:481`) is called from exactly one place, inside `fetch_and_cache`.

Net effect: a cache holding a feed **older than the embedded baseline** wins for up to `DEFAULT_TTL_HOURS = 24`, with `stale_feed` unset, so `format_stale_banner` never fires and the user sees nothing.

**Observed, not theoretical.** During the `v0.13.3` release, after `claude-opus-5` landed in the embedded baseline, `clyde cost pricing --show` still omitted it and showed no banner, because a 24h cache written the previous evening won over the newer embedded data. `CLAUDE_PRICING_TTL_HOURS=0` was the only way to pull the correct feed.

The consequence is a low dollar figure: unknown models are excluded from totals outright (the `UnknownModel` arm in `cost/src/lib.rs`), so the total is arithmetic that is correct over data that is wrong, which is the one failure mode the "Rust does the math" split exists to make impossible. The window is exactly when correctness matters most, the hours right after a release that adds a model.

**It is not silent on most paths, and the doc should not claim it is.** `warn_unknown_models` (`cost/src/lib.rs:691-694`) prints an unknown-model banner to stderr, from six call sites (`:716, :744, :776, :830, :904, :976`). In the `v0.13.3` repro, `clyde cost today` would have shown it. There is exactly one path where the under-count really is silent, and it is Problem 3.

**The reported repro does not currently reproduce.** Embedded and the on-disk cache are both `2026-07-25T01:56:53Z` on this box, so the cache has since caught up. The bug must be reproduced by planting an old cache, not by waiting for one.

### Problem 2: the Pages site root 404s

`.github/workflows/pages.yml:29-33` stages one file plus the Jekyll marker:

```yaml
mkdir -p _site
cp pricing/data/pricing.json _site/pricing.json
touch _site/.nojekyll
```

There is no `index.html` anywhere in the repo, so the site root has nothing to serve. The deploy is green and `/pricing.json` serves 200, so the effect is cosmetic, but it reads as a broken deploy: the GitHub Deployments tab links to the root URL, and following that link from a **successful** deployment lands on a 404.

### Problem 3: `Pricing::embedded()` reports no `data_version`, so the day-cost cache cannot tell one embedded baseline from another

Found by the Staff Engineer during design review, and it is the finding that decides whether fixing Problem 1 actually reaches the user.

- `pricing/src/feed.rs:78` sets `data_version: None` in `Pricing::embedded()`, even though the embedded JSON carries one and `embedded_data_version()` (`pricing/src/pricing.rs:90`) can read it.
- `cost/src/lib.rs:283` keys the day-cost cache on `pricing.data_version()`.
- `cost/src/cache.rs:81` folds `None` to the literal string `"none"`.
- So **every** embedded-resolved run, from every crate version ever built, shares one `"none"` key bucket.
- `cost/src/lib.rs:288-297` returns a day-cache hit without re-pricing and with `unknown_models: Vec::new()`. Its comment states the soundness condition out loud: *"That is sound only because the cache key includes the feed's `data_version`."* On the embedded path it does not.

The release-day sequence, which is precisely the window Problem 1 is about:

1. `v0.13.2` runs offline, resolves to embedded, writes a day cost under key `"none"`.
2. `v0.13.3` installs, now carrying `claude-opus-5` in its embedded baseline.
3. The Problem 1 gate correctly rejects the stale feed cache and routes to embedded.
4. The day cache hits **the same `"none"` key**, returns the old under-count without re-pricing, and reports no unknown models.

The banner from Problem 1's discussion does not fire here, because nothing re-priced. This is the one genuinely silent path, and fixing Problem 1 alone makes it *more* likely by routing more traffic to embedded. A fix that leaves the user-visible symptom intact is not a fix.

**Implementation wrinkle that must be decided here, not discovered later:** `EmbeddedData.data_version` is `#[cfg(feature = "fetch")]`-gated at `pricing/src/pricing.rs:63-64`, while `Pricing::embedded()` is not gated at all. Ungate the field. Cfg-gating the assignment instead would make day-cache behavior differ by feature flag, which is the same variant-asymmetry this codebase rejects elsewhere.

Nothing blocks the change: `pricing/src/feed/tests.rs:58` asserts `None` for a **legacy feed parsed via `from_bytes`**, not for `Pricing::embedded()`.

### Requirements, and who asked

| # | requirement | asked by |
|---|---|---|
| P1 | a feed older than the embedded baseline is never served, from any source | Scott, 2026-07-25, rolling both handoff items into this work |
| P2 | the fix is a design-doc change, not a targeted patch: it changes fetch/fallback resolution order | the handoff, citing `rules/taste.md` |
| P3 | the Pages root serves a page documenting the feed | Scott, same |
| P4 | the page cannot drift from the feed it documents | the handoff, as the reason to read `./pricing.json` client-side |
| P5 | the corrected data reaches the user's TOTAL, not just the resolved `Source` | Staff Engineer, design review 2026-07-25. Without this, P1 is satisfied while the wrong number is still printed |

### Goals

- One invariant, enforced at every point a feed is returned rather than only where one is written (P1).
- Sidecar semantics untouched: `fetch_and_cache` stays the only writer AND the only clearer of the stale marker (P2).
- A Pages root that serves the feed's published contract and fails visibly if it cannot read the feed (P3, P4).

### Non-Goals

- **No rework of the day-cost cache mechanism.** `v0.13.3` already keyed `~/.cache/clyde/cost/` on the feed version via `cost/src/cache.rs::compute_cache_key`, and that keying stays exactly as it is. This is the **feed** cache (`~/.cache/clyde/pricing/`). Different cache, different bug, do not redo the first one. **One exception, forced by Problem 3 below:** `Pricing::embedded()` must start reporting its `data_version` so that existing keying actually discriminates. That is a one-field fix to the feed type, not a change to the cache.
- **No provenance tracking for custom feed URLs.** `CLAUDE_PRICING_FEED_URL` (`pricing/src/fetch.rs:52`, `:79`) can repoint the source while `cache_path()` (`:86-88`) stays fixed and `load_from_cache` relabels with the current URL (`:353-359`), so a cache written from one feed can be served as another. Pre-existing, orthogonal to this change, and behind an env-var escape hatch. Excluded.
- **No change to the user override's position in `fallback_chain`.** An explicit local override is the operator's documented escape hatch even when embedded is newer (`fetch.rs:32-34`). It keeps winning.
- **No change to the TTL, the backoff window, or the banner text.**
- **No new stale-marker semantics.** Excluded, see Resolved Decisions.
- **The page is not a dashboard.** It documents the feed. No charts, no history, no analytics.

## Proposed Solution

### Overview

Today's invariant is about the write point. Restate it as a property of every read:

> **Never serve a feed older than the embedded baseline.** Whatever the source, the candidate loses to embedded when its `data_version` is older, missing, or not comparable.

The predicate that decides this already exists, is pure, and already handles the canonical-UTC comparison problem correctly (`fetch.rs:481`, with `is_canonical_utc` at `:502`). It is called from exactly one site, `fetch.rs:420`. The fix is to call it from the other two, and to rename it, because it stops being about fetched feeds:

```rust
// was: fn fetched_feed_is_stale(fetched: Option<&str>, embedded: Option<&str>) -> bool
fn loses_to_embedded(candidate: Option<&str>, embedded: Option<&str>) -> bool
```

`fetched_feed_is_stale` would be a lying name the moment a cached feed is passed to it. The behavior is unchanged: older, missing, or non-canonical candidate loses; a non-canonical **embedded** version disables the guard rather than rejecting everything.

### Architecture

Three call sites for one predicate, one behavior per site:

| site | today | after |
|---|---|---|
| `fetch_and_cache:418-430` | rejects the fetched feed before `write_cache_atomic`, writes the sidecar | unchanged, except the renamed call |
| `auto_with_config:124-135` | returns the cached feed | gate first; on loss, `warn!` once and **fall through** to the fetch path instead of returning |
| `fallback_chain:192-199` | returns the cached feed | gate first; on loss, **skip the cache** and continue to user override, then embedded |

Falling through rather than returning is what lets the fresh-cache case self-heal **when a fetch is actually reachable**: the next step is the fetch that replaces the bad cache.

**It does not self-heal while the failure-backoff window is open**, and the doc should not claim otherwise. With backoff active, `auto_with_config` short-circuits to `fallback_chain` (`fetch.rs:137-141`), which rejects the same cache again and resolves to embedded. Correct output, but the bad cache is re-read and re-rejected on every invocation until backoff expires. Terminating, not self-healing.

**Warn exactly once per resolution, and pin it with an AC.** The naive implementation warns twice on that path: once at the `auto_with_config` gate, once again inside `fallback_chain`. Two log lines for one fact, against a codebase that already keeps an explicit warn-once discipline (`fetch.rs:158-161`, where a stale rejection deliberately suppresses the generic fetch-failure warning because "the guard already logged exactly once"). The inverse failure is equally live: warn only at `auto_with_config` and a fallback-chain-only rejection after a fetch failure goes unlogged. Both directions get an AC.

**Sidecar semantics do not change, and this is load-bearing.** The cache path neither writes nor clears the stale marker. F1 ("`fetch_and_cache` is the only clearer") is preserved exactly, and no new writer is introduced either. The reason is that the sidecar means "the upstream FEED we fetched was behind embedded". A cache behind embedded is a different fact, and the sidecar has **three** consumers that would all be told a lie:

| consumer | what a spurious sidecar does |
|---|---|
| `format_stale_banner` | prints an upstream-feed-regressed banner for a feed that may be current |
| `cost/src/lib.rs:622-628` `resolve_stale_feed`, on `--offline` | reads the sidecar directly, because `with_user_override` never touches the fetch layer |
| the shipped statusline segments (`cost/src/statusline.rs:14`, fixtures at `cost/src/tests.rs:351-395`) | shell that stats `${XDG_CACHE_HOME:-$HOME/.cache}/clyde/pricing/stale_feed.json` and prepends a glyph iff it exists |

The statusline is the one that settles it. The sidecar is cleared only by a clean fetch, so writing it from the cache path would light a persistent glyph in the user's prompt for a condition that is not upstream staleness and that they cannot act on.

Observability instead: one `warn!` naming both versions and the cache path, matching the discipline of the sibling guard at `fetch.rs:425`, which warns once with both versions and the URL.

The module doc's ASCII state machine (`fetch.rs:10-26`) shows the cache-hit arm going straight to `load_from_cache`. It must show the version gate, or it becomes the next stale comment.

#### Edges, resolved

- **"Fall through" cannot loop.** From the rejected fresh-cache hit the next step is either the backoff short-circuit, which calls `fallback_chain` (where the same cache is rejected again, then user override, then embedded), or the fetch. Every path terminates in at most one extra step, and the cache is never consulted a third time.
- **A cache with no `data_version` loses.** `loses_to_embedded(None, Some(e))` hits the catch-all arm and returns true, matching how a fetched feed with a missing version is already treated. A cache we cannot date is a cache we cannot trust against embedded.
- **A non-canonical embedded version disables the gate**, exactly as it disables the fetch guard today. The cache is then served as it is now. The guard falls open rather than rejecting everything, which is the existing and correct choice.
- **`refresh()` (`fetch.rs:201`) is unaffected.** It calls `fetch_with_stale_persist` directly and never reads the cache, so it has no read site to gate.
- **The user override still beats embedded** even when embedded is newer. That is `fetch.rs:32-34`'s documented escape hatch and this design does not touch it. Only the cache moved.

### Why reject rather than serve-with-a-banner

The handoff left this open. Resolved: **reject.**

- The two options differ only when offline, and offline is exactly where reject wins. The embedded baseline is compiled into the binary and always available, so a cache older than embedded is never the best data on hand. There is no case where serving it helps.
- The banner reports a stale upstream feed. A stale cache is not that, and saying so would be a lie the user cannot act on.
- Rejecting produces strictly better data with strictly less user-facing noise.

### Data Model

None. No new file, no new sidecar field, no new config key, no schema change.

### The site index

- **Source at `pricing/site/index.html`.** Not `pricing/data/`, which holds generated artifacts only. `site` is a single lowercase word, per the naming convention.
- **`pages.yml` needs two edits, and missing the second fails silently:**
  1. a second `cp pricing/site/index.html _site/index.html` in the staging step
  2. `pricing/site/index.html` added to `on.push.paths`, which today lists only `pricing/data/pricing.json` and the workflow itself. Without it, edits to the page never deploy and nothing reports an error.
- **The page reads `./pricing.json` client-side** so it cannot drift from the feed it documents. Everything derived from the feed (`data_version`, `schema_version`, `min_library_version`, model count, the rates table) is rendered from the fetch, never transcribed into the HTML.
- **A failed fetch renders a loud, visible error, never an empty table.** An empty rates table looks like real data reporting zero models. Fail visibly.
- **Static prose covers what the feed cannot say about itself:** that clyde fetches this URL at runtime so a data refresh reaches consumers within about 24h with no crate bump or re-pin; how the feed is produced (`pricing/bin/update` with its dual parser, refuse-on-disagreement, 5x and absolute-bound regression guards, carry-forward of delisted models; the `refresh-pricing.yml` daily cron; `pages.yml` publish on merge); and the caveat from `pricing/CLAUDE.md` that new model launches are **not** hands-off, because date-tiered introductory pricing emits one row per tier instead of a clean model id and the bare `opus`/`sonnet`/`haiku` aliases in `normalization.json` are human-authored and never repointed automatically.

**HISTORICAL, AND DO NOT ACT ON IT NOW: `pricing/site/index.html` is SHIPPED, reviewed source as of Phase 3. Do not archive or delete it.** The paragraph below describes a DIFFERENT, uncommitted file that occupied that path before Phase 3 ran, and it was archived at the time exactly as instructed.

**The untracked `pricing/site/index.html` that was in the working tree before Phase 3 was scratch.** It was written from a misread instruction during the `v0.13.3` session, never committed, never reviewed, and `pages.yml` was reverted. It is not a sanctioned starting point. Archive it with `rkvr rmrf` before Phase 2 starts so it cannot be mistaken for prior art.

### Implementation Plan

Three phases. The prerequisite that makes the gate's benefit reach the user goes first, because landing the gate without it improves the resolved `Source` and not the printed number.

#### Phase 1: `Pricing::embedded()` reports its own `data_version`
**Model:** sonnet

- Ungate the whole chain, not one field. `fetch` is **not** a default feature, and `efficiency/Cargo.toml:18` and `common/Cargo.toml:17` both depend on `claude-pricing` without it, so a partial ungate will not compile for them:

| site | what it is |
|---|---|
| `pricing/src/pricing.rs:49` | `PricingFile.data_version`, the serde field |
| `pricing/src/pricing.rs:63` | `EmbeddedData.data_version` |
| `pricing/src/pricing.rs:75` | the assignment inside `embedded_data()` |
| `pricing/src/pricing.rs:89` | `embedded_data_version()`, which becomes the shared accessor rather than a fetch-only one |

- **Rewrite the comment at `pricing/src/pricing.rs:44-48`.** It exists solely to justify the gate ("the struct carries no never-read field when the crate is built without fetch"), and that justification dies here: after this change the field IS read in a non-fetch build, by `Pricing::embedded()`, which was never gated. Leaving it is the M3 stale-comment failure repeating itself, in the same session that catalogued M3.
- No new dependencies: the gate covers `dep:tempfile` and `dep:ureq`, and parsing a string out of already-parsed JSON needs neither.
- `Pricing::embedded()` (`pricing/src/feed.rs:78`) carries that version instead of `None`.
- Correct the day-cache-hit comment at `cost/src/lib.rs:293-297`: its soundness claim was false on the embedded path and is only true once this lands. It is now the thing that makes it true, so say that.
- **Both `compute_cache_key` sites, not one.** `cost/src/lib.rs:283` is the single-day READ; `cost/src/lib.rs:496` is the multi-day WRITE (cited as `:489` when this doc was written; Phase 1's added comment shifted it down seven lines). Fixing only the read side leaves the write side stamping `"none"` while the read side looks for a version, so every multi-day-written entry misses forever.
- **Add `cargo check -p claude-pricing` to the `check` task in `.otto.yml`.** This is the structural half of the fix and it matters more than the field change:

  `.otto.yml:49` and `:52` run `cargo check` / `cargo clippy` with `--workspace --all-targets --all-features`, and `:62` runs `cargo test --workspace`. `--all-features` turns `fetch` **ON**, and `--workspace` unifies features across members regardless. So **no CI job ever compiles `claude-pricing` without `fetch`**, even though `efficiency` and `common` consume it that way (`cargo tree -p efficiency -e features` resolves it to `default` only). A partial ungate, or any future edit assuming the field exists, passes `otto ci` green and breaks only for the non-fetch consumers, including the `ccu`/`cr` external pins that `pricing/src/pricing.rs:44-48` names as the whole reason the gate exists.

  Verified the guard is cheap and green today: `cargo check -p claude-pricing` returns rc=0 in under six seconds. Without it, this phase removes the gate and simultaneously removes the only thing that would have reported the removal was incomplete.

- **One-time cache orphaning, benign, worth one line so nobody reads it as a regression.** Day entries previously written under the `"none"` key become unreachable for embedded-resolved runs, so the first run after this lands recomputes them. No migration is warranted: the cache is explicitly disposable (`cost/src/cache.rs:84-85`, "not migrated by bootstrap, it rebuilds ... on first run").
- **Success criteria:**
  - `Pricing::embedded().data_version()` equals the `data_version` in `pricing/data/pricing.json`.
  - two embedded-resolved runs whose embedded baselines differ produce **different** `compute_cache_key` values at BOTH `cost/src/lib.rs:283` and `:489`; today both stamp `"none"`. The existing tests at `cost/src/cache.rs:211-235` already prove the key discriminates on version, so this is about the value reaching them.
  - `pricing/src/feed/tests.rs:58` passes unchanged: it asserts `None` for a legacy feed via `from_bytes`, not for `Pricing::embedded()`.
  - `cargo check -p claude-pricing` (fetch off) is green, and it runs in `otto ci` from now on.

#### Phase 2: close the read-side staleness gap
**Model:** opus

- Rename `fetched_feed_is_stale` -> `loses_to_embedded`; it is now the shared predicate for fetched and cached candidates alike.
- Factor the cache-candidate check into ONE shared helper that both read sites call, so a future third read site has an obvious thing to call and AC-P7 can catch it if it does not.
- Gate both cache-return sites. `auto_with_config` falls through on loss; `fallback_chain` skips to user override then embedded.
- Warn **exactly once per resolution**, naming the cached `data_version`, the embedded `data_version`, and the cache path. Both the double-warn (fresh-cache rejection with backoff active) and the no-warn (fallback-chain-only rejection) shapes are wrong.
- Sidecar untouched from the cache path: no write, no clear.
- Update the module-doc state machine to show the gate on the cache-hit and fallback arms.
- **Success criteria:**
  - a planted cache with `data_version` older than embedded is NOT served: with the network unavailable, resolution is `Source::Embedded` **and the reported total reprices**, not merely the resolved source.
  - a planted cache NEWER than embedded is still served with zero network calls (the TTL fast path is not broken).
  - exactly one warn on the backoff-active rejection path, and at least one on the fallback-chain-only path.
  - `stale_then_fresh_cache_hit_still_reports_stale` (`fetch/tests.rs:800`) passes **unchanged**. Its `V1_FEED` fixture carries `data_version: 2099-01-01T00:00:00Z`, deliberately far newer than embedded and commented as such at `tests.rs:61-66`, so the new gate cannot fire on it. The handoff predicted this test would need rethinking; it does not, and that prediction is worth not acting on.
  - `grep -rn 'fn fetched_feed_is_stale' pricing/src` returns nothing.

#### Phase 3: the Pages site index
**Model:** sonnet

- `rkvr rmrf` the untracked scratch `pricing/site/index.html` first, then author the real one. **DONE 2026-07-25; the scratch file was archived to `/var/tmp/rmrf/2026-07-24-223307-000/` and the real page authored in its place. The file at that path today is the SHIPPED page -- this bullet is a completed step, not a standing instruction.**
- Both `pages.yml` edits: the second `cp`, and `pricing/site/index.html` in `on.push.paths`.
- Update `pricing/CLAUDE.md`: it documents the feed pipeline and the `pages.yml` publish step, and this phase adds a published artifact and a new `pricing/site/` source directory. CLAUDE.md is living and tracks shipped reality.
- **Success criteria:**
  - `otto ci` exit 0 (the page is static, so CI only has to stay green).
  - locally: serving `pricing/site/` next to a copy of `pricing/data/pricing.json` as `pricing.json` renders the version fields, the model count, and the rates table; renaming the JSON so the fetch 404s renders a visible error rather than an empty table.
  - post-merge, live: `https://tatari-tv.github.io/clyde/` returns 200 and renders the feed's current `data_version`; `https://tatari-tv.github.io/clyde/pricing.json` still returns 200.

## Acceptance Criteria

Phase 1 owns AC-P8 and AC-P9. Phase 2 owns AC-P1, P2, P3, P6, P7. Phase 3 owns AC-P4 and P5.

- [x] AC-P8: `Pricing::embedded().data_version()` equals the `data_version` in `pricing/data/pricing.json`, and two embedded baselines with different versions produce different `compute_cache_key` values at **both** call sites (`cost/src/lib.rs:283` read, `:496` write). Today both collapse to the `"none"` bucket.
- [x] AC-P9: the non-fetch build is exercised by CI. `cargo check -p claude-pricing` runs as part of the `check` task in `.otto.yml` and is green. Falsifiable: re-gate any one of the four `pricing.rs` sites and `otto ci` must go red. Today it would stay green, because `--all-features` turns `fetch` on for every existing job.
- [x] AC-P1: a cache older than embedded is never served, **and the user-visible total is corrected**. With a planted older `data_version` and no network, `auto_with_config` resolves to `Source::Embedded` AND a day-cost run reprices rather than returning a cached under-count. Resolving the right `Source` while printing the wrong number does not pass this.
- [x] AC-P2: the fast path survives. With a planted cache newer than embedded and within TTL, resolution serves the cache and makes zero HTTP requests, asserted by a `mockito` mock with `.expect(0)`. Do **not** assert on `Source` here: `load_from_cache` labels a cache hit as `Source::Fetched` (`pricing/src/fetch.rs:353-359`), so a `Source` assertion cannot distinguish cache from network and would be vacuous.
- [x] AC-P3: sidecar semantics are unchanged. No cache-path code writes or clears the stale marker; `fetch_and_cache` remains the only clearer; `stale_then_fresh_cache_hit_still_reports_stale` passes unchanged.
- [x] AC-P6: exactly one rejection `warn!` per resolution. Asserted in both directions: the fresh-cache rejection with the failure-backoff window OPEN emits one warn, not two; a fallback-chain-only rejection after a fetch failure emits one, not zero.
- [x] AC-P7: an unguarded cache read is falsifiable. Every `load_from_cache` call site goes through the shared cache-candidate helper, asserted mechanically so a future third read site fails the check. Grepping for the old predicate name is not sufficient and does not count.
- [x] AC-P4: `https://tatari-tv.github.io/clyde/` returns 200 and renders `data_version`, `schema_version`, `min_library_version`, the model count, and the rates table, all read from `./pricing.json` at load time.

  **Closed live 2026-07-25** against the deployed site. Root and `/pricing.json` both return 200, and the served feed is byte-identical to `pricing/data/pricing.json` -- `cmp` clean, SHA-256 `52b86d79541f7ca7f215e89abfb0c5f8d01fd0ce1e56079c2dc284df3bf2997a` on both sides, 4147 bytes. Headless Chrome on the live URL renders `data_version` `2026-07-25T01:56:53Z`, `schema_version` `2`, `min_library_version` `2.0.0`, model count `18`, and 18 rate rows; `claude-fable-5` shows `$10 / $50 / $12.5 / $20 / $1`, matching the feed's rates rather than a transcription. The read-at-load-time half is supported negatively: none of `2026-07-25T01:56:53Z`, `2.0.0`, or `claude-fable-5` appears anywhere in `pricing/site/index.html`, so the rendered values are not hardcoded in the page. Provenance is closed by the page itself carrying exactly one `fetch` call and no other network API, with all four fields assigned from its parsed result (`index.html:273-277`, `:297`).

  **Verified locally in a real browser (headless Chrome), 2026-07-25**, serving `pricing/site/` beside a copy of the feed: all four version/count fields match `pricing/data/pricing.json` exactly, all 18 model rows render, and a spot-checked rate matches the feed rather than being transcribed. A synthetic feed carrying `*_above_200k` rates renders the long-context tier line (the real feed currently has none, so that branch would otherwise ship unexercised). With `pricing.json` absent the page shows the error banner, hides the feed block entirely, renders **zero** rate rows, and names the cause (`HTTP 404`) -- i.e. visibly broken rather than an empty table that reads as real data reporting zero models.
- [x] AC-P5: a **later, index-only** commit touching just `pricing/site/index.html` on `main` triggers a Pages deploy. The Phase 3 commit itself proves nothing here, because it also edits `.github/workflows/pages.yml`, which is already in `on.push.paths` (`pages.yml:8`) and would fire the deploy regardless of whether the new path was added.

  **Closed live 2026-07-25** by `5c76f97` (`fix(pricing): drop last row border stub`, #61), a follow-up PR opened for exactly this purpose. `git show --name-only 5c76f97` lists one file, `pricing/site/index.html`, so `pricing/data/pricing.json` and `pages.yml` were both untouched and neither could have fired the run. Pages run [30148682157](https://github.com/tatari-tv/clyde/actions/runs/30148682157) triggered on that push and deployed successfully, and the fixed rule is live in the served CSS. The deploy is therefore attributable to the `pricing/site/index.html` entry in `on.push.paths` and to nothing else, which is what F7 asked for and what the Phase 3 commit could not show.

  The two intervening runs do not close this and were not counted: #59 and #60 both changed `pricing/data/pricing.json`, which is its own `paths` entry.

## Resolved Decisions

| date | decision | rationale |
|---|---|---|
| 2026-07-25 | a cache older than embedded is REJECTED, not served with a banner | the two differ only offline, and offline is where reject wins: embedded is always available and always newer in this case. The banner reports a stale upstream feed, which a stale cache is not |
| 2026-07-25 | no sidecar write from the cache path | preserves F1 exactly and avoids making `--offline` report a stale feed when the feed may be current. One `warn!` carries the observability instead |
| 2026-07-25 | `fetched_feed_is_stale` -> `loses_to_embedded` | the predicate now judges cached candidates too. An identifier that says one thing and means another is not allowed |
| 2026-07-25 | on a fresh-cache rejection, fall THROUGH to the fetch rather than jumping to embedded | the next step is the fetch that replaces the bad cache, so the case self-heals in one tick instead of persisting until TTL |
| 2026-07-25 | page source at `pricing/site/index.html` | `pricing/data/` holds generated artifacts only; `site` is a single lowercase word |
| 2026-07-25 | the page reads `./pricing.json` client-side and fails visibly | a transcribed table drifts from the feed the moment the feed refreshes. A visibly broken page beats an empty table that looks like real data |
| 2026-07-25 | the untracked `pricing/site/index.html` is scratch, archived before Phase 2 | never committed, never reviewed, written from a misread instruction. Treating it as prior art would launder an unreviewed file into the repo |
| 2026-07-25 | one doc, two phases, separate from the render-ceilings work | Scott, 2026-07-25: "roll this into your work as a phase or two of separate work". Different crate, different branch, no shared code |

## Review Panel Dispositions (2026-07-25)

Design Review, both seats. Architect on Gemini, Staff Engineer on Codex, both rc=0. Every finding has a dispositive answer; nothing is dropped or deferred silently.

| # | finding | disposition |
|---|---|---|
| F1 | `Pricing::embedded()` carries `data_version: None`, so the day-cost cache cannot discriminate embedded baselines and the gate does not reach the user's total | **ACCEPTED, and it restructured the doc.** Independently verified at `feed.rs:78`, `cache.rs:81`, `lib.rs:283`, `lib.rs:293-297`. Became Problem 3, requirement P5, Phase 1, and AC-P8. Highest-value output of the panel |
| F2 | double-warn on the backoff path; "self-heals in one tick" is wrong when backoff is active | **ACCEPTED.** Both corrections folded into Architecture; AC-P6 pins the warn count in both directions. The finding's own citation (`fetch.rs:110-113`) is wrong, that is `env_hours`; the warn-once invariant is at `:158-161`. Substance right, pointer wrong |
| F3 | Alternative 2's stated rationale ("the callers want different behavior") is mechanically wrong | **ACCEPTED.** Verified at `fetch.rs:129` and `:193`. Rationale withdrawn and replaced; conclusion (gate at the callers) survives on a different reason. Took the alternative's real strength via a shared helper plus AC-P7 |
| F4 | "silently low dollar figure" is overstated; `warn_unknown_models` prints a banner from six sites | **ACCEPTED.** Problem 1 reworded; the genuinely silent path is now correctly identified as Problem 3 |
| F5 | the statusline reads the sidecar directly, a third and higher-visibility consumer | **ACCEPTED.** Folded into the sidecar argument as the consumer that settles it |
| F6 | `pricing/CLAUDE.md` is in neither phase's deliverables | **ACCEPTED.** Added to Phase 3 |
| F7 | AC-P5 passes vacuously: the Phase 3 commit also edits `pages.yml`, already in `paths` | **ACCEPTED.** AC-P5 now requires a later index-only commit |
| F8 | an AC-P2 `Source` assertion would be vacuous; `load_from_cache` labels a hit `Source::Fetched` | **ACCEPTED** as a guard against an implementer adding one. Written into AC-P2 |
| F9 | custom feed URLs let a cache written from one feed be served as another | **DEFERRED with a written reason.** Pre-existing, orthogonal, env-var gated. Disowned in Non-Goals rather than left implicit. Do not build for it |
| F10 | (Architect) why does a user override older than embedded still win? | **NOT RE-OPENED.** Already decided at `fetch.rs:31-34` and recorded as a Non-Goal. Settled items are not relitigated |
| - | Architect's `candidate_loses_to_embedded` rename | **REJECTED.** The signature already names the parameter `candidate`; the prefix restates the argument. `loses_to_embedded` ships |
| - | Architect's proposed replacement rationale for F3 ("caller-side gating avoids double-logging") | **REJECTED**, and it is refuted by F2: the caller-side design double-warns on exactly that path. Staff's framing was folded in instead |

### Round 2

Sent the panel two corrections (F2's citation, and that F1's fix is four sites) plus one question: does `Pricing::embedded()` reporting `Some(data_version)` change anything beyond the cache key? It accepted both corrections, answered the question **clean** on four independent checks (nothing branches on `data_version().is_none()` outside one unaffected test; `Pricing` is not `Serialize`, only `PricingFeed` is; no rendered or golden output carries it; every `Pricing::embedded()` consumer is price-lookup only), and returned three additions. All three verified and folded in.

| # | finding | disposition |
|---|---|---|
| F14 | there is a **second** `compute_cache_key` site at `cost/src/lib.rs:489`, the multi-day write path; F1 cited only `:283`, the single-day read | **ACCEPTED.** Verified. Fixing one side only would leave the write side stamping `"none"` while the read side looks for a version, so every multi-day-written entry misses forever. Both sites now in Phase 1 and AC-P8 |
| F15 | **`otto ci` cannot catch a non-fetch build break.** `.otto.yml:49`/`:52` use `--all-features`, which turns `fetch` ON, and `--workspace` unifies regardless, so no job ever compiles `claude-pricing` without it | **ACCEPTED, and it is the most valuable item of round 2.** Verified: `cargo tree -p efficiency -e features` resolves the dep to `default` only, and `cargo check -p claude-pricing` is green in under six seconds. Phase 1 now adds that command to the `check` task, with AC-P9 making it falsifiable. Without it, Phase 1 removes the gate and removes the only signal that the removal was incomplete |
| F16 | the change orphans day-cache entries written under the `"none"` key, causing a one-time recompute | **ACCEPTED** as a documentation item. Benign and self-healing; the cache is explicitly disposable (`cost/src/cache.rs:84-85`). One line so the implementer does not read the first-run recompute as a regression |

**Reviewer calibration note.** The Architect produced three citations in `cost/src/lib.rs` that were off by up to ~126 lines (`:496-503` for `resolve_stale_feed`, actual `:622-628`; `:519-523` for `run`, actual `:633`; `:114` for the stderr default, actual `:137`). Its conclusions on those points happened to be correct, but they were re-derived from lines read directly rather than carried forward on the strength of the citation. This is the second consecutive panel on this codebase where that reviewer's line numbers did not survive checking. Read its findings, do not trust its pointers.

## Alternatives Considered

### Alternative 1: serve the stale cache with the existing banner
- **Description:** return the cached feed, set `stale_feed`, let `format_stale_banner` warn.
- **Pros:** no resolution-order change; the user sees something is off.
- **Cons:** serves data that is known to be worse than what is already compiled into the binary, and repurposes a signal that means "the upstream feed regressed" to mean something else. The user cannot act on it either way.
- **Why not chosen:** there is no scenario, online or offline, where the older cache is the best available data.

### Alternative 2: check the version inside `load_from_cache`
- **Description:** push the gate down so every caller gets it for free.
- **Pros:** impossible to add a third unguarded read site later.
- **Cons:** `load_from_cache` is a deserializer. Folding a resolution-order decision into it hides the decision from the state machine the module doc exists to describe, and turns "this file did not parse" and "this file is out of date" into the same `Err` at every call site, which is the typed-values-at-seams rule in miniature.
- **Why not chosen:** policy belongs where the state machine is, at the callers. Genuinely the closest call in this doc.
- **Correction, both reviewers independently, 2026-07-25.** An earlier draft rejected this on the grounds that the two callers "want different behavior on rejection". That reason is mechanically wrong and is withdrawn: `auto_with_config` already has an `Err(e) =>` arm that falls through (`fetch.rs:129`) and `fallback_chain` already skips to the override on a non-`Ok` (`fetch.rs:193`), so returning `Err` would produce both desired continuations for free. Different continuation never required different validation. The conclusion survives on the reason above; the bad reason does not survive.
- **What the alternative was right about:** it cannot be bypassed by a future third read site, and gating at the callers can. So take the protection without the coupling: factor the check into one shared cache-candidate helper that both callers use, and add an AC (AC-P7) that makes an unguarded `load_from_cache` call site falsifiable. The doc's original structural check only grepped for the old predicate name, which would not catch a new read site at all.

### Alternative 3: drop the TTL fast path and always fetch
- **Description:** delete the cache-hit branch; correctness by always going to the network.
- **Pros:** the gap cannot exist.
- **Cons:** a network round trip on every `clyde cost` invocation, and total failure offline. The cache exists for good reasons.
- **Why not chosen:** trades a 24h correctness window for a permanent latency and availability cost.

### Alternative 4: transcribe the rates table into the HTML at publish time
- **Description:** have `pages.yml` generate the table from the JSON during staging.
- **Pros:** no client-side JS; the page works with scripting disabled.
- **Cons:** two representations of the same data in one deploy, which drift the moment anything publishes one without the other. The derived-field rule says drop the second representation rather than sync it.
- **Why not chosen:** the feed is right there at `./pricing.json`. Read it.

## Technical Considerations

### Dependencies

Zero new crates. Phase 2 adds one static HTML file and two lines of workflow YAML. Blast radius is one repo; no schema change, no crate-version implication (the crate major is locked to the feed `schema_version`, which does not move here).

### Performance

- The gate is a string comparison against a value already parsed. No measurable cost.
- The rejection path costs one extra fetch attempt in the window between a release and the next successful refresh. That is bounded by the existing 1h failure backoff (`DEFAULT_FAILURE_BACKOFF_HOURS`), so the worst case is one attempt per hour, not one per invocation. Worth stating because "reject the cache" reads like "fetch every time" and it does not.

### Security

No new surface. The page is static, served from GitHub Pages, and reads one same-origin JSON file. No credentials, no user input, no third-party script.

### Testing Strategy

- Unit, in `pricing/src/fetch/tests.rs`: plant-a-cache tests for both directions (older than embedded, newer than embedded) at both read sites, with `mockito` asserting the exact expected number of HTTP calls.
- Unit: the renamed predicate keeps its existing table of cases (older, equal, newer, missing, non-canonical candidate, non-canonical embedded).
- Regression: `stale_then_fresh_cache_hit_still_reports_stale` passes unchanged, which proves the sidecar path was not disturbed.
- Every new assertion proven to bite: break the gate, watch that specific test fail, restore from a copy rather than `git checkout`.
- Phase 2 is verified by serving the directory locally and by the post-merge live check. No CI test asserts on GitHub Pages.

### Rollout Plan

- Branch off current `main` (`0401997`, tagged `v0.13.3`). Suggested branch `feed-staleness-gap-and-site-index`, which fixes the PR title to `fix(pricing): feed staleness gap and site index` (a PreToolUse hook denies a mismatch).
- `main` is PR-gated. A hook requires a release-intent line in the PR body: `Release: rides this PR (vX.Y.Z)` or `Release: none -- <why>`.
- Order: `bump --no-tag` on the branch, PR, merge, then `bump --tag-only` on `main` and push the tag by name. Never plain `bump` on a gated repo, never a bump-only release branch.
- Phase 2's live criteria are only checkable after merge, because Pages deploys from `main`.
- After merge: `cargo install --path .`, then re-run the AC-P1 planted-cache check against the installed binary. Green CI is not done.
  - **Done 2026-07-25** against installed `clyde v0.14.0` (`v0.14.0` tagged on merged `main` via `bump --tag-only`). Recorded in the implementation notes under "Post-merge verification".
- **Nothing in this section runs without Scott's explicit approval.**

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Rejecting the cache turns into a fetch on every invocation | Low | Med | the existing 1h failure backoff bounds it to one attempt per hour; a successful fetch rewrites the cache and ends the condition |
| The gate accidentally rejects a good cache and forces embedded everywhere | Low | High | AC-P2 asserts the newer-cache fast path with a zero-call mock. `loses_to_embedded` already falls open when the embedded version is not canonical |
| The module-doc state machine goes stale against the new gate | Med | Med | updating it is a Phase 1 deliverable, not a follow-up. This codebase has a documented history of comments outliving the behavior they describe |
| `on.push.paths` omitted, so page edits never deploy | Med | Med | called out twice, and AC-P5 is exactly this assertion. The failure is silent, which is why it gets its own AC |
| The page's client-side fetch fails and shows nothing useful | Low | Med | the error state is a design requirement, not a nicety: visibly broken beats an empty table |
| The scratch `index.html` gets committed as if reviewed | Low | Med | archived with `rkvr rmrf` as the first step of Phase 2 |

## Corrections to the handoff brief

Recorded here rather than left in a chat log, because the next reader will read this doc, not that conversation.

- **"`stale_then_fresh_cache_hit_still_reports_stale` is most likely to need rethinking."** It does not. Its `V1_FEED` fixture carries `data_version: 2099-01-01T00:00:00Z`, chosen to be far newer than embedded and commented as such at `fetch/tests.rs:62`, so the new gate cannot fire on it. Acting on this prediction would mean rewriting a test that is already correct.
- **The stated repro does not currently reproduce.** Embedded and the on-disk cache are both `2026-07-25T01:56:53Z`, so the cache has caught up. The bug must be reproduced by planting an old cache.
- **"`rg` on this box applies a `--replace` from `~/.ripgreprc` and rewrites matched text."** There is no `~/.ripgreprc` and no ripgrep env var set. `rg` output is trustworthy here, which matters because several success criteria in both of today's docs are `rg` assertions.

## Open Questions

- None.

## References

- Handoff: `/tmp/claude-1000/-home-saidler-repos-tatari-tv-clyde/327e8d07-9777-4576-8692-9b4584a84ed3/scratchpad/handoff-pricing-staleness-gap.md`
- PR that surfaced both items: https://github.com/tatari-tv/clyde/pull/59
- `docs/design/2026-06-29-move-pricing-feed-publishing-to-clyde.md` (the governing feed design; D2/D3/F1/F2 are cited in the code comments)
- `pricing/src/fetch.rs:1-34` (the state-machine module doc), `:124-135` (fast path), `:192-199` (`fallback_chain`), `:341` (`load_from_cache`), `:412-430` (the existing guard), `:481` (`fetched_feed_is_stale`), `:502` (`is_canonical_utc`)
- `pricing/src/fetch/tests.rs:62` (the `V1_FEED` version comment), `:800` (`stale_then_fresh_cache_hit_still_reports_stale`)
- `cost/src/lib.rs` (`UnknownModel` exclusion, `resolve_stale_feed`), `cost/src/cache.rs::compute_cache_key` (the OTHER cache, already fixed)
- `.github/workflows/pages.yml`, `.github/workflows/refresh-pricing.yml`, `pricing/CLAUDE.md`
