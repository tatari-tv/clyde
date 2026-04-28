# Design Document: cr alignment pass

**Author:** Scott Idler
**Date:** 2026-04-27
**Status:** Implemented
**Review Passes Completed:** 5/5 + Architect round 5 (4 findings, 3 actioned, 1 pushed back)

## Summary

A consolidated batch of changes that brings the existing `cr` (claude-report) codebase into alignment with the architecture doc at `2026-04-27-claude-report.md` after several review rounds with the Architect. Includes the unshipped architect-round-3 outcomes (untracked-models tristate, persona integration), three small wiring tasks (`--include-tradeoffs` flag, `jsonl-paths` in YAML, baked-in default report prompt), and four drift fixes between the original doc and what code actually does.

## Problem Statement

### Background

`cr` is past its initial implementation. Recent commits already shipped: subcommand-driven CLI, schema v2 with per-model breakdown, atomic YAML write, UUID-v4 parent-stem validation, subagent rollup, Haiku titling, pandoc PDF rendering with `--pdf-engine`, `--prompt <pmt>` for Opus-rendered markdown, blocked-roots git climbing guard, and the `templates/justification.pmt` prompt template (just rewritten to be persona-aware and generic).

The original architecture doc went through five internal review passes plus four Architect rounds. The Architect's round-3 and round-4 outcomes (untracked-models tristate schema, in-report surfacing of unknown models) are in the doc but not yet in the code. Smaller items also drifted: persona integration was specified but never implemented, the `--include-tradeoffs` flag the pmt expects has no clap binding, `jsonl-paths` is in the doc's YAML schema but the code's `SessionEntry` does not emit it, the default `report.pmt` is required to be passed via `--prompt` instead of baked-in.

### Problem

The codebase and the design doc are out of sync. Closing the gap is a single, well-scoped pass.

### Goals

- **Untracked-models tristate.** `pricing::calculate_usd` returns `Result<f64, UnknownModel>`; `ModelTokens.spend_usd` is `Option<f64>`; `Totals.untracked_models: Vec<String>`; renderer flags this prominently.
- **Persona integration.** `cr render` shells out to `persona whoami --json`, wraps result + options + report into a context block sent to Opus. Graceful degrade when persona-cli is missing or fails (stderr warning, build report anonymously).
- **`--include-tradeoffs` flag.** Wired through CLI, config, and the render context block so the pmt's opt-in tradeoffs section actually fires.
- **`jsonl-paths` emitted in YAML.** Surface the source files for each session entry. Useful for debugging and for future re-titling.
- **Baked-in default `report.pmt`.** Rename `templates/justification.pmt` to `templates/report.pmt`. `include_str!` it into the binary. `cr render` uses the baked-in default when `--prompt` is omitted; `--prompt <path>` overrides.
- **Schema version revert 2 → 1.** This project hasn't shipped externally. Renumber the current shape as v1; future schema changes go to v2.
- ~~Add `data/pricing.yml` alongside `data/pricing.json`.~~ **Deferred** (architect-round-5): introducing two sources of truth without a `bin/update` sync script invites drift. `data/pricing.json` remains canonical until the sync script is ready.
- **Doc fixes.** Update `2026-04-27-claude-report.md` to match code reality: schema-version is 1, pricing files are both `.json` and `.yml`, subcommand is named `Collect` (default-when-bare), Phase 4's git-toplevel guard description matches the actual blocked-roots implementation.

### Non-Goals

- **Not extracting `claude-pricing` as a separate-repo crate.** Architect-round-3 conceded this could be deferred until both binaries' parallel scrapers actually produce a duplicate-fix incident. Both pricing modules work today.
- **Not changing the YAML's existing field shape** beyond the `untracked-models` addition and `spend-usd: Option<f64>` flip. Existing `claude-report.yml` files on disk continue to load (we only read `sessions.<id>.title` for preservation, which doesn't change).
- **Not implementing `cr merge` or any `cr title` standalone subcommand.** Those were already non-goals in the original design doc.
- **Not touching ccu.** ccu refactor is deferred per the original doc.

## Proposed Solution

### Overview

Two passes:

