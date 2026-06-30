# Design Document: `clyde session resume` - launch a session in its directory via fork/exec

**Author:** Scott Idler
**Date:** 2026-06-29
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

Replace `clyde sessions open` with a new `clyde session resume <id>` verb that opens a Claude
Code session in the directory it originally ran in, in one command and with no shell plumbing.
clyde resolves the session's recorded `cwd`, `chdir`s into it, and `exec`s `claude --resume
<id>` in place (process replacement, like `env -C`, `chroot`, `aws-vault exec`): no zsh
function, no `.zshrc` edit, no symlink. The old `open` verb (which only printed a
`claude --resume <id>` line) is removed entirely.

This doc also renames the parent subcommand from `sessions` to `session` (so `clyde session
resume <id>` reads naturally) as a clean break with no alias.

## Problem Statement

### Background

Claude Code stores each session's transcript under `~/.claude/projects/<slug>/`, where `<slug>`
is derived from the directory the session ran in (the cwd, slugified). `claude --resume <id>`
only finds a session when invoked from that original directory; run from anywhere else it
reports `No conversation found with session ID: <id>`.

clyde already catalogs every session and stores the original directory: `SessionRecord.cwd`
(`sessions/src/model.rs:16`, `Option<String>`) holds the first cwd seen in the transcript, and
`project_dir` holds the slug. So clyde knows exactly where each session must be resumed from.

Today the only resume-adjacent command is `clyde sessions open <id>` (`clyde/src/main.rs:225`,
`cmd_open`), which resolves the id and prints `claude --resume <id>` to stdout. It never uses
`cwd`, so copying that line and running it from the wrong directory fails. There is no command
that actually opens the session, and a verb named `open` that only prints is the wrong shape.

### Problem

A user who knows a valid session id cannot reliably open it: the resume only works from the
original directory, and no clyde command moves them there and launches it.

### Goals

- `clyde session resume <id>` opens the session in its original directory in one step.
- No shell function, no `.zshrc` change, no symlink, no per-machine setup. Works the instant the
  binary is installed.
- Forward arbitrary flags to `claude` (e.g. `--model opus`).
- Clear, actionable errors when the directory is unknown or gone.
- Remove the `open` verb entirely; `resume` is its replacement.
- Rename the parent subcommand `sessions` to `session` for readability (clean break, no alias).

### Non-Goals

- Leaving the user's interactive shell parked in the session's directory after `claude` exits.
  Process replacement returns the user to their original shell/cwd by design. The shell-wrapper
  alternatives (below) are the only way to relocate the shell itself, and are out of scope here.
- A `clyde shell-init` / `eval "$(...)"` integration. That is the separately-documented approach
  for the `clone` repo; this design is the deliberate no-shell counterpart.
- Keeping any "print the resume line" behavior. `open` is gone; there is no replacement printer.
- Resuming a session whose live transcript is gone (TTL-reaped). There is no live session for
  `claude --resume` to attach to.

## Proposed Solution

### Overview

Remove `SessionsCommand::Open` / `OpenArgs` / `cmd_open`, and add
`clyde session resume <id> [-- <extra args>]`:

1. Resolve the id, fetch the `SessionRecord`.
2. Decide what to do by transcript **existence** (not the `archived` flag - see below).
3. If the live transcript exists: `chdir` into `cwd` and `exec` `claude --resume <id> [extra]`.
   clyde is replaced by `claude`; on exit the user is back at their original shell prompt and
   cwd.
4. Otherwise: a clear error (staged-only or fully reaped); nothing is launched.

### Decide by existence, not `archived`

The existing MCP path already makes the live / staged / reaped decision by **file existence,
not the `archived` flag** (`sessions/src/mcp.rs:139`, whose comment says exactly this), because
`archived` can be stale and the lazy reindex is skippable (`--no-reindex`) or can fail. `resume`
reuses that same existence-based logic so it cannot disagree with the MCP path:

- `transcript_path` exists -> resumable (launch).
- else `staged_path` exists -> staged copy only; not resumable by `claude --resume`; error with
  the staged path.
- else -> fully reaped; error.

### CLI rename: `sessions` -> `session` (clean break)

Rename the canonical parent subcommand to the singular `session` so the verbs read naturally
(`clyde session resume`, `clyde session search`, `clyde session ls`). This is a **clean break:
no alias**. `clyde sessions ...` stops working. Two live integrations reference the old spelling
and must be re-pointed once (see Migration):

- The MCP server registration `claude mcp add clyde -s user -- clyde sessions serve`
  (`README.md:82`).
- The enrich systemd user timer `ExecStart=...clyde --log-level info sessions enrich`
  (`bootstrap.rs:836`).

In clap derive, rename the variant's command name (the Rust variant stays `Sessions`; the
`sessions` crate name is unrelated and unchanged):

```rust
// clyde/src/cli.rs, the Command enum variant:
/// Catalog, search, and resume sessions.
#[command(name = "session")]
Sessions {
    #[command(subcommand)]
    command: SessionsCommand,
},
```

### Architecture

Confined to the clyde shell crate (`clyde/src/main.rs`) plus its clap structs
(`clyde/src/cli.rs`). No library-crate changes; `sessions::Db` already exposes `resolve_id` and
`get`.

To keep the SQLite handle from being inherited across the exec and to make the decision
testable, `cmd_resume` does NOT exec inline. It computes a `ResumeAction` from the record and
returns it; `main` drops the `Db`, then performs the action. (rusqlite opens with `O_CLOEXEC`,
so the fd would not actually leak, but returning the action makes that explicit and makes the
decision a pure, unit-testable function.)

```rust
enum ResumeAction {
    Launch { dir: PathBuf, id: String, extra: Vec<String> },
    NoCwd { id: String },
    MissingDir { dir: PathBuf },
    StagedOnly { staged: PathBuf },
    Reaped,
}

