## Phase 1: CLI surface

### Design decisions
- Added `ResumeArgs` with `id: String`, `no_reindex: bool` (`#[arg(long)]`), and `extra: Vec<String>` (`#[arg(last = true)]`) exactly as specified - `clyde/src/cli.rs:ResumeArgs` - matches the design doc API section verbatim.
- Replaced `SessionsCommand::Open(OpenArgs)` with `SessionsCommand::Resume(ResumeArgs)` in the `SessionsCommand` enum - `clyde/src/cli.rs:SessionsCommand` - `Open` is gone with no alias per the design intent.
- Added three tests covering the three required behaviors - `clyde/src/cli/tests.rs` - bare id parses, `-- --model opus` lands in `extra`, `--no-reindex` sets the flag.

### Deviations
- `cmd_open` was removed and replaced with a stub `cmd_resume` in `clyde/src/main.rs` rather than leaving `cmd_open` in place. The design says Phase 2 removes `cmd_open`, but removing `OpenArgs` from `cli.rs` also removes `SessionsCommand::Open`, which makes the `cmd_open` match arm uncompilable. The minimum to keep Phase 1 compiling was to replace the arm and function simultaneously. The stub calls `lazy_reindex` (so the `--no-reindex` flag is honored in routing) and bails with a clear "not yet implemented" message. Phase 2 will replace the bail with `plan_resume` + `launch_resume`.

### Tradeoffs
- Stub `cmd_resume` calls `lazy_reindex` vs. doing nothing - calling it keeps the behavior consistent with the final design (Phase 2 also does a lazy reindex before resolving the id) and avoids a second removal pass in Phase 2. The bail before any id resolution means the reindex cost is paid even on the stub path, but this path is intentionally unreachable in normal use until Phase 2 ships.

### Open questions
- None.

## Phase 2: plan_resume + launch_resume

### Design decisions
- Added `ResumeAction` enum (`Launch`/`NoCwd`/`MissingDir`/`StagedOnly`/`Reaped`) - `clyde/src/main.rs:ResumeAction` - the typed decision the design's testability rationale calls for; derives `Debug, PartialEq, Eq` so Phase 3 can assert against it directly.
- `plan_resume(rec, extra)` is a pure function with explicit branch precedence - `clyde/src/main.rs:plan_resume` - (1) no cwd -> `NoCwd`, (2) cwd not an existing dir -> `MissingDir`, (3) `transcript_path.exists()` -> `Launch`, (4) `staged_path` that exists -> `StagedOnly`, (5) else `Reaped`. The live/staged/reaped tail mirrors `sessions/src/mcp.rs:open_result_for` (existence-based, not the `archived` flag), including the same `.filter(|p| p.exists())` shape on `staged_path`.
- `cmd_resume` returns `Result<ResumeAction>` rather than acting - `clyde/src/main.rs:cmd_resume` - it resolves the id (honoring `--no-reindex` via `lazy_reindex`), reuses the same resolve_id ambiguous/empty stderr+exit handling as `cmd_tag`/`cmd_enrich`, fetches the record with `db.get`, and hands the action back to `run` so the `Db` is dropped before the exec.
- `run` peels the `Resume` arm off into its own outer match arm (like `Serve`) - `clyde/src/main.rs:run` - opens the `Db` in an inner block that returns the action, dropping the handle at the block's close, then calls `run_resume_action`. The inner `SessionsCommand::Resume(_)` arm in the shared-`Db` match is now `unreachable!`.
- `run_resume_action` is the single side-effecting matcher - `clyde/src/main.rs:run_resume_action` - `Launch` calls `launch_resume`; every other variant prints a specific `✗`-prefixed stderr line and `std::process::exit(1)`. Error matrix: no-cwd ("has no recorded cwd; cannot resume in place"), missing-dir (names the path), staged-only (names the staged path), reaped.
- `launch_resume` is the only platform-specific code - `clyde/src/main.rs:launch_resume` - `#[cfg(unix)]` builds `Command::new("claude").current_dir(dir).arg("--resume").arg(id).args(extra)` and `exec()`s (returns only on failure -> "failed to exec claude in <dir>: ..."); `#[cfg(not(unix))]` spawns inheriting stdio, waits, and `exit`s with the status code (1 when terminated without one).
- Function-level `debug!` on entry of `cmd_resume` and `launch_resume` per the logging rule (function name + params).