1. **Code changes**, in dependency order: schema-version revert and tristate land first (touch shared types), then the small wiring (jsonl-paths, --include-tradeoffs), then the larger features (persona integration, baked-in default prompt, pricing.yml mirror).
2. **Doc fixes** to `2026-04-27-claude-report.md`, after the code changes are green so the doc reflects reality.

### Data Model

Three serde-visible changes to `report.rs`:

```rust
// before
pub struct ModelTokens {
    // ...
    pub spend_usd: f64,
}
pub struct SessionEntry {
    // ...
    pub spend_usd: f64,
}
pub struct Totals {
    pub sessions: usize,
    pub spend_usd: f64,
    pub models: BTreeMap<String, ModelTokens>,
}

// after
pub struct ModelTokens {
    // ...
    pub spend_usd: Option<f64>,           // None = tokens counted but pricing missing
}
pub struct SessionEntry {
    // ...
    pub spend_usd: Option<f64>,           // sum of priced models only; None when ALL untracked
    pub untracked_models: Vec<String>,    // new: per-session unknowns (flags partial spend)
    pub jsonl_paths: Vec<PathBuf>,        // new: source files for this session
}
pub struct Totals {
    pub sessions: usize,
    pub spend_usd: f64,                       // unchanged: sum of known spend only
    pub untracked_models: Vec<String>,        // new: deduped union of per-session lists
    pub models: BTreeMap<String, ModelTokens>,
}
```

**Mixed-session handling (architect-round-5 outcome):** A session that uses one untracked model alongside priced models has a meaningful partial spend AND a credibility-destroying gap if the partial number is taken at face value. The session-level `spend_usd` is `Some(partial)` reflecting the priced models only; the new per-session `untracked_models` list flags exactly which models contributed unpriced tokens. The renderer flags any session whose `untracked_models` list is non-empty so a 1M-untracked / 1-priced session cannot pass for a real `$0.01` charge. `spend_usd` is `None` only when every model in the session is untracked.

`pricing.rs` API change:

```rust
// before
pub fn calculate_usd(model: &str, usage: &TokenUsage) -> f64;  // 0.0 for unknown

// after
#[derive(Debug)]
pub struct UnknownModel(pub String);

pub fn calculate_usd(model: &str, usage: &TokenUsage) -> Result<f64, UnknownModel>;
```

### Render context block

`cr render` wraps the report YAML in a context block when invoking Opus:

```yaml
persona:
  name: Scott Idler
  title: Director, Engineering
  team: Platform
  manager: Mark Weiler (mark.weiler@tatari.tv)
  email: scott.idler@tatari.tv
  github: escote-tatari
  # ... or: persona: {} when persona whoami fails
options:
  include-tradeoffs: false
report:
  schema-version: 1
  generated: 2026-04-27T19:42:08Z
  # ... full report YAML embedded here
```

The pmt already handles `persona: {}` and `options.include-tradeoffs: false` gracefully (we wrote it that way deliberately).

### Implementation Plan

#### Phase 1: Schema fields and pricing tristate (combined)
**Model:** opus

Combines what was originally Phase 1 + Phase 2. Keeping them in one phase avoids touching `SessionEntry` twice. Per the Architect's sequencing concern, schema-shape changes land together.

Framing note: this is not a true "revert" from v2 to v1. The project is unshipped; no external consumer ever saw v2. We're collapsing the local (v1, v2) history into a single v1 label that describes the post-alignment shape. The shape itself is new (gains tristate `spend_usd`, `untracked_models`, `jsonl_paths`); the version number is reset because nothing else cared about it yet.

