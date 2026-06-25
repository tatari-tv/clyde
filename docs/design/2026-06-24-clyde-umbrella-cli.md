# Design Document: clyde — umbrella CLI for Claude Code tooling

**Author:** Scott Idler
**Date:** 2026-06-24
**Status:** In Review
**Review Passes Completed:** 5/5

## Summary

Rename the `klod` workspace to `clyde` and absorb four sibling repos — `claude-report` (`cr`), `claude-cost-usage` (`ccu`), `claude-permit`, and the shared `claude-pricing` library — into it as workspace member crates. The result is a single `clyde` binary that dispatches subcommands (`clyde sessions`, `clyde report`, `clyde cost`, `clyde permit`) over a set of focused library crates, following the established `second-brain`/`sb` umbrella pattern. Old binary names continue to work via compat shims, and a new `clyde bootstrap` repoints the live integrations (statusline, permission hook, enrich timer) at the renamed binary.

## Problem Statement

### Background

`klod` is a Cargo workspace (`tatari-tv/klod`) with a thin `klod` binary over two library crates (`session`, `sessions`). It catalogs and enriches Claude Code sessions and serves them over MCP.

Three other Claude-Code-adjacent CLIs live in their own repos and share an internal pricing library:

- `tatari-tv/claude-report` — binary `cr`, v0.2.1 — repo/session reporting.
- `tatari-tv/claude-cost-usage` — binary `ccu`, v0.5.3 — cost/usage; installs a Claude Code **statusline**.
- `tatari-tv/claude-permit` — binary `claude-permit`, v0.1.20 — permission hygiene; runs as a Claude Code **hook**.
- `tatari-tv/claude-pricing` — library `claude_pricing`, v2.0.0 — pricing data + live fetch; consumed by `cr` and `ccu` as a pinned git dependency. No other consumers.

(`scottidler/claude-permit` is a stale fork at v0.1.7; out of scope, left untouched.)

### Problem

The `klod` name grates and reads as a typo of "claude," while colliding conceptually with the `claude` binary on PATH. Separately, four tightly related tools are scattered across five repos with independent versions, duplicated CI/build scaffolding, and an external git-pinned dependency between two of them. There is no single entry point for "my Claude Code tooling."

### Goals

- Rename `klod` → `clyde` (repo, binary, crate, XDG paths) with data migration.
- Absorb `cr`, `ccu`, `claude-permit`, and `claude-pricing` into the `clyde` workspace as member crates, preserving git history.
- Expose them as subcommands of a single `clyde` binary using the `sb` umbrella pattern (library crates + thin dispatch bin).
- Keep the old binary names (`cr`, `ccu`, `claude-permit`) working during transition via compat shims.
- Repoint live integrations (ccu statusline, permit hook, enrich systemd timer) at `clyde` via a `clyde bootstrap` command, and report their health via `clyde doctor`.
- Collapse to a single flat workspace version line.

### Non-Goals

- Rewriting any tool's internal logic or feature set. Absorption is structural, not behavioral.
- Merging the per-tool config schemas into one file. Configs are relocated, not unified.
- Touching `scottidler/claude-permit`.
- Publishing to crates.io.
- Building a new statusline/hook/report feature. Only the wiring changes.

## Proposed Solution

### Overview

A single Cargo workspace, `tatari-tv/clyde`, where every tool is a **library crate that exports its own clap CLI struct, command enum, and `run()` entry**, and a thin `clyde` binary wraps those CLIs into one top-level command tree and dispatches one line per arm. This is exactly how `second-brain/sb` composes `borg`/`cortex`/`oracle`. (`git-tools`, by contrast, is a bag of independent binaries with no umbrella — explicitly *not* the model here.)

### Architecture

```
tatari-tv/clyde            (renamed from tatari-tv/klod)
  Cargo.toml               [workspace] default-members = ["clyde"]
    members = [clyde, session, sessions, report, cost, permit, pricing]
  clyde/                   thin umbrella bin (was klod/): top-level CLI, dispatch, bootstrap, doctor
  session/                 (unchanged lib)
  sessions/                (unchanged lib)
  report/                  was claude-report   → lib: ReportCli + run(); compat [[bin]] cr
  cost/                    was claude-cost-usage→ lib: CostCli   + run(); compat [[bin]] ccu
  permit/                  was claude-permit    → lib: PermitCli + run(); compat [[bin]] claude-permit
  pricing/                 was claude-pricing   → lib claude_pricing (no bin)
```

**Crate renaming** — member dir, package name, and lib name move to single words, except `pricing`, whose lib name must stay `claude_pricing` so consumers' `claude_pricing::…` paths and its public API are untouched:

