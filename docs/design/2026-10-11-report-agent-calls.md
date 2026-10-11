# Design Document: Calls and Sessions on the agent-type table

**Author:** Scott Idler
**Date:** 2026-10-11
**Status:** Draft
**Review Passes Completed:** 5/5

## Summary

- The Agent-Type Cost Attribution table gains the columns the skill and MCP tables got in `2026-10-11-report-workload-calls.md`: Calls, Sessions and $/call, plus a % of calls vs % of spend chart.
- A call is one subagent that ran. The count comes from the subagent list every v4 report already carries, so no schema change, no collect change and no reindex.
- Also fixes a shipped bug: a merged rollup report prints the `--no-rollup` caption.

## Problem Statement

### Background

- The agent table is `Agent Type | Tokens | Spend`, built from `WorkloadRow {name, tokens_human, spend}` (`report/src/render.rs:585-589`).
- Rows come from `agent_type_costs` (`report/src/report.rs:758-813`):
  - each subagent's per-model split is bucketed by `sub.agent_type`, or `unknown` when it is `None`
  - the residual `by_model(scope) - Σ subagents` is the `(main-session)` row (787-789)
  - zero-token buckets are dropped (796-800)
- Render apportions each session's spend in cents across those buckets (`session_agent_cents`, `report/src/render/workload.rs:325-355`), so the rows sum to the headline total exactly (the partition invariant, `workload.rs:6-10`).
- Every v4 `SessionEntry` carries the session's whole subagent list, with `agent-type`, in `efficiency.subagents` (`report.rs:186-191`). That passthrough exists so render can surface a signal without a re-collect.
- The skill and MCP tables got `CallRow` / `CallCounts`, `call_rows`, the `n/a`-never-0 rule and the calls/spend pairs chart in the last doc (`render.rs:598-634`, `render/visuals.rs:268-292`). The agent table was left on `WorkloadRow` on purpose (that doc, API Design).

### Problem

**P1: the agent table carries no volume.** Scott, 2026-10-11, on `marquee ~scott-idler/claude-report-2026-09-5`: "we could have added the same two things you did for skills and mcps here on agents, adding Calls and Sessions...."

- `phase-implementer` at $1,243.84 says nothing without a spawn count beside it. Neither does `general-purpose` at $1,421.70.
- It is the only workload table with no count and no chart.

**P2: a merged rollup report prints the `--no-rollup` caption.** Found by the design-research pass, reproduced on v0.26.1, 2026-10-11:

- `call_captions` decides "this is a `--no-rollup` report" from any session key containing `/` (`workload.rs:83`).
- `report merge` re-keys every session `<host>/<sid>` (`report/src/merge.rs:207`).
- Repro: `jq '.host="other"' 2026-09-report.json > other.json`, `clyde report merge 2026-09-report.json other.json -o merged.json`, render markdown. `merged.md:344` prints "Under --no-rollup, a subagent that inherited its parent's skill tag shows that skill's tokens with 0 calls". The report is a rollup.
- Scott's standing ruling covers it: "we are fixing everything we find."

### Goals

- G1 (Scott, "adding Calls and Sessions"): Calls and Sessions columns on the agent table, in HTML, Markdown and marquee formats.
- G2 (Scott, "the same two things you did for skills and mcps"): the table matches its siblings. That means $/call and the % of calls vs % of spend chart too, same columns, same chart shape, same `render.table-top-n`. Siblings behave identically (`rules/taste.md`).
- G3 (Scott, "we are fixing everything we find"): the `--no-rollup` caption prints only on a `--no-rollup` report.

### Non-Goals

- **Excluded: time per spawn.** Not asked for, and no data exists: no per-subagent timestamp in `RawCounters` (`efficiency/src/metrics.rs`), 0 of 1,159 September subagents carry `turn-durations-ms`, meta files carry no timing. It would need new extract fields and catalog v17.
- **Excluded: counting failed spawns.** 58 September `Agent` tool_uses errored and never ran; 27 teammate spawns left no transcript anywhere. A call is an agent that ran, which is the unit that carries spend.
- **Excluded: coverage machinery (`†`, "Calls counted on N of M").** Every v16 blob carries `subagents`: the field has no `#[serde(default)]` (`efficiency/src/fold.rs:65`), so a blob without it cannot parse. Measured: 0 blobs lack it. Spawns are always counted.
- **Excluded: facts for agent calls.** The skill and MCP rows register no facts either.

## Proposed Solution

### Overview