- `src/pricing.rs`: define `pub struct UnknownModel(pub String)`. Change `calculate_usd` return type from `f64` to `Result<f64, UnknownModel>`. Replace the silent `else { 0.0 }` branch with `else { Err(UnknownModel(model.into())) }`.
- `src/report.rs`: change `SCHEMA_VERSION` from 2 to 1. Change `ModelTokens.spend_usd: f64` to `Option<f64>`. Change `SessionEntry.spend_usd: f64` to `Option<f64>`. Add `SessionEntry.untracked_models: Vec<String>`. Add `SessionEntry.jsonl_paths: Vec<PathBuf>` (already in `SessionSummary`, just thread through). Add `Totals.untracked_models: Vec<String>`.
- `src/report.rs::ModelTokens::from_totals`: on `Ok(f)`, `spend_usd = Some(round_cents(f))`. On `Err(UnknownModel(_))`, `spend_usd = None`.
- `src/report.rs::to_entry`: collect the session's untracked model names into `entry.untracked_models`. Compute `entry.spend_usd` as `Some(sum_of_priced)` when at least one model is priced, `None` when every model is untracked.
- `src/report.rs::build_report`: dedupe-merge per-session `untracked_models` into `Totals.untracked_models`.
- `src/session.rs::SessionSummary::total_spend_usd`: handle the `Result` (sum priced models, ignore unknowns).
- Tests:
  - All-priced session: `spend_usd: Some(x)`, `untracked_models: []`.
  - All-untracked session: `spend_usd: None`, `untracked_models: [name]`.
  - Mixed (priced + untracked): `spend_usd: Some(partial)`, `untracked_models: [unknown_name]`. **This is the round-5 hazard test.**
  - `Totals.untracked_models` is the deduped union across all sessions.
  - Round-trip a YAML with `null` and confirm serde reads it back as `Option::None`.

#### Phase 2: `--include-tradeoffs` flag
**Model:** sonnet

- `src/cli.rs::RenderArgs`: add `#[arg(long)] pub include_tradeoffs: bool`.
- `src/config.rs::RenderConfig`: add `pub include_tradeoffs: bool`. Plumb from args.
- `src/render.rs`: when sending to Opus, build the context block YAML with `options.include-tradeoffs: <bool>`.
- Tests: `RenderConfig` with `include_tradeoffs: true` produces a context block whose `options.include-tradeoffs` is `true`.

#### Phase 3: Persona integration with timeout
**Model:** opus

Architect-round-5 raised the hang risk. `std::process::Command::output()` does not have a built-in timeout, so an expired Okta session could hang `cr render` indefinitely. Solution: use the `wait-timeout` crate (single-purpose, ~200 lines, no transitive deps) to bound the child process.

- Add `wait-timeout = "0.2"` to Cargo.toml via `cargo add wait-timeout`.
- New module `src/persona.rs`: `pub fn whoami() -> Option<PersonaBlock>`. Spawn `persona whoami --json` via `Command::spawn()` (not `output()`). Use `wait_timeout::ChildExt::wait_timeout(Duration::from_secs(5))`. On timeout, `kill()` the child and return `None`. On any other failure (spawn failure, non-zero exit, JSON parse error), log `warn!` and return `None`. Print a single line to stderr ("persona whoami failed; rendering anonymously") so the user knows the report won't carry their identity.
- Parse the known fields (name, title, team, organization, department, manager, email, github, location) into a `PersonaBlock` struct. All fields `Option<String>`; missing fields serialize as omitted YAML keys (the pmt handles this).
- `src/render.rs::render_via_opus`: call `persona::whoami()`. If `Some`, embed under `persona:`; if `None`, emit `persona: {}`.
- Tests:
  - `whoami()` returns `None` when binary is missing (mock via `PATH=`).
  - Parsing happy-path JSON populates fields correctly.
  - Missing fields stay `None` and round-trip as omitted YAML keys.
  - Timeout case: spawn a fake binary that sleeps longer than the timeout, confirm `whoami()` returns `None` within the timeout window and the child is reaped.

#### Phase 4: Baked-in default report.pmt with workspace override
**Model:** sonnet

Architect-round-5 raised local-dev staleness. `include_str!` bakes the template at compile time, so workspace edits are silently ignored. Fix: prefer a workspace-local file if present, fall back to the embedded copy.

- Rename `templates/justification.pmt` to `templates/report.pmt`.
- `src/render.rs`: add `const DEFAULT_PROMPT: &str = include_str!("../templates/report.pmt");`.
- Resolution order when `cfg.prompt` is `None`:
  1. `./templates/report.pmt` (workspace-relative). If exists, read it.
  2. Otherwise, use `DEFAULT_PROMPT`.