| Dir | Package name | Lib name (`use`) | Compat bin |
|-----|--------------|------------------|------------|
| `report/` | `report` | `report` | `cr` |
| `cost/` | `cost` | `cost` | `ccu` |
| `permit/` | `permit` | `permit` | `claude-permit` |
| `pricing/` | `claude-pricing` (unchanged) | `claude_pricing` (unchanged) | — |

Top-level dispatch in `clyde/src/cli.rs`, mirroring `sb` (`SessionsCli` is klod's existing `sessions` command tree, carried over verbatim):

```rust
pub enum Cmd {
    Sessions(SessionsCli),   // existing klod commands, unchanged
    Report(report::ReportCli),
    Cost(cost::CostCli),
    Permit(permit::PermitCli),
    Bootstrap(BootstrapArgs),
    Doctor(DoctorArgs),
}

match cli.cmd {
    Cmd::Sessions(c)  => sessions_dispatch(c),
    Cmd::Report(c)    => report::run(c),
    Cmd::Cost(c)      => cost::run(c),
    Cmd::Permit(c)    => permit::run(c),
    Cmd::Bootstrap(a) => bootstrap::run(a),
    Cmd::Doctor(a)    => doctor::run(a),
}
```

### Command Surface

| Command | Was | Notes |
|---------|-----|-------|
| `clyde sessions <search\|ls\|open\|tag\|reindex\|stage\|enrich\|doctor\|serve>` | `klod sessions …` | Unchanged; already nested under `sessions`. |
| `clyde report …` | `cr …` | `ReportCli` becomes a nested subcommand. |
| `clyde cost …` | `ccu …` | Includes `clyde cost statusline` (the installer). |
| `clyde permit …` | `claude-permit …` | Includes `clyde permit log` (the hook entry, reads stdin). |
| `clyde bootstrap` | — | New: install/repoint statusline, permit hook, enrich timer; migrate paths/config. |
| `clyde doctor` | — | New: health-check all integrations + data/config locations. |

### Compat Shims

Each domain lib crate declares a thin compat `[[bin]]` that parses its own top-level CLI and calls its `run()` — in-process, so it does not depend on `clyde` being on PATH and has identical behavior to the pre-merge tool:

```rust
// cost/src/bin/ccu.rs
fn main() -> eyre::Result<()> { cost::run(<cost::CostCli as clap::Parser>::parse()) }
```

- `report/` → `[[bin]] cr`
- `cost/` → `[[bin]] ccu`
- `permit/` → `[[bin]] claude-permit`

Shims are the transition safety net for muscle memory and any config we miss; `clyde bootstrap` does the clean repoint to `clyde …`. They install as their own targets (e.g. `cargo install --path cost --bin ccu`); `install.sh` installs `clyde` plus the three shims.

### Integration Rewiring

Three live integrations reference the **old binary names** and break on rename. `clyde bootstrap` installs/repoints them; `clyde doctor` verifies them. Shims cover the binary-name path as a fallback.

1. **Statusline** — `ccu` ships a `Statusline` installer writing a Claude Code statusline that invokes the binary. Repoint to `clyde cost`. Surfaced as `clyde cost statusline` and orchestrated by `clyde bootstrap`.
2. **Permission hook** — Claude Code `settings.json` hook calls `claude-permit log` on stdin. Repoint to `clyde permit log`.
3. **Enrich systemd user timer** — an external `~/.config/systemd/user/*.timer`/`.service` on the desk machine runs `klod sessions enrich` daily at 03:00 with an env-file API key. `bootstrap` regenerates the unit's `ExecStart` to `clyde sessions enrich` and reloads the unit. It only **repoints an existing** unit — it does not create a timer on machines that never had one — unless `bootstrap --install-timer` is passed explicitly.

### Data Model

No schema changes. The sessions SQLite catalog and each tool's config keep their current shapes; only their **filesystem locations** change:

| Concern | Before | After | Migration |
|---------|--------|-------|-----------|
| sessions data | `$XDG_DATA_HOME/klod/` | `$XDG_DATA_HOME/clyde/` | `bootstrap` moves dir if present; new default otherwise. |
| config | `$XDG_CONFIG_HOME/klod/` | `$XDG_CONFIG_HOME/clyde/` | same. |
| cache | `$XDG_CACHE_HOME/klod/` | `$XDG_CACHE_HOME/clyde/` | same. |
| permit config | `~/.config/claude-permit/…` (`claude-permit.yml`) | `~/.config/clyde/permit.yml` | `bootstrap` copies forward; falls back to old path read if new absent. |
| report config | existing path | `~/.config/clyde/report.yml` | same. |
| cost config | existing path | `~/.config/clyde/cost.yml` | same. |

### API Design

Each absorbed crate's public library surface is minimal and uniform:

```rust
// e.g. report/src/lib.rs
pub use cli::ReportCli;     // #[derive(Parser)] top-level args incl. its own subcommands
pub fn run(cli: ReportCli) -> eyre::Result<()>;
```

`pricing/` keeps its current public API (`claude_pricing::…`) and its `fetch` feature; consumers switch from the git dep to `claude-pricing = { path = "../pricing", features = ["fetch"] }`.

### Implementation Plan

#### Phase 0: Rename klod → clyde
**Model:** sonnet
- Create a feature branch.
- Rename the `klod/` member dir to `clyde/`; bin name `klod` → `clyde`; crate name and `default-members`.
- Update XDG path constants (`klod` → `clyde`) in `session/src/paths.rs` and `clyde/src/cli.rs`.
- Update workspace metadata, README, clippy/rustfmt configs, install.sh.
- `otto ci` green on the rename alone before absorbing anything.

#### Phase 1: Subtree-merge the four repos
**Model:** sonnet
- `git subtree add` each repo into its member dir preserving history: `claude-pricing`→`pricing/`, `claude-report`→`report/`, `claude-cost-usage`→`cost/`, `claude-permit`→`permit/`.
- Add the four to `[workspace] members`.
- Do not expect a clean build yet; this phase is purely the history import + tree placement.

#### Phase 2: Convert absorbed crates bin → library
**Model:** opus
- For `report`, `cost`, `permit`: split `main.rs` into `lib.rs` exposing `<Tool>Cli` + `run()`; reduce `main.rs` to a compat `[[bin]]` calling `run()`.
- Rewire `pricing` from git dep to `path` dep in `cost` and `report`; keep `features = ["fetch"]`.
- Reconcile shared dependencies into `[workspace.dependencies]`; align edition to the workspace.
- Reconcile lints: the workspace denies `dead_code` and `unused_variables`; absorbed code that trips these must be fixed (not blanket-`allow`d) to stay green.
- Subtree already places ancillary assets (`templates/`, `statusline.d/`, `assets/`, `settings/`, `build.rs`) under each crate dir; verify they resolve relative to the crate root and fix any include path that assumed the old repo root.
- `otto ci` green: all compat bins build and behave as before.

#### Phase 3: Wire the clyde umbrella
**Model:** opus
- Add top-level `Cmd` arms wrapping `report::ReportCli`, `cost::CostCli`, `permit::PermitCli`; dispatch each.
- Keep `clyde sessions …` exactly as today.
- Centralize logger/verbosity selection over the merged command tree (mirror `sb`'s logger inspection).
- Tests: `clyde <tool> --help` parity with the shim `<tool> --help`.

#### Phase 4: bootstrap + doctor
**Model:** opus
- `clyde bootstrap`: migrate XDG dirs and configs; install/repoint statusline (`clyde cost`), permit hook (`clyde permit log`), and the enrich systemd user timer (`clyde sessions enrich`); idempotent.
- `clyde doctor`: report binary location, data/config/cache paths, statusline presence + target, hook presence + target, timer presence + ExecStart + last-run.

#### Phase 5: Config relocation
**Model:** sonnet
- Default config reads to `~/.config/clyde/{permit,report,cost}.yml`, with read-fallback to legacy paths until `bootstrap` migrates.
- Update each crate's config loader + docs.

#### Phase 6: Tests, CI, docs
**Model:** sonnet
- Consolidate per-crate tests; ensure `otto ci` covers the whole workspace (incl. `whitespace -r`).
- Rewrite top-level README around `clyde` and its subcommands; per-crate READMEs trimmed to crate scope.
- Verify shims, then ship via `bump` (single flat `v*` tag on `main`).

#### Phase 7: Archive old repos
**Model:** sonnet
- After clyde ships and integrations verified: archive `claude-report`, `claude-cost-usage`, `claude-permit`, `claude-pricing` on GitHub (read-only), each README pointing at `clyde`. Nothing deleted; tags preserved.

## Alternatives Considered

### Alternative 1: Keep separate binaries in one workspace (the git-tools pattern)
- **Description:** Move all tools into the `clyde` workspace but keep them as independent binaries sharing a `common` lib; no umbrella binary.
- **Pros:** Least refactoring; each tool keeps its exact CLI.
- **Cons:** No single entry point; the "merge" is cosmetic; still N binaries to install and reason about.
- **Why not chosen:** Defeats the goal of one `clyde` command; `sb` demonstrates the umbrella is worth the refactor.

### Alternative 2: Monolithic modules inside the clyde bin
- **Description:** Copy each tool's source as plain modules in the `clyde` binary crate; no library boundaries.
- **Pros:** Fastest to wire; no inter-crate API to design.
- **Cons:** Loses isolation, independent testability, and clean compile units; one giant bin crate.
- **Why not chosen:** Violates the isolation the `sb` pattern provides; worst long-term maintainability.

### Alternative 3: Keep claude-pricing external
- **Description:** Absorb the three CLIs but leave `claude-pricing` as a pinned git dependency.
- **Pros:** Zero churn on pricing.
- **Cons:** One odd external git pin in an otherwise self-contained workspace; non-atomic, network-dependent builds; pin must be bumped manually.
- **Why not chosen:** Its only two consumers are being absorbed, so there is no external consumer to serve; absorbing makes builds atomic and offline.

## Technical Considerations

### Dependencies
- Internal: `clyde` bin depends on `session`, `sessions`, `report`, `cost`, `permit` (and transitively `pricing`).
- External: union of the four tools' deps, hoisted into `[workspace.dependencies]`; `claude_pricing`'s `fetch` feature pulls `ureq`/`tempfile`.

### Performance
- Single binary linking all libs is larger but cold-start and runtime are unaffected; dispatch is a match. Build time rises with the merged dep graph; mitigated by the workspace's relocated `target/` (intel SSD) and `bump`-time release builds only.

### Security
- The permit hook and the enrich timer's env-file API key are sensitive. `bootstrap` must preserve existing file permissions on the timer/env-file and must not log key material. `doctor` reports presence, never contents.

### Testing Strategy
- Per-crate unit tests carried over unchanged.
- New parity tests: `clyde <tool> --help` vs. shim `<tool> --help`; argument round-trips for each wrapped CLI.
- `bootstrap`/`doctor` tested against a temp `$HOME` (XDG overrides) with fake statusline/hook/timer fixtures; assert idempotency.
- Full `otto ci` green at the end of Phases 0, 2, 3, and 6.

### Rollout Plan
- Land on a branch; `otto ci` green; `bump` a single flat `v*` tag on `main` (gates checked per git rules).
- Install `clyde`; run `clyde bootstrap`; verify with `clyde doctor` and `sdv`-style live checks (statusline renders, a permission event logs, timer fires).
- Keep shims installed until `doctor` confirms all integrations point at `clyde`; archive old repos last.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Statusline/hook silently break after rename | Med | High | Compat shims keep old names working; `bootstrap` repoints; `doctor` verifies targets. |
| Enrich timer keeps calling `klod`, enrichment silently stops | Med | Med | `bootstrap` regenerates `ExecStart`; `doctor` reports timer target + last-run. |
| Subtree merge mangles history or paths | Low | Med | Use `git subtree add` per repo; verify `git log -- <dir>` after each; Phase 1 is import-only. |
| Dependency-version conflicts when hoisting to workspace | Med | Med | Reconcile in Phase 2 behind a green `otto ci`; pin shared versions in `[workspace.dependencies]`. |
| Data-dir migration loses the sessions catalog | Low | High | `bootstrap` moves (not deletes) `klod`→`clyde` dir; falls back to fresh only when absent; back up before move. |
| Config path change breaks a tool's load | Med | Med | Read-fallback to legacy paths until migrated; `doctor` shows resolved config path. |

## Resolved Decisions

Resolved by a three-lens review panel (pragmatist / conventions-purist / ops-risk), deferring to the `second-brain`/`sb` precedent where applicable.

- **No `klod` compat shim** (unanimous; matches `sb`'s zero-shim umbrella). The three tool shims (`cr`/`ccu`/`claude-permit`) remain as a transition bridge for muscle memory + live integrations, but `klod` gets none: its only machine consumer is the enrich timer, which `bootstrap` repoints and `doctor` verifies. A `klod` shim would re-entrench the retired name on PATH.
- **`bootstrap` manages the systemd timer directly** (unanimous; matches `sb`'s `register_systemd_units`). Idempotent; **repoints an existing unit only** (no creation on fresh machines without `--install-timer`); `--skip-systemd` opt-out; units write-if-missing by default and write-always under `--force`; preserve env-file permissions; never log key material; back up the unit before rewrite; `doctor` reads back `ExecStart` + last-run as verification.

## Open Questions

- [ ] Post-merge version number. Panel split: continue normally vs. a deliberate **minor** bump to signal the consolidation; no one favors a `1.0.0`. Per the no-version-in-docs convention, the number is not fixed here — recommendation is `bump -m` (minor) at release; a plain `bump` (patch) is acceptable. Awaiting Scott's pick.

## References

- `second-brain` (`sb`) umbrella pattern: `~/repos/scottidler/second-brain` (`sb/src/cli.rs`, `default-members = ["sb"]`).
- `git-tools` (independent-binaries counter-pattern): `~/repos/scottidler/git-tools`.
- klod XDG paths: `session/src/paths.rs`, `klod/src/cli.rs`.
- Memory: enrich scheduling (desk systemd user timer, daily 03:00, env-file key).
