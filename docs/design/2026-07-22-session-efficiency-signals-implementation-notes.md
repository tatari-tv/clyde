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

## Phase 4: Scoring + threshold flagging

### Design decisions

- **`efficiency:` config section is a distinct `EfficiencyConfig` struct**
  (`common/src/config.rs`) nested under `Config` via `#[serde(default)]`, mirroring
  the existing `RenderConfig`/`render:` pattern exactly: `#[serde(rename_all =
  "kebab-case")]` + `deny_unknown_fields`, a HAND-WRITTEN `impl Default` (not
  derived), per-field `#[serde(default = "...")]` free functions, and private
  fields with public getter methods. Hand-writing `Default` is load-bearing here
  (not just convention): a derived `Default` would give floor `0.0` / ceiling
  `0.0` / gates `0`, and a floor of `0.0` flags nothing while a ceiling of `0.0`
  flags everything -- both diverge from what a missing config must resolve to
  (the house "derived Default can produce an invalid value" footgun).
- **`tool-error rate` denominator = `tool_errors / tool_calls`** where `tool_calls`
  is the count of ALL `tool_result` blocks (errored or not), added as a new
  `RawCounters` field this phase. This is the FIRST denominator the design doc
  names ("tool_errors / total tool calls", line 69-adjacent Phase 4 guidance) and
  the most defensible: every completed tool call yields exactly one `tool_result`
  block, so `tool_errors` (the `is_error==true` subset) is structurally `<=
  tool_calls` and the rate is always in `[0, 1]`. The rejected alternative was
  `tool_errors / turns`: turns is available without touching the extractor, but
  it is a poor denominator (many turns make no tool call at all -> understates;
  one turn can make several -> a "rate" that exceeds 1.0). `tool_error_rate` is a
  derived metric computed in `metrics::finalize` (the ONE recompute path), so the
  aggregate's rate is a ratio-of-sums over unioned counters, not an average of
  per-scope rates -- the Aggregation invariant holds for it automatically.
- **`EfficiencyFlag` given three variants** (`efficiency/src/fold.rs`, filling the
  Phase 3 empty enum): `LowCacheReadShare { observed, floor }`,
  `HighToolErrorRate { observed, ceiling }`, `AutoCompaction { count }`. Each
  carries the observed value AND the threshold it crossed, so a flag is
  self-describing (fail loudly / legible, per the task) without the consumer
  re-deriving why it tripped.
- **Scoring is a pure function returning data** (`efficiency/src/score.rs`,
  `score(&EfficiencySignals, &EfficiencyConfig) -> Vec<EfficiencyFlag>`), with a
  thin `scored(SessionEfficiency, &EfficiencyConfig) -> SessionEfficiency` seam
  that populates `flags`. `fold` (Phase 3) stays config-free and always leaves
  `flags` empty, so its aggregation-invariant tests are untouched; the caller
  (Phase 5 run path) composes `scored(fold(...), config)`. The scoring entry fn
  DEBUG-logs every threshold and the observed value it checks, per the
  function-level logging rule.
- **Eligibility gate scopes to cache-waste ONLY.** `eligible = total_tokens >=
  minimum-total-tokens && turns >= minimum-turns` gates `LowCacheReadShare`.
  `HighToolErrorRate` and `AutoCompaction` are NOT gated: an error-prone or
  ran-to-the-wall session is worth surfacing regardless of size (an ineligible
  short session with an auto-compaction still flags the compaction).

### Deviations

- **`RawCounters` extended with `tool_calls: u64` and `EfficiencySignals` with
  `tool_error_rate: Option<f64>`.** The design's Data Model `RawCounters`
  (lines ~100-121) does not list a total-tool-call counter, and its
  `EfficiencySignals` does not list `tool_error_rate`. Both are required to
  compute the tool-error rate against the doc's own denominator ("tool_errors /
  total tool calls"). Same effect as the doc's intent, correct seam: the counter
  is populated in `extract::apply_tool_results` (one increment per `tool_result`
  block, alongside the existing `is_error` gate) and the rate is derived in
  `finalize`. Extending the Phase 2/3 structs (rather than inventing parallel
  ones) follows the Phase 2 note's "Phase N should EXTEND these same two structs"
  directive.
- **`Config` no longer derives `Eq`** (`common/src/config.rs`). `EfficiencyConfig`
  carries `f64` thresholds, and `f64` is not `Eq`; `PartialEq` (what every
  `assert_eq!` in the config tests needs) is retained. Verified no consumer
  requires `common::Config: Eq` (grep: `common::Config` is referenced only by the
  re-export; the crate-local `Config` types in `cost`/`permit` are unrelated).

