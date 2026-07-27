---
title: "Claude Usage Report - Jordan Rivera - 2026-05-01 to 2026-05-20"
date: 2026-06-01
type: note
domain: work
tags:

  - claude
  - enterprise
  - usage
  - report

---

# Claude Enterprise Usage Report

**Author:** Jordan Rivera
**Title:** Staff Software Engineer
**Team:** Platform Tooling
**Period:** 2026-05-01 - 2026-05-20
**Total Spend:** $49.48
**Pricing Basis:** Total spend is modeled Claude Code catalog spend at published list rates; account-level billed spend comes from Claude Enterprise Analytics.
**Sessions:** 12 across 1 repositories
**Active Days:** 9 of 20

---

## Executive Summary

Work fell on 9 of 20 days, split into two clusters with a long quiet stretch between them: an early run from 2026-05-01 through 2026-05-06, then nothing active from 2026-05-07 through 2026-05-14, then a second run from 2026-05-15 through 2026-05-19, plus $12.34 carried in from 3 sessions that began before the window. Every session in the window landed in a single repo, `jrivera/driftwood`, so all $49.48 of modeled spend concentrated there. The heaviest single days were 2026-05-18 at $5.83 and 2026-05-19 at $5.27, closing out the second cluster. Note: 0 of 12 sessions in the window (0.0%) carry an enrich summary; the rest are cited by title only, so the narrative below rests on session titles rather than summaries.

## Cost Summary

| Model | Sessions Using | Total Tokens | Spend |
|---|---|---|---|
| claude-sonnet-4-6 | 11 | 87.6M | $49.48 |
| claude-nimbus-2 | 1 | 10.3M | (untracked) |
| **Total** | 12 | 97.9M | $49.48 |

The `claude-nimbus-2` row is untracked and excluded from the total above.

> **Note: spend for the following models was not computed because they are not in this binary's pricing table: `claude-nimbus-2`. The total above understates actual spend. Update clyde's pricing data to include them.**

Unit-cost ratios for the period: the period spent $49.48 across 9 active days, a ratio of $5.50 per active day, and $4.12 per session. The median session spend was $4.30 (`session-spend-p50`) against a 90th-percentile of $5.27 (`session-spend-p90`); the per-session mean of $4.12 sits close to the median, indicating the spend was spread fairly evenly across sessions rather than carried by a few large ones.

## Reconciliation

No Claude Enterprise Analytics cost export was supplied for this render (--reconcile <file>); the total spend above is a modeled figure only and has not been reconciled against this operator's authoritative billed spend.

## Agent-Type Cost Attribution

| Agent Type | Tokens | Spend |
|---|---|---|
| (main-session) | 97.9M | $49.48 |

All work ran in the main session with no delegation to subagents; the `(main-session)` row is a true partition of the period spend and accounts for the whole $49.48.

## The Efficiency Story

Cache reads made up 93.3% of the context the model consumed (`cache-read-share`), meaning most of what the model read each turn was re-read from cache at a fraction of the fresh-input rate rather than sent fresh; against 1.0M fresh input tokens the period logged 91.1M cache-read tokens.

Other signals from this period:

- Tool-error rate: 5.2% of tool calls errored.
- 1h cache-write fraction: 3.0% of cache writes paid the 1h premium.
- Interrupts: 5 times the user interrupted.
- Compactions: 2 times the context was compacted.

Attribution by skill (tags, not a partition):

| Skill | Tokens | Spend |
|---|---|---|
| release-notes | 3.3M | $6.13 |
| schema-review | 1.9M | $3.44 |

Coverage: $9.57 of $49.48 (19.3%), embedded-price basis.

Attribution by MCP tool (tags, not a partition):

| MCP Tool | Tokens | Spend |
|---|---|---|
| wiki-write | 6.5M | $11.99 |
| tracker-search | 3.4M | $6.23 |

Coverage: $18.22 of $49.48 (36.8%), embedded-price basis.

## What This Funded

All 12 sessions ran in Jordan Rivera's personal org, `jrivera`, and all in one repo. `driftwood` is a parser/tooling project; the period's sessions centered on two lines of work named by their titles: teaching the tool to report a line number, and splitting its parser module. Because no session in the window carries an enrich summary (0 of 12), these themes rest on session titles rather than digests of the full transcripts.

- `jrivera/driftwood` (12 sessions, 97.9M tokens, $49.48 spend): sessions titled "Teach driftwood to report a line number" (e.g. `2b3bd835`, `3cecc43a`, `455f349d`) and "Split the driftwood parser module" (`1a45219a`, `0e6ec74b`) dominated the window, alongside several untitled sessions (`9587c1f7`, `eec33f5b`, `ef35bcda`). No output outcomes were recorded for this repo, so no commits, PRs, or file edits are reported.

## Usage Profile

- **Temporal distribution**: Work formed two active clusters. The first ran 2026-05-01 through 2026-05-06, the second 2026-05-15 through 2026-05-19, separated by an eight-day inactive gap from 2026-05-07 through 2026-05-14 (all `active: false`); the window also closed inactive on 2026-05-17 and 2026-05-20. The heaviest days were 2026-05-18 ($5.83) and 2026-05-19 ($5.27). Separately, $12.34 across 3 sessions carried in from before the window and is not reflected in any by-day row.
- **Model mix**: `claude-sonnet-4-6` carried all 11 tracked sessions across the line-number and parser-split work in `driftwood`. `claude-nimbus-2` appeared in a single session (`422fdead`, "Split the driftwood parser module") on 2026-05-03 and is untracked.
- **Outlier sessions**:

| Session | Repo | Tokens | Spend | What it produced |
|---|---|---|---|---|
| Teach driftwood to report a line number | jrivera/driftwood | 9.7M | $5.83 | Work on line-number reporting (title; no summary or outcomes recorded) |
| Split the driftwood parser module | jrivera/driftwood | 9.0M | $5.27 | Parser module split (title; no summary or outcomes recorded) |
| Teach driftwood to report a line number | jrivera/driftwood | 8.5M | $4.83 | Work on line-number reporting (title; no summary or outcomes recorded) |
| ef35bcda | jrivera/driftwood | 7.6M | $4.56 | Untitled session; no summary or outcomes recorded |
| Teach driftwood to report a line number | jrivera/driftwood | 8.0M | $4.49 | Work on line-number reporting (title; no summary or outcomes recorded) |
| eec33f5b | jrivera/driftwood | 7.3M | $4.30 | Untitled session; no summary or outcomes recorded |
| Teach driftwood to report a line number | jrivera/driftwood | 7.8M | $4.26 | Work on line-number reporting (title; no summary or outcomes recorded) |
| 9587c1f7 | jrivera/driftwood | 7.6M | $4.15 | Untitled session; no summary or outcomes recorded |
| Split the driftwood parser module | jrivera/driftwood | 7.5M | $4.04 | Parser module split (title; no summary or outcomes recorded) |
| Teach driftwood to report a line number | jrivera/driftwood | 7.5M | $4.04 | Work on line-number reporting (title; no summary or outcomes recorded) |

## Conclusion

Over 9 active days the period ran 12 sessions in `jrivera/driftwood` at $49.48 modeled spend, centered on line-number reporting and a parser-module split. Both lines of work were active through the window's second cluster (2026-05-15 through 2026-05-19), including the two highest-spend days.

## Methodology note

"window is session-level (M2): whole sessions whose catalog `modified` falls in [since, until]; not per-record like pre-v2 reports, so a boundary-straddling session's numbers can differ from a v1 report."