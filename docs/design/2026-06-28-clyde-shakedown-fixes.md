# Design Document: clyde v0.3.0 Shakedown Fixes

**Author:** Scott Idler
**Date:** 2026-06-28
**Status:** Approved
**Review Passes Completed:** 5/5 + cross-model panel (Architect/Gemini, Staff Engineer/Codex); all MUST-FIX findings incorporated; strategic decision (clyde is sole UX, shims retired) resolved with the user.

## Summary

A full shakedown of clyde v0.3.0 surfaced 13 issues — no crashes, but
inconsistencies, missing features, doc gaps, and an unfinished umbrella
migration. This doc sequences fixes into 9 independently-shippable phases
grouped by theme (migration-finish, `--since`/error consistency, missing
features, output/docs polish), front-loading zero-risk quick wins and deferring
the invasive migration-code change to the end — plus a gated Phase 10 (retire the
`cr`/`ccu`/`claude-permit` shims; breaking, likely its own release). Per the
resolved strategic decision, **clyde is the sole UX** and all fixes target it.

## Problem Statement

### Background

clyde is an umbrella CLI that absorbed four previously-standalone tools — the
`sessions` catalog (native), plus `report` (was `cr`), `cost` (was `ccu`), and
`permit` (was `claude-permit`). The merge prioritized wiring the tools under one
binary; the shakedown (`docs/shakedown-v0.3.0.md`) exercised every subcommand
and found the seams between the absorbed tools still show.

### Problem

13 issues across four themes. Common thread: the absorbed tools each kept their
own conventions (error formatting, `--since` parsing, output format, self-naming)
and the migration that consolidates them isn't finished or fully observable.

### Goals

- Resolve all 13 shakedown issues, or consciously defer with a documented reason.
- Unify cross-tool divergences (the `--since` parser, leaked-location errors,
  stale binary names) rather than patch them per-tool.
- **`clyde` becomes the sole UX; retire the `cr`/`ccu`/`claude-permit` shims**
  (binary removal gated on migration-complete — Phase 10).
- Keep every phase independently committable and CI-green.

### Non-Goals

- No new top-level features beyond completing what's stubbed (`report merge`) or
  promised (MCP `--sort` parity).
- Not converging `report render`'s artifact output (md/**pdf**) onto stdout — a
  PDF genuinely needs a file. Issue 10's unification covers `cost` autodetect and
  `report collect` stdout-streaming only; `render` keeps `-o`.
- No version bump or release in this doc's scope — that follows on merge.

## Proposed Solution

### Overview

Nine phases, ordered by risk (lowest first) and dependency. Themes A-D from the
shakedown map onto phases; the migration-code change (bootstrap dry-run) is late
so the surface is stable when it lands. **Phases 2 and 3 should land together**
(Phase 2 perturbs the very error path Phase 3 cleans up — splitting them risks an
intermediate commit with broken error rendering).

### Architecture / cross-crate constraints (load-bearing)

These constraints, verified during research, shape several fixes:

- **`report` must NOT depend on `sessions`.** `sessions` pulls
  rusqlite/rmcp/tokio; making the report tool depend on it is wrong. Shared
  `--since` parsing therefore lives in **`common/`** (today only holds
  `Globals`). Both `sessions::since` and `report::config` call `common::parse_since`.
- **`Globals` (`common/src/lib.rs:18`) is the existing seam** for passing context
  down into each absorbed tool's `run(args, globals)`. The invoked-program-name
  fix rides this seam.
- **`sessions` is intentionally clap-free** (`model.rs:53`). The MCP sort param
  must be a plain serde/schemars enum, not the clap `SortOrder`.
- **`cost` has no `--since`** (it uses `-d/--days <u32>`), so the `--since`
  unification is sessions-vs-report only.

### Issue-by-issue resolution

Each issue with its verified surface and fix. Severity: 🔴 correctness/UX,
🟡 consistency/feature-gap, 🟢 polish.

**Theme A — finish the umbrella migration**