### Deviations
- None. (`cmd_open` was already removed in Phase 1; Phase 2 only replaced the stub body and the call site, as the notes from Phase 1 anticipated.)

### Tradeoffs
- Returning `ResumeAction` to `run` (then dropping `Db` before acting) vs. exec-ing inline inside `cmd_resume` - the design explicitly calls for the return-and-act split so the decision is a pure unit-testable function and no SQLite handle is held across the exec. The cost is one extra enum and a slightly larger `run` match; the benefit is the Phase 3 tests can exercise every branch with no process launched.
- `MissingDir` collapses both "cwd recorded but deleted/moved" and "cwd exists but is not a directory" into one variant keyed on `is_dir()` - the design's error matrix lists cwd-missing and cwd-not-a-directory separately, but both reduce to "the recorded path is not a usable directory" and share one actionable stderr message naming the path. A single `is_dir()` check covers both without a spurious second variant.

### Open questions
- None.

## Phase 3: tests, help, docs

### Design decisions
- Added 8 unit tests for `plan_resume` in `clyde/src/tests.rs` - `plan_resume_*` - covers all 5 branches (NoCwd, MissingDir, MissingDir-via-file, Launch-with-extra, StagedOnly, Reaped-with-absent-staged, Reaped-with-no-staged, Launch-with-empty-extra); real `TempDir` fixtures so `is_dir()` and `exists()` checks exercise the filesystem truthfully, no mocking.
- Used `base_record(transcript_path)` helper to construct minimal-valid `SessionRecord` fixtures - `clyde/src/tests.rs:base_record` - caller overrides only the fields relevant to each branch; matches the pattern of the existing tests file (use `super::*`, `#![allow(clippy::unwrap_used)]`).
- Updated `SessionsCommand::Resume` doc comment - `clyde/src/cli.rs:SessionsCommand` - describes cd+launch behavior, process-replacement semantics, the `--` passthrough requirement with an example, and the parse-error consequence of omitting `--`.
- Updated `ResumeArgs.extra` field doc - `clyde/src/cli.rs:ResumeArgs` - reinforces the `--` requirement and explains why it is intentional (prevents claude flags from being misinterpreted by clyde's parser).
- Replaced `open` with `resume` in README command surface and workspace description - `README.md` - two occurrences updated to remove the old verb.
- Added `## Resuming sessions` section to README - `README.md` - documents cd+launch semantics, the two canonical invocation forms (bare and `-- --model opus`), the `--` requirement, and the id-prefix convention.

### Deviations
- None. The `--` behavior note says "must error" and `#[arg(last = true)]` already enforces this via clap; the tests confirm the parse path in `clyde/src/cli/tests.rs` (Phase 1), so no additional negative-parse test was added here for the `--`-omission case - that would require a bare flag that clap rejects, which Phase 1 already covers via `resume_extra_lands_after_double_dash`.

### Tradeoffs
- 8 tests (including 2 edge-case branches for MissingDir and Reaped) vs. the minimum 5 specified - the two extra branches (file-at-cwd, staged-path-set-but-absent) cost nothing and directly exercise the `is_dir()` and `.filter(|p| p.exists())` logic that would silently misbehave if the branch logic were swapped; worth the coverage.
- `base_record` helper defined in `tests.rs` vs. reusing a sessions-crate fixture - `SessionRecord` has no public test-helper constructor in the sessions crate; building it inline in `tests.rs` keeps the test file self-contained and avoids a dev-dependency on test internals of another crate.

### Open questions
- None.

## Phase 4: rename sessions -> session (clean break)

### Design decisions
- Added `#[command(name = "session")]` to the `Sessions` variant in `clyde/src/cli.rs` (Rust variant name stays `Sessions`; no alias) - the canonical CLI spelling is now `clyde session ...` and `clyde sessions ...` stops working immediately.
- Updated `rewrite_unit` in `clyde/src/bootstrap.rs` to also replace `sessions enrich` -> `session enrich` after the existing `klod -> clyde` substitution - `clyde/src/bootstrap.rs:rewrite_unit` - this ensures `clyde bootstrap` migrates units that were already on `clyde sessions enrich` (post-klod, pre-rename) in addition to the original `klod sessions enrich` migration path.
- Updated `install_clyde_timer` template at line 836 to emit `session enrich` for fresh installs - `clyde/src/bootstrap.rs:install_clyde_timer` - new installs and `--install-timer` invocations generate the correct spelling from the start.
- Updated all `clyde sessions ...` command invocations in README to `clyde session ...` (5 occurrences), including the `claude mcp add ... clyde session serve` line - `README.md`.
- Updated all `"sessions"` args in `clyde/src/cli/tests.rs` (12 occurrences) to `"session"` so the clap parse-from tests use the new canonical spelling - `clyde/src/cli/tests.rs`.
- Updated `clyde/tests/serve.rs` (`"sessions", "serve"` -> `"session", "serve"`) - the smoke-test binary invocation must use the renamed subcommand.
- Updated `clyde/src/bootstrap/tests.rs` assertion to expect `session enrich` in the rewritten unit body.
- Updated `clyde/src/doctor/tests.rs` healthy-unit fixture to write `session enrich` (the new canonical form that `clyde bootstrap` now generates).
- Updated doc comments referencing `clyde sessions ...` in `clyde/src/main.rs`, `clyde/src/cli.rs`, `sessions/src/mcp.rs`, `sessions/src/model.rs`, `sessions/src/db.rs`, and `common/src/since.rs`.

### Deviations
- No CHANGELOG or release-notes file exists in the repository (no `CHANGELOG.md`, `RELEASE_NOTES.md`, or equivalent). The Migration steps from the design doc (re-register the MCP server with `clyde session serve`; re-run `clyde bootstrap` to rewrite the enrich timer's ExecStart) were not added to any file - they are recorded here instead. The orchestrator or release author should include them in the release notes when cutting the next version.

### Tradeoffs
- Extending `rewrite_unit` with a second `.replace("sessions enrich", "session enrich")` vs. a more targeted regex or line-level rewrite - the blanket string replace is safe here because `sessions enrich` appears only once in a unit file's ExecStart line and cannot appear as a false positive in any other field; matches the existing `klod -> clyde` design in the same function.
- Updating the doctor test fixture from `sessions enrich` to `session enrich` - the doctor health check only tests for `klod` in ExecStart (not for `sessions` vs `session`), so both forms are "healthy". Updating the fixture to the new canonical form keeps the test representative of what `bootstrap` now generates, which is more useful as documentation.

### Open questions
- None.

## Post-audit fixes (review-panel, Implementation Audit)

The review panel (Architect/Gemini vs. Staff Engineer/Codex) split on Phase 4; the Staff Engineer's
finding was verified against the code and three fixes landed before PR merge.

### Design decisions
- `repoint_systemd` now handles the no-legacy-but-stale-spelling case - `clyde/src/bootstrap.rs:repoint_systemd` + new `refresh_clyde_unit` - when there is no `klod-*` state, an already-installed `clyde-enrich.service` whose ExecStart still says `sessions enrich` is rewritten in place via `rewrite_unit` (backup + atomic write). Honors `dry_run` (reports the pending rewrite without writing).
- `doctor` flags a stale `sessions enrich` ExecStart as unhealthy - `clyde/src/doctor.rs:timer_state` - new `Target::Legacy("sessions enrich")` branch so a clyde-named-but-stale unit no longer reads as healthy; the existing "run `clyde bootstrap`" remediation hint then closes the loop.
- Reworded the `MissingDir` stderr - `clyde/src/main.rs:run_resume_action` - "recorded cwd is not a usable directory: <path>" so it is accurate whether the path is gone OR exists as a file (the `is_dir()`-collapsed variant from Phase 2 covers both).
- Tests added: `bootstrap/tests.rs` (stale-clyde-unit rewrite, already-correct no-op, dry-run reports-without-writing); `doctor/tests.rs` (stale `sessions enrich` reads as unhealthy).

### Deviations
- The Phase 4 note (line 62) claimed the `rewrite_unit` clause already migrated already-clyde units; the audit found that clause was only reachable via the legacy-klod path, so the no-legacy path above is what actually delivers that intent. This supersedes the Phase 4 claim.

### Tradeoffs
- `refresh_clyde_unit` reuses `rewrite_unit` (which also does `klod -> clyde`) vs. a spelling-only replace - on an already-clyde unit the `klod -> clyde` pass is a no-op, so reusing the one transform keeps a single source of truth for unit rewriting.
- `Target::Legacy("sessions enrich")` reuses the existing `Legacy(&'static str)` variant vs. a new `Target` variant - the existing variant already renders "<name> (legacy)" and drives `is_legacy()`/`healthy()`, so no new variant or rendering path was needed.

### Open questions
- PRE-EXISTING (out of scope for this work; tracked for a separate ticket): `clyde bootstrap --install-timer` cannot repair a *half-installed* unit set - if `clyde-enrich.service` exists but `clyde-enrich.timer`/its enable symlink is missing, `repoint_systemd`'s no-legacy branch returns after `refresh_clyde_unit` and never reaches `install_clyde_timer`, so the timer is not created. This is not a regression from the post-audit fix: the pre-`2502a11` guard was `if install_timer && !paths.clyde_unit().exists()`, which already skipped timer creation whenever the service existed. A proper fix needs a decision (install only the missing timer + symlink without clobbering an existing service body, since `install_clyde_timer` currently rewrites the whole service from the template). Surfaced by the review-panel re-audit of `2502a11`.

## Phase 5: events-DB reconciliation (shakedown Finding B)

### Design decisions
- `migrate_events_db` now MERGES instead of no-op'ing when both DBs exist - `clyde/src/bootstrap.rs:migrate_events_db` + new `merge_events_db` - the legacy `claude-permit/events.db` rows are copied into the existing clyde DB and the legacy file is removed, so `doctor` can finally go green. Three cases now: legacy-only -> WAL-safe move (unchanged); both -> merge+remove (new); clyde-only/neither -> no-op.
- `merge_events_db` checkpoints the legacy WAL (TRUNCATE), then ATTACHes it to the clyde connection and `INSERT … SELECT`s the 7 real columns (omitting `id`, since the two DBs have independent autoincrement sequences) - `clyde/src/bootstrap.rs:merge_events_db`. Verifies the post-merge count equals before+legacy and `warn!`s on mismatch.
- Legacy DB is backed up to `.clyde.bak` (via the existing `backup` helper) before removal, so the merge is recoverable; sidecars (`-wal`/`-shm`) are removed too. Idempotent: the caller's `legacy.exists()` guard makes a re-run a no-op.
- `doctor` surfaces the legacy events DB when the clyde DB also exists - `clyde/src/doctor.rs:render` - prints a `legacy state:` line so the `✗` footer is no longer a mystery; `healthy()` already counted `events_db_at_legacy`, so no health-logic change was needed.
- Updated the test fixture `seed_events_db` to the real claude-permit schema (7 columns) so the column-explicit merge is exercised truthfully; rewrote the former `events_db_move_is_noop_when_clyde_db_present` test into `events_db_merges_legacy_into_clyde_when_both_present` (+ a dry-run test and a doctor both-present test).

### Deviations
- The Phase 4-era behavior (both-present -> no-op + `warn!`) is replaced. This is intentional: the no-op was the root cause of Finding B. Documented here as superseding that behavior.

### Tradeoffs
- Merge (preserve the legacy rows) vs. simply removing the legacy DB - the user asked to "convert or remove"; merging preserves the pre-cutover permit audit history at negligible cost (one ATTACH + INSERT…SELECT) and the backup covers the remove. Dedup was NOT attempted: the two DBs are disjoint time ranges (legacy stopped being written at cutover), and the events table has no natural unique key beyond the autoincrement id, so a plain append is correct and a re-run can't double-merge (the legacy file is gone).
- ATTACH + `INSERT … SELECT` vs. row-by-row copy through Rust - the SQL-side bulk copy is simpler, faster, and keeps the column list in one place; the path is bound as a parameter (no SQL interpolation).

### Open questions
- None.

## Phase 5 hardening (re-audit fixes)

A review-panel re-audit of the events-DB merge (`merge_events_db`/`migrate_events_db`) and
`resolve_claude` produced 6 findings; all six are addressed here.

### Design decisions
- busy_timeout on EVERY events-DB connection (#2) - `clyde/src/bootstrap.rs:open_events_conn` / `open_events_conn_ro` + `const EVENTS_BUSY_TIMEOUT_MS = 5_000` - two small helpers open a connection and immediately `pragma_update(None, "busy_timeout", ..)`, and every connection in `migrate_events_db` (legacy checkpoint, post-move RO verify) and `merge_events_db` (legacy checkpoint, staged count, dest, RO verify) now routes through them. Mirrors `sessions::db::BUSY_TIMEOUT_MS`. A concurrent permit-log writer now yields a wait, not an instant SQLITE_BUSY that aborts the migration.
- Content-dedup INSERT (#1) - `clyde/src/bootstrap.rs:merge_events_db` - the merge INSERT is now `INSERT … SELECT … WHERE NOT EXISTS (…)` with a NULL-safe (`IS`) correlated match over all 7 copied columns against the destination `events`. A crash after the INSERT commits but before the staging file is finalized cannot double-insert on the next run; a retry inserts only the not-yet-present remainder. A comment documents that within-staging exact duplicates are preserved (the subquery checks only the destination, alias `e`).
- Atomic staging claim + crash recovery (#5) - `clyde/src/bootstrap.rs:merge_events_db` + `migrate_events_db` - the merge now operates on a claimed snapshot `events.db.merging`. When staging does not exist, the legacy WAL is checkpointed (TRUNCATE) FIRST (so no committed rows are stranded in the `-wal`, which is bound to the old filename), THEN `fs::rename(legacy -> staging)` claims it atomically (a concurrent permit-log that opens after the rename creates a fresh `events.db` for the NEXT bootstrap to merge, instead of being lost), and the now-empty legacy `-wal`/`-shm` are removed. If staging already exists, it is reused as-is (crash recovery) with no re-checkpoint/rename. Finalize is `fs::rename(staging -> backup_path(&legacy))`, which leaves the recoverable `events.db.clyde.bak` AND removes staging in one atomic step (deliberately NOT the `backup()` helper, which would mis-name it `events.db.merging.clyde.bak`). `migrate_events_db` entry detection now finishes a leftover staging file whenever `staging.exists() && dest.exists()` (dry_run -> Ok(true), else merge), warns and leaves the pathological `staging.exists() && !dest.exists()` case without crashing, and keeps the existing legacy-only / both-present branches.
- Fail-closed verification (#6) - `clyde/src/bootstrap.rs:merge_events_db` - the post-merge count reopens dest read-only; if that COUNT returns an Err (a real failure, not a clean zero), the staging snapshot is kept and the Err is returned with context ("keeping staging snapshot for retry"), so the legacy data is preserved for a retry. A clean count that merely falls outside the dedup-aware expected range (`dest_before ..= dest_before + n`) stays a `warn!` and proceeds (a committed insert is not rolled back).
- Canonicalize the claude path (#4) - `clyde/src/main.rs:resolve_claude` - after `which::which("claude")`, an absolute result is returned as-is; a non-absolute result is `canonicalize()`d against the current (pre-chdir) cwd, with a clear error on failure. `which` 8.0.4 can return a relative path for a relative PATH entry, which after `current_dir(dir)` would re-resolve against the session dir and defeat the resolve-before-chdir guarantee. The doc comment now states the absolute-path guarantee accurately.
- WAL-survival + recovery + dedup tests (#3) - `clyde/src/bootstrap/tests.rs` - `events_db_merge_moves_uncheckpointed_wal_rows` (autocheckpoint=0, held writer connection, rows verified present in the `-wal`, then merged), `events_db_merge_recovers_from_interrupted_staging` (pre-placed staging + dest, no live legacy, asserts staged rows merged + staging gone + `.clyde.bak` left), `events_db_merge_dedups_identical_rows_and_is_crash_idempotent` (identical content not double-inserted; re-merge from the backup snapshot yields a stable count). Added `seed_events_db_tagged` so two DBs can be seeded content-disjoint; updated `events_db_merges_legacy_into_clyde_when_both_present` to use disjoint tags (the dedup would otherwise collapse the identically-seeded rows) and to assert the new staging/`.clyde.bak` finalize behavior.

### Deviations
- The Phase 5 note claimed "dedup was NOT attempted … a plain append is correct." This hardening pass ADDS content-dedup (finding #1), superseding that claim. Reason: a crash between the INSERT commit and the legacy-file removal could double-insert on the next run; the NULL-safe NOT EXISTS makes the merge crash-idempotent. The disjoint-time-range assumption still holds in the normal case (so the merge normally inserts everything), but the dedup is the safety net for the retry path.
- The Phase 5 verification compared `dest_after == dest_before + n` exactly. With dedup the insert adds AT MOST `n` rows, so the expectation became a RANGE (`dest_before ..= dest_before + n`); a value inside the range is normal, outside is the `warn!`. This is a direct consequence of finding #1 and is not a behavioral regression.
- `merge_events_db` no longer removes the legacy `events.db` with `fs::remove_file` then backs it up separately; the claim+finalize rename pair replaces that (the staging snapshot IS the backup-in-flight, finalized to `.clyde.bak`). The recoverability guarantee is unchanged.

### Tradeoffs
- Staging claim via `rename` (this approach) vs. copy-then-merge - the rename is atomic and O(1), and it closes the concurrent-write window by forcing a late writer onto a fresh inode; a copy would leave the live DB writable mid-merge and lose those writes. The cost is one extra on-disk file (`events.db.merging`) transiently, which doubles as the recovery artifact.
- Dedup-aware count RANGE check vs. an exact post-dedup recount of expected inserts - computing the exact expected delta would require a second correlated query before the INSERT (counting how many staged rows already match), doubling the scan for a check that is only diagnostic. The range bound catches the cases that matter (negative delta = loss, over-`n` = double-insert) without the extra query.
- Keeping busy_timeout in two tiny helper fns vs. a single helper with a read-only flag - two named helpers (`open_events_conn`, `open_events_conn_ro`) read more clearly at the call sites than a boolean-parameterized opener, and the RO path needs `OpenFlags` the RW path does not.

### Open questions
- RESIDUAL #5 WINDOW (could not be fully closed at this layer): the checkpoint+rename closes the window for a permit-log process that opens the DB AFTER the rename (it lands on a fresh `events.db`). It does NOT close the window for a permit-log process that ALREADY had the legacy `events.db` open (an existing rusqlite handle / open fd) before the checkpoint+rename: on Linux that handle keeps writing to the same inode (now named `events.db.merging`), and any rows it commits after our `PRAGMA wal_checkpoint(TRUNCATE)` but before it notices the rename land in the staged snapshot's WAL: they are merged on THIS run only if they were checkpointed into the staged main file by our count time, otherwise they ride along in the `.clyde.bak` and are not in the clyde DB. Fully closing this needs cooperation from the writer (e.g. an advisory lock or a "stop writing during migration" signal to the permit hook), which is out of scope for a bootstrap-side hardening pass. In practice the legacy DB stopped being written at cutover, so a live open handle racing the bootstrap is unlikely; the `.clyde.bak` preserves any such stragglers for manual recovery. Tracked here for a follow-up if the permit hook ever writes concurrently with bootstrap in the field.