```
render: build_efficiency_view(report)
  for entry in report.sessions:
    session_agent_cents(entry)         <- today: tokens + cents per bucket
      + spawns per bucket              <- new: count entry.efficiency.subagents by type, has_tokens only
      + sessions += 1 per bucket held  <- new
  agent rows -> CallRow                <- Calls | Sessions | Tokens | Spend | $/call
  agent figure -> call_pairs           <- (main-session) not drawn
```

### Architecture

**What a call is.**

- One `SubagentEfficiency` entry with tokens. Measured 2026-10-11:
  - one entry per `agentId`, one file per `agentId`: 0 `agentId`s in more than one file, 0 files holding two
  - for September, catalog subagent ids equal live file ids in all 174 sessions with subagents
  - a resumed agent (SendMessage, teammate continuation) appends to its own file under the same `agentId`: 66 resumes across 58 ids, each still one file. **A resumed agent counts once.**
  - a fork is an ordinary type: all 105 fork metas have `agentType: "fork"`, its own `agentId` and its own row
- **Zero-token subagents are not counted.** The same `has_tokens` rule `agent_type_costs` applies (`report.rs:701-703`), so the row set with calls equals the cost row set. Live: 0 in September, 8 catalog-wide (all untyped, June/July, 0 turns).
- `agent_type` of `None` counts under `unknown`, matching the cost bucket. `UNKNOWN_AGENT_TYPE` (`report.rs:53`) becomes `pub` so render keys it the same way.

**Counting.** In `build_efficiency_view`'s agent loop (`workload.rs:56-60`):

- `AgentCents` (`workload.rs:311-315`) gains `spawns: u64` and `sessions: usize`.
- Per entry: spawns per type from `entry.efficiency.subagents`; `sessions += 1` for every bucket the entry holds, `(main-session)` included.
- Spend stays the apportioned cents. `tally_calls` is not reused: it sums `WorkloadCost.cost_usd`, which would break the partition invariant.

**Rows.**

- Agent rows become `CallRow` (`render.rs:598`): `server: None`, `time: None`, `avg: None`, `partial: false`. `WorkloadRow` has no other user and is deleted (the workspace denies `dead_code`).
- Sort stays by cents, as `partition_rows` does today.
- `(main-session)`: Calls `n/a`, $/call `n/a`, Sessions as counted. It is not a spawn, and `n/a` is the precedent's rule for a figure that does not apply. Never `0`.
- $/call = row cents / spawns.

**Chart.**

- One calls/spend figure for the agent table through `call_pairs` (`visuals.rs:268-292`), `PairsKind::CALLS`.
- `call_pairs` returns `None` when any row has `counts == None` (`visuals.rs:268-270`). That check is how a skill or MCP table with nothing counted (N == 0, every row count-less) draws no figure.
- It changes to: skip count-less rows; return `None` when no row has counts. So `(main-session)` is left out instead of killing the figure, and the N == 0 case still draws nothing. A counted skill or MCP table has no count-less rows, so its figure does not change.
- Shares are of the drawn rows' own totals, the rule `call_pairs` documents today (`visuals.rs:263`). So the agent figure's spend share is of delegated spend, and the two bars compare the same population.
- `render.table-top-n` cuts the table first, as for the siblings. `(main-session)` is usually the top row, so the figure usually draws one row fewer than the table.

**`--no-rollup`.**

- The residual row carries `subagents: Vec::new()` (`report.rs:536`) and each subagent row carries `vec![sub]` (548). So per-type Calls sum to the rollup count.
- Sessions counts per entry (the documented tradeoff at `workload.rs:33-34`), so under `--no-rollup` a subagent type's Sessions equals its Calls. README says so.

**P2 fix: detect `--no-rollup` from the rows, not the key shape.**

- A `--no-rollup` subagent row is keyed `<sid>/<agent-id>` (`report.rs:551`) and holds exactly one subagent whose `agent_id` is that suffix.
- New predicate `is_subagent_row(key, entry)`: the key's last `/` segment equals the `agent_id` of the entry's only subagent.
- A merged rollup key `<host>/<sid>` never matches: no subagent's `agent_id` is a session id.
- `subagent_rows = report.sessions.iter().any(is_subagent_row)` replaces `keys().any(|k| k.contains('/'))`. Merged `--no-rollup` reports (`<host>/<sid>/<agent-id>`) still match.
- No schema change.

### Data Model

- No change to `SessionEntry`, the report schema (stays v4), the catalog (stays v16) or the efficiency blob.
- `fixtures/report/*/report.json` do not change. Only the goldens do.

### API Design

