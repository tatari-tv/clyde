# Implementation Notes: Session Efficiency & Behavior Signals

Running, append-only record of how the implementation diverges from or interprets
the design doc `docs/design/2026-07-22-session-efficiency-signals.md`. One section
per phase, four buckets each ("None." where empty).

## Phase 0: Signal-fixture spike

### Design decisions

- Fixtures live at the WORKSPACE root, `fixtures/efficiency/*.jsonl`, not inside
  a crate. The `efficiency` lib crate doesn't exist until Phase 1; rooting the
  fixtures one level up means Phase 1's `scaffold`-generated `efficiency/`
  directory cannot collide with them, and any future crate's tests can reach
  them via a relative `../../fixtures/efficiency/*.jsonl` `include_str!` path.
- One fixture file per signal class rather than one giant multi-signal file:
  `tool-errors.jsonl`, `interrupts.jsonl`, `compaction.jsonl`,
  `turn-duration.jsonl`, `usage.jsonl`, `clean-session.jsonl` — mirrors the
  design doc's own "Signals (full scope)" section headings, so Phase 3's
  extractor tests can name the fixture they're proving.
- `tool-errors.jsonl` deliberately carries all three cases (bash exit-code
  failure, non-Bash framework failure, healthy non-error call) in ONE file so
  the strict-subset invariant (`bash_command_failures <= tool_errors`, never
  equal, never independent) is provable from a single fixture rather than
  cross-referencing two files. Exact field paths, predicate definitions, and
  which session ids each shape was verified against are documented in
  `fixtures/efficiency/README.md` (not duplicated here — single source).