### Tradeoffs

- **Scoring unit tests use constructed `EfficiencySignals` (built through the real
  `finalize`) rather than new JSONL fixtures.** Scoring consumes the aggregate
  signals, not raw JSONL, so constructing counters directly pins the eligibility
  boundary and each threshold EXACTLY and legibly (e.g. 20000 tokens / 3 turns on
  the nose), which hunting for a fixture that happens to straddle a boundary
  cannot. The end-to-end path (`extract -> fold -> scored`) is still proven on a
  REAL fixture: `scored_on_multi_subagent_fixture_proves_gate_end_to_end` runs the
  full pipeline on `multi-subagent.jsonl` (2325 total tokens -> ineligible),
  asserting the below-floor cache share is gated OUT while the non-gated
  tool-error and auto-compaction flags fire. No new fixtures were needed; the
  existing set covers the end-to-end case, and constructed signals cover the four
  threshold/eligibility criteria more precisely.
- **Config-driven tests deserialize `EfficiencyConfig` directly via `serde_yaml`**
  (new dev-dependency on `efficiency`) rather than routing through `common`'s
  private `load_from`. Exercises the real kebab-case serde path + per-field
  defaults without exposing loader internals across the crate boundary. The
  `deny_unknown_fields` / bad-type / defaults tests live in `common`'s own config
  tests, against the real `load_from`.

### Open questions

- **The design doc `docs/design/2026-07-22-session-efficiency-signals.md` is still
  UNTRACKED in git** (was untracked at session start and through Phases 0-3; only
  the `-implementation-notes.md` companion is tracked). This phase followed the
  established pattern and did NOT sweep it into the Phase 4 commit. The parent
  orchestrator should decide whether the design doc gets committed (likely at
  finalization) -- flagging it so it does not stay orphaned. Not a code blocker.
  **[Finalization update: RESOLVED. The design doc was committed with the feature
  (Status flipped Approved -> Implemented); it is no longer untracked. This note
  records the point-in-time state during Phase 4.]**

## Phase 5: Output surfaces (subcommand)

### Design decisions

