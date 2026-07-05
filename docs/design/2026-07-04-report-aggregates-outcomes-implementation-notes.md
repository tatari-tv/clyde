# Implementation Notes: Report Aggregates and Outcome Extraction

Design doc: `docs/design/2026-07-04-report-aggregates-outcomes.md`

## Phase 1: aggregates module + slim context

### Design decisions

- `report/src/aggregate.rs` + `aggregate/tests.rs` implement pure `compute(&Report, outliers_n)
  -> Aggregates` with `by_org`/`by_repo`/`by_day`/`outliers` fields, exactly per the phase
  bullet — `aggregate::compute` — `aggregate.rs:compute`.
- Every `Aggregates` row type (`OrgRow`, `RepoRow`, `DayRow`, `OutlierRow`) carries a hidden
  `#[serde(skip)]` raw numeric field (`tokens`, `spend_raw`) alongside the serialized display
  string (`tokens-human`, `spend`) — `aggregate.rs` — lets unit tests assert exact sums
  (by-org sums equal totals, by-day invariants) without re-parsing formatted strings back into
  numbers, while the JSON the model sees still only carries display strings, preserving the
  no-arithmetic contract.
- `format_int`/`format_usd`/`format_optional_usd` moved verbatim from `render.rs` into a new
  `report/src/fmt.rs` (+ `fmt/tests.rs`); added `format_tokens_human` there (the "9.53B" /
  "287.8M" / "35,373" style) — `fmt.rs:format_tokens_human` — single shared formatting home per
  the phase bullet, instead of scattering format helpers across `render.rs` and `aggregate.rs`.
- Added `SessionEntry::total_tokens()` on `Report`'s own type (`report.rs`) and used it from both
  `aggregate.rs` and `render.rs`, replacing the two near-duplicate free functions
  (`render::session_total_tokens`, and an equivalent computation inline in `aggregate.rs`) that
  existed only because the sum-over-models logic had no single home.
- `render::group_by_repo` deleted; `render_built_in`'s "By repo" table now sources
  `aggregate::compute(report, 0).by_repo` — `render.rs:render_built_in` — outliers are unused by
  that table so `0` is passed rather than computing a table the built-in renderer never shows.
- Render context (`render::build_context_block`) rebuilt around new view structs
  (`PeriodView`, `TotalsView`, `ModelRow`, `TotalRow`, `SessionView`) per the design's "API
  Design" slim shape: `{persona, options, period, totals, aggregates, sessions}`. `Report` itself
  is untouched; only the render-time view is slim — `render.rs:build_context_block`.
