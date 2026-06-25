# Design Document: clyde - umbrella CLI for Claude Code tooling

**Author:** Scott Idler
**Date:** 2026-06-24
**Status:** Implemented
**Review Passes Completed:** 5/5 self + `/architect` + `/staff-engineer`

## Summary

Rename the `klod` workspace to `clyde` and absorb four sibling repos (`claude-report` / `cr`, `claude-cost-usage` / `ccu`, `claude-permit`, and the shared `claude-pricing` library) into it as workspace member crates. The result is a single `clyde` binary that dispatches subcommands (`clyde sessions`, `clyde report`, `clyde cost`, `clyde permit`) over a set of focused library crates, following the `second-brain`/`sb` umbrella pattern. The three old tool binaries (`cr`, `ccu`, `claude-permit`) survive as compat shims with behavior-exact semantics; there is no `klod` shim. A new `clyde bootstrap` migrates all config/data/cache under a single clyde home and repoints the live integrations (statusline, permission hook, enrich timer); `clyde doctor` verifies them.

## Problem Statement

### Background

`klod` is a Cargo workspace (`tatari-tv/klod`) with a thin `klod` binary over two library crates (`session`, `sessions`). It catalogs and enriches Claude Code sessions and serves them over MCP.

Three other Claude-Code-adjacent CLIs live in their own repos and share an internal pricing library:

- `tatari-tv/claude-report` - binary `cr`, v0.2.1 - repo/session reporting.
- `tatari-tv/claude-cost-usage` - binary `ccu`, v0.5.3 - cost/usage; installs a Claude Code **statusline**.
- `tatari-tv/claude-permit` - binary `claude-permit`, v0.1.20 - permission hygiene; runs as a Claude Code **hook** and keeps an events database.
- `tatari-tv/claude-pricing` - library `claude_pricing`, v2.0.0 - pricing data + live fetch; consumed by `cr` and `ccu` as a pinned git dependency. No other consumers.

(`scottidler/claude-permit` is a stale fork at v0.1.7; out of scope, left untouched.)

### Problem

The `klod` name grates and reads as a typo of "claude," while colliding conceptually with the `claude` binary on PATH. Separately, four tightly related tools are scattered across five repos with independent versions, duplicated CI/build scaffolding, and an external git-pinned dependency between two of them. There is no single entry point for "my Claude Code tooling."

### Goals

- Rename `klod` to `clyde` (repo, binary, crate, XDG paths) with data migration.
- Absorb `cr`, `ccu`, `claude-permit`, and `claude-pricing` into the `clyde` workspace as member crates, preserving git history.
- Expose them as subcommands of a single `clyde` binary using the `sb` umbrella pattern (library crates + thin dispatch bin).
- Keep the old binary names (`cr`, `ccu`, `claude-permit`) working with behavior-exact compat shims during transition.
- Unify all absorbed tools' config, data, and cache under a single clyde home, migrated by `clyde bootstrap`.
- Repoint live integrations (ccu statusline, permit hook, enrich systemd timer) at `clyde`, and report their health via `clyde doctor`.
- Collapse to a single flat workspace version line.

### Non-Goals

- Rewriting any tool's internal logic or feature set. Absorption is structural; observable behavior is preserved (see Compatibility Contract).
- Merging the per-tool config schemas into one file. Config files are relocated under one directory, not merged into one schema.
- Touching `scottidler/claude-permit`.
- Publishing to crates.io.
- Building a new statusline/hook/report feature. Only the wiring and packaging change.

## Compatibility Contract

This contract governs the shims, the `run()` signatures, and migration. Decided: **behavior-exact, name = clyde.**

- **Behavior-exact for the shims.** `cr`, `ccu`, `claude-permit` preserve their exit codes, stdout/stderr output, and behavior-critical contracts byte-for-byte. In particular, `claude-permit`'s hook contract is preserved exactly: on a `log` failure it prints `{}` to stdout and exits 0 so it never blocks Claude Code.
- **Name = clyde in help text only.** Help/usage strings render under `clyde <tool>`; the standalone shim help renders under the old name. No other observable difference.
- **Paths move, with fallback.** Config/data/cache relocate to the clyde home (see Data Model). Readers fall back to the legacy path until `bootstrap` migrates, so a tool invoked before bootstrap still finds its existing state.

## Proposed Solution