- `AgentCents { tokens, cents, spawns: u64, sessions: usize }`.
- `report::UNKNOWN_AGENT_TYPE` becomes `pub`.
- `EfficiencyView.agent_type_costs: Vec<CallRow>` (was `Vec<WorkloadRow>`). `facts.rs:234-238` reads `name` / `spend` / `tokens_human`, which `CallRow` has, so it compiles unchanged.
- Columns: `Agent Type | Calls | Sessions | Tokens | Spend | $/call`, in HTML (`render/html.rs:703-731`) and Markdown (`render/document.rs:409-428`). Charts stay HTML only.
- Number licensing: `CallRow` strings serialize into the `ContextBlock`. The new figure is added to `visuals::display_strings` (`visuals.rs:~699`) and the pairs row-count loop (`render/html/tests.rs:~160`).

### Implementation Plan

Every `cargo test ...` criterion requires the output line `test result: ok. 1 passed`. A filter that matches no test also exits 0.

#### Phase 1: Detect `--no-rollup` from the rows
**Model:** sonnet
- `is_subagent_row` in `render/workload.rs`, replacing the key-shape check at `workload.rs:83`.
- **Success criteria:**
  - `cargo test -p report render::tests::workload::merged_rollup_report_has_no_no_rollup_caption`: two counted rollup reports merged under different hosts render with no "Under --no-rollup" caption.
  - `cargo test -p report render::tests::workload::merged_no_rollup_report_keeps_caption`: the same with `--no-rollup` inputs still prints it.

#### Phase 2: Agent calls and sessions in the view
**Model:** opus
- `UNKNOWN_AGENT_TYPE` `pub`; `AgentCents` spawns and sessions; agent rows as `CallRow`; `(main-session)` `n/a`; delete `WorkloadRow`.
- **Success criteria:**
  - `cargo test -p report render::tests::workload::agent_calls_equal_subagent_entries`: on synth medium, Calls per type equal the count of tokened `efficiency.subagents` per type, and the rows still sum to `totals.spend` in cents.
  - `cargo test -p report render::tests::workload::agent_calls_same_under_no_rollup`: per-type Calls are equal with and without rollup.
  - `cargo test -p report render::tests::workload::zero_token_subagent_adds_no_call`: a 0-token untyped subagent creates no `unknown` row and no call; `(main-session)` shows Calls `n/a`.

#### Phase 3: Columns, chart, goldens
**Model:** sonnet
- HTML and Markdown columns; `call_pairs` skips count-less rows; the agent figure; `display_strings` and the row-count loop.
- Regenerate goldens with `cargo run -p clyde -- report eval --write-goldens` and review the diff.
- **Success criteria:**
  - `cargo test -p report render::html::tests::the_page_draws_every_chart_the_data_supports` expects 5 pair figures.
  - `cargo test -p report render::html::tests::agent_calls_pairs_labels_and_numbers`: asserts the labels; mutating an agent call share in the view fails licensing; `(main-session)` is not drawn.
  - `cargo test -p report render::visuals::tests::call_pairs_skips_countless_rows_and_none_when_all_are`: a mix of counted and count-less rows draws only the counted ones; all count-less returns `None`.
  - `git diff` of the goldens touches no skill or MCP table row and no skill or MCP figure.

#### Phase 4: Docs
**Model:** sonnet
- `report/README.md`: what an agent call is (an agent that ran; a resume counts once; failed spawns and `(main-session)` do not), `n/a` on `(main-session)`, the `--no-rollup` Sessions note.
- **Success criteria:** `rg -c 'agent call' report/README.md` prints 1 or more.

## Acceptance Criteria

Run against the installed build after re-rendering September from the existing `2026-09-report.json` (no re-collect, no reindex), as `report.html` and `report.md`. Observed lines are from v0.26.1 on 2026-10-11.

- [ ] HTML header:
  - Command: `rg -U -o --multiline-dotall '>Agent-Type Cost Attribution<.*?</thead>' report.html | rg -o '<th[^>]*>[^<]*</th>' | sed 's/<th[^>]*>//g;s/<\/th>/|/g' | tr -d '\n'`
  - Expected: `Agent Type|Calls|Sessions|Tokens|Spend|$/call|`
  - Observed on main: `Agent Type|Tokens|Spend|`
- [ ] Markdown header:
  - Command: `rg -n '^\| Agent Type ' report.md`
  - Expected: `| Agent Type | Calls | Sessions | Tokens | Spend | $/call |`
  - Observed on main: `149:| Agent Type | Tokens | Spend |`