- `--prompt <path>` still overrides everything when supplied.
- Tests:
  - No `--prompt`, no workspace file: uses `DEFAULT_PROMPT`.
  - No `--prompt`, workspace file present: reads the workspace file (and a regression test asserts modifying the workspace file changes the template at runtime).
  - `--prompt <path>` always wins.
  - Build-time invariant: a separate test asserts `DEFAULT_PROMPT` is byte-identical to `fs::read_to_string("templates/report.pmt")` (catches drift between the two when both are touched).

#### Phase 5: pricing.yml mirror (deferred)
**Model:** sonnet

Architect-round-5 rejected this in the same pass without a sync script. Agreed. Defer.

- This phase is **dropped from this batch**. cr keeps `data/pricing.json` only. The YAML mirror and `bin/update` synchronization land in a follow-up pass when we have time to write the deterministic scraper. Single source of truth (JSON) until then.

#### Phase 6: Doc-vs-code drift fixes
**Model:** sonnet

Update `docs/design/2026-04-27-claude-report.md`:

- Schema version: change every "schema-version: 2" / "v2" reference to 1, and add a sentence noting "this is the first stable schema; v2 is reserved for the next breaking shape change."
- Pricing data file: state that `data/pricing.json` is the canonical embedded form. Drop any reference to `pricing.yml` or `pricing-page.sha256` (deferred to a future pass).
- CLI surface: the default subcommand is named `Collect` (running `cr` with no subcommand is equivalent to `cr collect`); `cr merge` is a stub.
- Phase 4 git-toplevel-guard description: the implementation uses a blocked-roots list (`$HOME` by default), not a `toplevel == cwd || prefix-of-cwd` comparison. The blocked-roots approach correctly catches dotfiles climbing without rejecting legitimate climbs (e.g. cwd in a deep subdir of a tracked repo).
- `jsonl-paths`: mark as implemented (was an aspiration, now a fact).
- Schema diff: add a note that `SessionEntry.spend_usd` is `Option<f64>`, that `SessionEntry.untracked_models` and `Totals.untracked_models` exist per architect round 4-5.

## Alternatives Considered

### Alternative 1: Land each item as a separate small PR
- **Description:** Seven small PRs, one per item.
- **Pros:** Easier to review individually; smaller bisect surface if one regresses.
- **Cons:** Items 1 (tristate) and 4 (jsonl-paths) both touch `report.rs` data structures, and item 7 (doc fixes) wants to land *after* the code is correct. Sequencing seven PRs introduces more coordination cost than one well-scoped pass.
- **Why not chosen:** The items are tightly related and the pass is small enough to land in one batch. Single PR.

### Alternative 2: Per-session `spend_usd` always sum priced models, never `Option<f64>`
- **Description:** Simpler schema: per-session spend is always a number, treating untracked-only sessions as `0.0`.
- **Pros:** No `Option` to handle in consumers.
- **Cons:** Reintroduces the silent-undercount risk the architect-round-3 decision specifically eliminated. A session with 100% untracked tokens would report `$0.00` with no flag.
- **Why not chosen:** Tristate is the explicit decision from architect round 4. Stick with it.

### Alternative 3: Embed `pricing.yml` instead of `pricing.json`
- **Description:** `include_str!("../data/pricing.yml")` and parse with serde_yaml at startup.
- **Pros:** One file, one source of truth.
- **Cons:** YAML parsing is slower than JSON parsing (negligible at this size, but a real difference in micro-benchmarks); ccu's pattern carries both. Following ccu means lower mental load when both repos eventually merge into a shared crate.
- **Why not chosen:** Match ccu. Embed JSON for parsing speed; keep YAML for human edits and grep-friendly diffs.

## Technical Considerations

### Dependencies

One new crate: `wait-timeout` (~200 lines, no transitive deps), used to bound the `persona whoami` shell-out so an expired Okta session can't hang `cr render`. `persona` itself is shelled out via `std::process::Command` (same pattern `repo.rs` uses for `git`).

### Performance

Negligible impact. The tristate change adds an `Option` discriminant per `ModelTokens` (1 byte). The persona shell-out adds one fork+exec per `cr render` invocation, bounded by a short timeout. Pricing YAML mirror adds zero runtime cost (only the JSON copy is embedded).

### Security