/// Pure decision: maps a resolved record to what resume should do. Unit-tested directly.
fn plan_resume(rec: &SessionRecord, extra: Vec<String>) -> ResumeAction { ... }
```

`main` then matches: `Launch` -> `launch_resume`; the rest print a specific stderr message and
exit non-zero. `launch_resume` is the only platform-specific code:

```rust
/// Replace the clyde process with `claude --resume <id> [extra...]`, running in `dir` so Claude
/// resolves the session's ~/.claude/projects/<slug>. On unix this never returns on success (exec
/// replaces the image); it returns only if claude could not be launched.
#[cfg(unix)]
fn launch_resume(dir: &Path, id: &str, extra: &[String]) -> Result<()> {
    use std::os::unix::process::CommandExt;
    debug!("launch_resume: dir={} id={} extra={:?}", dir.display(), id, extra);
    let mut cmd = std::process::Command::new("claude");
    cmd.current_dir(dir).arg("--resume").arg(id).args(extra);
    let err = cmd.exec(); // returns only on failure
    Err(eyre::eyre!("failed to exec claude in {}: {err}", dir.display()))
}

/// Non-unix: no exec. Spawn claude inheriting stdio, wait, and exit with its status code (or a
/// fixed non-zero when terminated without a code).
#[cfg(not(unix))]
fn launch_resume(dir: &Path, id: &str, extra: &[String]) -> Result<()> { ... }
```

`Command::current_dir(dir).exec()` performs the `chdir` before `execvp` (verified against std's
unix impl), so claude starts in `dir`.

### Data Model

No schema change. Reuses existing `SessionRecord` fields:

- `cwd: Option<String>` - the directory to resume in.
- `transcript_path: PathBuf` - existence gates "resumable".
- `staged_path: Option<PathBuf>` - existence gates "staged only".

### API Design

`clyde/src/cli.rs` - remove `SessionsCommand::Open(OpenArgs)`; add `Resume(ResumeArgs)`:

```rust
/// Open (cd + launch) a session in its original directory.
Resume(ResumeArgs),

#[derive(clap::Args, Debug)]
pub struct ResumeArgs {
    /// Session id or a unique prefix of it.
    pub id: String,
    /// Skip the lazy reindex before resolving.
    #[arg(long)]
    pub no_reindex: bool,
    /// Extra args forwarded verbatim to `claude` after `--resume <id>`, e.g.
    /// `clyde session resume <id> -- --model opus`.
    #[arg(last = true)]
    pub extra: Vec<String>,
}
```

`#[arg(last = true)]` binds everything after a literal `--` into `extra`, so clyde never parses
claude's flags. Note this means the `--` is required: `clyde session resume <id> --model opus`
(no `--`) will NOT work and must error clearly. Invocations:

