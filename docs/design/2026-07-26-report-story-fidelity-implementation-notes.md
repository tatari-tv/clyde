## Phase 0: Spike the real outcome vocabulary and size the attribution recovery

### Design decisions
- Measured the rule-3 ceiling against the EXACT tool-call shape `efficiency::outcome::union` already
  extracts (`Edit`/`Write` only, `input.file_path`, confirmed by a non-error `tool_result`) rather
  than a looser scan of every edit-shaped tool -- `efficiency/src/outcome.rs:318-329` (`classify_tool`)
  is the ground truth for what Phase 3 will actually build `repos_touched` from, so the ceiling had
  to be measured against that exact filter, not an approximation.
- Reproduced the doc's own 562 / 283 / 279 session-and-dollar figures live before measuring anything
  new, to confirm the window (`--since 2026-06-26 --until 2026-07-25`) and the `$HOME`-or-temp-dir
  split are exactly reproducible against today's catalog, not drifted since the doc was authored.

### Deviations
- None. Zero code changed, per the phase's own constraint.

### Tradeoffs
- A naive scan (also counting `MultiEdit`/`NotebookEdit`, and not filtering on `tool_result`
  confirmation) gives 76 unique-argmax sessions instead of 73, and 83 touched-at-least-one instead
  of 80. Reported only the code-matching numbers in Resolved Decisions since those are what Phase 3
  will actually produce; the wider scan would overstate the ceiling Phase 3 is held to.

### Open questions
None.

## Phase 1: `common::repo` with the four-rule chain

### Design decisions
- Rule 2's port is a POINT lookup, not a prefix lookup: `PathMap::repo_for_path(&Path)` answers for
  exactly one path and `common::repo::from_known_path` walks `Path::ancestors()` longest-first.
  Putting the longest-prefix semantics in the chain (not in each implementation) means Phase 2's
  catalog-backed impl is a handful of `repo_paths` PRIMARY KEY point lookups rather than a scan, and
  there is exactly one place where "longest prefix" is defined.
- The rule-2 ancestor walk STOPS at a blocked root instead of skipping it -- `from_known_path`,
  `common/src/repo.rs`. Rule 1 can never record `$HOME` (it rejects a blocked toplevel), so this is
  belt-and-braces, but it makes the `$HOME` block a property of the whole chain rather than of one
  rule: nothing at or above `$HOME` can attribute a session even if a stray row lands there.
- `RepoSource` variants are declared best-first so the derived `Ord` and `rank()` agree, and a test
  pins that they do. Phase 2's upgrade-only upsert compares on `rank()`; any in-memory comparison
  uses `Ord`. Two orderings that could disagree would be exactly the "two signals encoding the same
  meaning" failure.
- `FromStr` lands with `as_str` in this phase even though Phase 2 is the first reader. The kebab
  spellings are a persistence contract, so the round trip is written and tested where the type is
  defined rather than re-derived at the call site, and an unknown value is a loud error naming the
  legal set (a dropped provenance would let a guess read back as an observation).
- `Resolver` keeps rule 1's memo and now owns the chain (`Resolver::resolve`), because the blocked
  roots and the git cache both already live there. Rules 2, 3, and 4 are free functions so each is
  testable with no `Resolver` at all.
- `Resolver::blocked_roots()` is exposed so a caller running the rules individually blocks exactly
  the set the chain does, rather than re-deriving `$HOME` and drifting.

### Deviations
- The doc's Data Model shows `pub enum RepoSource` / `pub struct Resolved` and nothing else; the
  shipped module adds `rank()`, `as_str()`, `Display`, `FromStr`, and the `PathMap` port. Same
  effect, correct seam: the rank ordering and the kebab spellings are both named in the doc's prose
  ("git-origin(0) < known-path(1) < ...", "git-origin | known-path | files-touched | path-guess"),
  so they are the type's contract, not new scope.
- `repo-root` validation is applied to an EXPLICITLY SET value only, not to the `<home>/repos`
  default. The doc says "validated at load (absolute path, existing directory)"; validating the
  default too would make every clyde command fail on a machine with no `~/repos`, which is a
  fail-OPEN-to-fail-BRICKED trade nobody asked for. An unset root that does not exist simply means
  rule 4 never fires, which is the fail-closed answer. The distinction is documented on
  `de_repo_root` and in the README paragraph.
- Phase 1's bullet also names Phase 3's `--min-enrichment` ("same treatment"). Not shipped here: its
  consumer (`report collect`'s enrichment warning) is Phase 3, and a config key with no reader is a
  surface that cannot be tested end to end. Phase 3 ships the key, the CLI override, and the example
  together, exactly as this phase shipped `repo-root`.

### Tradeoffs
- `PathMap` as a generic (`fn resolve<M: PathMap>`) vs `&dyn PathMap` -- house DI rule, and it keeps
  `common` free of any SQLite linkage. Cost: `Resolver::resolve` is monomorphized per caller, which
  is irrelevant at two call sites.
- Rule 3's tie handling abstains rather than tie-breaking on slug order. Measured cost from Phase 0:
  7 sessions / `$159.42` fall through to rule 4 instead of being resolved. Accepted, per the doc:
  a tie is evidence of ambiguity, and a slug-ordered winner would fire precisely in the cold-cwd
  case rule 3 exists to serve.
- Rule 4 declines a non-UTF-8 path component instead of lossily converting it. A mangled slug would
  be silently wrong forever in the catalog; declining just means that one session stays
  unattributed.
- `report` re-exports `common::repo` (`pub use common::repo;`) rather than every call site being
  rewritten to `common::repo::...`. Keeps this phase to the move plus the new rules; Phase 3 deletes
  the `report` call site entirely when collect starts reading the persisted column.

### Open questions
None.

## Phase 2: Catalog schema v10, index-time repo, monotonic upsert

### Design decisions
- Repo resolution runs on EVERY session on EVERY reindex pass, independent of the content-mtime skip
  key (`index::reindex`). The decay-regression and precedence success criteria both require a session
  whose transcript did NOT change to still be re-evaluated (a cwd can lose or gain git-origin evidence
  with zero transcript change), so gating repo resolution behind `upsert_session`'s existing
  unchanged-mtime short-circuit would make the whole feature inert on the second reindex of a session.
- `Db::upsert_repo` is a standalone method, called from `index::reindex` right after
  `Db::upsert_session`, rather than a new parameter threaded through `upsert_session` itself. Keeps
  the change surgical: `upsert_session`'s signature (and its ~14 unrelated call sites across
  `sessions`/`efficiency`/`report` tests) is untouched.
- `Db::upsert_repo`'s upgrade-only write is a single `WHERE session_id = ?1 AND ?2 < repo_rank`
  UPDATE, not the doc's sketched unconditional `CASE WHEN :rank < repo_rank ... END` form (see
  Deviations). Both forms are upgrade-only in DATA effect; the `WHERE`-gated form additionally touches
  zero rows on a no-improvement call, which matters here specifically because this write runs on every
  session on every reindex (unlike every other content column, gated behind the mtime skip) -- an
  unconditional `UPDATE` would fire the v5 revision (`updated_at`) trigger on nearly every row of every
  reindex, forcing every `session export --cursor` consumer to re-fetch the whole catalog each pass.
  That is exactly the mass-churn defect the v6 efficiency-annotation trigger-suppression exists to
  prevent, reached by a different route; the `WHERE` guard prevents it without needing
  suppress/restore machinery at all, and lets a genuine improvement (rare: at most 3 writes per
  session over its lifetime, rank 3->2->1->0) advance the cursor, which is correct since `repo` is
  persisted catalog content (like `git_branch`), not a derived-only annotation like `efficiency_json`.