- Verification script `fixtures/efficiency/bin/verify-fixtures.sh` (throwaway
  `jq`, per the phase's own success criteria) asserts every path in the README
  resolves and the subset invariant holds; it is deliberately NOT wired into
  `otto ci` — it is a one-off spike artifact Phase 3's real Rust tests will
  supersede, per the phase-implementer's "never fake or stub" guidance
  applied to test tooling: don't manufacture permanent CI machinery for a
  phase whose own success criteria call it a spike.
- Everything is redacted or synthesized from a *verified real record shape*:
  no raw prompt, diff, file content, or command output survives into a
  fixture; only field names/nesting, booleans, fixed framework marker
  strings (the interrupt text markers), and non-sensitive numeric values
  (token counts, durations) are real.

### Deviations

- **`toolUseResult.interrupted:true` does not occur anywhere in the sampled
  corpus.** `fixtures/efficiency/interrupts.jsonl`'s structured-interrupt
  record is SYNTHESIZED (real object shape, `interrupted` field hand-flipped
  to `true`), not harvested verbatim, because a full scan of all 2,883
  session files / 39,358 occurrences of the `interrupted` key found zero
  `true` values. Same effect, correct seam: the shape is real, only the value
  is invented. Phase 3 should treat this predicate as untested-against-a-real-
  positive until a genuine interrupted-Bash-call transcript surfaces.
- **`compactMetadata.trigger:"manual"` does not occur anywhere in the sampled
  corpus.** Every compaction observed live was `"auto"`. The `manual` record
  in `fixtures/efficiency/compaction.jsonl` is SYNTHESIZED (real
  `compactMetadata` shape, `trigger` hand-set to `"manual"`) — same
  same-effect-correct-seam reasoning as above.
- **`bash_command_failures`'s text pattern lives in the top-level
  `toolUseResult` field, not `message.content[].content`.** The design doc
  says "the result text matches the `Error: Exit code N` shape" without
  naming which field; live data shows the `message.content[]` tool_result
  block's own `content` string is `"Exit code N\n..."` (no `Error:` prefix),
  while the sibling top-level `toolUseResult` field collapses to the string
  `"Error: Exit code N\n..."` ONLY on a Bash failure (it is the
  `{stdout,stderr,interrupted,isImage,noOutputExpected}` object on success).
  Fixtures and the README lock the predicate onto `toolUseResult` (the field
  that actually carries the literal `"Error: Exit code N"` text) — Phase 3
  should implement against this field, not `message.content[].content`.

### Tradeoffs

- Redacted/reconstructed fixtures over raw-copied live files: raw copies
  would be the most "verbatim harvested," but would ship real prompts, file
  paths, and command output (some referencing internal Tatari repos/infra)
  into a public-shaped git history. Chose structurally-faithful redaction
  (real schema, placeholder content) over verbatim copies — matches the
  task's explicit redaction requirement and the org's "never commit secrets"
  policy; the tradeoff is fixtures are hand-assembled JSON rather than a
  straight `cp`, so a subtle schema quirk not seen in the *specific* records I
  sampled could still be missed. Mitigated by keeping the field-by-field
  provenance trail in the README so later phases can re-verify against fresh
  live samples if a metric looks wrong.
- One `jq` script covering all six fixtures over six small standalone
  scripts: less "throwaway per fixture," but a single script means the
  subset-invariant check (which spans only `tool-errors.jsonl`) sits next to
  the per-fixture path checks instead of being orphaned in its own file.

### Open questions

None. Phase 0 has no design decisions requiring Scott's confirmation — it is
a data-gathering spike with no production code or API surface.

## Phase 1: Scaffold `efficiency` lib crate + umbrella wiring

### Design decisions

- New workspace member `efficiency` created as a plain clyde-native lib crate
  (`efficiency/Cargo.toml`, `efficiency/src/lib.rs`, `efficiency/src/cli.rs`),
  copying the `pub mod cli; pub fn run(args, common::Globals) -> eyre::Result<i32>`
  shape from `report`/`cost`/`permit` exactly, since the design doc's
  Phase 5+ dispatch (`dispatch_tool(efficiency::run(args, globals), debug)`)
  requires that signature.
- No `build.rs` and no `[[bin]]`. `report`/`cost`/`permit` carry a `build.rs`
  (`GIT_DESCRIBE` for their own now-retired standalone `ccu`/`cr`/
  `claude-permit` shims), but `efficiency` never had one — it matches
  `session`/`sessions`/`common`, the genuinely clyde-native crates in this
  workspace, none of which carry `build.rs`.
- `efficiency::run` does **not** install its own logger (no `log_file_path`/
  `setup_logging` duplicated from the sibling crates). `clyde/src/main.rs`
  already initializes one shared logger for every clyde-native arm
  (`setup_logging(&level, &log_path)`) before the `Command` match runs;
  `report`/`cost`/`permit` are the ONLY arms excluded from that
  (`matches!(cli.command, Command::Report(_) | Command::Cost(_) |
  Command::Permit(_))`, `clyde/src/main.rs:102`) because they are absorbed
  legacy shims that must stay behavior-exact with their pre-merge standalone
  logging. `efficiency` has no such legacy behavior to preserve, so it is
  deliberately **not** added to that skip-list — it reuses clyde's logger
  like `Bootstrap`/`Doctor`/the `Sessions` subtree do. This is documented
  inline at both call sites (`efficiency/src/lib.rs`, `clyde/src/main.rs`).
- `EfficiencyArgs` (`efficiency/src/cli.rs`) is an empty `#[derive(Args)]`
  struct for this phase — no subcommands or flags. The design doc's
  `session <id>` / `daily` / `weekly` / `--worst` / `--json` surface is
  explicitly Phase 5 (Output surfaces), not this phase.
- `Command::Efficiency(efficiency::EfficiencyArgs)` added to the `Command`
  enum in `clyde/src/cli.rs` (grouped with the other tool-shaped variants,
  right after `Permit`), and the dispatch arm
  `Command::Efficiency(args) => dispatch_tool(efficiency::run(args, globals), debug)`
  added to `clyde/src/main.rs`'s `run()` match (right after the `Permit` arm).
  Workspace `Cargo.toml:2` members list and `clyde/Cargo.toml` gained the new
  path dependency, both hand-edited to match the existing alphabetical/lint
  style (no external crate versions to fetch via `cargo add` — the only new
  deps are `clap`/`common`/`eyre`/`log`, all already pinned at the workspace
  level or path-local).

### Deviations

- The design doc's own Architecture section (line ~90) names the wiring
  check as "the `Report|Cost|Permit` special-case at `clyde/src/main.rs:102`
  (help_target)" as if it were one thing. It is actually two distinct
  mechanisms at different locations: (1) the own-logging skip-list at
  `clyde/src/main.rs:102` (a `matches!` on `cli.command`), and (2) the
  REQUIRED-TOOLS `--help` special-case (`cli::help_target`, defined in
  `clyde/src/cli.rs:290-321`, invoked in `clyde/src/main.rs:51-74`). Both
  were checked explicitly: (1) resolved as "do not add" per the design
  decision above; (2) does not need an entry either, since `efficiency` in
  Phase 1 shells out to no external binary and has no subcommand-specific
  `--help` block to attach. Same effect as the doc intended (both special
  cases audited), correct seam identified for each.

### Tradeoffs

- Reusing clyde's shared logger (vs. giving `efficiency` its own
  `log_file_path`/`setup_logging` copy like the three absorbed tools) trades
  away symmetry with `report`/`cost`/`permit`'s file layout for avoiding a
  needless duplicate `env_logger::Builder::init()` call, which would panic
  ("attempted to set a logger after the logging system was already
  initialized") the moment `efficiency` was NOT added to the main.rs
  skip-list. Since there is no legacy-shim reason to own a separate logger,
  simpler and correct beats sibling-mirroring here.

### Open questions

None. Phase 1 is a pure scaffold with no design ambiguity: `clyde efficiency`
runs and exits 0, `otto ci` is green, and every choice above follows directly
from "match the sibling crates' idioms" plus "`efficiency` is clyde-native,
not absorbed."

## Phase 2: Per-session token aggregation (pure Rust math)

### Design decisions

- **Shared-helper refactor (disclosed, in-scope per the design doc):** extracted
  `cache_read_share` into a new `common::metrics` module
  (`common/src/metrics.rs`, `pub fn cache_read_share(input, cache_read,
  cache_5m_write, cache_1h_write) -> Option<f64>`), re-exported as
  `common::cache_read_share`. Repointed `report::aggregate::compute_cache_stats`
  (`report/src/aggregate.rs`) to call it instead of hand-rolling the ratio
  inline. `report`'s pre-existing zero-denominator convention (render `"0.0%"`,
  never blank) is preserved at the call site via
  `.map(|r| r * 100.0).unwrap_or(0.0)` -- the shared helper itself returns
  `None` on a zero denominator (what `efficiency` needs to render `n/a`);
  `report` chooses to fold that `None` back to `0.0` for its own display
  convention. Verified by TEMPORARILY perturbing the formula
  (`+ 1.0` on the percentage) and confirming `report`'s existing
  `cache_counterfactual_equals_hand_computed_value` test failed
  (`left: "91.0%" right: "90.0%"`), then reverting -- proves the repoint is
  real wiring, not a no-op, and that `report`'s tests stay green UNCHANGED
  against the shared helper.