- **New modules, not a monolithic `output.rs`:** `collect.rs` (discovery: scan ->
  group-by-`group_id` -> `extract`+`fold`+`scored` per session, `collect_all` and
  `collect_matching`), `rank.rs` (`--worst N` ordering), `rollup.rs`
  (`daily`/`weekly` bucketing), and `output.rs` (render-only: `*Json` view types +
  `wants_json`/`render`). `collect` is the one seam every surface shares (mirrors
  `cost::compute_summaries`'s single-seam shape); `rank`/`rollup` each own their
  one concern so `output.rs` stays render-only and under the 1500-line limit.
- **`CollectedSession { session_id, last_active, efficiency }`** (`collect.rs`)
  is the unit every surface operates on. `last_active` is the MAX file `mtime`
  across the session's group (parent + subagents), converted to
  `chrono::DateTime<Local>`. This is a NEW field the Data Model never named --
  see Deviations.
- **`wants_json`/`render` copy `cost::wants_json` (`cost/src/lib.rs:637`) EXACTLY**:
  `explicit_json || !stdout().is_terminal()`. Human output is YAML, not a
  hand-rolled table -- the house rule ("yaml for humans, json when piped, one
  `--format` override, no boolean format flags") over-rides the "copy cost's
  literal table shape" instinct, since the design doc names `wants_json` +
  `IsTerminal` as the pattern to copy, not `cost`'s specific text renderer.
  Verified live (`script -qec ... /dev/null` to force a real TTY): TTY renders
  YAML, a pipe renders JSON, and `--json` forces JSON on a TTY.
- **`--path` and `--json` are `global = true`** on `EfficiencyArgs` (`cli.rs`).
  Discovered live: without `global`, clap rejects `--json` typed AFTER a
  subcommand (`clyde efficiency session <id> --json` errored `unexpected
  argument`) because a non-global parent flag only parses BEFORE the
  subcommand token. `cost`'s own `--offline` uses the same `global = true`
  escape for exactly this reason; `--worst` stays non-global since it is
  meaningless with a subcommand present.
- **Domain types (`EfficiencySignals`/`RawCounters`/...) still carry no `serde`
  derive** (per the Phase 3 decision -- Phase 6 owns persistence's shape).
  `output.rs` owns its OWN lightweight `Serialize`-deriving view types
  (`SignalsJson`, `SessionJson`, `WorstEntryJson`, `PeriodJson`, ...) built via
  `From`/helper functions, the same split `cost::output` keeps between
  `SessionSummary`/`DaySummary` (internal) and `TodayJson`/`DailyJson`
  (rendered) -- avoids coupling Phase 6's export/persistence schema to Phase 5's
  CLI rendering schema.
- **`FlagJson`** is a tagged enum (`#[serde(tag = "kind", rename_all =
  "kebab-case")]`) so a rendered flag is self-describing (`{"kind":
  "low-cache-read-share", "observed": ..., "floor": ...}`) without the reader
  re-deriving which threshold fired, matching the design's "self-describing"
  intent for `EfficiencyFlag` itself.
- **`session <id>` matching reuses `SessionFile.group_id` prefix matching**,
  mirroring `cost`'s `Command::Session` id-prefix match exactly (0 matches ->
  "No session found matching '<id>'"; 1 -> render; >1 -> list every matching
  id). Only the matched group's files are extracted (not the whole catalog),
  so a single-session lookup does not pay the full-catalog scan cost.
- **`--worst N` ranking** (`rank.rs`): `sort_by` a comparator where `None`
  (`Ordering::Greater` against every `Some`) always sorts last, and `Some`
  values compare via `f64::total_cmp` (ascending, so `Some(0.0)` -- wrote
  cache, never read it, genuine waste -- sorts first). Verified against the
  REAL catalog on this machine (1557 sessions, `--worst <total>`): 26
  `None`-share sessions all landed at the tail, contiguous, never interleaved
  with a computable share.
- **`daily`/`weekly` bucket by `CollectedSession::last_active`'s local date**,
  union each bucket's `RawCounters` via the existing `merge`, and `finalize`
  ONCE per bucket -- the same Aggregation invariant `fold` enforces per
  session, applied one level up (never a field-sum/average of per-session
  derived metrics). Week bucketing mirrors `cost::dispatch`'s Sunday-start
  logic (`days_since_sunday`) exactly.

### Deviations

- **`CollectedSession.last_active` (a file-mtime-derived date) is not in the
  design's Data Model.** The Phase 3 `RawCounters`/`EfficiencySignals` (by
  design) retain no per-record timestamp -- only aggregated counters and the
  turn-duration SAMPLE survive per scope, so there is no session-level
  timestamp to bucket `daily`/`weekly` by by construction. The session's own
  files' mtime is the correct-seam substitute: it is the SAME signal
  `common::scan::filter_by_date_range`'s existing date prefilter already
  relies on (a file's mtime is a safe proxy for "when this session was last
  touched" under the scan module's own append-only-file assumption). Same
  effect as the doc's "aggregate rollups (mirror cost)" intent, correct seam
  given what Phase 3 actually retained.
- **No `--since`/date-range flag on `session <id>`.** The design's API Design
  section does not name one for the single-session drill-down (only `--worst`
  and `daily`/`weekly` are period-scoped), so `session <id>` scans the WHOLE
  catalog for a matching group id rather than a bounded recent window (unlike
  `cost session <id>`'s 30-day window). This is more correct for efficiency
  drill-down (a session's efficiency doesn't change if queried later) at the
  cost of a full scan per lookup; acceptable since only the MATCHED group's
  files are extracted, not the whole catalog's signals.
- **`--path`/`--json` promoted to `global = true`** (see Design decisions) --
  not explicitly specified by the design doc's flag list, but required for
  `--json` to parse on either side of a subcommand; same effect intended
  ("`--json` -- force JSON; TTY-detect otherwise"), correct seam for clap's
  subcommand-argument scoping rules.

### Tradeoffs

- **YAML (not a hand-rolled ASCII table) for the human/TTY branch.** `cost`'s
  own text renderer is a `comfy-table`-style table; efficiency's session
  signals are a nested structure (totals, ratios, per-workflow maps,
  compaction list) that does not flatten cleanly into table rows. YAML renders
  the full nested shape with zero extra formatting code and matches the
  general "yaml for humans" convention; the tradeoff is `clyde efficiency`'s
  human output looks different from `clyde cost`'s (table) rather than
  visually uniform across the two sibling tools. Chosen because building a
  second table renderer just to look like `cost` would duplicate effort the
  house rule already resolves in YAML's favor for exactly this multi-level
  case.
- **`collect_all` parallelizes over `rayon`'s `par_iter` on the session-group
  map** (one `extract`+`fold`+`scored` per session, in parallel), rather than
  parallelizing at the per-file level like `report`'s collect pass. Simpler
  (one parallel unit = one full session's worth of work, no separate
  reassembly step) at a slight cost: a session with many subagent files does
  its multi-file `extract` sequentially within that one session's rayon task.
  Acceptable since a single session's subagent file count is small relative
  to the session count driving `--worst`/`daily`/`weekly`'s catalog-wide scans.
- **`session <id>`'s "ambiguous prefix" branch prints session ids only** (no
  score/signals per candidate), mirroring `cost`'s own ambiguous-match listing
  shape (`cost` prints `id  $cost (entries)` per candidate; efficiency prints
  just the id) -- kept minimal since disambiguating on id alone is the
  standard fix (retype a longer prefix), and printing full signals for every
  candidate would bury the actual disambiguation prompt.

### Open questions

None. `--path`/`--json` global-flag placement was a live clap parsing failure,
not a judgment call -- resolved directly, documented above. The pre-existing
open question (design doc untracked in git) is unchanged by this phase; still
the parent orchestrator's to resolve at finalization. **[Finalization update:
RESOLVED -- the design doc was committed with the feature.]**

## Phase 6: Catalog persistence + export contract

### Design decisions

- **Dependency direction is `efficiency -> sessions`** (the design's own
  "Dependencies" section: efficiency depends on `sessions` for Phase 6/7). So the
  three sub-parts split by crate along that seam: `sessions` owns the STORAGE
  primitives (schema, write, missing-query, export block) with NO knowledge of
  efficiency's types; the `efficiency` crate owns the COMPUTE+WRITE composition
  (`persist::reindex_efficiency`) that reads `sessions::Db`. No cycle: `sessions`
  depends only on `common`/`session`.
- **Schema v6 = one blob column + three flat indexed scalars.** `efficiency_json`
  TEXT holds the whole nested `SessionEfficiency`; `cache_read_share` REAL /
  `tool_errors` INTEGER / `cost_usd` REAL are the ranking scalars, each with a
  `CREATE INDEX IF NOT EXISTS` (`db.rs` `migrate_v6_efficiency`) so `--worst`/sort
  never parses JSON per row. All four columns are in BOTH `SCHEMA_SQL`'s
  `CREATE TABLE` (fresh DBs) AND `ensure_column`'d in `migrate_v6_efficiency` (old
  DBs) -- the exact v5 `updated_at` pattern.