- `repo_paths` (rule 2's backing store) gets its own write path, `Db::record_repo_path`, using
  `INSERT ... ON CONFLICT(path) DO UPDATE` for latest-observation-wins -- the OPPOSITE policy from
  `sessions.repo`, per the doc's "the two tables need opposite write policies". `first_seen` is
  preserved across an update (only set on the initial insert); `last_seen`/`repo` always refresh.
- The catalog-backed `common::repo::PathMap` is implemented directly on `Db` (`impl PathMap for Db`)
  rather than a wrapper newtype, so `index::reindex` can pass `db` straight into
  `Resolver::resolve(cwd, db, ...)`. It is a single `repo_paths` PRIMARY KEY point lookup per call,
  never a scan -- `common::repo::from_known_path` (Phase 1) already owns the longest-prefix walk over
  `Path::ancestors()`.
- New repo-attribution code lives in its own submodule, `sessions/src/db/repo.rs` (+
  `sessions/src/db/repo/tests.rs`), mirroring the existing `catalog`/`query` split-out pattern, rather
  than growing `db.rs` (already 1360 lines pre-Phase-2, close to the 1500-line file-size cap).
- Migration v10 snapshots the on-disk DB to `<path>.pre-v10.bak` (+ `-wal`/`-shm` sidecars if present)
  before its first run, per the house migration-verification rule. The check is gated on the
  PRE-migration `user_version` being in `1..10` (a genuinely pre-existing, non-fresh catalog) so a
  brand-new install does not pay a pointless copy of an empty file, and the predicate can never match
  again once the migration bumps the version -- the snapshot fires exactly once per DB, ever.
- `clyde session reindex --reresolve-repo [--session <id>...]` ships alongside the migration, per the
  doc's explicit instruction that a durable-by-design field needs its repair path shipped in the same
  phase. `--session` is space-separated (house CLI rule), resolved through the existing
  `Db::resolve_id` fuzzy-prefix lookup (same mechanism `clyde session tag` already uses), and requires
  `--reresolve-repo` (validated at the command, not via clap `requires`, to keep the error message a
  plain, readable sentence).
- Added `Db::repo_of` (returns a small `RepoAttribution { repo, source, rank }` struct, not a bare
  tuple -- `clippy::type_complexity` rejected the tuple form) purely for introspection/testing. This
  is NOT the report/export-facing surface; Phase 3 explicitly owns pointing `SessionRecord` /
  `session export` at the persisted column.

### Deviations
- `Db::upsert_repo`'s SQL is a `WHERE`-gated single UPDATE, not the doc's sketched unconditional
  `CASE WHEN :rank < repo_rank ... END` form. Same effect, correct seam -- see the Design decisions
  entry above for the full cursor-churn rationale. The doc's semantic contract (upgrade-only by source
  precedence, `<` not `<=`, never `COALESCE`) is honored exactly; only the SQL shape differs, and the
  reason is specific to this write's unusual calling pattern (every session, every pass).
- `sessions.repo`/`repo_source`/`repo_rank` are NOT yet exposed through `SessionRecord`, `COLS`, or
  `session export` -- the doc's Phase 3 bullet ("Point `clyde session export`'s `repo` at the
  PERSISTED column") explicitly owns that wiring. Phase 2 ships only the storage + write path + CLI
  repair flag, per the phase table's own scope line ("Schema v10, index-time repo, upgrade-only
  upsert"). `Db::repo_of` (introspection-only) fills the gap needed for this phase's own tests.
- Rule 3 (`repos_touched`) is not available yet (Phase 3), so `index::reindex` calls
  `Resolver::resolve` with an always-empty `BTreeMap<String, u64>`. The chain correctly degrades to
  rules 1/2/4 only, which is explicitly how the doc describes phases 4-9 degrading against a stale
  catalog -- the same shape applies here to Phase 2 running ahead of Phase 3.

### Tradeoffs
- `lazy_reindex` (the cheap incremental refresh before every query) falls back to
  `Config::default().repo_root()` on a clyde.yml load failure rather than aborting the reindex
  entirely. `lazy_reindex`'s own doc comment already commits to "failures warn but never abort the
  query -- stale data beats no answer"; disabling the whole incremental refresh over a broken
  `repo-root` key specifically would be a worse regression than degrading rule 4 to its default root.
  `clyde session reindex` (the explicit command) still fails closed on a bad config, unchanged.
- `--session` resolution reuses `Db::resolve_id` (ambiguous-prefix matching) rather than requiring an
  exact id. Consistent with `clyde session tag`'s existing UX; the cost is one more fuzzy-match
  surface to keep in sync if `resolve_id`'s semantics ever change, accepted since it is already the
  house pattern for every other id-taking session subcommand.

### Open questions
None.

## Phase 3: `repos_touched`, rule 3, and report reads the catalog

