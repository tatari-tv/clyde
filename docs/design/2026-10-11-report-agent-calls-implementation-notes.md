# Implementation Notes: 2026-10-11-report-agent-calls

## Phase 1: Detect `--no-rollup` from the rows

### Design decisions
- `is_subagent_row(key, entry)` is true when the entry holds exactly one subagent and the key has a `/` whose last segment equals that subagent's `agent_id`. `build_efficiency_view` asks it of every session row instead of testing the key shape. The doc comment states the contract limit (an agent id equal to a session id, or containing `/`, would fool it; none in the corpus). (`report/src/render/workload.rs:is_subagent_row`)
- The merged-report tests go through the real `merge::merge_reports` (two copies of one report under different hosts), so the `<host>/<sid>` re-keying under test is the shipped one, not a hand-built imitation. (`report/src/render/tests/workload.rs:merged`)

### Deviations
- `merge::merge_reports` became `pub(crate)` (was private) so the render tests can call it. The spec did not name this; same effect, correct seam.
- The old test `no_rollup_report_explains_inherited_skill_tags` pinned the key shape (it re-keyed a session `<sid>/agent-a1` with no subagent). It is replaced, not left green by accident, by `no_rollup_report_keeps_caption`, whose `--no-rollup` rows hold their one subagent.

### Tradeoffs
- Checking the last `/` segment against the single subagent's id vs. adding a `rollup: bool` to the report: the rows already prove it, and a schema change is Alternative 4 in the doc.

### Open questions
- None.

## Phase 2: Agent calls and sessions in the view

### Design decisions
- Calls per type come from a new `session_agent_spawns(entry)`, separate from `session_agent_cents`, so the cents split (the partition) is untouched. `build_efficiency_view` adds its spawns into `AgentCents.spawns`; `sessions += 1` rides the `session_agent_cents` loop, so a session counts once per bucket it holds, `(main-session)` included (including the spend-with-no-bucket fallback, which lands on `(main-session)`). (`report/src/render/workload.rs:session_agent_spawns`, `build_efficiency_view`)
- The token rule is shared, not copied: `report::subagent_has_tokens(sub)` is `by_model.values().any(has_tokens)`, the same predicate that decides whether `agent_type_costs` keeps a type's bucket. (`report/src/report.rs:subagent_has_tokens`)
- `partition_rows` returns `Vec<CallRow>`. `(main-session)` is keyed on `MAIN_SESSION_BUCKET`: Calls and $/call `n/a`, Sessions as counted, and `counts: None` so Phase 3's `call_pairs` (once it skips count-less rows) leaves it out. Every other row gets `CallCounts { calls: spawns, spend: dollars, counted_spend: dollars, partial: false }`. $/call is `cents::to_dollars(cents) / spawns`; a non-main row with 0 spawns (only reachable in hand-built data) shows Calls `0` and $/call `n/a`, the skill/MCP precedent. (`report/src/render/workload.rs:partition_rows`)
- The `build_efficiency_view` closing debug line gains `agent-calls=<sum of spawns>`.
- `agent_calls_equal_subagent_entries` counts the expected calls from the raw `by_model` totals, not through `subagent_has_tokens`, so the test does not grade the predicate with itself. Both `zero_token_subagent_adds_no_call` (filter removed) and `agent_sessions_and_per_call_values` (raw cents divided: `$150.00`) were mutation-checked to fail.

### Deviations
- `eval::synth` gained a `#[cfg(test)] pub(crate) fn build_no_rollup(kind, pricing)`; `build` now delegates to a private `build_view(kind, pricing, no_rollup)`. The `agent_calls_same_under_no_rollup` criterion needs synth medium under `--no-rollup` and the generator's session list is private. Same effect, correct seam; `build`'s output is byte-identical (the reproducibility test still passes).
- `render/html.rs:workload_rows` takes `&[CallRow]` instead of the deleted `WorkloadRow`, still emitting the same three cells. HTML and Markdown output are unchanged and no golden moved (`git status` shows no fixture diff after `otto ci`). The ContextBlock JSON (`agent-type-costs`) now carries `calls`, `sessions` and `per-call` strings, as the API Design says.

### Tradeoffs
- A separate `session_agent_spawns` pass vs. folding spawns into `session_agent_cents`'s tuple: the cents function is the partition's authority and its tests pin its shape; a second pass over an in-memory list keeps it single-purpose.
- Spawns are added to whatever type they name, even if a hand-built entry has no matching cost bucket (that row then shows `$0.00`). Report-built entries cannot diverge, because the bucket and the call use the same token rule; restricting calls to existing buckets would hide a broken entry instead of showing it.

### Open questions
- None.

## Phase 3: Columns, chart, goldens
### Design decisions
- `call_pairs` returns `None` only when every row is count-less; count-less rows are skipped, so `(main-session)` drops out of the agent figure and shares are of the counted rows' totals — render/visuals.rs:call_pairs — keeps the skill/MCP figures unchanged because `call_rows` makes those tables homogeneous.
- The agent figure sits between the section lead and the table, titled "Agent types: calls and spend" — render/html.rs:agent_type_costs — the section has no subheading, so the figure follows the lead rather than a `subheading`.
- `Visuals.agent_calls` added and listed in `display_strings` and the licensing row-count loop — render/visuals.rs, render/html/tests.rs.
- Agent table columns are unconditional (no `counted` gate as in `call_table`) — render/html.rs:workload_rows, render/document.rs:agent_type_costs — every agent row is built with call cells by Phase 2.
### Deviations
- None.
### Tradeoffs
- Unconditional columns vs. mirroring `call_table`'s counted gate — the gate exists for reports with no counted sessions; agent counts always exist (design: "Spawns are always counted").
### Open questions
- None.

Goldens: medium/small/pathological `.md` and `.html` regenerated. The diff touches only the Agent-Type table header and rows (plus, in medium, one new agent calls figure of three rows); no skill or MCP row or figure changed.

## Phase 4: Docs
### Design decisions
- New "Agent-type calls" section placed after the fork-dedupe list and before the closing line about skill and MCP cost — report/README.md — keeps the agent table beside its sibling tables' wording (Calls, Sessions, $/call).
- The design doc's Status line flipped to Implemented in this commit, as the final-phase step — docs/design/2026-10-11-report-agent-calls.md.
### Deviations
- None.
### Tradeoffs
- A separate section vs. extending the skill/MCP bullet list: the agent table has its own definition of a call (subagent with tokens) and its own `n/a` rule, so a section reads cleaner than more bullets on a list about invocations.
### Open questions
- None.
