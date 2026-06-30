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