- `efficiency::metrics` (`efficiency/src/metrics.rs`) holds `RawCounters`
  (token/cost fields only -- see Deviations), `EfficiencySignals`, and
  `aggregate_tokens(entries: &[AssistantEntry]) -> EfficiencySignals`, which
  sums one scope's `claude_pricing::AssistantEntry`s (as returned by the
  crate's existing `parse_jsonl_file`) into `RawCounters` and derives:
  `cache_read_share` (via the shared `common::cache_read_share` helper),
  `cache_1h_write_fraction`, `tokens_per_turn`, `cost_per_turn_usd`.
  `RawCounters::total_tokens()` mirrors `report::session::TokenTotals::total`
  (sum of input/output/cache-read/cache-5m/cache-1h) for consistency with the
  existing in-house pattern.
- Cost is computed via the existing free function
  `claude_pricing::calculate_usd(model, usage)` (embedded default pricing, no
  `fetch` feature, no network) -- per entry, summed. An unpriced model
  contributes `$0` to `cost_usd` and logs a `warn!` once per occurrence,
  matching the exact skip-and-warn pattern already used at `cost/src/lib.rs:360`
  and `report/src/report.rs:90` (never a hard failure over one unknown model
  id; no new invented behavior).
- `claude-pricing` was added to `efficiency/Cargo.toml` via
  `cargo add --path ../pricing` (no `fetch` feature enabled), confirmed by
  `git diff 41153a1 --stat -- pricing/` showing zero pricing-crate changes.
