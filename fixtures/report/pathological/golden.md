---
title: "Claude Usage Report - Jordan Rivera - 2026-05-01 - 2026-05-20"
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

Work landed on 9 of 20 days, split into two clusters with a long dormant stretch between them: activity ran 2026-05-01 through 2026-05-06, went quiet for the run of inactive days from 2026-05-07 through 2026-05-14, then resumed 2026-05-15 through 2026-05-19, plus $12.34 carried in from 3 sessions that began before the window. Every session in the period touched a single repo, jrivera/driftwood, so all $49.47 of attributed spend concentrated there. The heaviest single days were 2026-05-18 ($5.83) and 2026-05-19 ($5.27), at the tail of the second cluster. No commits, PRs, or other tool-recorded outputs were captured for this period, so the narrative below rests on session titles rather than observed artifacts.

## Cost Summary

| Model | Sessions Using | Total Tokens | Spend |
|---|---|---|---|
| claude-sonnet-4-6 | 11 | 87.6M | $49.48 |
| claude-nimbus-2 | 1 | 10.3M | (untracked) |
| **Total** | 12 | 97.9M | $49.48 |

The claude-nimbus-2 row is untracked and excluded from the total.

> **Note: spend for the following models was not computed because they are not in this binary's pricing table: `claude-nimbus-2`. The total above understates actual spend. Update clyde's pricing data to include them.**

Two or three unit-cost ratios are worth stating. The period spent $49.48 across 9 active days, a ratio of $5.50 per active day, and across 12 sessions, a ratio of $4.12 per session. The median session spend was $4.30 and the 90th-percentile session spend was $5.27; the per-session mean of $4.12 sits below that median, indicating the sessions were closely clustered in size rather than driven by a few large outliers.

## Reconciliation

No Claude Enterprise Analytics cost export was supplied for this render (--reconcile <file>); the total spend above is a modeled figure only and has not been reconciled against the account's authoritative billed spend.

## Agent-Type Cost Attribution

| Agent Type | Tokens | Spend |
|---|---|---|
| (main-session) | 97.9M | $49.48 |

All spend attributed to the main session with no subagent delegation observed; this table partitions the whole period spend of $49.48.

## The Efficiency Story

Cache reads accounted for 93.3% of context reads (cache-read-share), meaning most of the context the model read each turn was re-read from cache at a fraction of the fresh-input rate, which is what makes sustained agentic sessions economical.

Additional signals from this period:

- Tool-error rate: 5.2% of tool calls errored.
- Cache 1h-write fraction: 3.0% of cache writes paid the 1h premium.
- Interrupts: 5 times the user interrupted.
- Compactions: 2 times the context was compacted.

Skill attribution (tags, not a partition):

| Skill | Tokens | Spend |
|---|---|---|
| release-notes | 3.3M | $6.13 |
| schema-review | 1.9M | $3.44 |

Skill attribution covers $9.57 of $49.48 (19.3%), embedded-price basis.

MCP-tool attribution (tags, not a partition):

| MCP Tool | Tokens | Spend |
|---|---|---|
| wiki-write | 6.5M | $11.99 |
| tracker-search | 3.4M | $6.23 |

MCP attribution covers $18.22 of $49.48 (36.8%), embedded-price basis.

## What This Funded

All sessions this period ran in Jordan Rivera's personal org (jrivera). None carry an enrich summary: 0 of 12 sessions in the window (0.0%) carry an enrich summary; the rest are cited by title only, so the themes below rest on session titles alone.

### Personal-org work

`jrivera/driftwood` (12 sessions, 97.9M tokens, $49.47 spend): driftwood is a parser tool the user is developing. Two threads of work dominate the period's session titles: teaching the parser to report line numbers ("Teach driftwood to report a line number", sessions 2b3bd835, 3cecc43a, 455f349d, f9a7f9d2, 2439cf48) and restructuring the parser into separate modules ("Split the driftwood parser module", sessions 1a45219a, 0e6ec74b). Three additional sessions (ef35bcda, eec33f5b, 9587c1f7) carry no title. No commits, PRs, or other outputs were recorded for these sessions.

## Usage Profile

- **Temporal distribution**: Work fell into two clusters. The first ran 2026-05-01 through 2026-05-06, with the highest early-cluster day at 2026-05-02 ($4.83). A gap of inactive days followed, from 2026-05-07 through 2026-05-14. The second cluster ran 2026-05-15 through 2026-05-19, carrying the period's two heaviest days at 2026-05-18 ($5.83) and 2026-05-19 ($5.27). Note that 2026-05-03 was active but recorded $0.00 in by-day spend (its lone session ran on the untracked claude-nimbus-2 model). Separately, $12.34 across 3 sessions carried in from before the window; the by-day series does not cover that spend.
- **Model mix**: claude-sonnet-4-6 carried all 11 tracked sessions in jrivera/driftwood, spanning both the line-number and parser-split work. claude-nimbus-2 appeared in a single session (422fdead, titled "Split the driftwood parser module") and is untracked.
- **Outlier sessions**:

| Session | Repo | Tokens | Spend | What it produced |
|---|---|---|---|---|
| Teach driftwood to report a line number | jrivera/driftwood | 9.7M | $5.83 | Titled work on line-number reporting; no recorded outcomes. |
| Split the driftwood parser module | jrivera/driftwood | 9.0M | $5.27 | Titled work splitting the parser module; no recorded outcomes. |
| Teach driftwood to report a line number | jrivera/driftwood | 8.5M | $4.83 | Titled work on line-number reporting; no recorded outcomes. |
| ef35bcda | jrivera/driftwood | 7.6M | $4.56 | Untitled driftwood session; no recorded outcomes. |
| Teach driftwood to report a line number | jrivera/driftwood | 8.0M | $4.49 | Titled work on line-number reporting; no recorded outcomes. |
| eec33f5b | jrivera/driftwood | 7.3M | $4.30 | Untitled driftwood session; no recorded outcomes. |
| Teach driftwood to report a line number | jrivera/driftwood | 7.8M | $4.26 | Titled work on line-number reporting; no recorded outcomes. |
| 9587c1f7 | jrivera/driftwood | 7.6M | $4.15 | Untitled driftwood session; no recorded outcomes. |
| Split the driftwood parser module | jrivera/driftwood | 7.5M | $4.04 | Titled work splitting the parser module; no recorded outcomes. |
| Teach driftwood to report a line number | jrivera/driftwood | 7.5M | $4.04 | Titled work on line-number reporting; no recorded outcomes. |

## Conclusion

This period's work concentrated entirely in jrivera/driftwood, split between teaching the parser to report line numbers and splitting the parser module, at a modeled $49.48 across 12 sessions. Both threads appear active into the second cluster's end on 2026-05-19, with no tool-recorded outputs captured for the window.