- `totals.models` is rebuilt as a spend-sorted `Vec<ModelRow>` (the persisted
  `Report.totals.models` `BTreeMap` iterates alphabetically and cannot back the "pre-sorted,
  never re-sort" prompt promise) — `render.rs:build_totals_view`.
- `by-day` attribution clamps `entry.begin.date_naive()` into `[since.date_naive(),
  until.date_naive()]` using `Ord::clamp`, never trusting that a `SessionEntry.begin` already
  lies in period — `aggregate.rs:compute_by_day` — pinned by the boundary fixture test
  (`by_day_clamps_boundary_session_into_period_and_preserves_spend_sum`).
- `org_of` derives the org from `repo.split_once('/')`'s first component; `repo: None` sessions
  bucket into the literal `(unattributed)` constant (`aggregate::UNATTRIBUTED_ORG`) —
  `aggregate.rs:org_of` — matches the Definitions section verbatim.
- `outliers` rank by `spend_raw.unwrap_or(0.0)` descending, ties broken by `short-id` for
  deterministic ordering across runs — `aggregate.rs:compute_outliers`.
- `DEFAULT_OUTLIERS: usize = 10` defined in `aggregate.rs` now (not deferred to Phase 5) because
  `compute`'s signature already requires an outlier count and `build_context_block` must call it
  today; Phase 5 only needs to wire a CLI flag on top of this existing constant.

### Deviations

- **`CacheStats` omitted entirely from `Aggregates` this phase**, per the task's explicit
  instruction to pick a seam and record it. The design's Phase 1 bullet (lines 357-371) lists
  only by-org/by-repo/by-day/outliers/active-days as in-scope; `cache` is not mentioned there or
  in the Phase 1 success criteria, even though the Architecture section says cache-read-share and
  token strings "need no pricing and land in Phase 1." Chose the narrower reading (omit
  entirely) over a partial `CacheStats` with `None` counterfactual fields, to avoid Phase 1 code
  half-shipping a struct Phase 2 will need to revisit anyway, and because no test in this phase's
  success criteria exercises it. Phase 2 adds the `cache: CacheStats` field to `Aggregates`.
- **`outcomes` key omitted entirely from the render context.** The design's full context schema
  (API Design section) includes `outcomes.totals`, but `Outcomes`/`OutcomeTotals` don't exist yet
  (Phase 3/4 land the schema and extractor). Phase 1's bullet list doesn't mention outcomes, so
  the context simply has no `outcomes` key until Phase 4 wires it through
  `render::build_context_block`.
- **Per-session `outcomes?` field omitted from `OutlierRow` and `SessionView`** for the same
  reason — the underlying `SessionEntry.outcomes` field doesn't exist until Phase 4.
- **`render_built_in`'s "By repo" table row ordering changed** from alphabetical (old
  `BTreeMap<String, _>` iteration) to spend-descending (`aggregate::compute`'s pre-sorted
  `by_repo`). `render_built_in` is unreachable from `report::render::run` in the current CLI (only
  `Template::Custom` or the Opus path are ever selected; `Template::BuiltIn` is exercised solely
  by direct unit tests), so this has no user-facing effect, but is noted since the design didn't
  explicitly call out row-order in this path.
- **`period.since`/`period.until`/`period.generated` formatted as `%Y-%m-%d` date strings**, not
  full RFC3339 timestamps. The design only says "generated: display date" explicitly for
  `generated`; applied the same display-string convention to `since`/`until` for consistency with
  the header's "**Period:** <since> - <until>" requirement and with the rest of the no-arithmetic,
  display-string-only design intent. `sessions[].begin`/`end` were left as full `DateTime<Utc>`
  (RFC3339) since the design doesn't ask for date-only there and the old context already
  serialized them that way.

### Tradeoffs

- Chose to add hidden raw fields (`tokens`, `spend_raw`) on every `Aggregates` row type over
  computing separate "raw" accumulator structs discarded after formatting: keeps one row type
  per concept, keeps tests able to assert exact sums, and the `#[serde(skip)]` keeps the raw
  numbers out of what Opus actually sees, preserving the no-arithmetic contract.
- `aggregate::compute`'s `outliers_n: usize` (rather than an `Option<usize>` or a config struct)
  matches the design's stated eventual CLI shape (`--outliers <N>`, Phase 5) and keeps this
  phase's function signature stable across Phase 5's wiring — only the call site changes from a
  constant to a resolved config value.
- `render_built_in`'s reuse of `aggregate::compute(report, 0)` instead of hand-writing an
  aggregate-free repo grouping keeps `group_by_repo` genuinely deleted (per the phase bullet)
  rather than reintroducing an equivalent private helper under a different name.

### Open questions

- The design's Phase 1 Architecture text ("cache-read-share and token strings need no pricing
  and land in Phase 1") reads as if some `CacheStats` fields belong in this phase, while the
  Phase 1 bullet/success-criteria list omits `cache` entirely. Confirm the omit-entirely reading
  above is correct before Phase 2 adds `CacheStats`, or whether Phase 2 should instead be a
  smaller diff that only adds the two pricing-dependent fields to an existing (Phase-1-created)
  struct.

## Phase 2: cache stats + counterfactual

### Design decisions

- Phase 2 owns the ENTIRE `CacheStats` struct (both the no-pricing fields and the two pricing-
  dependent ones), resolving Phase 1's open question above per the parent's explicit instruction.
  Added `cache: CacheStats` to `Aggregates` and `CacheStats` itself in `aggregate.rs`, wired into
  the context block via `render::build_context_block` — `aggregate.rs:CacheStats`,
  `aggregate.rs:compute_cache_stats`.