- **Serde on the efficiency domain types** (`metrics.rs`, `fold.rs`):
  `SessionEfficiency`/`EfficiencySignals`/`RawCounters`/`SubagentEfficiency`/
  `Compaction`/`CompactionTrigger`/`WorkloadCost` gained
  `#[derive(Serialize, Deserialize)] #[serde(rename_all = "kebab-case")]`;
  `EfficiencyFlag` gained the internally-tagged `#[serde(tag = "kind", rename_all
  = "kebab-case")]` form -- byte-identical to the Phase 5 `output::FlagJson`
  rendering, so a persisted flag reads the same as a live-rendered one. This is
  the Phase-3-deferred "serialization is Phase 6" work.
- **Writing efficiency never advances `updated_at`** (`db.rs`
  `set_efficiency_many`): the v5 revision trigger fires on ANY content-less
  `UPDATE`, so a plain efficiency write would bump the export cursor and force
  every `session export --cursor` consumer to re-download the catalog after a
  backfill. `set_efficiency_many` DROPs the `sessions_updated_at_update` trigger,
  does the batch of writes, and recreates it -- all in ONE transaction (rolls
  back / restores the trigger on any error), mirroring the v5 "backfill before
  the triggers exist" precedent exactly. Proven by
  `v6_set_efficiency_stores_columns_without_advancing_updated_at`.
- **Backfill drives off `efficiency IS NULL`, independent of the mtime skip-key**
  (`db.rs` `sessions_missing_efficiency` -> `efficiency::reindex_efficiency` ->
  `collect_ids` -> `set_efficiency_many`). This is the panel-caught gap: because
  `upsert_session` skips unchanged rows by transcript mtime, a bare migration
  leaves every existing session `NULL` forever; the `NULL` predicate finds them
  regardless of mtime. Only the missing set is recomputed (via the new
  `collect::collect_ids`), not the whole tree.