- **#1 🟡 `bootstrap --dry-run`** — **CORRECTED per design review: the gate must
  wrap `run()`, not just `bootstrap()`.** Two mutation sites escape a gate
  threaded only into `bootstrap()`:
  - `daemon_reload()` (`bootstrap.rs:121`) and `start_enrich_timer()` (`:125`) are
    called in the **outer `run()`** (`:113`), *outside* `bootstrap()` (`:159`).
  - `migrate_events_db` runs `PRAGMA wal_checkpoint(TRUNCATE)` (`:401`) — a
    **write to the user's events DB** — *before* the gated `fs::rename`; dry-run
    must not open that DB in a writing mode.

  **Full mutation inventory to gate** (verified): `migrate_dir` (`:285`),
  `migrate_events_db` incl. the WAL checkpoint (`:384,401`), `migrate_permit_config`/
  `migrate_file` (`:239,340`), `merge_pricing_overrides` (`:441`),
  `repoint_statusline` (`:483`), `repoint_hook`×2 (`:518`), `repoint_systemd`
  (`:570`) incl. `move_env_file` (`:664`), `repoint_wants_symlink` symlink +
  `create_dir_all` (`:636`), `install_clyde_timer` (`:685`), and the two
  `systemctl` shell-outs (`:121,125`). **Approach:** thread `dry_run` through
  `run()` *and* `bootstrap()`; each `migrate_*`/`repoint_*` (already returns
  `Result<bool>`) returns a *planned action* without performing it under
  `dry_run`; a dedicated test asserts **zero** filesystem/DB/systemctl writes in
  dry-run. Justified despite the "no `--dry-run` on opt-in destructive flags" rule
  because bootstrap is **default-destructive** (no opt-in gate) — the carve-out.
- **#2 🟢 `doctor` legacy-state framing** — `doctor.rs:267`/`:94-118`. Research
  finding: **doctor's logic is correct.** It reports legacy state because
  bootstrap was never actually run on this host (shakedown ran it discovery-only).
  No doctor code change. The real defect is the `permit check` disagreement,
  folded into #3. Resolution: a sequencing note (run `bootstrap`, then `doctor`)
  + the #3 fix.