```
clyde session resume 3bc0a20d                  # cd + launch, default model
clyde session resume 3bc0a20d -- --model opus  # cd + launch, forwarding --model opus
```

### Implementation Plan

#### Phase 1: CLI surface
**Model:** sonnet
- Remove `SessionsCommand::Open(OpenArgs)` and the `OpenArgs` struct.
- Add `SessionsCommand::Resume(ResumeArgs)` with `id`, `no_reindex`, `extra` (`last = true`).
- `cli/tests.rs`: drop the open tests; `resume <id>` parses; `-- --model opus` lands in `extra`;
  bare `resume <id>` leaves `extra` empty.

#### Phase 2: plan_resume + launch_resume
**Model:** opus
- Remove `cmd_open`.
- Add `ResumeAction` enum and the pure `plan_resume(rec, extra)` decision (existence-based).
- Add `launch_resume` (`#[cfg(unix)]` exec + `#[cfg(not(unix))]` spawn-and-wait fallback).
- `cmd_resume` resolves the id, calls `plan_resume`, returns the action to `main`; `main` drops
  `Db` and executes it.
- Error matrix, each to stderr with a non-zero exit: no-cwd, cwd-missing, cwd-not-a-directory,
  staged-only (name the staged path), reaped, `claude` exec failure.
- Function-level `debug!` on entry of `cmd_resume` / `launch_resume`.

#### Phase 3: tests, help, docs
**Model:** sonnet
- Unit-test `plan_resume` across every branch (live, staged-only, reaped, no-cwd, missing-dir)
  with no process launched.
- `resume`'s `about` / `--help` describe cd + launch and the `--` passthrough rule.
- README: replace `open` docs with `clyde session resume <id>`.

#### Phase 4: rename `sessions` -> `session` (clean break)
**Model:** sonnet
- `#[command(name = "session")]` on the `Sessions` variant (no alias).
- `bootstrap.rs`: change the generated `ExecStart` to `... session enrich`.
- README: update all `clyde sessions ...` to `clyde session ...`, including the
  `claude mcp add ... clyde session serve` line.
- Add the Migration steps (below) to the release notes.

#### Phase 5: events-DB reconciliation in bootstrap (shakedown follow-on)
**Model:** opus
- Surfaced by the local shakedown (`docs/shakedown-session-resume.md`, Finding B): when BOTH the
  clyde events DB and the legacy `claude-permit/events.db` exist, `migrate_events_db` was a no-op,
  so the legacy DB was stranded forever and `clyde doctor` stayed red with no on-screen cause -- and
  its own remediation (`clyde bootstrap`) could never clear it.
- `bootstrap.rs`: when both DBs exist, MERGE the legacy rows into the clyde DB (identical schema;
  insert with fresh autoincrement ids) and remove the legacy DB (backed up to `.clyde.bak` first).
  Idempotent: once the legacy DB is gone a re-run is a no-op. Dry-run reports the merge, writes
  nothing.
- `doctor.rs`: surface the legacy events DB explicitly when the clyde DB also exists (it already
  counted toward `healthy() == false`, but the display hid it).

## Migration (clean break)

After installing the renamed binary, the old spelling is dead. One-time fixes:

- MCP server: `claude mcp remove clyde` then
  `claude mcp add clyde -s user -- clyde session serve`.
- Enrich timer: re-run `clyde bootstrap` so the unit's `ExecStart` is rewritten to
  `... session enrich` (or edit the unit and `systemctl --user daemon-reload`).

## Alternatives Considered

### Alternative 1: zsh wrapper (static `shell-functions.sh` + symlink)
- **Description:** A `clyde()` shell function symlinked into `~/.shell-functions.d/` that `eval`s
  a `cd '<cwd>' && claude --resume <id>` line emitted by the binary.
