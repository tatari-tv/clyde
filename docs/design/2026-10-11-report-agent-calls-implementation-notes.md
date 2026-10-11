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