- **#3 🟡 stale binary names in output** — `report/src/lib.rs:48` (`cr: merge…`),
  `:56` (`cr collect: jq…`), `cli.rs:161-164` (`cr render`/`cr collect` in help
  table); `permit/src/cmd/check.rs:50,93,98,104,113` and `install.rs:32,40,53`
  (`claude-permit`). **Fix (two cheap moves, no `Globals` refactor):**
  (a) *Drop the tool-name prefix* from the error/help strings — `cr: merge is not
  implemented` → `merge is not implemented`; the tool-name table in `cli.rs:161`
  becomes invocation-neutral. This avoids threading an invoked-program name
  through every tool's `run()` (the heavier alternative — see Alternatives).
  (b) *Fix `permit check`'s detection logic* (a real fix, not cosmetic): accept
  **either** `claude-permit log` or `clyde permit log` as the installed hook
  (mirror `install.rs:78`) and report the clyde form — this also reconciles #2's
  doctor/permit-check disagreement. Leave `report/src/cli.rs:23 name="cr"`
  (correct: it IS the shim binary's name) and `cost/src/lib.rs:48-59` `ccu=`
  log-filter strings (internal, not user-facing).

**Theme B — `--since` / error consistency**

- **#4 🔴 `--since` parser divergence** — `sessions/src/since.rs:11` `parse_since`
  accepts relative spans (`7d/24h/2w`), RFC 3339, and `YYYY-MM-DD`;
  `report/src/config.rs:155` `parse_datetime` accepts only the latter two (hence
  `--since 2d` fails). They also differ on bare-date midnight: sessions uses UTC,
  report uses local. Fix: move the canonical parser to `common::parse_since`,
  taking a timezone mode. **Decision:** the bare-date midnight convention is
  **configurable** via `~/.config/clyde/clyde.yml` (e.g. `date-tz: utc | local`),
  **defaulting to UTC** when unset. The CLI layer reads the config and passes the
  mode into `common::parse_since`; the parser itself stays a pure function. (If no
  clyde config file exists yet, this introduces a minimal one per the
  XDG-config convention.)
- **#5 🔴 internal source location leaks** — `report/src/lib.rs:38` `run()`
  bubbles the `bail!` (config.rs:168) to `{e:?}` printers at
  `report/src/bin/cr.rs:13` and `clyde/src/main.rs:95`. The `{:?}` Debug format on
  an eyre `Report` renders eyre's `Location:` capture. **CORRECTED per review:**
  (i) use `{e:#}` (the full **cause chain**, no location/backtrace) as the default,
  NOT `{e}` — plain `{e}` Display hides the causal chain and would *degrade*
  normal-failure UX; reserve `{e:?}` for `--log-level debug`. (ii) `dispatch_tool`
  (`clyde/src/main.rs:93`) currently takes only `Result<i32>` and has **no access
  to the log level** — its signature must change to carry a `debug: bool` (or the
  resolved level) so it can pick the format. (iii) Scope: **clyde-only** — fix
  `dispatch_tool` (`main.rs:95`), the shared path for `clyde report/cost/permit`.
  The shim `main`s (`ccu.rs:24`, `cr.rs:13`, `claude-permit` bin) are being
  retired (Phase 10), so they get no error-rendering investment — they're deleted,
  not fixed.

**Theme C — missing features**

- **#6 🟡 `report merge` unimplemented** — stub at `report/src/lib.rs:47-50,104-106`;
  `MergeConfig { inputs }` (config.rs:42), `MergeArgs` (cli.rs:98). Implement:
  read each input JSON (the `collect` schema `{generated,host,schema-version,
  sessions,since,totals,until}` from `report::write_json`), assert matching
  `schema-version`, union `sessions`, recompute `totals`, widen `since`/`until`
  to min/max, set `host` to a multi-host marker. **CORRECTED per review — two
  schema decisions, not edge cases:** (i) sessions are a
  `BTreeMap<String, SessionEntry>` (`report.rs:24`), so two hosts sharing a
  session id **collide** (one silently overwrites). "Keep-both" requires
  **re-keying** to `host/session_id` or adding a per-entry `host` field — a schema
  change. (ii) `totals` must be **recomputed by re-summing the merged session
  set**, NOT blind-summing each input's `totals` (which double-counts any overlap).
  Remaining edge cases: **0 inputs** → error; **1 input** → identity;
  **schema-version mismatch** → typed error naming both versions.
- **#7 🟡 MCP `sessions_search` `--sort` parity** — `sessions/src/mcp/tools.rs:23`
  `SessionsSearchRequest` (no sort); call site `mcp.rs:186` passes
  `SortBy::Relevance`. Add `sort: Option<String>` (schemars-described) to the
  request, parse to `SortBy` at the call site (default `Relevance`). Plain
  serde/schemars — do NOT reuse the clap `SortOrder` (sessions is clap-free).
- **#8 🟡 `sessions tag` can't clear; provenance** — `clyde/src/cli.rs:158`
  `TagArgs.tags` is `required=true, num_args=1..`; `sessions/src/db.rs:245`
  `set_tags` hardcodes `tags_source='manual'` (`:260`). Fix: make tags optional
  (`num_args = 0..`, drop `required`) so `tag <id>` with no tags clears them;
  `cmd_tag` (main.rs:225) calls `set_tags(&id, &[])`. **Decision:** clearing
  **resets `tags_source` to NULL** (cleared = no manual provenance), so a later
  `enrich` can re-tag the session. `set_tags` (db.rs:259-260) currently
  **hardcodes** `tags_source = 'manual'` in the SQL — make it conditional (write
  `NULL` when the tag slice is empty). Add a **clap parse test** that `tag <ID>`
  (zero tags) parses without the `<ID>` positional and the `num_args=0..` tags
  list becoming ambiguous (load-bearing — review flagged it as needing a test).

**Theme D — output-format + docs polish**

- **#9 🟢 `report` flags undocumented** — `report/src/cli.rs` `CollectArgs`/
  `RenderArgs`/`MergeArgs` and the `Command` enum have no doc-comments. Add `///`
  to every field and the three variants. Pure docs.
- **#10 🟡 three output-format conventions** — sessions auto-detects TTY
  (`main.rs:326,346` `IsTerminal`), cost prints human unless `-j/--json`, report
  writes only to `-o <file>`. **Decision: unify on the sessions model.**
  (a) **`cost`**: adopt `IsTerminal` autodetect — human on a TTY, JSON when piped;
  keep `-j` as an explicit override. So `cost today | jq` works without `-j`.
  Touches each `cost` subcommand's print path (`cost/src/lib.rs`) and the now-
  redundant-but-retained `-j` flag. (b) **`report collect`**: stream JSON to
  stdout when `-o` is omitted. **CORRECTED per review — this is not a one-liner:**
  (i) `report::run` does `println!("wrote N sessions to PATH")` (`lib.rs:63`) on
  **stdout** — that must move to **stderr** or it corrupts the JSON stream;
  (ii) the paid Haiku title cache seeds off `cfg.output.exists()` /
  `latest_prior_report(&cfg.output)` (`lib.rs:128-131`), so with no path the
  title carry-forward stops and **every run re-bills the Anthropic API** — the
  streaming path must still resolve a title-cache *source* directory (e.g. the
  default report dir) even when output goes to stdout. Model `CollectConfig.output`
  as `enum Output { File(PathBuf), Stdout }` and thread it through. `report
  render`→pdf keeps `-o` (a PDF needs a file — see Non-Goals).
- **#11 🟢 `tags_source` not in JSON** — `sessions/src/model.rs:11` `SessionRecord`
  has no `tags_source` field; `db.rs:72 COLS` doesn't SELECT it; `map_record`
  (db.rs:737) doesn't populate it. Add `tags_source: Option<String>` to the
  struct, `s.tags_source` to `COLS`, populate in `map_record` (mind the column
  index ordering). Serializes automatically as kebab-case `tags-source`.
- **#12 🟢 `serve` doesn't exit on stdin EOF** — `sessions/src/mcp.rs:316`
  `service.waiting().await` over `tokio::io::stdin()` (rmcp pinned 1.7.0; runtime
  showed 1.8.0 — version skew). Needs an rmcp-docs check before fixing
  (select-on-EOF vs upgrade). Low impact (hosts kill the child). Open Question.
- **#13 🟢 `cost session current` resolution** — `cost/src/lib.rs:633` resolves
  "current" to `max_by_key(last_active)` over a 30-day scan — most-recently-*active
  by content*, not the live session (the `6e427ce3` vs `049209b7` mismatch).
  Needs the live-session signal (env var? mtime?) before fixing. Open Question.
  (Note: `&id[..8.min(len)]` byte-slices at lib.rs:643 are safe — IDs are ASCII hex.)

### Implementation Plan

Phases are ordered lowest-risk-first and by dependency. Each is one commit, CI-green.

#### Phase 1: Docs + stale-name fixes
**Model:** sonnet
- #9: add `///` doc-comments to all `report` cli args + subcommand variants.
- #3(a): drop the `cr:`/tool-name prefixes from user-facing strings
  (`report/src/lib.rs:48,56`, `cli.rs:161-164`); make them invocation-neutral.
- #3(b): fix `permit check` to accept `clyde permit log` OR `claude-permit log`
  and report the clyde form (`check.rs`); align `install.rs` messages.
- Near-zero logic risk (only #3b touches a code path); independently shippable.

#### Phase 2: Shared `--since` parser in `common`
**Model:** opus
- #4: move `parse_since` to `common`, reconcile UTC-vs-local midnight (decision
  below), wire `sessions::since` and `report::config` to call it. Add tests for
  span + RFC3339 + bare-date in `common`.

#### Phase 3: Clean error rendering boundary
**Model:** opus
- #5: change `cr.rs:13` and `clyde/src/main.rs:95` to print `{e}` for user errors,
  `{e:#}`/`{e:?}` only under `--log-level debug`. Pairs with Phase 2 (the `--since`
  parse error is the trigger). Verify both the shim and `clyde report` paths.

#### Phase 4: `tags_source` exposure + tag clearing
**Model:** sonnet
- #11: add `tags_source` to `SessionRecord`, `COLS`, `map_record`.
- #8: make `TagArgs.tags` optional (`num_args = 0..`), `cmd_tag` clears on empty;
  decide `tags_source` reset semantics. Both touch the same db/model area.

#### Phase 5: MCP `sessions_search` sort param
**Model:** sonnet
- #7: add `sort: Option<String>` to `SessionsSearchRequest` (schemars-described),
  parse to `SortBy` at the `mcp.rs:186` call site, default `Relevance`.

#### Phase 6: Output-format autodetect + report output abstraction
**Model:** opus
- #10(a): `cost` adopts `IsTerminal` autodetect (human on TTY, JSON when piped),
  `-j` retained as override. Touches each `cost` subcommand print path.
- #10(b): introduce `enum Output { File(PathBuf), Stdout }` for `report collect`;
  move the `wrote N sessions` message to **stderr**; keep a title-cache *source*
  dir even when streaming (so the paid title cache still carries forward).
- Opus (not sonnet): this changes report's output **contract**, and #6 (merge)
  reuses the abstraction — lands before merge so merge inherits stdout/`-o`.

#### Phase 7: Implement `report merge`
**Model:** opus
- #6: merge multiple collect JSONs over the documented schema (union sessions,
  recompute totals, widen window, multi-host marker; edge cases above). Honors the
  Phase 6 stdout/`-o` convention. Net-new logic + tests.