- **Content change invalidates stale efficiency** (`db.rs` `upsert_session`
  UPDATE branch NULLs the four columns): a grown transcript makes the row a
  backfill candidate again so the next pass recomputes it (derived fields must
  never diverge from their source). This invalidation rides the content UPDATE's
  own legitimate cursor bump. Proven by `v6_content_update_nulls_stale_efficiency`.
- **Export efficiency block = opaque `serde_json::Value`** (`export.rs`
  `ExportRecord.efficiency: Option<serde_json::Value>`): the nested shape is OWNED
  by the `efficiency` crate, and (per the dep direction) `sessions` cannot name
  those types anyway, so the contract passes the blob through as the
  forward-compatible-envelope carve-out. Always emitted (`null` when absent). The
  three flat scalar columns are deliberately NOT re-emitted on the wire -- they
  are derivable from the block, and duplicating a derived value where it could
  diverge is a taste violation; they exist purely for server-side ranking.
- **`EXPORT_SCHEMA_VERSION` stays 1** (additive), pinned by
  `export_schema_version_stays_one_after_efficiency_block`. Golden fixtures
  regenerated in this phase: `"efficiency": null` added to the four existing
  fixtures, plus a new `with-efficiency.json` carrying a fully-populated block.
  Contract doc `docs/session-export-contract.md` gained an "Efficiency block"
  section documenting the nested shape and the additive/opaque promise.

### Deviations

- **Necessary migration-hazard fix: `migrate_v5_cursor` now takes `from_version`
  and gates its rowid backfill+seed on `from_version < 5`.** Bumping
  `SCHEMA_VERSION` to 6 makes `migrate` re-enter `migrate_v5_cursor` for every v5
  DB; its UNCONDITIONAL rowid-order backfill would have RESET every live
  `updated_at` revision to its rowid position and reseeded the counter, silently
  rewinding consumers' `--cursor` paging. The `from_version` gate makes the
  backfill run ONLY on a genuine v4->v5 upgrade. Same effect the v5 code intended
  (backfill once), correct seam (version-gated, not re-entrant). Proven to BITE by
  `v6_migration_from_v5_preserves_cursor_and_adds_efficiency_columns` (revisions
  10/20 and counter 20 preserved, not reset to 1/2/2).