### Design decisions
- `common::repo::slug_under_root` is extracted as the ONE definition of the
  `<repo-root>/<org>/<repo>` shape, and both readers go through it: rule 4 (`from_path_guess`, on a
  session's cwd) and `efficiency::outcome::union` (on every edited file). Two readers deriving the
  same shape independently is precisely how the two would drift, and rule 3's whole value depends on
  its slugs being comparable to rule 4's.
- `repos_touched` is read from each edited file's PARENT directory, not the file path. That is what
  makes the depth requirement fall out for free: `<root>/<org>/<repo>/src/main.rs` and
  `<root>/<org>/<repo>/README.md` both bucket to `<org>/<repo>`, while a loose file at
  `<root>/<org>/notes.txt` buckets to nothing instead of fabricating the slug `<org>/notes.txt`.
- `union` gained ONE parameter, `repo_root: &Path`, and nothing else -- no cwd, no config struct, no
  map. The function stays pure over `&[FileOutcomes]` plus a path, so its tests need no SQLite and no
  catalog, which is the property the doc's "PURE path parsing" bullet is protecting.
- `build_session`'s `with_outcomes: bool` became `outcomes_repo_root: Option<&Path>`
  (`efficiency/src/collect.rs`). One parameter instead of a bool plus a path makes "extract outcomes
  without a parsing root" unrepresentable, rather than a runtime decision about what to pass.
- **`sessions::index::resolve_repos` is new, and it closes an ordering hole the doc does not name.**
  Rule 3's input is written by `efficiency::reindex_efficiency`, which runs AFTER `sessions::reindex`
  in `cmd_reindex`. So on the pass that follows the v10 reset, every `outcome_json` is still NULL
  while the repo chain is running and rule 3 can fire for nobody. Without a second pass the feature
  would need two consecutive `clyde session reindex` runs to converge, which is exactly the kind of
  "run it twice and it gets better" behavior that makes a number untrustworthy. `resolve_repos` is
  catalog-driven (cwd and the outcome blob are both columns, so no transcript is re-read), scoped to
  `repo_rank > files-touched` (the upgrade-only write means a better-ranked session cannot change),
  and shares `apply_chain` with `reindex` so the two passes can never disagree on which rules run.
- `Db::repos_touched` / `Db::repo_candidates` read ONE key (`repos-touched`) out of the otherwise
  opaque `outcome_json`. `sessions` still does not depend on `efficiency` (that dependency runs the
  other way, to persist), so the key is spelled as a const in `db/repo.rs` rather than imported.
- `attribution`'s `(unattributed)` row absorbs the difference between `totals.spend-usd` and the sum
  of the per-session spends, which is what makes "the rows sum to `totals.spend-usd`" true by
  construction rather than approximately. The difference is a pricing artifact, not attribution:
  `totals.spend-usd` prices the UNIONED per-model token counts once, each session is priced on its
  own, and the two diverge whenever a model's >200k long-context tier is crossed by the union but not
  by any one session. Measured on the 30-day window it is `-$0.06`. It is WARNed above a cent so it
  can never grow unnoticed, and folding it anywhere else would attribute money to a repo that no
  session's own price supports.
- `RepoRow.repo_source` carries the STRONGEST evidence any session in the row has, not the weakest.
  The question a marked row answers is "is this repo real?", and a slug is fabricated only if NO
  session ever observed it -- so `tatari-tv/clyde-ft` (all guesses) is marked and `tatari-tv/clyde`
  (500 observations plus one guess) is not. Per-source spend, which is the "how much of this is
  guessed" question, lives in `attribution`.
- `(unknown-source)` is a real bucket, not defensive padding: `report merge` can fold in a pre-v10
  artifact whose repos were resolved before provenance existed. Bucketing those as `observed` would
  launder them; bucketing them as `(unattributed)` would contradict `by-repo`, which does count them.
  A locally-collected window can never produce it -- `to_collected` fails loudly on a slug with no
  source, because the catalog writes both columns in one statement.
- The v10 reset is gated on `6 <= from_version < 10`, and the second half of that range matters as
  much as the first: without it every `Db::open_at` on an already-migrated catalog would wipe the
  efficiency a reindex just paid to compute. There is a test for that specific case.
- `report::repo` (Phase 1's compatibility re-export) is DELETED along with its last caller. Leaving
  an alias to a resolver `report` must never call again is an invitation to reintroduce the
  collect-time decay this phase exists to remove.
- `session::repo_slug` is deleted for the same reason: `session export` was its only production
  caller, and a second cwd-to-slug function sitting next to the persisted column is the "two fields
  with one name and two answers" hazard the doc names.

### Deviations
- The doc's Data Model shows `AttributionRow { source, sessions, spend, confidence }`; the shipped
  struct adds a `#[serde(skip)] spend_raw: f64`. Same effect, correct seam: the rows must be
  pre-sorted by spend descending (every other context table is), and the string-only context rule
  forbids a numeric operand reaching the model, so the sort key is carried and skipped -- exactly the
  shape `OrgRow`/`RepoRow`/`ModelRow` already use for `spend_raw`.
- `Attribution` is computed by `aggregate::compute_attribution` and placed at the TOP level of the
  context block, not inside `Aggregates`. The doc's context-additions list spells it `attribution`
  (top-level), and it is a statement about the whole figure rather than another rollup of it.
- Phase 3 edits NO prompt template, per the phase table's "touches prompts: no" and the prompt-edit
  ledger (Phase 3 is not one of the seven). `RepoRow.repo_source` and `attribution` are therefore in
  the context block and unread by either prompt until a later phase quotes them. The doc's "both
  templates mark guessed rows where `by-repo` renders" lands with Phase 7, which owns `by-repo`'s
  prompt surface.
- The doc's Phase 3 bullet for `--min-enrichment` says only "config key + CLI override + example";
  the shipped change also includes the READER (the collect warning), because Phase 1 flagged that a
  config key with no reader cannot be tested end to end. Phase 9 still owns `enrichment-coverage` in
  the context block.
- `report` gains `rusqlite` as a DEV-dependency. The catalog's write path lands `efficiency_json` and
  `outcome_json` in one statement by design, so the state the new fail-closed guard exists to catch
  is not reachable through it; the test reaches past it with a raw connection. No production code in
  `report` touches SQLite.

### Tradeoffs
- `resolve_repos` runs on every explicit `clyde session reindex`, costing one `git` attempt per
  distinct unresolved cwd (memoized per pass; a vanished directory short-circuits on `exists()` with
  no spawn). Measured on the live 1,706-session catalog the whole reindex including this pass ran
  well inside a minute. The alternative -- resolving repo only inside `reindex`, before outcomes
  exist -- is free but needs two reindex runs to converge, and a number that improves on a second
  identical run is a number nobody should trust.
- `resolve_repos` is wired into `cmd_reindex` only, NOT into `lazy_reindex` (the cheap incremental
  refresh before every query). Same reasoning the efficiency pass already uses: a `clyde session ls`
  must not pay a catalog-wide repo re-resolution.
- The enrich-coverage check returns its message rather than printing it (`enrichment_warning`), so
  the threshold behavior is unit-testable without capturing stderr. Cost: the caller does the
  `eprintln!`, one extra line at the call site.
- `min-enrichment` is a fraction, not a percent, matching `cache-read-share-floor` and
  `tool-error-rate-ceiling`. `min-enrichment: 50` is rejected BY NAME at load and `--min-enrichment
  50` at resolution, because silently accepting it configures a floor no window can meet and warns on
  every single run.

### Open questions
None.

### Measured on the live 30-day window (`--since 2026-06-26 --until 2026-07-25`, after a full reindex)

Same window as the doc's baseline table: 1,523 sessions, `$9,450.31`.

| repo-source | sessions | spend | share |
|---|---|---|---|
| `git-origin` | 961 | `$5,604.45` | 59.3% |
| `files-touched` | 87 | `$1,585.18` | 16.8% |
| `known-path` | 243 | `$1,561.49` | 16.5% |
| `path-guess` | 27 | `$44.86` | 0.5% |
| `(unattributed)` | 205 | `$654.33` | 6.9% |

- **`by-repo` coverage: 93.1% (`$8,795.98` of `$9,450.31`), against the 59.3% baseline.** Unattributed
  spend fell from `$3,845.92` to `$654.33`, and the repo count rose from 47 to 57.
- The `git-origin` row reproduces the baseline exactly (`$5,604.45` vs the doc's measured
  `$5,604.39`), which is the check that the other rows are recovery rather than reshuffling.
- **Rule 3 against its ceiling.** Phase 0 measured 73 sessions / `$1,207.92` of unique-argmax recovery
  in the 279-session cold-cwd (`$HOME` / temp-dir) subset. Rule 3 delivered **74 sessions /
  `$1,245.68`** there -- the ceiling is met, and the one-session difference is catalog movement
  between the two measurements the same day (this reindex upserted 3 sessions). Rule 3 ALSO served 13
  sessions / `$339.50` whose cwd is rule-4 shaped but was never seen alive, beating rule 4 in the
  chain as designed (inferred outranks guessed); those are outside the ceiling's subset, which is why
  the row totals 87 sessions rather than 74.
- 206 of the 562 previously-unattributed sessions remain unattributed, all cold-cwd, matching Phase
  0's "199 sessions rule 3 cannot serve at all" plus the 7 that tie and abstain.
- `path-guess` is `$44.86`, 0.5% of the window: rule 4's fabrication hazard is real but tiny, and it
  is now labeled everywhere it renders.
- Attribution rows sum to `totals.spend-usd`: the per-session spends summed to `$9,450.37` against a
  headline of `$9,450.31`, and the `-$0.06` pricing residual is carried in `(unattributed)`.
- The enrich-coverage warning fired live: 550 of 1,523 sessions (36.1%) below the 50% floor.

## Phase 4: by-day correctness

### Design decisions
- `compute_by_day` returns `(Vec<DayRow>, CarriedIn)` instead of `Vec<DayRow>` alone -- `aggregate.rs:compute_by_day`.
  The zero-fill and the carried-in split are one pass over `report.sessions`, so there is exactly one
  place that decides "does this session's date belong to a `by-day` row, or to `carried-in`."
- Zero-fill walks `since_date..=until_date` inclusive with a plain `while` loop over `NaiveDate`
  (`chrono::Duration::days(1)` step) before folding sessions in, so every calendar date gets a
  `DayAcc::default()` row even when no session ever touches it -- `aggregate.rs:compute_by_day`.
- `DayRow.active: bool` is `sessions > 0`, computed once per row after the fold, not tracked
  incrementally -- keeps the "was this ever touched" question answerable from the row alone, with no
  separate bookkeeping to drift from it.
- `period.days` becomes `(until_date - since_date).num_days() + 1` (inclusive) in
  `render.rs:build_period_view`, and `period.active_days` becomes
  `aggregates.by_day.iter().filter(|r| r.active).count()` -- replacing the old `aggregates.by_day.len()`
  (which, pre-Phase-4, WAS the active-day count only because inactive days had no row at all; now every
  day has a row, so the row count and the active count are different questions and must be computed
  differently).
- The pre-`since` boundary case keeps its OWN defensive clamp direction: a `begin` before `since`
  now goes to `carried_in` (the fix), while a `begin` after `until` still clamps DOWN to `until_date`
  (the pre-existing defensive guard, unchanged) -- `begin <= modified <= until` should make the latter
  unreachable, but the guard was already there and Phase 4's scope is the lower bound only.
- Both prompt templates gained a `carried-in` schema bullet plus a rewritten `by-day` bullet
  (`report.pmt`, `report-html.pmt`) stating the by-day series no longer accounts for `totals.spend`,
  instructing the model to cite `aggregates.carried-in` as its own fact rather than folding it into a
  day or inferring it as a gap. The Executive Summary and Usage Profile / Temporal distribution
  sections in `report.pmt` were also touched, since the doc calls out that both prompts make the
  temporal-shape claim the first sentence of the Executive Summary -- the sentence that used to rest on
  the most distorted bar in the series.

### Deviations
None. Implemented at the spec's own seam (`aggregate.rs:compute_by_day`, `render.rs:build_period_view`)
with no signature surprises against the design doc's `DayRow`/`CarriedIn` structs.

### Tradeoffs
- `compute_by_day` returning a tuple `(Vec<DayRow>, CarriedIn)` vs. a struct wrapping both: chose the
  tuple because it is a private function's return, used in exactly one call site (`compute`), and a
  wrapper struct would exist only to be destructured immediately. `Aggregates` (the public, serialized
  type) is where `carried_in` gets a named field.
- Kept the upper-bound defensive clamp (`begin > until` clamps down) rather than also carrying those
  sessions somewhere: the design doc's Phase 4 bullets and `CarriedIn` doc comment both describe only
  the pre-`since` case. Since `begin <= modified <= until` should make the upper case unreachable in
  practice, preserving the pre-existing silent clamp (rather than inventing a new bucket the doc never
  asked for) keeps the change scoped to what was specified.

### Open questions
None -- both live-data success criteria (the since-row session count and the carried-in figures) were
verified against the real window rather than asked about; see below.

### Measured on the live 30-day window (`--since 2026-06-26 --until 2026-07-25`, after Phase 3's reindex)

Re-collected fresh (`clyde report collect --since 2026-06-26 --until 2026-07-25`) and probed
`aggregate::compute` directly against the resulting JSON (1,523 sessions, matching the doc's baseline
window):

| metric | value |
|---|---|
| `period.days` | 30 |
| `period.active-days` | 29 |
| `aggregates.by-day.len()` | 30 |
| `since` row (`2026-06-26`) sessions | 14 |
| `since` row spend | `$104.94` |
| `carried-in.sessions` | 16 |
| `carried-in.spend` | `$354.22` |
| `carried-in.tokens-human` | 391.0M |

Every figure matches the design doc's stated success criteria exactly: the since-day session count is
14 (not the old clamped 30), and carried-in is 16 sessions / `$354.22` -- the same figure the doc
states, so Phase 3's repo-attribution rework did not move this date-windowing number, as expected
(attribution and by-day partition the same session set on different axes; changing one should never
move the other, and it didn't).

## Phase 5: Agent-type becomes a partition

### Design decisions
- `agent-type-costs` is now built from the SCOPE's own `raw.by_model`, not from the catalog's scalar
  `cost_usd` -- `report/src/report.rs::agent_type_costs` -- so every bucket is priced through the
  same fetched `Pricing` that produces `totals.spend-usd`. The catalog's embedded-pricing decision is
  untouched, exactly as the Resolved Decision "catalog pricing stays embedded" requires: report
  re-prices at read time.
- `MAIN_SESSION_BUCKET = "(main-session)"` is a `pub const` beside `WINDOW_NOTE` --
  `report/src/report.rs` -- parenthesized so it cannot collide with a real agent type, matching
  `aggregate::UNATTRIBUTED_ORG`'s precedent.
- The residual is taken ONCE, against the union of every subagent's `by_model`, rather than once per
  subagent -- `report/src/report.rs::agent_type_costs` -- so the existing `subtract_token_totals`
  (`report/src/report.rs:558`, the `--no-rollup` path's own helper) is reused unchanged.
- `check_fold_invariant` runs BEFORE the subtraction -- `report/src/report.rs` -- because
  `subtract_token_totals` clamps every field at zero and would therefore absorb the impossible state
  silently, leaving the rows summing above `totals` with nothing to explain why.
- Bucket costs are NOT rounded to cents -- `report/src/report.rs::price_bucket` -- price is summed
  LAST over the accumulated per-model `TokenTotals` and rounding happens once at display. Rounding
  per bucket would drift the partition by up to a cent per row (24 rows on the live window).
- Coverage strings live on the render view, not the artifact -- `report/src/render.rs::coverage_note`
  -- because they are a statement ABOUT `totals.spend`, which only exists once the report is
  assembled. A zero total renders `0.0%`, matching `compute_attribution`'s precedent for the same
  divide-by-zero.
- An untyped subagent keeps its own `unknown` row rather than folding into the residual, so its spend
  stays visible as unattributed-but-delegated rather than being laundered into main-session work.

### Deviations
- The doc's signature is `fn agent_type_costs(eff: &SessionEfficiency, pricing: &Pricing) ->
  BTreeMap<String, WorkloadCost>`. Shipped as `fn agent_type_costs(session_id: &str, raw:
  &RawCounters, subagents: &[SubagentEfficiency], pricing: &Pricing) ->
  Result<BTreeMap<String, WorkloadCost>>`. Same effect, correct seam: the loud-failure requirement
  forces `Result`, the error text has to name the session (a `--no-rollup` subagent row's
  `SessionEfficiency.session_id` is the AGENT id, not the session's), and `entry_from_scope` already
  holds the scope's `raw` -- which is `efficiency.aggregate.raw` at all three call sites -- so taking
  it directly avoids implying the function may read the whole passthrough.
- `build_report`, `expand_entries`, and `entry_from_scope` became fallible to carry that error out.
  `build_json` was already `Result`, so the public surface is unchanged apart from `build_report`.
- The fold-invariant check is a strict superset of the doc's bullet. The doc names only "a subagent
  model absent from the aggregate's `by_model`"; the same clamp also swallows a model that IS present
  but whose subagent tokens exceed the scope's, which is the review panel's noted clamp hazard. Both
  states are impossible under `fold` (`efficiency/src/fold.rs:95-99` is an exact integer union), and
  both are now errors naming the session and the model.
- Neither template retains the literal phrase "never reconcile" anywhere, rather than keeping it on
  the skill/MCP sentences. The success criterion only bans it from the agent-type section, but a test
  cannot scope a substring to a prompt section without brittle heading parsing, so the phrase was
  reworded to "cannot be reconciled against it" on the tag sets. The non-reconcilable FRAMING for
  by-skill / by-mcp is kept in full and is asserted by
  `render::tests::both_templates_declare_agent_type_costs_a_partition`.
- The render fixture `report_with_efficiency`'s `by_skill` / `by_mcp` costs were lowered
  (`$1.25`/`$0.30` -> `$0.20`/`$0.05`) so the new coverage strings read as a real share of the
  fixture's `$0.60` total rather than 208%.

### Tradeoffs
- Re-pricing in `report` vs re-pricing in the catalog: re-pricing here keeps `cost_usd` reproducible
  from the same JSONL on a later reindex (`efficiency/src/metrics.rs:130-138`) and needs no
  `efficiency_json` reset. The cost is that the artifact now carries two pricing bases side by side
  (fetched for agent-type and models, embedded for by-skill / by-mcp), which is exactly why the
  coverage strings name their basis in the string itself.
- Failing the WHOLE report on one broken session vs skipping that session: fail-closed was chosen
  because the alternative publishes a partition that is quietly short by one session's spend, which
  is the same class of defect this phase exists to remove. The error names the session and the
  remedy, so the operator can reindex exactly one session rather than the catalog.
- Checking every token field rather than just `total`: costs five comparisons per subagent-model
  pair and catches a split that sums right but distributes wrong (which would misprice, since cache
  reads and writes carry different rates).

### Open questions
- The live window emits an `unknown` agent-type row at `$0.00` / `0` tokens (an untyped subagent that
  consumed nothing). Phase 6's resolved decision drops zero-token models from `totals.models` for
  precisely this reason ("a model that consumed nothing is not part of the cost story"); the same
  argument applies to a zero-token agent-type bucket, but the doc scopes that decision to
  `totals.models`, so this phase left the row alone rather than widening Phase 6's rule unasked.

### Measured on the live 30-day window (`--since 2026-06-26 --until 2026-07-25`)

`clyde report collect --since 2026-06-26 --until 2026-07-25`, 1,523 sessions:

| metric | before Phase 5 | after Phase 5 |
|---|---|---|
| `sum(agent-type-costs.spend)` | `$2,427.31` | `$9,450.31` |
| coverage of `totals.spend-usd` (`$9,450.31`) | 25.7% | 100.0% |
| unrounded delta against `totals.spend-usd` | n/a | `$0.0046` |
| agent-type rows | 8 | 24 |

The residual dominates, which is the finding the defect predicted: `(main-session)` is `$7,023.01`
(7.70B tokens) against `$1,591.85` for `phase-implementer`, the largest delegated bucket. Summing the
CENT-ROUNDED display strings gives `$9,450.32`, one cent over, since 24 rows each round
independently; the prompts tell the model to copy `totals.spend` for the total rather than add the
column, so no reader-facing figure carries that cent.

Coverage strings on the same window:

- `efficiency.by-skill-coverage`: `$1,113.10 of $9,450.31 (11.8%), embedded-price basis` (50 tags)
- `efficiency.by-mcp-coverage`: `$278.74 of $9,450.31 (2.9%), embedded-price basis` (83 tags)

## Phase 6: Untracked gate, pricing basis, disclosure

### Design decisions
- The zero-token gate (`has_tokens`, `report/src/report.rs`) filters `by_model` BEFORE pricing, in
  both `price_models` (per-session `models` + `untracked_models`) and `build_report`'s report-wide
  `totals_model_entries` build over the unioned `grand.by_model`. One predicate, two call sites, so a
  model dropped from every session's `models` cannot resurface in the report-wide union either.
- `Basis` (design "Pricing basis, always present") lives in `render.rs` as a render-time-only
  `ContextBlock` field, built from the `Pricing` the CURRENT render resolves -- never persisted in the
  collected `Report`/JSON artifact. Reasoning: the API Design section's "Context block additions"
  list places `basis` alongside `unit-costs` and `prior`, i.e. among the render-only view structs, not
  the artifact schema; and `report collect` and `report render` are separate CLI invocations that each
  call `Pricing::auto` independently, so a persisted-at-collect-time basis could describe a DIFFERENT
  feed resolution than the one a later render actually used for its own re-derived figures
  (`by-skill-coverage`, cache counterfactual, etc.).
- The exact disclosure sentence ships as a module constant (`BASIS_NOTE`, `render.rs`) rather than
  being assembled from `Basis`'s other fields at each call site, so the "verbatim, never paraphrased"
  requirement is enforced by construction (one string, one place it's written) rather than by
  convention.
- Added the note to the OFFLINE built-in renderer (`render_built_in`, `to_markdown`'s
  `Template::BuiltIn` arm) and a `{{basis-note}}` placeholder to the user-authored custom-template
  path (`render_custom`), even though the design doc's own bullets name only `report.pmt` and
  `report-html.pmt`. The success criterion says "every rendered artifact", and both additions were
  one line each.

### Deviations
- **`feed_source`'s vocabulary is `embedded | fetched | override`, not the doc's `embedded | cached |
  fetched`.** Traced into `pricing/src/fetch.rs`: `claude-pricing`'s `Source` enum has exactly three
  variants (`Embedded`, `UserOverride`, `Fetched`), and BOTH a live network fetch and an on-disk cache
  hit resolve to `Source::Fetched` -- confirmed by the crate's own test commentary
  (`pricing/src/fetch/tests.rs:1148-1150`: "a `Source` assertion could not tell cache from network and
  would be vacuous"). So "cached" is not a distinguishable case with the current public API; both
  surface as `"fetched"` here. `Source::UserOverride` is a real, live case (`Pricing::auto` falls
  through to it) that the doc's three-value vocabulary does not name at all, so it surfaces as
  `"override"` rather than being force-fit into "cached" (a `~/.config/<app>/pricing.json` override
  file is not a cache of the fetched feed; conflating the two would misdescribe a real
  operator-supplied file as network-derived). Distinguishing a live fetch from a cache hit would
  require a new field on `claude-pricing::Pricing`, which is out of this phase's scope (`claude-pricing
  is untouched`, Technical Considerations, Dependencies).
- Basis is a `render.rs`-only view rather than a field on `report::Report` (see Design decisions
  above for the reasoning); the doc's Data Model section places the `Basis` struct definition
  generically rather than inside either module, so this is a seam choice, not a contradiction of an
  explicit instruction.

### Tradeoffs
- Filtering `by_model` before pricing (rather than pricing everything and filtering the priced
  `ModelTokens` map after) means a zero-token model never reaches `ModelTokens::from_totals` at all --
  cheaper, and it also means a zero-token model can never accidentally acquire a nonzero `spend_usd`
  from some future pricing rule that prices on turn-count rather than tokens. Chose this over
  filtering post-pricing for that reason, even though either would satisfy today's success criteria.
- Kept `has_tokens` checking `TokenTotals::total` (the derived sum) rather than all five component
  fields individually. `total` is recomputed from the components on every mutation (`TokenTotals::add`,
  `common/src/metrics.rs`) and can only be zero when every component is zero, so the two checks are
  equivalent; `total` reads as the more direct statement of intent ("this model spent nothing").

### Open questions
None. The `feed_source` vocabulary gap (Deviations) is a factual limitation of the current
`claude-pricing` public API, not a decision needing Scott's input; widening `Source` to distinguish a
live fetch from a cache hit is a `claude-pricing` change this phase's scope excludes.

### Verification
- `cargo test -p report`: 274 tests (existing 272 plus two new: `zero_token_model_is_dropped_from_models_and_untracked`,
  `nonzero_token_unpriced_model_still_flagged_untracked`) plus three render tests
  (`build_context_block_always_carries_the_pricing_basis`, `render_built_in_includes_the_pricing_basis_note`,
  `custom_template_substitutes_the_basis_note_placeholder`); all green.
- Mutation-checked the gate: reverted `has_tokens` to always return `true` and confirmed
  `zero_token_model_is_dropped_from_models_and_untracked` fails (`<synthetic>` reappears in
  `entry.models`); restored and reconfirmed green. `nonzero_token_unpriced_model_still_flagged_untracked`
  proves the negative case: a real unpriced NONZERO-token model still lands in `untracked-models`, so
  the gate is a token-count filter, not a blanket suppression of the warning.
- `otto ci`: green (fmt, clippy, check, test, whitespace).
- Live data, `clyde report collect --since 2026-06-26 --until 2026-07-25` (1,523 sessions,
  `$9,450.31`, unchanged from Phase 5): `totals.untracked-models` is `[]` and `<synthetic>` is absent
  from `totals.models` (7 models remain, all priced). Confirmed the disclosure sentence reaches a real
  rendered artifact end-to-end via `clyde report render --template <file containing {{basis-note}}>`
  against that same collected JSON (no LLM call): emits the exact sentence verbatim. Did not run the
  LLM-authored `report.pmt`/`report-html.pmt` path live against the full window (the local `claude` CLI
  transport did not return within 2 minutes on this 1,523-session context); the header-line instruction
  and the underlying `basis` context field are verified by unit test
  (`build_context_block_always_carries_the_pricing_basis`) and by the prompt text itself, but a live
  opus-authored render was not observed to confirm the model actually reproduces the line.

## Phase 7: by-repo outcomes, extended counters, unit costs

### Design decisions
- Per-repo outcomes are built by the SAME `outcome::rollup` the report-wide totals use, restricted to
  the repo's sessions -- `aggregate::repo_outcomes` -- so a row's dedupe rules (commits by sha, PRs by
  url) cannot drift from the global ones they are a restriction of. No second dedupe implementation.
- `RepoOutcomes` carries commits / prs-opened / files-edited / lines-written / lines-replaced and
  deliberately NOT the Confluence/Jira/Slack counts -- `aggregate.rs` -- because those are not
  repo-scoped work and a per-repo row is the wrong place to count them. They stay in `outcomes.totals`.
- Outcome attribution follows the SESSION's repo, the same key the row's spend is bucketed under
  (`compute_by_repo`), so the two numbers in a spend-against-output comparison describe the same set
  of sessions. A PR opened against a different repo than the session's lands under the session's repo;
  `prs[].repository` still carries the PR's own slug for anyone who needs the other view.
- `RepoRow::outcomes` is ABSENT (not a row of zeroes) for a repo that observed nothing, and its
  `commits-percent-of-max` / `prs-percent-of-max` are absent with it -- `compute_by_repo` -- so the
  house "no scale field -> not drawn" rule applies with no special case, and no unobserved repo gets a
  zero-length bar that reads as a measured zero.
- New counters are `lines_written` / `lines_replaced` on `efficiency::Outcomes`, counted off the same
  success-confirmed Edit/Write calls `files_edited` already rides (`outcome::extract`,
  `apply_confirmed`). A call with no `file_path` contributes neither, so the two counters always
  describe the same set of calls.
- A `Write` contributes lines WRITTEN only, never replaced (`efficiency/src/outcome.rs`): the record
  carries the new content and not what it overwrote, so the replaced count is unobservable and this
  fails closed toward an undercount rather than sniffing the tool_result prose for "File created
  successfully at" to guess whether the file was new.
- `UnitCosts` divides by the Phase 4 active-day figure by taking `by_day` as a parameter
  (`compute_unit_costs(report, &aggregates.by_day)`) rather than recounting active days, so there is
  one definition of "active day" in the codebase, not two.
- Percentiles are NEAREST-RANK (`percentile`), so `session-spend-p50` is always a real session's own
  spend and never an interpolated figure nobody spent. Unpriced sessions (`spend_usd: None`) are
  excluded from the distribution rather than folded in as `$0`: an untracked model's spend is unknown,
  and counting it as zero would drag both percentiles down with a number nobody measured.
- Both templates carry the exact ratio wording, and `both_templates_license_unit_costs_as_ratios_and_by_repo_outcomes`
  (`render/tests.rs`) pins it mechanically: the "These are RATIOS, not prices" sentence, the statement
  that the numerator includes sessions that produced no commit, and the by-name ban on "each commit
  cost". The prompt-edit ledger is a test, not a habit.

### Deviations
- **No PR-merged counter, and this contradicts the doc's Phase 0 finding.** The doc records "**Yes**
  to a PR-merged counter (`gitOperation.pr.action == "merged"`)". Phase 0 proved the FIELD occurs; it
  did not check what the field MEANS. All four live `pr.action == "merged"` records were read in full
  this phase, and none of them is a merge: `pr#23` and `pr#67` are `"! Pull request tatari-tv/marquee#23
  was already merged"` (an idempotent no-op on an already-merged PR), and `pr#92` and `pr#1963` are
  `"X Pull request tatari-tv/platform-infra#1786 is not mergeable: the base branch policy prohibits the
  merge"` (a FAILED merge). The field classifies the `gh pr merge` ATTEMPT, not its outcome. Shipping
  the counter would have published "4 PRs merged" for a period in which zero PRs were merged by these
  sessions, in a finance-facing document whose entire premise is that its numbers are observed and
  verifiable. Also noted: 3 of the 4 records carry no `url`, so the counter had no dedupe key either,
  and on `pr#92` the recorded url (`private-helm-charts/pull/92`) belongs to a different PR than the
  one the command acted on (`platform-infra#1786`).
- **No branches-merged counter.** `branch.action == "merged"` (18 live occurrences) IS a confirmed
  completed `git merge` ("Fast-forward", "Merge made by the 'ort' strategy"), but its `ref` mixes two
  opposite meanings: `ref: reject-dotted-names` is a feature branch landing (delivery), while
  `ref: origin/main` is main being merged INTO a feature branch (a sync). Separating them needs a
  default-branch-name heuristic that Phase 0 never measured and the doc never authorized, so it is not
  built. Recorded as an open question rather than guessed at.
- The line counters are named `lines-written` / `lines-replaced`, not the doc's "line delta". A signed
  net would hide the volume (a 500-line rewrite and a no-op both net ~0) and "lines added/removed"
  would read as a `git diff` stat, which this is not: an Edit that rewrites a 3-line block counts 3
  written and 3 replaced. Both templates carry that caveat verbatim. No `net` field is emitted, per the
  house rule that a field derived from two others is dropped rather than kept in sync.
- No migration ships with this phase and none is needed: v10 already nulls `efficiency_json` and
  `outcome_json` and the whole doc ships as one release, so the single full reindex v10 already forces
  populates `lines-written` in the same pass as `repos-touched`. Until that reindex runs, the counters
  read 0 and `outcomes.totals` OMITS them (present-if-nonzero), so a stale catalog says nothing rather
  than claiming 4,242 edited files produced no lines.

### Tradeoffs
- Per-repo counts deduped WITHIN the repo, vs a globally-deduped split that assigns each shared commit
  to exactly one repo. Chose within-repo: a split would need an arbitrary tie-break rule, and each row
  is then honest about its own repo. Cost: the rows do not sum to `totals.outcomes` (measured live: row
  sum 487 vs 484 deduped across attributed sessions, 3 commits observed in sessions attributed to two
  different repos). Both templates therefore state that the per-repo counts are never summed and that
  `outcomes.totals` is the period figure.
- `RepoOutcomes` as its own struct vs reusing `OutcomeTotals` on the row. Chose a dedicated struct: it
  keeps the non-repo-scoped MCP write counts out of a repo row, and it makes the chartable pair
  (`commits`, `prs-opened`) the visible shape of the type.
- Labels live in the templates, not the binary (the doc's `UnitCosts` is six `Option<String>` fields
  and nothing else). A binary-owned label string would be drift-proof, but it deviates from a spec the
  doc is explicit about; the mechanical template test is the mitigation.

### Open questions
- **The doc's Phase 0 finding "Yes to a PR-merged counter" is wrong on semantics and should be
  corrected in Resolved Decisions** with the four live payloads above. Phase 7 shipped no merged
  counter as a result. If Scott wants a merged-PR figure anyway, the options are: (a) sniff the
  `toolUseResult.stdout` for a success pattern, which has ZERO live positive examples to calibrate
  against and fails open on a wording change; (b) build it on `branch.action == "merged"` plus a
  default-branch heuristic to separate landings from syncs; or (c) leave PRs-opened as the only PR
  outcome. Recommendation: (c), and correct the doc.
- Phase 13's synthesized fixtures should include a zero-commit fixture, which is what makes the
  "`unit-costs.per-commit` is absent on a zero-commit fixture" criterion an artifact-level check rather
  than the unit-level one this phase shipped (`unit_costs_are_absent_on_every_zero_denominator`).

### Verification
- `otto ci`: green (fmt, clippy, check, test, whitespace).
- 12 new tests: 3 in `efficiency/src/outcome/tests.rs` (happy path, failed-edit error path, insertion +
  unconfirmed-call edge), 9 in `report/src/aggregate/tests.rs`, 2 in `report/src/render/tests.rs`, plus
  the template ledger test.
- Mutation-checked that the tests bite: removing the zero-denominator guard in `per_unit` and emitting
  output geometry for an outcome-less repo failed exactly four tests
  (`unit_costs_are_absent_on_every_zero_denominator`, `unit_costs_are_absent_when_the_report_carries_no_outcome_rollup`,
  `unit_costs_are_all_absent_on_an_empty_window`, `by_repo_output_geometry_is_absent_for_a_repo_with_no_outcomes`);
  restored and reconfirmed green.
- **Live figures**, `clyde report collect --since 2026-06-26 --until 2026-07-25` (1,523 sessions,
  `$9,450.31`, 29 of 30 active days, 490 commits, 166 PRs opened), computed by `compute_unit_costs`
  over the collected artifact:

  | field | value |
  |---|---|
  | `per-commit` | `$19.29` |
  | `per-pr` | `$56.93` |
  | `per-active-day` | `$325.87` |
  | `per-session` | `$6.21` |
  | `session-spend-p50` | `$0.63` |
  | `session-spend-p90` | `$17.58` |

  The mean (`$6.21`) is nearly 10x the median (`$0.63`): the period is carried by a small number of
  large sessions, which is exactly the shape the p50/p90 pair exists to expose and which no figure in
  the artifact could show before.
- **Live by-repo outcomes**: 46 of 57 rows carry an `outcomes` object. Top rows:
  `tatari-tv/clyde` `$1,336.82` / 84 commits / 43 PRs (100% on all three bars),
  `scottidler/second-brain` `$1,127.43` / 49 commits / 1 PR (spend 84.3%, commits 58.3%, PRs 2.3%),
  `tatari-tv/marquee` `$1,032.14` / 42 commits / 23 PRs. The spend-and-output divergence on
  `second-brain` (second-highest spend, near-zero PRs) is precisely the comparison gap 8 said could not
  be drawn.
- **Global-dedupe criterion, measured**: over the sessions that HAVE a repo, the deduped rollup is 484
  commits / 159 PRs and the by-repo row sum is 487 / 159, so the row sum double-counts 3 cross-repo
  commits and the dedupe is doing its job. `totals.outcomes` is 490 / 166; the residual (6 commits, 7
  PRs) belongs to sessions with NO repo, which by-repo has no row to hold. The criterion as literally
  written ("global dedupe matches `totals.outcomes`") therefore holds only on a window where every
  outcome-bearing session is attributed; both halves are pinned by test
  (`by_repo_outcomes_globally_dedupe_to_totals`, `by_repo_outcomes_cannot_carry_an_unattributed_session`).
- **Live line counters**: the catalog's stored `outcome_json` predates these fields, so a collect today
  reports `lines-written: 0` (and the context omits the key). Extraction itself is confirmed against
  real transcripts: running `efficiency::outcome::extract` over the 46 transcripts under
  `~/.claude/projects/-home-saidler-repos-tatari-tv-clyde` yields **36,057 lines written / 7,833 lines
  replaced** (e.g. one session: 8 files edited, 947 written, 33 replaced). The release's v10 reindex is
  what makes those figures reach a report.

## Phase 8: `--prior` and Month over Month

### Design decisions
- `build_prior_view` reads, schema-gates (the same `check_schema_version` the primary `-i` input
  uses), and aggregates the `--prior` file through the SAME `aggregate::compute` as the current
  period, then reuses the existing `build_totals_view` for `prior.totals` and the existing
  `aggregate::RepoRow` / `OrgRow` types verbatim for `prior.by-repo` / `prior.by-org` -- one code
  path computes both sides of the comparison, so they cannot drift apart the way two independent
  builders could.
- `predates_fidelity_fields` (`report/src/render.rs`) detects a pre-Phase-3 prior artifact by a
  single, already-in-the-artifact signal: at least one session carries `repo` but none carries
  `repo_source`. Phase 3 is the first phase that ever persists `repo_source` alongside `repo`, so
  this predicate is reliable without a schema bump: an artifact from before Phase 3 has `repo` (the
  old cwd-resolved value) but the field for provenance simply never existed in that JSON.
- When `predates_fidelity_fields` fires, `prior.outcomes` is omitted ENTIRELY (not zeroed, not
  partially redacted) and `prior.predates-fields` carries the caveat sentence instead. `prior.totals`
  / `prior.by-repo` / `prior.by-org` still render with real figures on a pre-change artifact: spend
  and session counts are v1-era fields, unaffected by anything this design added, so gating them too
  would suppress real data for no honesty gain.
- `comparable` is `prior.days == period.days`, both computed by the SAME inclusive
  `(until.date_naive() - since.date_naive()).num_days() + 1` formula Phase 4 already established for
  `period.days` -- one formula, not two that could silently diverge on an edge date.
- Extracted the shared `outcome_totals_view` helper out of `build_outcomes_view` so the current
  period and `prior` build their `outcomes` fields through one present-if-nonzero conversion rather
  than two copies that could drift.

### Deviations
- The doc says "define behavior on a PRE-CHANGE prior artifact" without naming the exact detection
  mechanism (schema-version does not change, so it cannot be the signal). Implemented the
  `repo`-present-but-no-`repo_source` heuristic described above; documented here since it is a
  judgment call the doc left open, not a literal spec.
- The doc's field list for `prior` says "totals, by-repo, by-org, outcomes"; implemented `by-repo`
  and `by-org` as the SAME `aggregate::RepoRow`/`OrgRow` types the current period's `aggregates` uses
  (chart-scale fields and all) rather than a stripped-down prior-only shape. Same effect, correct
  seam: it is exactly the reuse the design's "aggregated through the same `aggregate::compute`"
  language calls for, and it avoids a second row shape the prompts would have to learn.

### Tradeoffs
- Gating only `prior.outcomes` (not `prior.totals`/`by-repo`/`by-org`) on `predates-fields` is a
  judgment call about scope: the Phase 7 fields at risk of a "zero read as measured" fabrication
  (`lines-written`/`lines-replaced`) live only inside `outcomes` (report-wide) and `RepoOutcomes`
  (per-repo); a narrower per-field redaction was considered and rejected as more code for the same
  reader-facing outcome (the caveat sentence already tells the reader not to trust any of the
  period's outcome figures).
- `prior_path` is threaded into `build_context_block` as `Option<&Path>` rather than a
  pre-loaded `Option<Report>`, so the file read/parse/schema-gate stays inside `render.rs` next to
  the primary input's identical gate, and every call site (`generate_markdown`/`generate_html`)
  stays a one-line pass-through of `cfg.prior.as_deref()`.

### Open questions
- **`--llm cli` failed non-interactively against the real 1,523-session / 131-session windows**
  during manual verification (`claude -p failed (exit 1)`, empty stderr, both with and without
  `--prior`), while `claude -p "say hi"` succeeded directly in the same session. `--llm api` (with
  `ESCOTE_ANTHROPIC_API_KEY` remapped to `ANTHROPIC_API_KEY`) rendered both windows successfully with
  no retry. This reads as an environment/session issue with the `claude` CLI transport under this
  agent's sandboxed/non-interactive process rather than anything Phase 8 touches (the transport
  predates this design), but it is worth a look before relying on `--llm cli` for a real monthly
  render: confirm `claude` behaves the same from a genuinely interactive shell on the same host.

### Verification
- `otto ci`: green (fmt, clippy, check, test, whitespace, file-size bloat check).
- 9 new tests in `report/src/render/tests/prior.rs` (split out of `render/tests.rs` to stay under the
  1500-line file limit, `#[cfg(test)] mod prior;` declared at the bottom of `render/tests.rs`):
  `predates_fidelity_fields` true/false/no-repo-at-all, `--prior` present vs. absent context key,
  `comparable` true vs. false on a length mismatch, the pre-change caveat replacing zeros, a
  wrong-schema `--prior` bail, and a missing-path `--prior` bail.
- Mutation-checked the sharpest test: removed the `predates_fields.is_none()` gate around
  `prior.outcomes` and reran `build_context_block_prior_states_predates_fields_instead_of_zeros` --
  it failed, showing the pre-change artifact's `commits: 3` alongside the real fixture's
  `lines-written`/`lines-replaced` defaults, exactly the fabricated-zero shape the design calls out.
  Restored and reconfirmed green.
- **Live figures**, two real adjacent 30-day windows collected off today's catalog:
  `--since 2026-05-27 --until 2026-06-25` (prior: 131 sessions, `$2,144.58`, 27 repos, 21 commits, 60
  PRs opened) and `--since 2026-06-26 --until 2026-07-25` (current: 1,523 sessions, `$9,450.31`, 57
  repos, 490 commits, 166 PRs opened). Rendered end-to-end via `report render --prior` (`--llm api`,
  since `--llm cli` hit the open question above): the Month over Month section states "both cover 30
  days ... directly comparable", then quotes `$2,144.58` / `131` against `$9,450.31` / `1523`, `21`
  commits / `60` PRs against `490` / `166`, and `27` against `57` repos -- every figure matches the
  two collected artifacts byte-for-byte (checked via `jq` against both `report.json`s), confirming
  the render-invents-nothing guard's implicit proof: nothing was computed, only copied. The model
  also correctly named `tatari-tv/klod` as present in the prior period and absent from the current
  one (confirmed by grepping both artifacts' session repos) and named four repos that newly carried
  work this period, none of them fabricated.
- Reran `report render` on the prior-only artifact WITHOUT `--prior`: the rendered markdown carries
  no "Month over Month" section at all, live-confirming the absence case alongside the unit test.

## Phase 9: Narrative evidence

### Design decisions
- `CollectedSession`/`SessionEntry` (`report/src/report.rs`) each gain `summary: Option<String>` and
  `tags: Vec<String>`, threaded straight through `to_collected` (`report/src/lib.rs`, reading
  `sessions::SessionRecord.summary`/`.tags`, already persisted by the enrich pass) and
  `entry_from_scope` (`report/src/report.rs`) alongside the existing `title` passthrough. Both are
  `#[serde(default, skip_serializing_if = ...)]` (`Option::is_none` / `Vec::is_empty`) so an
  unenriched session's artifact row omits both keys entirely rather than emitting `"summary": null` /
  `"tags": []` for the common case (36.1% enriched on the live window, so most rows take this path).
- `SessionView` (`report/src/render.rs`) gains the same two fields with the same skip rule, so the
  context block carries the identical contrast: an enriched session's row has `summary`/`tags`, an
  unenriched one has neither, and the model can tell the two apart without a sentinel value.
- New top-level `enrichment-coverage: String` context field (`build_enrichment_coverage`,
  `report/src/render.rs`), counted over `report.sessions` -- the SAME collection `sessions[]` in the
  context is built from -- so the quoted "N of M sessions carry an enrich summary" figure can never
  drift from what the model can actually see in the same render. This is deliberately a render-time
  fact, not a copy of `run_collect`'s collect-time `--min-enrichment` warning (Phase 3): the two can
  differ (e.g. a merged report, or a re-render of an older collected JSON), and the context field
  must describe the artifact actually being rendered.
- Both prompt templates (`report.pmt`, `report-html.pmt`) gained the same paragraph naming the
  defect plainly: `title` is Claude Code's own `ai-title`, resolved from a session's OPENING exchange
  alone, and is a LABEL only; `summary` is the enrich pass's digest of the FULL transcript and is the
  evidence a theme claim should cite. Every place either template previously said "session titles" as
  evidence (Hard Prohibition 1's qualitative-narrative clause, the per-repo summary-line format, the
  "Synthesize, don't enumerate" paragraph, the Usage Profile "Model mix" bullet, the Tradeoffs
  citation instruction) now says "summary (falling back to title when a session carries none)"; the
  Outlier Sessions table (`aggregates.outliers`, an `OutlierRow` with no `summary` field -- see
  Deviations) is told to cross-reference the same session's `summary` in `sessions[]` by `short-id`
  rather than gaining a duplicate field.
- `report/src/title.rs` and `report/src/title/tests.rs` deleted (`git rm`); `pub mod title;` removed
  from `report/src/lib.rs`. Nothing called `title::haiku` or `title::extract_prefix` anywhere in the
  tree (confirmed by `rg`), so this is dead-code removal, not a behavior change. The one stray
  reference (`report/src/summarize/api.rs`'s doc comment naming `title::haiku` as the other
  `ANTHROPIC_API_KEY` consumer) is reworded to note the path was removed rather than left dangling.

### Deviations
- The doc's context-field list (API Design section) also names `notes` as a future context addition
  ("`Report.notes` exists today and never reaches the prompt"). That bullet is not assigned to any
  phase in the Implementation Plan table, and the team-lead task scoped Phase 9 to exactly
  `summary`/`tags`/`enrichment-coverage`/the prompt edits/the `title.rs` deletion. Left `notes`
  unaddressed here; it is either a later phase's job or a gap in the plan worth flagging (see Open
  Questions).
- `aggregates.outliers` (`OutlierRow`, `report/src/aggregate.rs`) was NOT extended with a `summary`
  field, even though the design's "Systemic property" paragraph and this phase's narrative-evidence
  goal apply equally to the Outlier Sessions table. The team-lead task scoped this phase to
  `SessionView` (the `sessions[]` context field) specifically; extending a second struct was outside
  that scope. Same effect, correct seam for THIS phase: the outlier table's `short-id` already lets
  the model cross-reference the same session in `sessions[]` for its `summary`, so both templates
  were worded to do that rather than claim a field that does not exist. Flagged as an open question
  for whichever phase (or a follow-up) is authorized to touch `OutlierRow`.

### Tradeoffs
- `enrichment-coverage` is a single pre-formatted display string (matching the house
  `by-skill-coverage`/`by-mcp-coverage` precedent) rather than a structured
  `{enriched, total, coverage}` object. A structured object would let the prompt quote the raw counts
  independently of the sentence wording, but every other coverage-style field in this codebase is a
  single string the model quotes verbatim, and splitting this one would be an unrequested
  inconsistency for no reader-facing gain.
- Considered computing `enrichment-coverage` from the ORIGINAL catalog window (mirroring
  `run_collect`'s `enrichment_warning` in `lib.rs` exactly) instead of from `report.sessions` at
  render time. Rejected: a merged report or a re-render of an older `report.json` has no live catalog
  to re-query, and the render-time count is the one guaranteed to match what the model is actually
  looking at in `sessions[]` for THIS render, which is the property the field exists to guarantee.

### Open questions
- Should `Report.notes` become a context field in a later phase (or is it already covered by a phase
  not yet implemented)? It is in the design's consolidated API-Design field list but not assigned a
  phase number in the Implementation Plan table.
- Should `OutlierRow` gain its own `summary` field in a later phase, so the Outlier Sessions
  table's "What it produced" column can cite it directly instead of requiring the model to
  cross-reference `sessions[]` by `short-id`? Left as a cross-reference in this phase per its scope.
- **Context block size grew substantially more than the doc's Performance section anticipated.**
  Measured on the real 30-day window (`--since 2026-06-26 --until 2026-07-25`, 1,523 sessions,
  550 enriched, 36.1% coverage): the context block WITH `summary`/`tags`/`enrichment-coverage` is
  942,127 bytes; the same block with those three additions stripped back out (simulating
  pre-Phase-9) is 636,515 bytes -- a 305,612-byte (48%) increase, not the "roughly cancel" the doc
  predicted against dropping `<synthetic>` (that drop already shipped in Phase 6, before this
  measurement). Using a crude bytes/4 approximation (not a real tokenizer -- none is vendored here),
  that is approximately 159,000 tokens before and 236,000 tokens after. `render.markdown-max-output-
  tokens` (default 32,000) governs the model's OUTPUT length, not the input context, so this growth
  does not trip that ceiling directly -- but a context block this large is worth checking against the
  actual model's context-window budget (alongside the system prompt and the rest of the request)
  before relying on this render for a full month at low enrichment coverage. Left as an open question
  rather than a size guard, since the design did not ask for one in this phase and 500K-char-summary
  sessions are the design's own committed shape (Phase 2 catalog work, `2026-07-24` design), not
  something Phase 9 introduced a defect in.