#### Phase 8: `bootstrap --dry-run`
**Model:** opus
- #1: thread `dry_run` through `bootstrap()`, gate every mutation + the two
  `systemctl` shell-outs, print the planned action list. Late because it's the
  most invasive migration-code change.

#### Phase 9: Investigate-then-fix (gated on evidence)
**Model:** opus
- #12 (serve EOF — confirm rmcp 1.7/1.8 stdio behavior via Context7 first) and
  #13 (`cost session current` — confirm the live-session signal first). Each is a
  fix only if evidence supports one; otherwise a documented limitation. #2 is
  resolved by Phase 1's `permit check` update plus a sequencing note.

#### Phase 10: Retire the shims (breaking — likely its own release)
**Model:** opus
- Remove the `cr`/`ccu`/`claude-permit` shim `main`s and their `[[bin]]` entries
  once `clyde` is the sole UX and the migration has repointed all hooks/statusline/
  systemd units away from the legacy binaries. **Gated:** `clyde doctor` must
  report clean (no legacy targets) — i.e. `bootstrap` has run everywhere — before
  removal, or live hooks break. Because it's a breaking change with a deployment
  dependency, this likely ships as its **own release** after Phases 1-9 land, not
  bundled with them. Until then the shims keep working unchanged (they're not
  *fixed*, just not yet *deleted*).