- Chose the seam the design's Architecture (lines 123-127) names explicitly: `aggregate::compute`
  now takes `&Pricing` as a third parameter and remains the single aggregate entry point;
  `render::run` gains a `&Pricing` param threaded from `lib.rs` `run_with_pricing` (which already
  held a `Pricing` and only passed it to collect). Rejected the alternative "dedicated cache step
  outside compute" because it would fork the aggregate entry point — `render.rs:run`,
  `lib.rs:run_with_pricing`.
- Counterfactual repricing folds ALL cache tokens (reads AND 5m/1h writes) into `input_tokens`
  and zeroes the cache fields, then calls the crate's own `Pricing::calculate_usd`, which reapplies
  the >200k long-context tiering (`total_input` becomes the folded input) — no tiering logic is
  reimplemented in the report crate — `aggregate.rs:compute_cache_stats`.
- Actual priced spend for `cache-savings` is `report.totals.spend_usd` (the sum of priced-model
  spends the collect phase already rounded), so `cache-savings = list_price - totals.spend_usd`.
- Both counterfactual fields carry `#[serde(skip_serializing_if = "Option::is_none")]`, so `None`
  means the key is ABSENT from the context JSON (never a `$0` stand-in), satisfying the success
  criterion literally — `aggregate.rs:CacheStats`.
- Fail-closed nullification: a model is only allowed to nullify the counterfactual when it is
  unpriced AND carries nonzero cache tokens; an unpriced model with zero cache tokens is skipped
  (its cost is unknowable but it cannot poison the cache story) — `aggregate.rs:compute_cache_stats`
  `Err(_) if cache_tokens > 0` vs `Err(_) => {}`.
- Function-level DEBUG logging on `compute_cache_stats` (entry: model count + actual spend; a
  per-unpriced-model DEBUG when the counterfactual is dropped) and an extended `compute` exit log
  carrying `cache-read-share` and whether the counterfactual is present.
- `render_built_in` / `to_markdown` also gained a `&Pricing` param (they call `compute` for the
  by-repo table); threaded rather than forking a pricing-free grouping helper, keeping `compute`
  the only aggregate path.

### Deviations

- Design signature `aggregate::compute(&report, outliers_n, &pricing)` (Architecture line 108)
  vs the Phase 1 `compute(&Report, outliers_n)` — implemented exactly as the design's Architecture
  spells it. No seam difference; recorded only because the two-arg form existed after Phase 1.
- `render::run(cfg)` -> `render::run(cfg, pricing)`, and the public `render::to_markdown` /
  `build_context_block` gained a trailing `&Pricing` arg. These are crate-internal (no callers
  outside `report/src`, verified by grep), so no external API break.

### Tradeoffs

- Read the counterfactual rates in tests via the public `Pricing::lookup` (embedded feed) and
  hand-compute the folding formula independently, rather than pinning hardcoded per-mtok rates.
  `Pricing::from_bytes` (which would let a test inject a fully controlled feed) is `pub(crate)` in
  the pricing crate and not reachable from the report crate. The lookup-based test tracks the feed
  yet still fails if `compute_cache_stats` mis-folds or mis-tiers, because the expected value is
  written from the folding formula, not by calling the function under test.
