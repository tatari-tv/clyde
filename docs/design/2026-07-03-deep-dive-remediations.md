# Design Document: Deep-Dive Findings Remediation

**Author:** Scott Idler
**Date:** 2026-07-03
**Status:** Implemented
**Review Passes Completed:** 5/5 self + external review panel (Architect/Gemini + Staff
Engineer/Codex), 2026-07-03; panel findings folded in below

## Summary

A full-source deep dive of the clyde workspace (published at
`https://marquee.internal.tatari.dev/p/~scott-idler/clyde-deep-dive`) surfaced nine findings
across the `pricing`, `permit`, `cost`, and `report` crates. This doc verifies each finding
against current code, records the decisions taken on the four that needed a call, and lays
out a phased remediation plan. Two findings were reframed by verification: the pricing feed
URL is the *intended* pre-cutover state (the new work is a staleness guard), and the permit
string slices are boundary-safe today (the real gap is missing lints plus a panic escape
hatch in the hook contract).

## Problem Statement

### Background

clyde v0.5.1 absorbed four formerly-separate tools (`cr`, `ccu`, `claude-permit`,
`claude-pricing`) under a behavior-exact compat contract
(`docs/design/2026-06-24-clyde-umbrella-cli.md`). The absorbed crates carry pre-merge code
that predates the workspace's hardening idioms (atomic writes, char-boundary lints, SQLite
pragma discipline), and the pricing feed-publishing move
(`docs/design/2026-06-29-move-pricing-feed-publishing-to-clyde.md`) is mid-cutover.

### Problem

Nine verified defects and inconsistencies, in severity order (referenced as F1-F9 by the
implementation phases below):

1. **Pricing staleness has no guard.** Fallback to the embedded baseline triggers only on
   fetch *failure* (`pricing/src/fetch.rs:83-93`). A reachable-but-stale feed (HTTP 200,
   schema-valid, older `data_version`) always wins over a newer embedded baseline; the
   embedded side discards `data_version` entirely (`Pricing::embedded()` hardcodes `None`,
   `pricing/src/feed.rs:62`), so the comparison is not even possible today. Additionally,
   both refresh crons (old `claude-pricing` repo and clyde) are live right now - the
   split-brain window the cutover design flagged is open.
2. **A panic on the permit `log` path escapes the hook contract.** `permit/src/lib.rs:66-81`
   converts `Err` to `{}` + exit 0 so a broken hook never blocks Claude Code, but a panic
   unwinds straight past it. No slice site is panic-prone *today* (all four flagged sites in
   `permit/src/risk/tier.rs` are boundary-guarded by `find`/`ends_with`/`starts_with`), but
   nothing enforces that: `permit/src/lib.rs` has no crate-root lint denies at all
   (no `clippy::unwrap_used`, no `clippy::string_slice`), unlike `session`/`sessions`/`clyde`.
3. **`apply`'s dry-run message names the wrong flag.** The standalone `apply` path prints
   "Pass --apply to write these changes." (`permit/src/cmd/apply.rs:81-84`) but its gate is
   `--yes`. (`audit --apply` writing immediately is *correct* per the CLI conventions: an
   explicit destructive flag is the opt-in gate; no second gate is added. Decision below.)
4. **Permit settings writes are unsafe.** `install.rs:53` and `apply.rs:146-147` use plain
   `fs::write` (truncate-in-place; a crash can destroy `settings.json`); `apply` writes
   `settings.local.json` even when it never existed and nothing touched it (materializing a
   spurious file); `get_allow_array` (`apply.rs:288-303`) and `apply.rs:97,99` `.expect()`
   and panic on malformed-but-parseable settings.
5. **events.db opens without `busy_timeout`.** `permit/src/db/store.rs:17-19` sets only
   `journal_mode=WAL`. The hook fires on every tool call and can race `suggest`/`report`/
   `clean`; a lost write is silent because log failures become `{}`. clyde's own bootstrap
   already applies a 5s timeout to this same DB (`clyde/src/bootstrap.rs:22-23`); the
   permit-side open is the outlier.
6. **Log directories are stranded on legacy names.** `cost` logs to `ccu/logs/`, `permit` to
   `claude-permit/logs/`, `report` to `claude-report/logs/`, while clyde logs to
   `clyde/logs/`. Data and config migrated under the clyde home; logs did not, and doctor
   knows nothing about them.