`persona whoami --json` is read-only and authenticated via Okta (already part of Scott's environment). Failure to obtain persona just falls back to anonymous mode. No new secrets or network surface.

### Testing Strategy

- `pricing.rs`: unit tests for known model gives `Ok(spend)`, unknown gives `Err(UnknownModel)`, normalization edges still work.
- `report.rs`: unit tests for `build_report` populating `untracked_models`, `to_entry` setting `spend_usd: Option<f64>` correctly across mixed/all-priced/all-untracked sessions, `jsonl_paths` round-tripped to YAML.
- `render.rs`: unit tests for context-block construction with persona present/absent, `include_tradeoffs` true/false, baked-in default prompt invoked when `--prompt` is omitted.
- `persona.rs`: unit test for `whoami()` returning `None` when binary is missing (mock via `PATH=`), parsing happy-path JSON, missing-fields-are-None.
- End-to-end: a single fixture `~/.claude/projects` tree with a known-priced model and a synthetic unknown-model entry, run `cr` (collect), verify YAML has `untracked-models: [...]` and `spend-usd: null` on the unknown row.

### Rollout Plan

- Local install via `cargo install --path .` after the changes land.
- Bump version via `bump` after one successful end-to-end run on real `~/.claude` data.
- Existing on-disk `claude-report.yml` files (if any) are read for title preservation only (`sessions.<id>.title`); the rest is regenerated. Schema-version drop from 2 to 1 in the on-disk file is harmless because nothing else cares about the version field yet.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| `Option<f64>` serde edge cases (e.g. distinguishing missing from null) | Low | Low | serde_yaml emits `null` for `Some(None)`-style cases; round-trip is clean. Tests cover the three cases (Some, None, missing). |
| Persona binary returns valid JSON but unexpected schema | Low | Low | All persona fields are `Option<String>` in the local struct. Missing fields serialize as omitted keys. Pmt handles missing fields. |
| Schema-version revert breaks an older on-disk file | Very Low | Very Low | Project hasn't shipped externally. Title preservation only reads `sessions.<id>.title`. The version label is informational for now. |
| Embedded default report.pmt drifts from the file in `templates/` | Low | Med | `include_str!` is build-time; the binary always reflects the file at build. A regression test asserts the embedded string equals the file on disk, catching out-of-band edits. |
| `--include-tradeoffs` set but custom `--prompt` doesn't reference it | Low | Low | The flag is in the context block regardless. Custom prompts that ignore it still get the same context; no breakage. |
| Persona shell-out hangs (e.g. Okta auth prompt) | Med | Med | `wait-timeout` crate enforces a 5s budget. On timeout, kill the child and fall back to anonymous mode. Without this, `std::process::Command::output()` would hang indefinitely (architect-round-5 finding). |
| Embedded prompt drifts from workspace `templates/report.pmt` | Med | Med | Workspace-local `templates/report.pmt` takes precedence over the embedded copy at runtime; a build-time invariant test asserts both are byte-identical at compile (architect-round-5 finding). |
| YAML consumer breakage from `Option<f64>` (`null` for unpriced) | Low | Low | No external consumers of the YAML exist yet. The point of `null` is to make consumers crash on missing data rather than silently misreport `0.0`. The renderer (only consumer) is updated in the same pass. |

## Open Questions

All resolved post-architect-round-5:

- [x] **Embed both JSON and YAML pricing, or only JSON?** Defer the YAML mirror entirely until the sync script lands. JSON only.
- [x] **Persona shell-out timeout duration?** 5 seconds, enforced via `wait-timeout`.
- [x] **Should `cr render` warn the user (not just log) when persona is missing?** Yes, single stderr line in addition to the `warn!` log.
- [x] **Mixed-session spend reporting?** Per-session `spend_usd: Some(partial)` of priced models, plus per-session `untracked_models: Vec<String>` flagging which models contributed unpriced tokens. Renderer flags any session with a non-empty list.

## References

- Original architecture doc: `docs/design/2026-04-27-claude-report.md`
- Architect rounds 1-4 (concerns and resolutions) summarized in the original doc's review-passes header
- ccu pricing pattern: `~/repos/tatari-tv/claude-cost-usage/data/pricing.{json,yml,sha256}` and `bin/update`
- Existing implementation modules: `src/{cli,config,parse,pricing,render,repo,report,scan,session,summarize,title}.rs`