- **Pros:** Leaves the user's shell in the session directory after exit.
- **Cons:** Per-machine setup; the static file drifts from the binary; brittle argv parsing.
- **Why not chosen:** The per-machine plumbing is exactly what fork/exec eliminates.

### Alternative 2: `clyde shell-init zsh` (shell-init form)
- **Description:** The binary emits its own `clyde()` wrapper; user adds `eval "$(...)"`.
- **Pros:** Single source of truth; leaves the shell in the session dir.
- **Cons:** Still a `.zshrc` line and a shell wrapper, with per-shell emitters to maintain.
- **Why not chosen:** Right when relocating the shell is the goal (the `clone` direction); here
  the goal is "open correctly with zero setup," for which fork/exec is simpler.

### Alternative 3: keep `open` and add a `--launch` flag
- **Description:** Leave `open` printing by default, add a flag that makes it cd + exec.
- **Pros:** No removed command.
- **Cons:** Keeps a verb whose default ("open" that only prints) is the wrong shape; the launch
  behavior hides behind a flag.
- **Why not chosen:** The verb should mean what it says; `resume` does, and `open` is dropped.

## Technical Considerations

### Dependencies
None added. `std::os::unix::process::CommandExt` is in std; `claude` is resolved from `PATH` via
`Command::new("claude")` (the same assumption today's printed line makes).

### Performance
Negligible: one `Db::get` and a `stat` before an exec that would happen anyway.

### Security
`cwd` comes from clyde's own local catalog and is passed to `Command::current_dir` (validated
with `is_dir()`), never interpolated into a shell string, so there is no shell-injection surface.
`extra` args are passed as distinct argv entries to `claude`, never through a shell.

### Testing Strategy
- clap parsing tests for `resume` and the `-- <extra>` passthrough.
- `plan_resume` unit-tested across every branch (no process launched).
- Manual smoke: `clyde session resume <known-id>` from `~` lands in the right repo and resumes;
  `-- --model opus` is honored; a deleted dir and a staged-only session each error cleanly.

### Rollout Plan
Ships in the next clyde release. `open` is removed and the rename is a clean break, both
documented in the release notes with the Migration steps.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Scripts/muscle-memory used `open` | Med | Low | Removal documented in release notes; `resume` is the replacement |
| `cwd` recorded but directory since deleted/moved | Med | Low | Validate `is_dir()`; clear stderr error naming the missing path |
| `cwd` is `None` for an old/partial record | Low | Low | Explicit error: "session has no recorded cwd; cannot resume in place" |
| `claude` not on `PATH` | Low | Low | exec failure surfaces as "failed to exec claude in <dir>: ..." |
| Non-unix platform (no `exec`) | Low | Low | `#[cfg(not(unix))]` fallback spawns claude, waits, exits with its status |
| Rename breaks the registered MCP server | High | High | One-time `claude mcp add ... session serve` (Migration) |
| Rename breaks the enrich systemd timer | High | Med | Re-run `clyde bootstrap` to rewrite `ExecStart` (Migration) |
| `resume <id> --model x` without `--` silently misparses | Med | Low | `last = true` makes it an error; documented in `--help`; tested |

## Decisions (resolved)

- **Remove `open` entirely; add a `resume` verb** that does cd + launch. (Resolved 2026-06-29.)
- **Decide by transcript existence**, not the `archived` flag, matching `sessions/src/mcp.rs:139`.
  (From review panel, 2026-06-29.)
- **Rename `sessions` -> `session`, clean break, no alias.** (Resolved 2026-06-29.)
- **`-- <extra>` is forwarded as distinct argv** to claude.

## References

- `clyde/src/main.rs:225` - current `cmd_open` (being removed)
- `sessions/src/mcp.rs:139` - existence-based live/staged/reaped decision (reused by `resume`)
- `sessions/src/model.rs:16` - `SessionRecord.cwd`; also `transcript_path`, `staged_path`
- `sessions/src/db.rs` - `resolve_id`, `get`
- `clyde/src/cli.rs` - `Command` / `SessionsCommand` enums
- `bootstrap.rs:836` - generated enrich `ExecStart` (rename target)
- `README.md:82` - MCP registration (rename target)
- `std::os::unix::process::CommandExt::exec` - process-replacement semantics