- Tests load the Phase 0 golden fixtures directly from disk via
  `concat!(env!("CARGO_MANIFEST_DIR"), "/../fixtures/efficiency/<name>.jsonl")`
  and `claude_pricing::parse_jsonl_file`, rather than embedding fixture text
  with `include_str!` + a temp file -- one fewer moving part, and it exercises
  the real parse path end-to-end (same as `pricing`'s own test pattern, minus
  the temp-file step since the fixtures already live on disk at a fixed
  relative path).

### Deviations

- **Partial `RawCounters`/`EfficiencySignals`, not the full Data Model struct.**
  Per the task's explicit "your call, document it": Phase 2's `RawCounters`
  carries ONLY the token/cost fields this phase computes
  (`input_tokens`, `output_tokens`, `cache_read_tokens`,
  `cache_5m_write_tokens`, `cache_1h_write_tokens`, `cost_usd`, `turns`). The
  design doc's full `RawCounters` (Data Model section) also lists
  `turn_durations_ms`, `compactions`, `tool_errors`, `bash_command_failures`,
  `interrupts_structured`, `interrupts_text`, `web_search_requests`,
  `web_fetch_requests`, `effort_high`, `effort_xhigh`, `model_mix`,
  `by_skill`, `by_mcp_tool` -- all behavioral counters Phase 3's extractor
  populates. Adding those fields now with nothing to write them would be dead
  weight (and `#![deny(dead_code)]` would flag them). Same reasoning for
  `EfficiencySignals`: Phase 2 has `cache_read_share`,
  `cache_1h_write_fraction`, `tokens_per_turn`, `cost_per_turn_usd`; the
  design's `turn_ms_p50`/`turn_ms_p90`/`turn_ms_max` land in Phase 3 alongside
  turn-duration extraction. Phase 3 should EXTEND these same two structs
  (not invent parallel ones) so the "one struct per scope" shape in the
  design doc holds by the time Phase 3 lands.
- **`tokens_per_turn`/`cost_per_turn_usd` are not named in the design doc's
  `EfficiencySignals` struct** (the doc's Data Model only shows
  `cache_read_share`/`cache_1h_write_fraction`/`turn_ms_*`), but the Phase 2
  bullet explicitly requires "per-turn token and cost figures." Added as two
  more `Option<f64>` fields on the same struct rather than a separate type --
  same scope (per-session derived metric), same None-on-zero-turns rule.

### Tradeoffs

- Cost aggregation calls `claude_pricing::calculate_usd` (the free function,
  embedded pricing) rather than `Pricing::calculate_usd` (the `fetch`-feature
  method `report`/`cost` use for a possibly-refreshed feed). Tradeoff: no
  live-feed fetch means `efficiency`'s cost figures use whatever pricing data
  is embedded in the pinned `claude-pricing` crate at build time, not a
  network-refreshed feed. Chose this because (a) the Phase 2 bullet names
  `claude_pricing::calculate_usd` literally, (b) it avoids pulling in the
  `fetch` feature's `ureq`/`tempfile` network dependency for a lib crate whose
  own design doc states "no new pricing blast radius," and (c) cost here is a
  secondary signal (behavioral efficiency is the point), not the primary
  billing source of truth clyde already has in `cost`.

### Open questions

None. The struct-scoping call (partial vs. full `RawCounters`) was resolved
per the task's own "your call, document it" instruction; Phase 3 has enough
here (the exact field names deferred, and the reasoning) to extend rather
than redesign.

