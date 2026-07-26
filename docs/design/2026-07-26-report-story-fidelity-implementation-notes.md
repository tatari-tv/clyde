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
