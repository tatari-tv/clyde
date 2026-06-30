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