7. **Two risk classifiers disagree on `Read`.** `classify_rule` rates a bare `Read` rule
   Moderate (carte-blanche grant, commented); `classify_tool_input` rates a live `Read`
   event Safe (uncommented). The asymmetry is defensible - a persisted rule grants unbounded
   future access, one event is one read - but it is undocumented, and `suggest` builds rule
   proposals from event classifications.
8. **Report help text misdescribes its own machinery.** `--template` claims "Jinja2/Tera"
   (`report/src/cli.rs:116`) but `render_custom` is plain string `.replace` over exactly six
   placeholders; `--pdf-engine` help doesn't say pandoc is the actual binary invoked
   (wkhtmltopdf is pandoc's sub-engine); the REQUIRED TOOLS block hardcodes the log path.
9. **Cost cache key uses `DefaultHasher`.** SipHash keys are not stable across Rust
   releases, so a toolchain bump silently invalidates every cached day
   (`cost/src/cache.rs:5,28`). Self-healing but wasteful, and it contradicts the intent of a
   persisted, versioned cache (`CACHE_VERSION = 4`).

### Goals

- Close the stale-feed hole: a fetched feed older than the embedded baseline loses, loudly.
- Make the permit hook contract panic-proof, and enforce char-boundary/unwrap hygiene by
  lint across the crates that lack it.
- Make every settings.json mutation atomic, non-spurious, and panic-free.
- Bring events.db pragma discipline up to the `sessions/src/db.rs` standard.
- Unify all tool logs under `~/.local/share/clyde/logs/` (decision below), with doctor
  reporting them.
- Fix every misleading help string and the wrong-flag dry-run message.
- Make the cost cache key stable across toolchains.

### Non-Goals

- **Redesigning the pricing cutover.** Phases 4-6 of the 2026-06-29 feed-publishing doc
  (enable Pages on clyde, flip `DEFAULT_FEED_URL`, disable the legacy cron) execute as
  designed there; this doc only *adds* the staleness guard, which is independent of cutover
  ordering.
- **Retiring the compat shims.** `cr`/`ccu`/`claude-permit` remain; the one deliberate
  behavior change (log paths) is carved out of the behavior-exact surface explicitly.
- **Changing permit's risk model.** The Read rule-vs-event asymmetry is documented, not
  reworked; no tier assignments change.
- **Adding `--yes` to `audit --apply`.** Per the CLI conventions, an explicit destructive
  flag is already the opt-in; a second gate is anti-convention.
- **Consolidating all four existing atomic-write implementations** (bootstrap, report x2,
  pricing) onto the new shared helper in this doc. The helper lands in `common` and the two
  *unsafe* call sites move onto it; migrating the already-safe private copies is follow-up
  mechanical work.

## Decisions (settled 2026-07-03)

| # | Question | Decision |
|---|----------|----------|
| D1 | `audit --apply` gate | `--apply` alone is the gate (matches "no --dry-run on opt-in destructive flags"). Only fix the standalone `apply` message to name `--yes`. |
| D2 | Staleness guard semantics | **Prefer embedded when newer**: if fetched `data_version` < embedded `data_version`, use embedded and `warn!` loudly. |
| D3 | Log unification | **Unify everywhere** (shims included) under `clyde/logs/<tool>.log`, declare log paths outside the behavior-exact surface, update help strings, teach doctor. |
| D4 | Read divergence | **Document as intentional** at `classify_tool_input`; audit remains the backstop for suggest-promoted rules. |

## Proposed Solution

### Overview

Ten phases in three tracks. Track A (permit hardening, phases 1-5) closes the crash/loss
surfaces around the hook. Track B (polish, phases 6-8) is mechanical: cache key, help text,
classifier comment. Track C (phases 9-10) carries the two cross-cutting changes: the pricing
staleness guard and the log unification, each with its own compat note.

### Architecture

No new components. One new shared helper (`common::write_atomic`), one new guard inside
`pricing`'s feed-resolution chain, and lint attributes across five crate roots.

**Pricing staleness guard (D2).** Today: `auto` -> fresh cache | backoff | fetch ->
`fallback_chain` (cache -> user override -> embedded). New: `PricingFile` gains an optional
`data_version` field so the embedded baseline stops discarding its own timestamp, and the
staleness check lives **inside `fetch_and_cache`, before `write_cache_atomic`**
(`pricing/src/fetch.rs:195-217`), alongside the existing guard that already refuses to
overwrite a valid cache with an incompatible feed. Placement is load-bearing (panel
finding): a check at the higher composition points would run *after* the fetched bytes were
written to disk, poisoning the cache with a stale feed that the caller then discards in
memory. Semantics: if fetched `data_version` < embedded `data_version`, the fetch is treated
like an invalid feed - not cached, `warn!` naming both versions and the URL, and resolution
proceeds down the existing `fallback_chain` (existing cache -> user override -> embedded).
ISO-8601 strings compare lexicographically; a feed with *no* `data_version` or a malformed
(non-canonical-UTC) one is treated as stale rather than string-compared as garbage. The
user override keeps its existing position in the chain - an explicit local override is the
operator's call.

**Hook panic containment.** In `permit::run`, once `is_log` is known, wrap the **entire log
path** - `run_inner`'s logging setup, `Config::load`, `EventStore::open`, and the
`Command::Log` arm - in `std::panic::catch_unwind`, not just the match arm (panel finding:
`run_inner` runs setup before the dispatch, and the contract at `lib.rs:62-64` promises
`{}` for *any* failure). On unwind: print `{}`, log the panic payload, return `Ok(0)` - the
same degradation as `Err` today. No global panic-hook mutation (`panic::set_hook`
swap-and-restore races other threads); the default hook's stderr backtrace is acceptable
because Claude Code parses only stdout. `panic = "unwind"` is pinned explicitly in the
workspace profile so a future `panic="abort"` override cannot silently turn the boundary
into a no-op.

**Atomic settings writes.** `common` gains
`write_atomic(path: &Path, bytes: &[u8]) -> Result<()>`: temp file created *in the target's
own directory* (cross-fs rename would fail from /tmp), write, flush, rename over target.
Mirrors `clyde/src/bootstrap.rs:356`. When the target exists, its file mode is captured and
re-applied after the rename (the same exec-bit lesson bootstrap already learned at
`bootstrap.rs:880`); a read-only parent directory surfaces as a typed error, not a panic.
`install.rs` and `apply.rs` route through it.
`apply.rs` additionally tracks whether the local settings document was (a) loaded from an
existing file or (b) defaulted to `{}`, and whether any mutation touched it; it writes the
local file only if (a) or mutated.

**Log unification (D3).** Each absorbed tool's `log_file_path()` moves to
`<xdg-data>/clyde/logs/<tool>.log` (`cost.log`, `permit.log`, `report.log`; clyde keeps
`clyde.log`). No migration of old log *content* (logs are disposable diagnostics); legacy
log dirs are left in place and listed by doctor as informational legacy state, not failures.
Help strings that render the log path (`report/src/cli.rs:232`, cost after-help) render from
the path function, never a hardcoded string.

### Data Model

- `PricingFile` (`pricing/src/pricing.rs:42-49`): add
  `#[serde(default)] data_version: Option<String>`. Embedded `data/pricing.json` already
  carries the field; the struct starts reading it.
- `cost/src/cache.rs`: `CACHE_VERSION` 4 -> 5 (old entries miss cleanly on version, not
  probabilistically on hash). `compute_mtime_hash` switches to inline FNV-1a over the same
  `(path, mtime-secs, size)` tuple stream (~10 lines, two constants, no new dependency).
- No schema changes to events.db or sessions.db.

### API Design

- `common::write_atomic(path, bytes) -> eyre::Result<()>` - new, public.
- `permit`: `get_allow_array` returns `Result<&mut Vec<Value>>` (typed error on non-array
  `permissions.allow`); the two `.expect("valid path")` sites return errors.
- `pricing`: `embedded_data_version() -> Option<&'static str>` (internal); guard logic lives
  in `fetch_and_cache` before the cache write (see Architecture), not in `from_bytes`
  itself, so parsing stays pure and the on-disk cache can never hold a known-stale feed.
- CLI surfaces: **no flag changes anywhere.** Only help-string text and one dry-run message
  change.

### Implementation Plan

#### Phase 1: events.db pragmas + apply/install panic removal (F5, F4-panics, F3)
**Model:** sonnet
- `permit/src/db/store.rs::open`: add `busy_timeout=5000` (named const, mirroring
  `sessions/src/db.rs::BUSY_TIMEOUT_MS`) and `synchronous=NORMAL` after the WAL pragma.
- `get_allow_array` -> typed error; `apply.rs:97,99` `.expect("valid path")` -> errors.
- Fix the standalone `apply` dry-run message to name `--yes` (D1). `audit --apply` untouched.
- Tests: non-array `allow` returns an error (not a panic); message text pinned per path.

#### Phase 2: `common::write_atomic` + route permit writes through it (F4)
**Model:** sonnet
- Add the helper to `common` (temp-in-target-dir + rename); unit tests cover overwrite of
  an existing file, creation of a new one, and that the temp file lands in the target's own
  directory (the property that makes the rename atomic).
- `install.rs:53`, `apply.rs:146-147` use it. `apply.rs` skips writing an untouched,
  defaulted local file; test asserts `settings.local.json` is NOT created when absent and
  unmutated (tightens the existing `missing_local_file_handled` test).

#### Phase 3: hook panic containment (F2)
**Model:** opus
- `catch_unwind` around the entire log path in `permit::run` (setup + config + DB open +
  dispatch, per Architecture); unwind -> log + `{}` + `Ok(0)`. No panic-hook swapping;
  stderr backtrace tolerated (stdout-only contract).
- Pin `panic = "unwind"` in the workspace `[profile.release]`/`[profile.dev]` (explicitly,
  as insurance for the catch_unwind boundary).
- Test: an injected panic (test-only panicking store path) still yields stdout `{}` and
  exit 0; a panic in setup (before dispatch) is also covered.

#### Phase 4: lint hardening pass (F2)
**Model:** sonnet
- **Ordering constraint: lands after Phases 1-3** (all manual unwrap/expect removals must
  precede the denies or CI breaks on its own new lints).
- Add `#![deny(clippy::unwrap_used)]` where missing (`permit`, `common`) and
  `#![deny(clippy::string_slice)]` to `permit`, `cost`, `report`, `pricing`, `common`
  (matching `session`/`sessions`/`clyde`).
- Rewrite every now-flagged (safe-but-linted) slice site, not just `tier.rs`. Known sites
  from the panel's sweep: `permit/src/risk/tier.rs` (4 sites), `pricing/src/pricing.rs:116`,
  `cost/src/output.rs:192`, `cost/src/lib.rs:699`, `report/src/title.rs:170`; re-sweep at
  implementation time. Use `strip_prefix`/`strip_suffix`/`split_once`/`char_indices`
  equivalents; `#[allow]` in test modules as the existing crates do. Behavior identical;
  existing tests must pass unchanged.

#### Phase 5: cost cache stable hash (F9)
**Model:** sonnet
- Inline FNV-1a in `cache.rs`; `CACHE_VERSION = 5`. Existing property-based cache tests
  survive as-is (none pin hash values); add one test pinning a known tuple -> hash value so
  future accidental algorithm changes are caught.

#### Phase 6: report help-text truth (F8)
**Model:** sonnet
- `--template` help: plain `{{token}}` replacement; enumerate the six placeholders
  (`{{host}}`, `{{since}}`, `{{until}}`, `{{session-count}}`, `{{total-tokens}}`,
  `{{total-spend}}`).
- `--pdf-engine` help: "passed to pandoc as --pdf-engine; pandoc is the required binary."
- REQUIRED TOOLS block renders the log path from the path function (lands fully in Phase 8).

#### Phase 7: Read asymmetry comment (F7)
**Model:** sonnet
- Comment at `classify_tool_input` (`tier.rs:~321`) documenting the rule-vs-event
  distinction (D4) and pointing at `classify_rule`'s Moderate rationale; note that audit is
  the backstop for suggest-promoted bare-Read rules.

#### Phase 8: log unification + doctor awareness (F6)
**Model:** sonnet
- First extract a public `log_file_path()` in `permit` and `report` (today only `cost` has
  one; the other two bury the path inside private `setup_logging` - panel finding), then
  move all three to `clyde/logs/<tool>.log`; update every help string that names a log path
  to render from the function.
- doctor: report each tool's log location; list legacy log dirs (if present) in a **new
  informational field**, NOT `legacy_state` - that field feeds `healthy()` and would flip
  doctor red over disposable logs.
- Compat note in README + this doc: log paths are declared outside the behavior-exact shim
  surface as of the release that ships this.

#### Phase 9: pricing staleness guard (F1)
**Model:** opus
- `PricingFile.data_version`; parse the embedded baseline's version once
  (`OnceLock`, alongside the existing `embedded_data` cell). If the embedded baseline
  somehow lacks a parseable `data_version`, the guard is disabled (fail-open to today's
  behavior) rather than treating every fetched feed as stale.
- Guard inside `fetch_and_cache` before `write_cache_atomic`, alongside the existing
  incompatible-feed cache guard (per Architecture; placement is the panel's top finding).
  Stale feed -> not cached, `warn!` with both versions + URL, resolution falls through the
  existing `fallback_chain`. Malformed or missing `data_version` -> treated as stale.
- Document the full source-selection state machine (cache-hit / expired / fetch-newer /
  fetch-stale / fetch-fail / override / embedded, and the single cache-write point) in the
  `fetch.rs` module doc - it now has enough states that prose in three functions no longer
  carries it.
- Tests (mockito, alongside the existing fetch tests): stale-200 feed -> not written to
  cache, embedded (or existing cache) wins + warning; equal/newer feed -> fetched wins and
  is cached; version-less feed -> stale; malformed version -> stale; user override keeps its
  chain position. Regression: the existing cache-poisoning tests still hold.

#### Phase 10: pricing cutover follow-through (F1, tracking only)
**Model:** n/a (Scott-manual + existing design doc)
- Execute Phases 4-6 of `2026-06-29-move-pricing-feed-publishing-to-clyde.md`: enable Pages
  on `tatari-tv/clyde` (manual), flip `DEFAULT_FEED_URL`, disable the legacy
  `claude-pricing` cron. **Note: both refresh crons are active today** - two daily
  competing refresh PRs until Phase 6 lands. This doc changes nothing about that sequencing;
  it is listed so the finding has a tracked home.

## Alternatives Considered

### Alternative 1: `--yes` gate on `audit --apply`
- **Description:** Make `audit --apply` a preview and require `--apply --yes` to write.
- **Pros:** Symmetric with the standalone `apply` subcommand.
- **Cons:** Violates the house CLI rule that an explicit destructive flag *is* the opt-in;
  stacks two gates on one action; observable shim behavior change with no safety win (the
  write is already rkvr-backed-up).
- **Why not chosen:** D1. The asymmetry is principled: standalone `apply`'s *default* would
  be destructive (hence `--yes`); `audit`'s default is read-only and `--apply` is the gate.

### Alternative 2: warn-only staleness handling
- **Description:** Keep the fetched feed authoritative; log a warning when older than
  embedded.
- **Pros:** Preserves "published feed is authoritative"; zero pricing-behavior change.
- **Cons:** The original finding stays open - during a publish lag every consumer silently
  prices with stale data despite shipping a newer baseline; warnings in a statusline
  context are read by no one.
- **Why not chosen:** D2. The binary's embedded baseline is itself published, reviewed data;
  preferring the newer of two published datasets is strictly better.

### Alternative 3: leave log paths until shim retirement
- **Description:** Defer unification entirely.
- **Pros:** Zero risk; contract untouched.
- **Cons:** Three stranded log dirs accumulate indefinitely; doctor stays blind to them; the
  clyde home remains incomplete for the one artifact class users actually go looking for
  when something breaks.
- **Why not chosen:** D3. Logs are disposable diagnostics, not behavior; carving them out of
  the compat surface is a one-line note.

### Alternative 4: shared Read classifier with a rule-vs-event context enum
- **Description:** One classifier, explicit `RuleOrEvent` parameter.
- **Pros:** Typed consistency; suggest could label proposals with their eventual rule tier.
- **Cons:** Churn in the most behavior-sensitive module of permit for zero tier changes;
  the two classifiers legitimately answer different questions.
- **Why not chosen:** D4. A comment carries the intent; audit already catches the one real
  interaction (suggest promoting a bare-Read rule).

### Alternative 5: `fnv` crate instead of inline FNV-1a
- **Description:** `cargo add fnv` (already in Cargo.lock transitively).
- **Pros:** No hand-rolled hashing.
- **Cons:** A public dependency for ~10 lines of arithmetic with two published constants;
  the house rule prefers no dependency over a trivial one.
- **Why not chosen:** Inline is smaller than the Cargo.toml diff and testable against known
  vectors.

## Technical Considerations

### Dependencies
- No new external dependencies. `common` promotes `tempfile` from dev-dependency to
  dependency (it is already a workspace-level dep; note it is dev-only in both `common` and
  `permit` today); `write_atomic` uses `std::fs` + `NamedTempFile::persist`.

### Performance
- FNV-1a is faster than SipHash for these tiny inputs; cache behavior otherwise identical.
- `busy_timeout=5000` can add up to 5s latency to a hook write under contention - identical
  to the tradeoff `sessions.db` already accepts, and strictly better than a silently dropped
  event.
- The staleness guard adds one lazy parse of the embedded JSON (already parsed for other
  paths) and one string compare per pricing construction.

### Security
- Atomic settings writes close a corruption window on the file that gates every tool
  permission Claude Code enforces.
- `catch_unwind` on the hook path means a crafted tool input that finds a future panic can
  no longer wedge the hook into a Claude-blocking state; it degrades to observe-nothing,
  which is the contract's chosen failure direction.

### Testing Strategy
- Every phase carries its tests inline (listed per phase). CI is `otto ci` across the
  workspace (clippy -D warnings, fmt, test); the Phase 4 lint denies make regressions
  compile-time failures.
- The two behavior-observable changes (log paths, spurious-local-file suppression) get
  explicit tests pinning the *new* behavior.

### Rollout Plan
- Single release line (flat workspace version). Phases 1-9 land as normal PRs; this repo is
  gated (PR -> admin-merge -> tag on main per the established release flow).
- Compat notes for the two observable changes ride the release notes: log paths (D3) and
  the untouched-local-file suppression (bug-fix-shaped).
- Phase 10 follows the 2026-06-29 doc's own sequencing and is not release-coupled here.
- Phase 8 landed: log paths are declared outside the behavior-exact shim surface as of the
  release that ships it (see README's "Compat shims" section for the user-facing note).

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Staleness guard inverts authority during a legitimate feed rollback (embedded newer than an intentionally reverted feed) | Low | Med | User override is exempt and documented as the escape hatch; warning names both versions so the state is visible. |
| `catch_unwind` masks a real bug class on the log path | Med | Low | The unwind is logged with payload before `{}` is printed; observe-only degradation is the contract's explicit choice. |
| Lint pass forces rewrites that subtly change tier semantics | Low | High | Phase 4 requires the existing tier test suite to pass unchanged; rewrites are mechanical `strip_prefix`/`split_once` equivalents. |
| Log-path change breaks a user script tailing an old path | Low | Low | Compat note; legacy dirs listed by doctor; old logs left in place. |
| Both refresh crons open competing PRs until Phase 10 | High (already occurring) | Low | Known, time-boxed by the cutover doc's own sequencing; PRs are visible and mergeable-or-closeable. |
| FNV-1a collision on `(path, mtime, size)` yields a false cache hit | Very Low | Low | 64-bit FNV over short structured input; version bump flushes all priors; worst case is one stale day-cost display fixed by `--no-cache`. |

## Open Questions
- [ ] Phase 9 stale-feed observability: both panel reviewers push to surface the state in
  `clyde cost pricing` output (and possibly the statusline), not just `warn!`, and to
  decide a debounce so a legitimately-lagging feed doesn't warn on every statusline tick.
  Lean yes on output surfacing; decide debounce at implementation.
- [ ] Legacy log-dir lifecycle after Phase 8: doctor reports them informationally, but
  nothing ever removes them. Fold into an eventual `clyde clean`, or leave forever?
- [ ] When the four pre-existing private atomic-write copies migrate onto
  `common::write_atomic` (follow-up mechanical pass, not this doc).

## References
- Deep-dive report: `https://marquee.internal.tatari.dev/p/~scott-idler/clyde-deep-dive`
- `docs/design/2026-06-24-clyde-umbrella-cli.md` (behavior-exact compat contract)
- `docs/design/2026-06-29-move-pricing-feed-publishing-to-clyde.md` (cutover Phases 4-6)
- `sessions/src/db.rs` (pragma prior art), `clyde/src/bootstrap.rs` (write_atomic prior art)
