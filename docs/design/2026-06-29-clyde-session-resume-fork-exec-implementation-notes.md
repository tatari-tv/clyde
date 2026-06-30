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