## Phase 3: Behavioral signal extractor

### Design decisions

- **Scope partition by `agentId`, not by file** (`efficiency/src/extract.rs`,
  `extract` + `Scope`). `outcome.rs` is scope-blind (one `FileOutcomes` per
  file, unioned by `group_id` in `session::fold`). Phase 3 instead attributes
  EACH record to a `Scope::Parent` (no `agentId`) or `Scope::Subagent(agentId)`,
  so `extract` returns a `FileEfficiency { parent, subagents }`. This works for
  BOTH transcript layouts uniformly: the live layout (parent and each subagent
  in SEPARATE files under `<session>/subagents/`, each subagent file's records
  carrying one `agentId`) AND the Phase 0 fixture layout (parent + subagent
  records interleaved in ONE file). `fold` (`efficiency/src/fold.rs`) unions the
  per-file `FileEfficiency` across a session group's files. The multi-file live
  layout is covered by `fold::tests::fold_unions_parent_scope_across_multiple_files`.
- **EXTENDED the existing Phase 2 `RawCounters`/`EfficiencySignals`** (not
  parallel structs, per the Phase 2 note): added all behavioral counters
  (`turn_durations_ms`, `compactions`, `tool_errors`, `bash_command_failures`,
  `interrupts_structured`, `interrupts_text`, `web_search_requests`,
  `web_fetch_requests`, `effort_high`, `effort_xhigh`, `model_mix`, `by_skill`,
  `by_mcp_tool`) to `RawCounters`, and `turn_ms_p50`/`p90`/`max` to
  `EfficiencySignals`. Phase 2's `tokens_per_turn`/`cost_per_turn_usd` are kept.
- **`add_usage`/`finalize`/`merge` as the single math seam** (`metrics.rs`).
  Phase 2's `add_entry(&AssistantEntry)` now delegates to a new
  `add_usage(model, &TokenUsage) -> f64` (returns the turn's `$` so the extractor
  can attribute it to a skill/MCP bucket without recomputing cost). `finalize`
  is the ONE recompute path for every derived metric; `aggregate_tokens` (Phase 2)
  now delegates to it. `RawCounters::merge` is the additive union step.