### Overview

A single Cargo workspace, `tatari-tv/clyde`, where each absorbed tool is a library crate and a thin `clyde` binary composes them into one top-level command tree. This follows `second-brain/sb`. Note the precedent precisely: `sb`'s per-tool wrapper CLIs live in `sb/src/cli/*` and dispatch to wrapper methods, the member CLI structs derive `Args` (not `Parser`), and `sb` ships no compat shims. clyde adopts the same `Args`-based composition and adds shims of its own (a deliberate extension, not part of the sb precedent). `git-tools`, by contrast, is a bag of independent binaries with no umbrella, and is explicitly not the model here.

### Architecture

```
tatari-tv/clyde            (renamed from tatari-tv/klod)
  Cargo.toml               [workspace] default-members = ["clyde"]
    members = [clyde, session, sessions, report, cost, permit, pricing]
  clyde/                   thin umbrella bin (was klod/): top-level CLI, dispatch, bootstrap, doctor
  session/                 (unchanged lib)
  sessions/                (unchanged lib)
  report/                  was claude-report    -> lib; compat [[bin]] cr
  cost/                    was claude-cost-usage -> lib; compat [[bin]] ccu
  permit/                  was claude-permit     -> lib; compat [[bin]] claude-permit
  pricing/                 was claude-pricing    -> lib claude_pricing (no bin)
```

**Crate renaming.** Member dir, package name, and lib name move to single words, except `pricing`, whose lib name stays `claude_pricing` so consumers' `claude_pricing::...` paths and its public API are untouched.

| Dir | Package name | Lib name (`use`) | Compat bin |
|-----|--------------|------------------|------------|
| `report/` | `report` | `report` | `cr` |
| `cost/` | `cost` | `cost` | `ccu` |
| `permit/` | `permit` | `permit` | `claude-permit` |
| `pricing/` | `claude-pricing` (unchanged) | `claude_pricing` (unchanged) | - |

### The clap two-type shape (load-bearing)

Both reviewers flagged that the naive "one struct that derives `Parser`, nested directly under clyde" cannot work in clap v4:

- A type used as a `Subcommand` tuple-variant payload must derive `Args`, not `Parser`.
- An `Args` struct has no `::parse()`, so a shim cannot call `<T as Parser>::parse()` on the same type.
- Common globals collide: clyde and each tool both declaring a global `--log-level` panics at startup.

Resolution, mirroring how `sb` composes members. Each absorbed crate exposes two types plus one entry function:

```rust
// report/src/lib.rs
#[derive(clap::Args)]
pub struct ReportArgs { /* tool-specific flags + its own #[command(subcommand)] */ }

// Standalone wrapper for the `cr` shim: owns the common globals so `cr --log-level ...` still works.
#[derive(clap::Parser)]
pub struct ReportCli {
    #[command(flatten)] pub args: ReportArgs,
    #[arg(long, global = true)] pub log_level: Option<LevelFilter>,
}

// Entry point. Returns the intended process exit code; callers map it to process::exit.
pub fn run(args: ReportArgs, globals: Globals) -> eyre::Result<i32>;
```