- [ ] Figures:
  - Command: `rg -c '<figure class="chart pairs-fig' report.html`
  - Expected: `5`
  - Observed on main: `4` (org, repo, skill calls, MCP calls; one figure per line)
- [ ] Calls match the data:
  - Command: `jq -r '[.sessions[].efficiency.subagents[] | select(([.signals.raw."by-model"[].total]|add // 0) > 0) | (."agent-type" // "unknown")] | group_by(.) | map("\(.[0]) \(length)") | .[]' 2026-09-report.json`
  - Expected: each type's Calls cell in `report.md` equals its count here (e.g. `general-purpose 557`, `phase-implementer 265`, `review-panel 168`).
  - Observed on main: the jq prints 17 types, `general-purpose 557` through nine types at `1`; `report.md` has no Calls column.
- [ ] Merged caption (P2):
  - Command: `jq '.host="other"' 2026-09-report.json > other.json; clyde report merge 2026-09-report.json other.json -o merged.json; clyde report render -i merged.json --format markdown -o merged.md; rg -c 'Under --no-rollup' merged.md`
  - Expected: prints nothing, exit 1.
  - Observed on main: `merged.md:344` holds the caption (`rg -c` prints `1`).

## Resolved Decisions

- 2026-10-11, Scott: the agent table gets the same treatment as the skill and MCP tables.
- 2026-10-11, Scott: design doc, not a targeted fix (he invoked `/create-design-doc` after "targetted fix or full design doc?").
- 2026-10-11, author, from the design-research brief:
  - Count in render from `efficiency.subagents`; add no `SessionEntry` field. A stored count would be a derived field that can drift from the list it is derived from.
  - Zero-token subagents are not calls, so the call rows and the cost rows are the same set.
  - `(main-session)` shows `n/a` for Calls and $/call.
  - P2 is fixed here, under Scott's "we are fixing everything we find."

## Alternatives Considered

### Alternative 1: Count `Agent` tool_use calls in extract
- **Description:** count spawn attempts the way skill calls are counted, store them in the blob.
- **Why not chosen:** it counts failed spawns (58 in September) that never ran and carry no spend, and it needs catalog v17 plus a reindex. The subagent list already answers "how many ran".

### Alternative 2: A `SessionEntry` field for spawns per type
- **Description:** collect writes `agent-type-calls`, render reads it.
- **Why not chosen:** it duplicates `efficiency.subagents` in the same entry. Two sources for one number can disagree; one cannot.

### Alternative 3: Calls and Sessions only, no $/call or chart
- **Description:** the two columns Scott named, nothing else.
- **Why not chosen:** Scott asked for "the same two things you did for skills and mcps". Those tables carry $/call and a chart, and siblings behave identically. A table with Calls but no $/call is the one inconsistent table in the report.

### Alternative 4: Mark `--no-rollup` in the report JSON
- **Description:** a `rollup: bool` field on `Report` for P2.
- **Why not chosen:** a schema change for a fact the rows already prove.

## Technical Considerations

### Dependencies
- None new. Pure render code in `report`.

### Performance
- One extra pass over each session's subagent list, already in memory.

### Security
- No new inputs or outputs. Names render through the escaped `text()` / `attr()` emitters.

### Testing Strategy
- Calls against the subagent list on synth medium, rollup and `--no-rollup`.
- Negative cases: a 0-token untyped subagent, `(main-session)` `n/a`, a merged rollup report.
- Licensing for the new figure, plus a label test.
- Goldens regenerated, diff reviewed.

### Rollout Plan
- One PR, the phases as commits, then `bump release` -> merge -> `bump finish`.
- Re-render September from the existing `2026-09-report.json` and replace `~scott-idler/claude-report-2026-09-5`. No reindex, no re-collect.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| A future Claude Code splits one agent over several files | Low | Calls over-count | The Phase 2 test pins entries per type; the corpus measure is in this doc |
| `call_pairs` change moves the skill or MCP figures | Low | Goldens drift | Phase 3 criterion: no skill or MCP figure in the golden diff |
| `is_subagent_row` misses a `--no-rollup` row | Low | The caption disappears where it belongs | Phase 1 tests both merged and unmerged `--no-rollup` |

## Open Questions

- None.

## References

- `docs/design/2026-10-11-report-workload-calls.md`: the precedent (columns, `n/a`, chart, licensing).
- `docs/design/2026-07-26-report-story-fidelity.md`: the agent-type partition.
- `marquee ~scott-idler/claude-report-2026-09-5`.