- **Aggregation invariant enforced in `fold`** (`efficiency/src/fold.rs`, `fold`):
  `parent_own` is unioned across the group's files, subagents unioned by
  `agentId`, and `aggregate = finalize(parent_own ⊎ every subagent's counters)`
  -- a ratio-of-sums for the cache ratios and a percentile recompute over the
  UNIONED `turn_durations_ms` sample, never a field-sum/average of sub-scope
  derived metrics. `fold::tests::aggregate_equals_recompute_of_parent_and_subagents`
  pins `aggregate == recompute(parent_own ⊎ subagents)`; the accompanying
  `aggregate_cache_share_is_ratio_of_sums_not_average_of_ratios` and
  `aggregate_percentiles_recompute_from_unioned_sample` prove the invariant BITES
  (the ratio-of-sums differs from the mean of per-scope ratios; the union p50
  differs from any single scope's).
- **`tool_errors`/`bash_command_failures` are self-contained on the tool_result's
  own user record** -- no `tool_use_id` pending map (`apply_tool_results`).
  `tool_errors` = `is_error==true` blocks; `bash_command_failures` increments at
  most once per record, only when the record already contributed to `tool_errors`
  AND its top-level `toolUseResult` string matches `^Error: Exit code \d+`. This
  structurally guarantees `bash <= tool_errors` in every scope (asserted across
  every fixture in `extract::tests::bash_command_failures_never_exceeds_tool_errors_per_scope`).
- **`CompactionTrigger` is a typed enum** (`Auto`/`Manual`) parsed from
  `compactMetadata.trigger`; both live-`auto` and synthesized-`manual` values are
  handled, and an unrecognized trigger is `warn!`-and-skipped (fail closed, never
  fabricate a trigger), per the Phase 0 note that neither `interrupted:true` nor
  `trigger:"manual"` occur in the live corpus.
- **Own raw serde structs, tolerant (not `deny_unknown_fields`)** for the
  non-pricing fields (`extract.rs` `Record`/`Message`/`Usage`/`CacheCreation`/
  `ServerToolUse`/`CompactMetadata`). These are external Claude Code logs -- a
  deliberately forward-compatible wire shape (the house carve-out for tolerant
  wire frames), so a newer Claude Code field can't fail the parse. `Usage`
  replicates `claude_pricing::parse.rs:173-180`'s exact 5m/1h derivation so
  Phase 3 token totals stay identical to Phase 2's `parse_jsonl_file` path
  (cross-checked by `fold::tests::scope_split_then_refold_equals_flat_whole_file_totals`).
- **New fixtures** (documented in `fixtures/efficiency/README.md`):
  `multi-subagent.jsonl` (parent + 2 subagents; the scope-split + aggregation
  invariant + positive coverage for effort/web/model_mix/by_skill/by_mcp_tool)
  and `malformed-line.jsonl` (skip-and-log proof).

### Deviations

- **Cost math reuses `claude_pricing::{TokenUsage, calculate_usd}`, NOT
  `parse_jsonl_file`.** The design's Overview says token totals reuse
  `parse_jsonl_file -> AssistantEntry`, but `AssistantEntry` carries no
  `agentId`, so it cannot support the per-scope split Phase 3 requires. `extract`
  parses each record's `usage` in its own raw struct and reuses the pricing
  crate's `TokenUsage` + `calculate_usd` for the cost. Same cost engine, correct
  seam; zero pricing change (verified: `git diff ebd1fed -- pricing/` empty).
- **`service_tier` / `inference_geo` / `iterations` are tolerated but NOT
  stored.** The design lists them as "retained, informational" but the Data
  Model `RawCounters` has no field for them and the Phase 3 counter list omits
  them; storing an unconsumed field is dead weight under `#![deny(dead_code)]`.
  The tolerant raw structs simply ignore them (they remain available to a later
  phase that gives them a home). Same effect as the doc's intent (they don't
  break the parse), correct seam (no phantom field).
- **`bash_command_failures` keys on the TOP-LEVEL `toolUseResult` string**, per
  the Phase 0 note, not `message.content[].content` -- the latter reads
  `"Exit code N"` without the `Error:` prefix.
- **`SessionEfficiency.flags` is `Vec<EfficiencyFlag>` with an empty enum.**
  Scoring is Phase 4; the field exists in the Data Model shape but is always
  empty in Phase 3 (the enum has no variants yet), so nothing pretends to score.
- **Phase 3 in-memory types carry no `serde` derive** (only
  `Debug`/`Clone`/`Default`/`PartialEq`), mirroring `outcome.rs`'s `FileOutcomes`.
  Serialization/persistence is Phase 6; adding `Serialize` now (and resolving the
  empty-enum-serde question for `EfficiencyFlag`) would be premature.

### Tradeoffs

- **Percentiles use the nearest-rank method** (`ceil(p*n)`-th value). Simple,
  deterministic, integer-valued (no interpolation/rounding ambiguity), and it
  reproduces the README's stated median (44268) for `turn-duration.jsonl`. The
  tradeoff: for small samples p90 can coincide with max (e.g. n=7 here), which
  is mathematically correct for nearest-rank; the distinct-p50/p90/max property
  is proven separately on a 10-element sample in
  `metrics::tests::turn_duration_percentiles_p50_p90_max_are_distinct`.
- **One unified line-by-line parse in `extract`** rather than a pricing pass plus
  a separate behavioral pass. Slightly more parsing logic in one place, but it
  is the only way to split tokens by `agentId`, and it keeps the "second read of
  a page-cache-hot file" performance shape the design calls for.

### Open questions

None. The one spec ambiguity (which parse seam supports per-scope token
splitting) was resolvable from the code -- `AssistantEntry` has no `agentId`, so
the own-struct parse + `calculate_usd` reuse is forced, not a judgment call.
Cross-repo / system-mutating bullets: none in Phase 3 (clyde-only).