## Alternatives Considered

### Alternative 1: Patch each tool's `--since` in place (no `common`)
- **Description:** Add relative-span parsing to `report/src/config.rs` directly.
- **Pros:** No cross-crate change; smaller diff.
- **Cons:** Two parsers drift again; the UTC/local divergence persists.
- **Why not chosen:** The whole theme is *unifying* divergences; a shared
  `common::parse_since` is the durable fix and `common` already exists as the seam.

### Alternative 2: Document-and-defer the output-format unification (Issue 10)
- **Description:** Just write down the intended convention; change no code now.
- **Pros:** Smallest scope; no risk of regressing cost's output.
- **Cons:** Leaves `cost today | jq` broken (emits text, not JSON, without `-j`)
  and `report collect` file-only — the inconsistency persists.
- **Why not chosen:** Decided to unify (Phase 6): `cost` autodetect + `report
  collect` stdout. `report render`→pdf stays file-based (Non-Goals) since a PDF
  needs a file — that's the one piece deliberately left un-converged.

### Alternative 3: Thread invoked-program name through `Globals` (Issue 3)
- **Description:** Add `prog: String` to `Globals` (`common/src/lib.rs:18`), set by
  the clyde dispatcher (`clyde report`) vs the shim (`cr`), and interpolate it
  into messages so they read `clyde report merge: …` vs `cr merge: …`.