- **Export block is `serde_json::Value`, not a typed `ExportEfficiency` struct**
  (see Design decisions) -- forced by the dep direction (sessions can't see
  efficiency's types) and the no-duplicate-derived-field rule. Consequence:
  `ExportRecord`/`ExportEnvelope` DROP `Eq` (kept `PartialEq`) because
  `serde_json::Value`/`f64` are not `Eq`; verified no consumer needs `Eq`
  (mirrors the Phase 4 `common::Config` `Eq` drop).
- **`reindex_efficiency` wired into the explicit `clyde session reindex` only,
  NOT `lazy_reindex`.** A query's cheap incremental refresh must not pay the
  transcript-re-read cost the efficiency compute incurs. Same mechanism, invoked
  at the deterministic reindex point; MCP/lazy wiring can follow if it becomes a
  need.
- **`db/tests.rs` decomposed:** the Phase 6 tests pushed `db/tests.rs` over the
  1500-line bloat limit, so they live in the `db::tests::efficiency` submodule
  (`db/tests/efficiency.rs`), reusing the parent test helpers via `use super::*`.
  A self-contained test surface, extracted per the large-file rule -- no behavior
  change to the existing v1-v5 tests.

### Tradeoffs

- **Trigger DROP/recreate per `set_efficiency_many` batch** vs. re-shaping the v5
  trigger to ignore efficiency-only writes. The batch-level suppression touches
  the trigger twice per backfill run (not per row) and rolls back safely; a
  smarter trigger guard would have to enumerate ~25 content columns to detect
  "efficiency-only", which is fragile and drifts as columns are added. Chose the
  proven-precedent suppression.
- **`reindex_efficiency` recomputes only the `NULL` set** (via `collect_ids`)
  rather than reusing `collect_all` and filtering. Slightly more surface (a new
  public `collect_ids`) but it does not re-read every already-annotated session's
  transcripts on every reindex -- the incremental win the design's `efficiency
  IS NULL` predicate is for.
- **The `with-efficiency.json` fixture's block is hand-authored** (opaque to the
  export round-trip, which only proves lossless passthrough). Serde correctness on
  the REAL `SessionEfficiency` shape is proven separately in the efficiency crate
  (`from_session_scalars_match_the_serialized_json` builds it through the real
  extract->fold->scored pipeline), so the hand-authored fixture is documentation +
  passthrough proof, not the serde-correctness gate.

### Open questions

- **Cross-repo blast radius: the versioned export contract now carries an
  `efficiency` block.** It is additive (`EXPORT_SCHEMA_VERSION` stays 1) and
  opaque, so existing external consumers keep working, but any consumer that wants
  the new signals reads an efficiency-crate-owned nested shape. The parent should
  confirm no external export consumer needs a typed/frozen efficiency schema
  before ship (the design accepted an additive block; this flags it for the
  finalization checklist). Not a code blocker.
- The pre-existing open question (design doc still UNTRACKED in git) is unchanged;
  this phase did NOT commit the design doc, per the phase boundary -- still the
  parent's to resolve at finalization. **[Finalization update: RESOLVED -- the
  design doc was committed with the feature and is no longer untracked.]**

## Phase 7: MCP tool

### Design decisions

- **`session_efficiency` reads the PERSISTED `efficiency_json` blob; it does NOT
  recompute via the pipeline.** This is forced, not a preference: the design's
  dependency direction is `efficiency -> sessions` (Phase 6), so `sessions`
  (where `mcp.rs` lives) CANNOT depend on the `efficiency` crate -- calling the
  Phase 2-5 pipeline from the MCP handler would be a dependency cycle. The tool
  therefore mirrors the export contract exactly: it pulls the stored blob back
  out as an opaque `serde_json::Value` (shape owned by the `efficiency` crate)
  and passes it through verbatim. New DB read `Db::get_efficiency_json`
  (`sessions/src/db.rs`, the READ half of `set_efficiency_many`) returns the raw
  `efficiency_json` TEXT (`NULL` column or absent row -> `None`).
- **Mirrors `session_read` precisely** (`sessions/src/mcp.rs`): same dispatch-match
  entry (`SessionsMcpServer::dispatch`), same `#[tool]` handler shape, same
  `block_in_place_compat` + `self.db.lock()` + `Self::resolve_record` id/prefix
  resolution (so unknown-id and ambiguous-prefix are `invalid_params`, byte-for-byte
  the sibling behavior), same `CallToolResult::success(vec![ContentBlock::json(..)])`
  return. Request type reuses `SessionRef` (the id-only request `session_open`
  already reuses -- efficiency takes only an id, exactly like `session_open`).
- **Response cap reuses `session_read`'s cap verbatim.** New
  `tools::EFFICIENCY_RESPONSE_MAX_CHARS` is DEFINED AS `READ_RESPONSE_MAX_CHARS`
  (`pub const EFFICIENCY_RESPONSE_MAX_CHARS: usize = READ_RESPONSE_MAX_CHARS;`),
  so the two read-side tools share ONE tool-result budget and cannot drift
  (siblings behave identically; one definition kills the class). The cap-enforcement
  test names both constants and asserts their equality.
- **`EfficiencyResult` is a `tag = "state"` union** mirroring
  `OpenResult`/`GrepResult`/`ReadResult`: `Computed { session-id, efficiency }`
  (opaque blob within cap), `Oversized { session-id, chars, cap }` (blob exceeds
  the cap -> WITHHELD, size + cap reported), `NotComputed { record }` (efficiency
  NULL -> the `session_read`-style `Unavailable` analog, boxed record, no
  `efficiency` key).
- **Fail-loud on a corrupt blob** (`session_efficiency` handler): a non-JSON
  `efficiency_json` surfaces as a server-fault `internal_error`
  ("unparseable efficiency_json blob: ..."), never a silent `null` -- the exact
  fail-closed posture `build_export_record` (`db/query.rs`) takes on the same column.
- **Function-level debug logging**: the handler logs entry (`id=`), and a DEBUG
  outcome per branch (not-computed / oversized-with-`warn!` / computed with char
  count); `Db::get_efficiency_json` logs entry + `present=` outcome.
- **`get_info` instructions updated** to list `session_efficiency` alongside the
  other tools, so the served tool description stays in sync with the registry.

### Deviations

- **Cap semantics differ from `session_read`'s: withhold, not truncate.**
  `session_read` cuts its message window short and returns partial data with
  `truncated: true`. A nested JSON document cannot be cut mid-structure and stay
  valid, so an over-cap efficiency blob is WITHHELD entirely (`Oversized` state,
  reporting `chars`/`cap`) and the caller is pointed at `clyde efficiency session
  <id>`. Same cap CONSTANT and same "stay within the tool-result budget" effect,
  correct seam for an atomic-document payload.
- **Data source is the persisted blob, not a fresh compute.** `session_read`
  reads the on-disk transcript fresh each call; `session_efficiency` reads the
  persisted catalog annotation (see Design decisions -- the dependency direction
  forbids computing in `sessions`). Consequence: a session not yet reindexed
  returns `not-computed` (the honest state) rather than an on-the-fly result;
  `clyde session reindex` populates it. Same effect the doc intends ("returns
  signals for a known session id"), correct seam given Phase 6's persistence and
  the `efficiency -> sessions` dep direction.

### Tradeoffs

- **Reused `SessionRef` for the request rather than a bespoke
  `SessionEfficiencyRequest`.** `session_read`/`session_grep` have their own request
  types because they carry extra params (offset/limit, query/context); efficiency
  takes only an id, exactly like `session_open`, which already reuses `SessionRef`.
  Reusing it keeps the id-only tools symmetric; the cost is the tool's input schema
  carries `SessionRef`'s generic "id or unique prefix" description rather than an
  efficiency-specific one (acceptable -- the resolution semantics ARE identical).
- **`Oversized` withholds instead of paging.** A paged/summarized large-blob path
  was rejected as scope the design does not name (Phase 7 is "mirror `session_read`
  incl. its cap", not "add efficiency pagination"). Withhold-and-redirect is the
  minimal fail-loud honoring of the shared cap; a pagination surface can follow if
  a real over-cap blob becomes an observed problem ("make it be a problem first").

### Open questions

None. The one genuine ambiguity (read persisted vs. recompute) was resolved by the
code, not judgment: `sessions` cannot depend on `efficiency` (Phase 6 dep direction),
so recompute-in-`sessions` is impossible and reading the persisted blob is forced.
Cross-repo / system-mutating bullets: none in Phase 7 (clyde-only).

## Phase 8: LLM narrative (prose only, math-free)

### Design decisions

- **The math-free guard is STRUCTURAL, enforced by the type, not just the prompt.**
  `narrate` (`efficiency/src/narrate.rs`) takes `&NarrationInput`, whose EVERY
  field is a `String`/`Vec<String>` of pre-formatted facts (`cache_read_share:
  "62%"`, `worst_signal: "auto-compacted twice, reclaiming 155k tokens over 9s of
  dead wall-clock"`) -- there is NO raw `u64`/`f64` token/cost field anywhere on
  the struct. The panel's finding 3 (design line ~253): passing the full
  `SessionEfficiency` would still let the LLM derive cost-per-turn / rates /
  "projected savings" from the operands it was handed. `NarrationInput` gives it
  no operands at all -- its job is to SELECT and PHRASE the strings, never to
  calculate. Proven structurally by `narration_input_carries_only_string_facts`,
  which serializes a real `NarrationInput` and asserts NO `serde_json::Value::Number`
  appears anywhere in the tree (walks objects + arrays recursively).
- **All formatting/arithmetic lives in `narration_input(&SessionEfficiency) ->
  NarrationInput`** (Rust). This is the single seam where the already-computed
  aggregate numbers are consumed and turned into display strings (percent
  rounding, `155000 -> "155k"` / `1.2M` humanization, `2/61 -> "3%"`, ms ->
  seconds, per-turn `${:.2}`, compaction reclaimed-token/dead-wall-clock summing,
  flag phrasing, worst-signal selection). Downstream -- the prompt, the LLM --
  sees strings only. `narration_input_formats_the_computed_numbers_as_display_strings`
  pins each field against hand-computed expected strings (break the fixture ->
  the assert fails), and `empty_scope_formats_to_na_never_nan` proves the
  zero-scope path renders `n/a`/`none`/`no tool calls`, never `NaN`.
- **The prompt output contract (`NARRATE_SYSTEM_PROMPT`) is the prompt-level HALF
  of the guard:** it forbids introducing any number/percentage/dollar/duration/
  token count absent from the supplied facts, and forbids compute/sum/average/
  project/estimate. The structural half (no operands on the type) is the primary
  defense; the prompt is defense-in-depth.
- **Function-level debug logging** on `narrate` (entry logs every already-safe
  display-string field of the `NarrationInput`, exit logs the produced prose char
  count) and `narration_input` (entry: session id + flag count). Per the security
  rule these are Rust-computed pre-formatted facts, so logging them in full is
  safe (no raw prompt, no secret, no LLM payload inlined).

### Deviations

- **The LLM-client seam is a NEW `sessions::Narrator` port (prose completion),
  sibling to the existing enrichment `Completer`, implemented by the SAME
  `AnthropicClient`.** The Phase 8 bullet says "reuse the `sessions` enrichment
  LLM path (`sessions/src/llm.rs`)", but the existing `Completer::enrich` returns
  the structured `LlmEnrichment` (tags + summary), not free prose -- its contract
  does not fit a narrative. Rather than invent a second HTTP integration (which
  the task forbids) or bend `Completer`'s enrichment-shaped return, I added a
  `Narrator` trait (`fn narrate(&self, system, user) -> Result<String>`) and
  impl'd it on `AnthropicClient`, refactoring the body-build + POST into a shared
  private `AnthropicClient::messages(model, system, user, max_tokens)` +
  `first_text(resp)` that BOTH `enrich` and `narrate` now ride. Same key
  (`ANTHROPIC_API_KEY`), same timeout, same `post_with_retry` (429/5xx bounded
  retry), same error handling -- genuinely ONE integration, zero new LLM
  dependency (no new crate; `reqwest`/`serde_json` were already `sessions` deps).
  `NARRATE_MODEL` is a const alias of `ENRICH_MODEL` so the two callers share one
  pinned model. This mirrors how the codebase already crosses the
  `efficiency -> sessions` boundary (Phase 6/7): `efficiency` depends on
  `sessions`, so `efficiency::narrate` depending on `sessions::Narrator` is the
  correct dep direction (no cycle). Same effect as "reuse the enrichment LLM
  path," correct seam for a prose (not schema) return.
- **`narrate` is a library capability, not wired to a new CLI flag.** The design's
  Architecture lists `src/narrate.rs`; the API Design section names NO
  `--narrate`/narrate surface, and Phase 8's success criteria are only about the
  `NarrationInput` type and the golden-input test. Adding an unrequested CLI flag
  would be out-of-scope gold-plating (unrequested scope is illegitimate). The
  module + `pub use narrate::{NarrationInput, narrate, narration_input}` is the
  exported seam a later phase/flag can call; no CLI change was made.

### Tradeoffs

- **The no-invented-number check is a TEST verification helper, not a hard runtime
  gate on `narrate`'s output.** The design asks the *prompt contract* to forbid
  invented numbers and the *test* to assert the output contains no numeric token
  absent from the input. I implemented exactly that: `foreign_numbers(prose,
  input)` (regex `\d+(?:\.\d+)?` extraction, subtract tokens present in the input
  strings) is used by `narrate_returns_prose_and_sends_the_facts_no_network`
  (well-behaved reply -> zero foreign numbers) AND
  `foreign_number_checker_bites_on_an_invented_figure` (a fabricated "$99
  projected savings" -> caught), proving the check is not vacuous. I deliberately
  did NOT make `narrate` itself reject output on a foreign number: a legitimate
  paraphrase ("two" for "2", or reformatting "155k" as "155,000") would false-
  positive and fail a valid narration. The structural guard (no operands on the
  type) is the real enforcement; a runtime output gate would trade correctness
  for a weaker, noisier signal.
- **Tests inject a `FakeNarrator` (deterministic canned prose) and a
  `FailingNarrator`; no real network call in `otto ci`.** This mirrors the
  enrichment path's own `Fake` `Completer` (`sessions/src/enrich/tests.rs`). The
  `FakeNarrator` also records the `(system, user)` it was handed so the test
  asserts `narrate` sent `NARRATE_SYSTEM_PROMPT` + the formatted facts (never raw
  operands). The real `AnthropicClient::narrate` HTTP path is exercised only in
  production, exactly as `enrich`'s is.

### Open questions

None. The one genuine call (how to reuse the LLM path for a prose return without a
new integration) was resolved by mirroring the existing `Completer` DI shape with
a sibling `Narrator` port on the same client. Cross-repo / system-mutating bullets:
none in Phase 8 (clyde-only). The pre-existing open question (design doc still
UNTRACKED in git through Phases 0-7) is unchanged; this phase did NOT commit or
modify the design doc, per the phase boundary -- still the parent orchestrator's to
resolve at finalization. **[Finalization update: RESOLVED -- the parent committed
the design doc with the feature (Status -> Implemented); it is no longer untracked.]**