- `clyde` owns the single set of **common** globals (`--log-level`, verbosity) at the top level and passes them down as `Globals`. Nested `*Args` structs do not redeclare common globals (that is what removes the collision). Tool-unique globals (for example ccu's `--offline`) stay on the tool's `*Args` and remain reachable as `clyde cost --offline`.
- `Globals` is a small shared struct (log level, verbosity) defined in the clyde-common surface; each outer `*Cli` exposes a `globals()` accessor that reconstructs it from the wrapper's fields. This accessor is the real integration seam between the two types and must be implemented as code in Phase 2, with a parity test that passes a common global through a shim (for example `ccu --log-level debug`) to prove the reconstruction round-trips.
- `clyde` nests the `Args` inner: `Cmd::Report(report::ReportArgs)`, and dispatches `report::run(args, globals)`.
- The `cr` shim parses the `Parser` outer (`ReportCli::parse()`), reconstructs `Globals`, and calls the same `report::run`. Same code path, same behavior.

### Output and exit ownership

To honor the behavior-exact contract, each tool's exit codes, final printing, and pre-flight special-casing move into its `run()` so both the shim and `clyde <tool>` produce identical results:

- `report::run` owns the jq validation, the exit-code-2 path, and the final output printing currently in `claude-report/src/main.rs`.
- `cost::run` owns the statusline and pricing special-casing that currently precedes normal run in `claude-cost-usage/src/main.rs`.
- `permit::run` owns the hook-safe contract: on `log` failure, print `{}` and return 0, never blocking Claude Code (currently in `claude-permit/src/main.rs`).

`run()` returns `eyre::Result<i32>`; both shim `main` and the clyde dispatch arm do `process::exit(code)`.

### Command Surface

| Command | Was | Notes |
|---------|-----|-------|
| `clyde sessions <search\|ls\|open\|tag\|reindex\|stage\|enrich\|doctor\|serve>` | `klod sessions ...` | Unchanged; already nested under `sessions`. |
| `clyde report ...` | `cr ...` | `ReportArgs` nested under clyde. |
| `clyde cost ...` | `ccu ...` | Includes `clyde cost statusline` (the installer). |
| `clyde permit ...` | `claude-permit ...` | Includes `clyde permit log` (hook entry, reads stdin) and `clyde permit install`. |
| `clyde bootstrap` | - | New: migrate paths/config/data; install/repoint statusline, permit hook, enrich timer. |
| `clyde doctor` | - | New: health-check integrations + resolved data/config/cache locations. |

### Compat Shims

Each domain lib crate declares a thin compat `[[bin]]` that parses its `Parser` outer wrapper and calls its `run()` in-process, so it does not depend on `clyde` being on PATH and behaves identically to the pre-merge tool:

```rust
// cost/src/bin/ccu.rs
fn main() {
    let cli = <cost::CostCli as clap::Parser>::parse();
    let code = cost::run(cli.args, cli.globals()).unwrap_or_else(|e| { eprintln!("{e:?}"); 1 });
    std::process::exit(code);
}
```

- `report/` -> `[[bin]] cr`
- `cost/` -> `[[bin]] ccu`
- `permit/` -> `[[bin]] claude-permit`

Shims are a transition bridge; `clyde bootstrap` does the clean repoint to `clyde ...`. They install as their own targets (for example `cargo install --path cost --bin ccu`); `install.sh` installs `clyde` plus the three shims. There is no `klod` shim (see Resolved Decisions).

### Integration Rewiring

Every place a tool writes its own name into an external integration must emit the clyde form. That is two distinct surfaces, not one:

1. **The tools' own installers** (fresh installs): `claude-cost-usage`'s statusline installer (`statusline.d/*` templates hard-code `ccu`) and `claude-permit`'s `cmd/install.rs` (writes `claude-permit log` into `~/.claude/settings.json`) must be rewritten to emit `clyde cost` / `clyde permit log`.
2. **`clyde bootstrap`** (existing installs): migrates and repoints what is already on disk.

The three live integrations:

1. **Statusline.** `ccu`'s statusline installer writes a shell snippet invoking the binary. New installs emit `clyde cost`; `bootstrap` rewrites an existing snippet. `doctor` reports the snippet's target.
2. **Permission hook.** Claude Code `settings.json` calls `claude-permit log` on stdin. `bootstrap` rewrites the exact `claude-permit log` invocation to `clyde permit log` in both global and local settings, preserving matchers and ordering. New installs (`clyde permit install`) emit the clyde form.
3. **Enrich systemd user timer.** The live unit on the desk machine (`~/.config/systemd/user/klod-enrich.service`) has `EnvironmentFile=%h/.config/klod/enrich.env` and `ExecStart=%h/.cargo/bin/klod --log-level info sessions enrich`. `bootstrap` rewrites `ExecStart` to `clyde sessions enrich`, moves the `EnvironmentFile` to `%h/.config/clyde/enrich.env`, renames the unit file to `clyde-enrich.service` (removing the old unit), and runs `systemctl --user daemon-reload`. It repoints an existing unit only (no creation without `--install-timer`), preserves env-file permissions, never logs key material, and verifies via `systemctl --user cat`. `doctor` reports the unit name, `ExecStart`, and last-run.

### Atomic bootstrap semantics

`bootstrap` is idempotent and fails safe:

- Each file it rewrites (settings.json, statusline snippet, systemd unit, env file) is backed up first to `<path>.clyde.bak` before modification.
- Write order: migrate data and config first (so a repointed integration finds its state), then rewrite integration references. Caches are not migrated (disposable; they rebuild at the clyde path).
- `--force` governs only re-writing config that already exists at the destination on a re-run. Integration repointing always applies (it must be correct), and cache rebuild needs no flag.
- If any step fails after an earlier step succeeded, `bootstrap` reports exactly which steps completed and leaves the backups in place; re-running is safe (already-migrated steps are no-ops).
- `doctor` fails hard (non-zero) while any integration still resolves to an old binary name or any tool's state still lives only at a legacy path.

### Data Model

No schema changes. Stores and configs keep their current shapes; locations move under one clyde home, and the `claude-permit` events database is migrated (its omission would silently orphan permission history). Verified source paths are cited.

| Concern | Before | After | Migration |
|---------|--------|-------|-----------|
| sessions data | `$XDG_DATA_HOME/klod/` | `$XDG_DATA_HOME/clyde/` | move dir if present, else fresh. (`session/src/paths.rs`) |
| klod config/cache | `$XDG_{CONFIG,CACHE}_HOME/klod/` | `.../clyde/` | move. |
| permit events DB | `~/.local/share/claude-permit/events.db` (WAL mode) | `~/.local/share/clyde/events.db` | **move** (not copy, not fresh): `PRAGMA wal_checkpoint(TRUNCATE)` first, then move `events.db` plus any `-wal`/`-shm` sidecars together; `doctor` checks presence + row count. (`claude-permit/src/db/store.rs` is WAL: `PRAGMA journal_mode=WAL`) |
| permit config | `~/.config/claude-permit/` | `~/.config/clyde/permit.yml` | move; read-fallback to old until migrated. |
| cost config | `~/.config/ccu/ccu.yml` | `~/.config/clyde/cost.yml` | move; read-fallback. (`claude-cost-usage/src/config.rs`) |
| cost day cache | `dirs::cache_dir()/ccu` | `dirs::cache_dir()/clyde/cost` | **not migrated**; disposable, rebuilds at new path on next run (one cold statusline render after bootstrap). (`claude-cost-usage/src/cache.rs`) |
| pricing override | `~/.config/ccu/pricing.json`, `~/.config/cr/pricing.json` (disjoint) | `~/.config/clyde/pricing.json` (single) | merge old overrides; `cost`/`report` pass `app_name = "clyde"`. (`claude-pricing/src/fetch.rs`) |
| pricing cache | `dirs::cache_dir()/claude-pricing` | `dirs::cache_dir()/clyde/pricing` | **not migrated**; disposable, refetches at new path on next run. (`claude-pricing/src/feed.rs`) |

Note: `claude-report` has no YAML config file (its config is CLI-derived; output goes to XDG data). There is no `report.yml`. (`claude-report/src/config.rs`)

### API Design

```rust
// each absorbed crate, e.g. report
pub struct ReportArgs;          // #[derive(Args)], nested under clyde
pub struct ReportCli;           // #[derive(Parser)], wraps ReportArgs + common globals, for the cr shim
pub fn run(args: ReportArgs, globals: Globals) -> eyre::Result<i32>;
```

`pricing/` keeps its public API (`claude_pricing::...`) and its `fetch` feature; consumers switch from the git dep to `claude-pricing = { path = "../pricing", features = ["fetch"] }` and pass `app_name = "clyde"`.

### Implementation Plan

#### Phase 0: Rename klod to clyde
**Model:** sonnet
- Create a feature branch.
- Rename the `klod/` member dir to `clyde/`; bin name `klod` to `clyde`; crate name and `default-members`.
- Update XDG path constants (`klod` to `clyde`) in `session/src/paths.rs` and `clyde/src/cli.rs`.
- Update workspace metadata, README, clippy/rustfmt configs, install.sh.
- `otto ci` green on the rename alone before absorbing anything.

#### Phase 1: Subtree-merge the four repos
**Model:** sonnet
- `git subtree add --prefix=<dir> <repo> main` for each (all four default to `main`): `claude-pricing` to `pricing/`, `claude-report` to `report/`, `claude-cost-usage` to `cost/`, `claude-permit` to `permit/`.
- Add the four to `[workspace] members`.
- Import-only; no clean build expected yet. Verify `git log -- <dir>` shows imported history.

#### Phase 2: Convert absorbed crates to libraries (two-type clap shape)
**Model:** opus
- For `report`, `cost`, `permit`: introduce the `*Args` (`Args`) inner and `*Cli` (`Parser`) outer wrapper; define `Globals` and each `*Cli::globals()` accessor as real code (the two-type seam); move exit-code/output/special-case ownership into `run()` per the Output and exit ownership section. `cost` needs a new `lib.rs` (it has none today). Reduce each `main.rs` to the compat `[[bin]]`.
- Preserve `permit`'s `{}`-on-failure hook contract inside `permit::run`.
- Rewire `pricing` from git dep to `path` dep in `cost` and `report`; keep `features = ["fetch"]`; pass `app_name = "clyde"`.
- Reconcile diverging deps into `[workspace.dependencies]`: `rusqlite` (sessions 0.40.1 vs permit 0.39.0), `clap` (ccu 4.5.60 vs others 4.6.x), `serde_json` (1.0.150 vs 1.0.149). Treat as behavior-affecting, not clerical; re-run tests after each bump.
- Reconcile lints: workspace denies `dead_code` and `unused_variables`; fix offenders rather than blanket-allow.
- Asset paths: `include_dir!("$CARGO_MANIFEST_DIR/...")` resolves correctly post-subtree (verified for the ccu statusline); scope the asset audit to any cwd-relative runtime path only.
- `otto ci` green: all three compat bins build and behave as before.

#### Phase 3: Wire the clyde umbrella
**Model:** opus
- Add top-level `Cmd` arms nesting `report::ReportArgs`, `cost::CostArgs`, `permit::PermitArgs`; dispatch each to `run(args, globals)`.
- Keep `clyde sessions ...` exactly as today.
- clyde owns common globals; centralize logger/verbosity over the merged tree (mirror sb's logger inspection).
- Tests: `clyde <tool> --help` and behavior parity with the shim `<tool>`.

#### Phase 4: bootstrap + doctor
**Model:** opus
- `clyde bootstrap`: data/config migration (incl. WAL-safe permit events-DB move with sidecars, pricing override merge) first, caches left to rebuild; then rewrite statusline, permit hook (global + local), and systemd unit (ExecStart + EnvironmentFile + file rename + daemon-reload). Back up every file before write; idempotent; repoint-existing-only unless `--install-timer`; `--skip-systemd`; `--force` only re-writes existing destination config.
- Rewrite the tools' own installers (`cost statusline`, `permit install`) to emit the clyde form for fresh installs.
- `clyde doctor`: binary location; resolved data/config/cache paths; statusline target; hook target (global + local); timer unit name + ExecStart + last-run; permit events DB presence + row count. Non-zero exit while any old target or legacy-only state remains.

#### Phase 5: Config relocation
**Model:** sonnet
- Config readers default to `~/.config/clyde/{permit,cost}.yml` with read-fallback to legacy paths until `bootstrap` migrates. (No `report.yml`.)
- Update each crate's config loader + docs.

#### Phase 6: Tests, CI, docs
**Model:** sonnet
- Consolidate per-crate tests; `otto ci` covers the whole workspace (incl. `whitespace -r`).
- Rewrite top-level README around `clyde`; per-crate READMEs trimmed.
- Verify shims and integrations live; ship via `bump -m` (single flat `v*` tag on `main`).

#### Phase 7: Archive old repos
**Model:** sonnet
- After clyde ships and integrations verified: archive `claude-report`, `claude-cost-usage`, `claude-permit`, `claude-pricing` on GitHub (read-only), READMEs pointing at clyde. Nothing deleted; tags preserved.

## Alternatives Considered

### Alternative 1: Keep separate binaries in one workspace (the git-tools pattern)
- **Description:** Move all tools into the `clyde` workspace but keep them as independent binaries sharing a `common` lib; no umbrella binary.
- **Pros:** Least refactoring; each tool keeps its exact CLI.
- **Cons:** No single entry point; the merge is cosmetic; still N binaries.
- **Why not chosen:** Defeats the goal of one `clyde` command; `sb` shows the umbrella is worth the refactor.

### Alternative 2: Monolithic modules inside the clyde bin
- **Description:** Copy each tool's source as plain modules in the `clyde` binary crate; no library boundaries.
- **Pros:** Fastest to wire; no inter-crate API.
- **Cons:** Loses isolation, independent testability, clean compile units; one giant bin crate.
- **Why not chosen:** Violates the isolation the sb pattern provides; worst long-term maintainability.

### Alternative 3: Keep claude-pricing external
- **Description:** Absorb the three CLIs but leave `claude-pricing` a pinned git dependency.
- **Pros:** Zero churn on pricing.
- **Cons:** One odd external git pin in an otherwise self-contained workspace; non-atomic, network-dependent builds; manual pin bumps.
- **Why not chosen:** Its only two consumers are being absorbed; absorbing makes builds atomic and offline.

## Technical Considerations

### Dependencies
- Internal: `clyde` bin depends on `session`, `sessions`, `report`, `cost`, `permit` (transitively `pricing`).
- External: union of the four tools' deps, hoisted into `[workspace.dependencies]`; version divergence reconciled in Phase 2; `claude_pricing`'s `fetch` feature pulls `ureq`/`tempfile`.

### Performance
- One binary linking all libs is larger but cold-start and runtime are unaffected; dispatch is a match. Build time rises with the merged dep graph; mitigated by the relocated `target/` (intel SSD) and release builds only at `bump` time.

### Security
- The permit hook and the enrich timer's env-file API key are sensitive. `bootstrap` preserves file permissions on the timer/env-file, never logs key material, and backs up before rewrite. `doctor` reports presence, never contents.

### Testing Strategy
- Per-crate unit tests carried over.
- Parity tests: `clyde <tool>` vs. shim `<tool>` for help, output, and exit code; argument round-trips; a common global passed through a shim (`ccu --log-level debug`) to prove `globals()` round-trips; explicit test that `permit log` prints `{}` and exits 0 on induced failure.
- `bootstrap`/`doctor` tested against a temp `$HOME` (XDG overrides) with fake statusline/hook/timer/events-DB fixtures; assert idempotency, and that the events-DB move checkpoints WAL and preserves row count (sidecars handled).
- Full `otto ci` green at the end of Phases 0, 2, 3, and 6.

### Rollout Plan

Staged delivery (both reviewers; do not land as one mega-PR):

- **PR-A = Phase 0** (pure rename): the only change touching the live XDG path constant and the enrich timer's binary name. Ship alone, `otto ci` green on the rename alone, `bump -m`, then bootstrap + `doctor`-verify the timer repoint in isolation before any absorption churn.
- **PR-B = Phases 1-3** (subtree import + lib conversion + umbrella wiring): the merged tree's first green build as a unit. Phase 1 is history-rewriting and import-only; reviewers approve on that basis. (Split Phase 1 into its own sub-PR if the import diff is too noisy to review bundled.)
- **PR-C = Phases 4-5** (bootstrap/doctor + config relocation): the risky live-machine logic (WAL-safe events-DB move, global+local settings.json rewrite, systemd rename+daemon-reload), independently testable against a temp `$HOME`.
- **Phase 7** (archive old repos) is post-ship ops, not a PR.

Why staged: Phase 0 alone gives an unambiguous green-CI signal and isolates the only change that can break the live timer; a rename regression and a dep-hoist regression (Phase 2) are never the same bisect; the WAL-safe events-DB move gets its own reviewable PR rather than burial in a mega-diff.

Audit checkpoints (verify-against-code, not paper):
- **Implementation Audit after Phase 3 builds green** (`/architect` Mode 2): confirm `run()` preserved each tool's exit/output contract byte-for-behavior (notably permit's `{}`-on-failure and report's exit-code-2 jq path).
- **Bootstrap verify pass before first live run** (`/staff-engineer`): WAL sidecar handling, settings.json rewrite (global + local), systemd rename+reload, against a temp `$HOME`.

Each PR: branch -> PR -> merge; `bump -m` and the single flat `v*` tag happen OFF main per git rules. Keep shims installed until `doctor` confirms all integrations point at clyde; archive old repos last.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| clap derive/global collision panics or fails to compile | Med | High | Two-type shape (Args inner + Parser outer); clyde owns common globals; covered by build + parity tests. |
| permit events DB orphaned by migration gap | Med | High | Data Model row added; `bootstrap` moves it; `doctor` checks presence + row count; parity test on the move. |
| Partial bootstrap strands an integration | Med | High | Backups + ordered writes + idempotent re-run; `doctor` fails hard while any old target/legacy state remains; no `klod` shim so a missed timer repoint fails loud. |
| Behavior drift in a shim (exit code/output) | Med | High | Behavior-exact contract; `run()` owns exit/output; parity tests per tool. |
| Dependency hoist changes behavior | Med | Med | Reconcile versions explicitly in Phase 2; re-run tests after each bump. |
| Subtree merge mangles history/paths | Low | Med | `git subtree add` per repo; verify `git log -- <dir>`; Phase 1 import-only. |
| Sessions data-dir migration loses catalog | Low | High | Move (not delete); fall back to fresh only when absent; back up before move. |

## Resolved Decisions

Resolved across a three-lens panel (pragmatist / conventions-purist / ops-risk) and the `/architect` + `/staff-engineer` reviews, deferring to the `sb` precedent where applicable.

- **No `klod` compat shim** (unanimous; matches sb's zero-shim umbrella). The three tool shims remain as a transition bridge; `klod`'s only machine consumer is the enrich timer, which `bootstrap` repoints and `doctor` verifies. A `klod` shim would re-entrench the retired name and could mask a missed bootstrap.
- **`bootstrap` manages the systemd timer directly** (unanimous; matches sb's `register_systemd_units`). Idempotent; repoint-existing-only without `--install-timer`; `--skip-systemd`; write-if-missing default, write-always under `--force`; preserve env-file permissions; never log key material; back up before rewrite; `doctor` verifies.
- **Behavior-exact, name = clyde** compatibility contract (see Compatibility Contract).
- **Unify config/data/cache under one clyde home with migration** (see Data Model), including the permit events DB and a single merged `pricing.json` under `app_name = "clyde"`.
- **Version: minor bump via `bump -m`** at release. Signals the structural consolidation and re-bootstrap without a hardcoded number or a 1.0 the workspace has not earned. Per the no-version-in-docs convention the number is not fixed here.
- **Staged delivery, not one PR** (both reviewers). PR-A = Phase 0 (rename) ships and is `doctor`-verified in isolation; PR-B = Phases 1-3 (import + lib + umbrella, first green merged build); PR-C = Phases 4-5 (bootstrap/doctor/config). Phase 7 archive is post-ship ops. No pre-code re-review; instead an Implementation Audit after Phase 3 builds green and a bootstrap verify pass before the first live run.

## Open Questions

None blocking. All design-review findings are folded into v2.

## Review (v2)

`/architect` (Gemini) and `/staff-engineer` (Codex) both reviewed v1 against all six repos and converged: the umbrella architecture and the sb choice are sound; every gap was in absorption mechanics. Folded in:

- clap one-type composition was unworkable -> two-type Args/Parser shape (both reviewers, top finding).
- permit events DB was missing from migration -> added as a move with a doctor check (architect, highest-consequence).
- uniform `run()` could change behavior -> per-tool output/exit ownership, behavior-exact contract (staff-engineer).
- fresh-install paths also write old names -> rewired the tools' own installers, not just bootstrap (architect, generalized).
- pricing `app_name` override namespace -> unified under clyde with migration.
- config/data paths were wrong/invented -> Data Model rewritten against verified source; `report.yml` removed.
- sb precedent overstated -> corrected (Args composition, wrappers in sb/src/cli/*, no shims in sb).
- `include_dir!`/`$CARGO_MANIFEST_DIR` resolves post-subtree -> Phase 2 asset audit softened to cwd-relative paths only.
- dependency divergence is behavioral -> explicit versions and re-test in Phase 2.

## References

- sb umbrella pattern: `~/repos/scottidler/second-brain` (`sb/src/cli.rs`, `sb/src/cli/*`, `default-members = ["sb"]`; members derive `Args`).
- git-tools counter-pattern: `~/repos/scottidler/git-tools`.
- Verified source: `claude-permit/src/{main.rs,cmd/install.rs,db/store.rs}`, `claude-cost-usage/src/{main.rs,config.rs,cache.rs,statusline.rs}`, `claude-report/src/{main.rs,lib.rs,config.rs}`, `claude-pricing/src/{fetch.rs,feed.rs}`, `klod` `session/src/paths.rs`.
- Live unit: `~/.config/systemd/user/klod-enrich.service`.
- Memory: enrich scheduling (desk systemd user timer, daily 03:00, env-file key).