- Token counts in the hand-computed test kept under the 200k long-context threshold so the linear
  standard-rate formula applies; tiering itself is already covered by the pricing crate's own tests
  and is exercised transitively (folded input flows through `calculate_cost`'s `total_input`).

### Open questions

- None.

## Phase 3: outcome extractor

### Design decisions

- `report/src/outcome.rs` (+ `outcome/tests.rs`) implements per-file extraction of all six
  signals per the extraction-rules table (design lines 315-353) - `outcome::extract`. It reads the
  JSONL a second time (page-cache-hot) inside the collect `par_iter` closure, right after
  `parse_jsonl_file`.
- Lines are parsed to `serde_json::Value` and navigated with `.get()` chains rather than typed
  serde structs, because `toolUseResult` is polymorphic across records (a string for most tools, an
  object carrying `gitOperation` for git records); a typed struct on that field would fail the whole
  line parse on the common string shape - `outcome.rs:extract`. The semantic decision is always on
  the parsed JSON, never the raw string (design line 349).
- Substring prescreen skips any line lacking both `gitOperation` and `tool_use` before JSON parse.
  The `tool_use` marker also matches confirming `tool_result` blocks (via their `tool_use_id`
  field), so one marker captures both halves of a pairing; `pr-link` records carry neither marker
  and are correctly skipped (they are never counted) - `outcome.rs:extract`.
- The outcome vocabulary is a typed `OutcomeKind` enum matched via `classify_tool`, which takes the
  suffix after the final `__` (`name.rsplit("__").next()`) so duplicate-server aliases
  (`mcp__atlassian__` vs `mcp__claude_ai_Atlassian__`) collapse to one suffix - `outcome.rs:classify_tool`.
- Success pairing is a single forward pass: in-window `tool_use` ids of interest are held in a
  `pending` map, resolved when a later `tool_result` with the same `tool_use_id` arrives; an
  explicit `is_error: true` drops the call, anything else confirms it, and unresolved ids at EOF are
  dropped (design lines 344-346, D5) - `outcome.rs:extract`.
- Period filter (D8) is applied at extraction time on the INITIATING timestamp: the user record's
  own timestamp for `gitOperation`, the assistant record's timestamp for `tool_use`. A record with
  no parseable timestamp fails closed (not counted) - `outcome.rs:in_window`,
  `outcome.rs:handle_git_operation`. A confirming `tool_result` after `until` still confirms because
  results are matched only by id, never re-filtered by time (pinned by
  `confirming_result_after_until_still_confirms_in_window_use`).
- Commit kinds `committed`/`cherry-picked` count; `amended` never counts (design line 321), pinned
  by `commit_then_amend_counts_one` - `outcome.rs:handle_git_operation`.
- `PrRef.repository` (D10) via `derive_repository`: parses ONLY the exact
  `github.com/<org>/<repo>/pull/<N>` shape (four path segments, numeric id) and returns `None` on
  anything else (foreign host, extra subgroup segment, non-numeric id) - never a corrupted string.
  The PR still counts.
- `session::fold` gains an `outcomes: &HashMap<PathBuf, FileOutcomes>` parameter and a
  `union_outcomes` helper that unions a session group's parent + subagent files into the persisted
  `Outcomes` shape: commits deduped by sha, PRs deduped by url, edited file paths deduped then
  counted, MCP counts summed. `SessionSummary` gains `outcomes: Option<Outcomes>` (None when nothing
  observed) - `session.rs:fold`, `session.rs:union_outcomes`.
- Collect closure in `lib.rs:run_collect` now produces `(PathBuf, ParseResult, FileOutcomes)` per
  file and splits into two maps (`parsed`, `outcomes`) passed to `fold`. Extraction failure for a
  file is WARN-and-continue with an empty `FileOutcomes` (usage still counts) - fail closed toward
  absent outcomes.
- Function-level DEBUG logging on `extract` (entry: path + period; exit: per-signal counts) and on
  `fold` (emitted summary count); per-line handling is TRACE; unparseable lines WARN with path+line
  and are skipped (design lines 594-598).

### Deviations

- **`extract` returns `FileOutcomes`, not `Outcomes`.** The design's Architecture (line 119) and the
  Data Model (`files_edited: u64`) cannot both hold: a per-file `u64` count cannot be UNIONED across
  a session group's files to a DISTINCT file-path count (design line 326: "distinct input.file_path
  across ... the session group"). So `extract` returns a per-file `FileOutcomes` carrying the
  distinct sets (`commits: BTreeSet`, `files_edited: BTreeSet`) and the url-deduped `prs`, and the
  fold collapses them into the persisted `Outcomes` (`files_edited: u64`). Same effect, correct seam:
  both `Outcomes` and `PrRef` types are produced exactly per the Data Model; a third per-file type
  carries the sets the union needs. The `HashMap<PathBuf, Outcomes>` in the Architecture note is
  correspondingly `HashMap<PathBuf, FileOutcomes>`.
- **`extract` returns `Result<FileOutcomes>`** (Err only on file-open failure) rather than an
  infallible return, mirroring `parse_jsonl_file`'s error contract so the collect closure can
  WARN-and-skip a single unreadable file without aborting the run.
- **Prescreen substring set is `gitOperation` + `tool_use`, dropping `pr-link`** from the design's
  listed triple (line 349). `pr-link` is never counted, and its records contain neither marker, so
  including it would only parse-then-discard lines; omitting it skips them cheaply. No semantic
  change.
- Scope-honored: NO `outcomes` fields were added to the report JSON / `SessionEntry` / `Totals`
  (Phase 4), and no test asserts against serialized report output (design lines 391-393). The
  `Serialize`/`Deserialize` derives on `Outcomes`/`PrRef` exist per the Data Model but are not yet
  wired into any persisted struct.

### Tradeoffs

- Fixtures are compact JSONL string literals written to a `TempDir` at test time (via small
  per-record builder fns) rather than checked-in fixture files: keeps the six-signal contract, its
  expected counts, and the record shapes co-located and diffable in one test file, and avoids a
  fixtures directory whose drift from the assertions would be invisible.
- `FileOutcomes` uses `BTreeSet` (deterministic order) for commits and file paths so the folded
  `Outcomes.commits` Vec is stably sorted across runs without a separate sort step.
- Kept a single forward pass with a `pending` map over a two-pass (collect all uses, then all
  results) design: results always follow their use in file order, so one pass suffices and holds
  less state.

### Open questions

- None.

## Phase 4: schema + merge

### Design decisions

- `SessionEntry.outcomes` and `Totals.outcomes` added exactly per the Data Model
  (`#[serde(default, skip_serializing_if = "Option::is_none")]`), and `Report.outcomes_enabled`
  (`#[serde(default)] Option<bool>`) added as a report-level metadata field — `report.rs`. `to_entry`
  now carries `s.outcomes.clone()` onto `SessionEntry` untouched — `report.rs:to_entry`.
- `OutcomeTotals` (the `Totals.outcomes` rollup struct) lives in `outcome.rs`, not `report.rs`: it
  is outcome-domain data (same file as `Outcomes`/`PrRef`), and `report.rs` imports it rather than
  redefining it. A single `pub fn rollup<'a>(sessions: impl Iterator<Item = Option<&'a Outcomes>>)
  -> OutcomeTotals` in `outcome.rs` does the GLOBAL dedupe (commit shas via `BTreeSet`, PR urls via
  `HashSet`) and is shared by both `report::build_report` (collect path, design line 402) and
  `merge::recompute_totals` (merge path) — one dedupe implementation, two call sites —
  `outcome.rs:rollup`.
- `report::build_report` always sets `outcomes_enabled: Some(true)` and `Totals.outcomes:
  Some(rollup(..))` (never `None`), because collect always runs extraction today (the
  `--no-outcomes` escape hatch is Phase 5's job, per the task's explicit instruction to hardcode
  `Some(true)` on the collect path this phase) — `report.rs:build_report`.
- `merge::recompute_totals` gained an `outcomes_enabled: bool` parameter and gates the rollup on
  it: `Some(rollup(..))` when true, `None` when false — never a partial rollup that reads as
  complete — `merge.rs:recompute_totals`.
- `merge::merge_reports` computes `all_outcomes_enabled = reports.iter().all(|r|
  r.outcomes_enabled == Some(true))` BEFORE the `for report in reports` loop consumes the Vec (the
  coverage check needs each INPUT report's own flag, which is gone once sessions are drained into
  the merged map), and this check lives strictly in the multi-input path, after the existing
  single-input identity-passthrough early return — never ahead of it, per the design's explicit
  warning against breaking the round-trip contract. The merged `Report.outcomes_enabled` is set to
  `Some(all_outcomes_enabled)` — `Some(false)` when any input is absent/false, `Some(true)` only
  when every input is enabled — `merge.rs:merge_reports`.
- An absent flag (`None`, a pre-Phase-4 JSON) is treated identically to `Some(false)` in the
  `all()` check: fail closed toward "not enabled" rather than assuming an old binary's silence
  means it secretly extracted outcomes — `merge.rs:merge_reports`.
- Per-session `outcomes` fields ride through the merge untouched: `merge_reports`'s existing
  `for (sid, entry) in report.sessions { sessions.insert(key, entry) }` loop already moves the
  whole `SessionEntry` (including its `outcomes` field) verbatim; no additional code was needed to
  satisfy "per-session outcomes fields always ride through untouched" — it was already true by
  construction once the field existed on the struct.
- Function-level DEBUG logging: `outcome::rollup` logs its exit (sessions observed, distinct
  commit/PR counts, per-signal sums); `merge::recompute_totals`'s existing entry log gained the
  `outcomes-enabled` value.

### Deviations

- None. The design's exact field shapes, serde attributes, and merge coverage rules were
  implementable at the seam the design named; no signature or type substitution was needed.

### Tradeoffs

- Placed `OutcomeTotals` in `outcome.rs` rather than `report.rs` (the design's code block for it
  appears in the Data Model discussion of `Totals`, with no explicit file marker, unlike
  `outcome.rs`'s marked `Outcomes`/`PrRef` block): kept every outcome-domain type in one file
  rather than splitting the rollup struct across `report.rs` and `outcome.rs`, and let both
  `report::build_report` and `merge::recompute_totals` share one `rollup` implementation instead
  of duplicating (or re-exporting through an awkward re-import cycle) the dedupe logic.
- Did NOT thread an `outcomes_enabled` parameter through `report::build_json`/`write_json`'s public
  signatures for Phase 4; hardcoded `Some(true)` inside `build_report` instead. The design says
  Phase 5 will change this value's *source* (a real CLI flag) regardless of whether the plumbing
  exists now, so adding an unused parameter this phase would only be threaded again next phase —
  chose the smaller diff that matches "collect always runs extraction today" literally.
- Left `render::build_context_block` untouched (no `outcomes.totals` key in the render context).
  The design's placement contract explicitly separates "persisted rollup lives at `Totals.outcomes`"
  (this phase) from "the context builder's `outcomes.totals` re-exposure is a render concern" (the
  task's own framing, matching the design's Phase 1/6 split); no test in this phase's success
  criteria or `render/tests.rs` asserts an `outcomes` key in the context block, so it was left for
  the render-owning phase.

### Open questions

- None.

## Phase 5: CLI flags

### Design decisions

- `CollectArgs` gains `--no-outcomes` (bare `bool`, `#[arg(long)]`, no value) mirroring
  `--skip-title`'s exact shape; `RenderArgs` gains `--outliers <N>` (`usize`, `#[arg(long,
  default_value_t = DEFAULT_OUTLIERS)]`) so the clap-rendered default and help both stay
  accurate without hand-duplicating the literal `10` — `cli.rs:CollectArgs`,
  `cli.rs:RenderArgs`.
- `config::resolve_command` threads both flags one-for-one into their resolved configs:
  `CollectConfig.no_outcomes` (mirrors `skip_title`'s field naming) and
  `RenderConfig.outliers` — `config.rs:collect_config_from_args`,
  `config.rs:resolve_command`.
- `run_collect` (`lib.rs`) gates the SECOND read pass on the flag: when `cfg.no_outcomes` is
  true, `outcome::extract` is never called for any file (not called-then-discarded) - the
  per-file closure substitutes `outcome::FileOutcomes::default()` directly, which is exactly
  the "escape hatch" the design's Performance section describes - `lib.rs:run_collect`.
- `report::build_report`/`build_json`/`write_json` gained an `outcomes_enabled: bool`
  parameter (this is the piece Phase 4 explicitly deferred, see its Tradeoffs bucket above):
  `Report.outcomes_enabled` now reflects the real flag (`Some(true)`/`Some(false)`) instead of
  the Phase-4 hardcoded `Some(true)`, and `Totals.outcomes` is `None` (absent key, not a
  zeroed rollup) whenever `outcomes_enabled` is false - `report.rs:build_report`.
- `build_report` also strips any stray per-session `outcomes` value when
  `outcomes_enabled` is false, regardless of what the caller's `SessionSummary`s carry. This
  is redundant with `run_collect` never populating session outcomes in the first place, but
  makes the "no outcomes fields on sessions/totals" contract hold at the seam that PERSISTS
  the report, independent of what happens upstream (fail closed at the boundary that matters,
  per the house correctness-footguns convention) - `report.rs:build_report`.
- `render::build_context_block` gained an `outliers_n: usize` parameter, replacing the
  Phase-1/2 hardcoded `DEFAULT_OUTLIERS` constant at its one call site in `render::run`; the
  `by_repo`-only call in the built-in markdown renderer (`render.rs`, passes literal `0`) is
  unrelated and untouched, since that table never shows outliers.
- Function-level DEBUG logging: `run_collect`'s entry log now carries `no-outcomes`;
  `report::build_json`/`write_json`/`build_report` entry logs carry `outcomes-enabled`;
  `render::run`/`build_context_block` entry logs carry `outliers`/`outliers-n`.

### Deviations

- None. `DEFAULT_OUTLIERS` already existed (defined in Phase 1, since `aggregate::compute`'s
  signature required an outlier count from the start); this phase only adds the CLI surface
  and config plumbing on top of it, exactly as Phase 1's own notes anticipated.

### Tradeoffs

- Added `outcomes_enabled: bool` as a new trailing parameter to `build_json`/`write_json`/
  `build_report` (touching every existing call site, all in this crate's own tests) rather than
  a builder/options-struct - matches the existing parameter-list style of these three functions
  and keeps the diff mechanical (append `true` at every pre-existing call site, `false` only at
  the new Phase-5 tests).
- Defensive session-outcomes stripping inside `build_report` (belt-and-suspenders alongside
  `run_collect` never populating them) was chosen over relying solely on the upstream
  invariant, because `build_report` is also exercised directly by unit tests that construct
  `SessionSummary`s by hand; without the strip, a test (or a future caller) could pass
  `outcomes_enabled: false` with summaries that already carry outcome data and silently violate
  the design's "no outcomes fields on sessions" contract.

### Open questions

- None.

## Phase 6: prompt rewrite + live end-to-end

### Design decisions

- `report/templates/report.pmt` replaced byte-for-byte with Appendix A (design doc lines
  626-884, extracted verbatim with `sed -n '626,884p'` and diff-verified IDENTICAL against the
  doc). `DEFAULT_PROMPT` picks it up via `include_str!`; the existing
  `baked_in_default_matches_workspace_template` test pins the embedded/on-disk identity.
- **Context-block outcomes wiring landed in this phase**: the render context now carries
  `outcomes.totals` (rollup re-exposed with fields present-if-nonzero, so "only fields present
  were observed" holds), per-session `outcomes` on the slim session view, and `outcomes` on
  `OutlierRow` ("outcome fields when available" per the prompt schema) -
  `render.rs:build_outcomes_view`, `render.rs:SessionView`, `aggregate.rs:OutlierRow`. Phase 4's
  notes explicitly deferred this re-exposure to "the render-owning phase", and this phase's
  success criterion (a Quantified Output table whose rows equal `outcomes.totals`) is
  unfulfillable without it.
- The `outcomes` key is ABSENT from the context (never null, never zeroed) when the report
  carries no rollup (`--no-outcomes`, pre-outcomes JSONs, mixed-capability merges); the prompt
  then omits the Quantified Output section - `render.rs:ContextBlock.outcomes`.
- Truth-up of crate docs: the two `render.rs` doc comments describing the raw `spend-usd` /
  session `spend` / `short-id` fields as "interim-compatibility for the OLD prompt (Phases
  1-5)" rewritten to describe them as part of the new prompt's documented context-block schema.
  `README.md` and `report/README.md` were audited for stale claims ("cr binary" pricing note,
  LLM-computed rollups, jsonl-paths in context) and contained none; left untouched.
- New tests: `build_context_block_carries_outcomes_totals_present_if_nonzero` (nonzero fields
  present, zero fields absent, session outcomes ride the view, outlier rows carry PR refs) and
  `build_context_block_omits_outcomes_key_when_rollup_absent` - `render/tests.rs`.

### Deviations

- The Phase 6 bullet names only "prompt + README/crate docs + live run", but the context-block
  outcomes wiring (design API section, context shape lines 286-289) landed here - same effect,
  correct seam: Phase 4's own notes assigned it to the render-owning phase and the success
  criteria require it.
- First live collect used `--until 2026-06-30` (29-day window, 461 sessions, missed June 30);
  redone with `--until 2026-07-01` to match the reference June collect's full-month convention
  (30 days, 542 sessions - exactly the research brief's June measurement).

### Tradeoffs

- `OutcomeTotalsView` (all `Option<u64>`, `skip_serializing_if`) duplicates `OutcomeTotals`
  field-for-field vs serializing the persisted struct directly - the design wants
  present-if-nonzero in the CONTEXT only; a serde attribute on the persisted struct would
  change the report JSON schema (zero counts would vanish from collected/merged JSONs).
- Live verification captured the exact context block via a temporary env-gated test in
  `render/tests.rs` (removed before commit) vs adding a `--dump-context` flag - no such flag is
  in the design, and the temp test kept the production surface unchanged while reproducing the
  render's exact inputs (`Pricing::auto("clyde")`, `persona::whoami()`, `DEFAULT_OUTLIERS`).

### Open questions

- In the live render, the Model mix bullet says "`<synthetic>` shows up in 38 sessions and is
  untracked" - the figure is verbatim from the context (models row `sessions-using`), but the
  prompt's "No per-model numbers here; the Cost Summary already has them" style rule was not
  followed by Opus in this run. No success criterion is violated (no arithmetic, no hedge, no
  fabricated figure); flagging as a cosmetic prompt-compliance nit to watch in future months.

## Post-audit remediation (review panel, 2026-07-04)

The implementation-audit review panel (Architect + Staff Engineer) raised three findings; one
was a genuine undisclosed spec violation, two were demoted against the code.

### Fixed
- **Merged-report `short-id` corruption (commit e471c40).** `merge.rs` re-keys sessions as
  `host/uuid` (keep-both across hosts), but the three `short-id` sites (`aggregate.rs`
  compute_outliers, `render.rs` build_session_view and the built-in markdown renderer) truncated
  that composite key directly via `sid.get(..8)`, so a merged report rendered `short-id` as e.g.
  `laptop/9` instead of the `9d4c1f28` UUID prefix design line 305 promises. Fixed with a shared
  `fmt::short_id` helper that strips any host prefix before truncating; pinned by
  `fmt/tests.rs::short_id_is_uuid_prefix_for_bare_and_merged_keys`. Undisclosed at the time of the
  Phase 4/6 commits; recorded here.

### Reviewed and intentionally not changed
- **Success pairing treats absent `is_error` as success** (`outcome.rs`). Correct as written: the
  Anthropic transcript format omits `is_error` on successful `tool_result` blocks and only sets it
  on errors. Requiring a literal `is_error: false` would drop nearly every real MCP/Edit outcome.
  Already disclosed in the Phase 3 notes; the design's "is_error: false" is shorthand for "not
  errored."
- **Zero totals rollup for unobservable months** (`report.rs` build_report). Intentional: `Some(0s)`
  = "extractor ran, found nothing" vs `None` = "extractor did not run" (`--no-outcomes`). Preserves
  a useful distinction and is consistent with per-session outcomes staying absent for pre-
  `gitOperation` transcripts (D4). Disclosed in the Phase 4 notes.