- **Pros:** Messages name the actual invocation; future-proof for more shims.
- **Cons:** Touches every absorbed tool's `run(args, globals)` call chain for a
  cosmetic gain; larger blast radius than the problem warrants.
- **Why not chosen:** #3(a) drops the prefix entirely — simpler and sufficient.
  Keep this in the back pocket if invocation-aware messaging is ever needed.

### Alternative 4: One big "definalize the migration" PR
- **Description:** Land all 13 in a single change.
- **Pros:** One review.
- **Cons:** Couples zero-risk doc fixes to invasive bootstrap-mutation gating;
  a single CI failure blocks everything; hard to bisect.
- **Why not chosen:** Phased, independently-green commits per the execution
  conventions.

## Technical Considerations

### Dependencies
- **`common/Cargo.toml` has NO `[dependencies]` today** (verified — the "chrono
  transitively available" assumption was wrong). Phase 2 adds `chrono` + `eyre` to
  `common` to host `parse_since`. (`report → common` already exists;
  `report → sessions` must NOT be added — constraint verified.)
- **#4 adds the first top-level config**: `clyde.yml`. No crate reads it today and
  `clyde/Cargo.toml` has no `serde_yaml`. Phase 2 introduces a `clyde.yml` loader
  (`serde_yaml`, `#[serde(deny_unknown_fields)]`) and injects the `date-tz` value
  via `Globals` (which today holds only `log_level`). See the new "Config seam"
  note and Open Questions.
- `report merge` uses existing `serde_json`.

### Performance
- Negligible. `report merge` is O(total sessions across inputs); bootstrap
  dry-run does strictly less work than a real run.

### Security
- `bootstrap --dry-run` strictly reduces side effects. The error-rendering change
  (#5) must not *hide* genuine internal errors — they still print at
  `--log-level debug`. No new injection surface.

### Testing Strategy
- `common::parse_since`: unit tests for span/RFC3339/bare-date + the chosen
  midnight convention.
- `report merge`: fixture JSONs (2 hosts) → assert unioned totals/window.
- MCP sort: a serve-level test that `sessions_search` honors `sort: "recency"`.
- `tags_source`/clear: db tests (set, read back source; clear → empty + source reset).
- bootstrap dry-run: assert no filesystem mutation occurs and the plan lists each step.

### Rollout Plan
- Per-phase commits on `shakedown-fixes`; PR → admin-merge (gated) → tag/bump as a
  follow-up release (likely v0.4.0 given new features: merge, MCP sort, tag clear).

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| `common::parse_since` changes report's bare-date semantics (UTC vs local) | Med | Med | Decide convention explicitly (Open Q); document; test both |
| #5 error change hides real internal failures | Low | Med | `{e:#}` under `--log-level debug`; keep context chains |
| bootstrap dry-run misses a mutation site (false "safe") | Med | High | Centralize: gate at each `migrate_*`/`repoint_*` return + the systemctl calls; test asserts zero fs writes |
| MCP sort param breaks existing relevance-only consumers | Low | Low | `sort` optional, defaults to `Relevance` — unchanged when omitted |
| Column-index drift when adding `tags_source` to `COLS`/`map_record` | Med | Med | Add at the end of `COLS`; index the new column explicitly; db test |

## Open Questions

**Strategic — RESOLVED (2026-06-28): clyde is the sole UX; the shims
(`cr`/`ccu`/`claude-permit`) are to be RETIRED. "There can be only one."**
This simplifies every cross-cutting fix to a single entry point: #4 config,
#5 error rendering, and #10 autodetect target `clyde` only — no shim sync. The
shim `main`s (`report/src/bin/cr.rs`, `cost/src/bin/ccu.rs`, the `claude-permit`
bin) are slated for removal rather than maintenance. **Caveat (sequencing):**
removing the shim *binaries* is a **breaking change** gated on the migration
being complete on every host — a hook/statusline/systemd unit still pointing at
`claude-permit`/`ccu` breaks the instant its binary disappears. So the *fixes* in
this doc go clyde-only now; the *binary removal* is the final, separately-gated
step (see Phase 10) and likely its own release once `bootstrap` has run everywhere.

**Resolved (2026-06-28):**
- [x] **#4 date semantics:** configurable via `~/.config/clyde/clyde.yml`
      (`date-tz: utc | local`), **defaulting to UTC** when unset.
- [x] **#8 provenance:** clearing tags **resets `tags_source` to NULL**.
- [x] **#10 scope:** unify — `cost` autodetect + `report collect` stdout;
      `report render`→pdf stays file-based.

**Still open (decide during implementation):**
- [ ] **#5 rendering:** confirm `{e}` top-level + `{e:#}` under `--log-level
      debug` is the desired strategy (recommended).
- [ ] **#12:** does rmcp 1.7/1.8 stdio resolve `waiting()` on stdin EOF? (Context7
      check.) Note: rmcp is pinned `1.7.0` in **both `Cargo.toml` and `Cargo.lock`**
      (verified) — the shakedown's "runtime 1.8.0" observation must be reconciled
      (likely a misread) before any fix. *Evidence-gated — Phase 9.*
- [ ] **#13:** does Claude Code export a live session id (env var / marker) that
      `cost session current` can bind to, or is most-recent-mtime the best signal?
      *Evidence-gated — Phase 9.*

## Design Review Disposition (2026-06-28)

Cross-model panel (Architect/Gemini, Staff Engineer/Codex), Mode 1. All findings
re-verified against code by the main agent before incorporation.

| Finding | Severity | Disposition |
|---------|----------|-------------|
| #1 bootstrap gate leaky — `daemon_reload`/`start_enrich_timer` in outer `run()`, WAL-checkpoint write at :401; doc inventory wrong | MUST-FIX | Fixed — corrected inventory; gate wraps `run()`; zero-write dry-run test |
| `common` has no deps ("chrono transitively available" false) | MUST-FIX | Fixed — Phase 2 adds chrono+eyre to `common` |
| `report collect`→stdout breaks title cache + `println!` corrupts JSON | MUST-FIX | Fixed — `enum Output`, message→stderr, keep title-cache source; Phase 6 now opus |
| #5 error boundary: `dispatch_tool` has no log-level; `{e}` hides chain | MUST-FIX | Fixed — signature carries debug; use `{e:#}`; umbrella-wide scope |
| `report merge` keep-both impossible (BTreeMap collision); totals math | MUST-FIX | Fixed — re-key `host/session_id`; recompute totals from merged set |
| #4 first `clyde.yml` — new config-deser + injection seam | CHEAP-WIN | Fixed — serde_yaml + `deny_unknown_fields`, injected via `Globals` |
| #8 `set_tags` hardcodes `'manual'`; clap parse test | CHEAP-WIN | Fixed — conditional NULL; add parse test |
| Shims first-class vs clyde-authoritative (drives #4/#5/#10) | STRATEGIC | RESOLVED — clyde is sole UX; shims retired (Phase 10). Fixes go clyde-only |
| "8 phases" prose; Phase 3-with-2; rmcp pin 1.7.0 vs "1.8.0" | NIT | Fixed — prose→9, coupling noted, pin reconciled |
| #12/#13 deferral to Phase 9 | — | Endorsed by both reviewers, unchanged |
| report-not-dep-on-sessions; `--since` divergence; cost-no-autodetect | — | Verified correct, no change |

## References
- `docs/shakedown-v0.3.0.md` — the full shakedown and 13-issue list
- `docs/design/2026-06-28-sessions-search-sort.md` — the v0.3.0 `--sort` feature
  (origin of the MCP parity gap #7)
- Research brief: design-research agent, 2026-06-28 (verified path:line surfaces)
