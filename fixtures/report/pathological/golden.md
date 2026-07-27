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

Work occurred on 9 of 20 days, and the active days fall into two clusters separated by a long inactive run: an early stretch spanning 2026-05-01 through 2026-05-06, then a gap of 2026-05-07 through 2026-05-14 with no sessions, then a second cluster from 2026-05-15 through 2026-05-19. In addition, $12.34 carried in from 3 sessions that began before the window. All twelve sessions landed in a single repository, `jrivera/driftwood`. The session titles across the period center on two lines of work: teaching driftwood to report a line number and splitting the driftwood parser module. Note: 0 of 12 sessions in the window (0.0%) carry an enrich summary; the rest are cited by title only, so the narrative below rests on session titles rather than summaries.

## Cost Summary

| Model | Sessions Using | Total Tokens | Spend |
|---|---|---|---|
| claude-sonnet-4-6 | 11 | 87.6M | $49.48 |
| claude-nimbus-2 | 1 | 10.3M | (untracked) |
| **Total** | 12 | 97.9M | $49.48 |

The `claude-nimbus-2` row is excluded from the total because it is not priced in this binary.

> **Note: spend for the following models was not computed because they are not in this binary's pricing table: `claude-nimbus-2`. The total above understates actual spend. Update clyde's pricing data to include them.**

Unit costs, each a ratio of two stated figures: the period spent $49.48 across 9 active days, a ratio of $5.50 per active day, and across 12 sessions, a ratio of $4.12 per session. Session spend has a median (`session-spend-p50`) of $4.30 and a 90th percentile (`session-spend-p90`) of $5.27; the closeness of the per-session ratio to the median indicates the sessions were similar in size rather than a few large ones carrying the period.

## Reconciliation

No Claude Enterprise Analytics cost export was supplied for this render (--reconcile <file>); the total spend above is a modeled figure only and has not been reconciled against this operator's authoritative billed spend.

## Agent-Type Cost Attribution

| Agent Type | Tokens | Spend |
|---|---|---|
| (main-session) | 97.9M | $49.48 |

All spend attributes to the `(main-session)` row, meaning the work was done in the main session rather than delegated to any subagent; this table is a partition accounting for the whole period spend of $49.48.

## The Efficiency Story

Cache reads accounted for 93.3% of context read (`cache-read-share`), meaning most of the context the model read each turn was re-read from cache at a fraction of the fresh-input rate, which is what makes sustained agentic sessions economical.

Other signals from this period:

- Tool error rate: 5.2% of tool calls errored.
- Cache 1h write fraction: 3.0% of cache writes paid the 1h premium.
- Interrupts: 5 times the user interrupted.
- Compactions: 2 times the context was compacted.

Skill attribution (tags, not a partition):

| Skill | Tokens | Spend |
|---|---|---|
| release-notes | 3.3M | $6.13 |
| schema-review | 1.9M | $3.44 |

Skill attribution covers $9.57 of $49.48 (19.3%), embedded-price basis.

MCP tool attribution (tags, not a partition):

| MCP Tool | Tokens | Spend |
|---|---|---|
| wiki-write | 6.5M | $11.99 |
| tracker-search | 3.4M | $6.23 |

MCP attribution covers $18.22 of $49.48 (36.8%), embedded-price basis.

## What This Funded

All sessions this period ran in the user's personal org, `jrivera`, in a single repository. `jrivera/driftwood` is the sole locus of work.

- `jrivera/driftwood` (12 sessions, 97.9M tokens, $49.47 spend): work split between two efforts named by session titles, teaching driftwood to report a line number (sessions `2b3bd835`, `3cecc43a`, `455f349d`, `f9a7f9d2`, `a1d0adc8`, `2439cf48`) and splitting the driftwood parser module (sessions `1a45219a`, `0e6ec74b`, `422fdead`). No enrich summaries are available for these sessions, so these themes rest on titles alone; three sessions (`ef35bcda`, `eec33f5b`, `9587c1f7`) carry no title. No output outcomes were observed for this repo.

## Usage Profile

- **Temporal distribution**: Active days cluster into an early run (2026-05-01, 2026-05-02, 2026-05-03, 2026-05-05, 2026-05-06) and a later run (2026-05-15, 2026-05-16, 2026-05-18, 2026-05-19), separated by an inactive run from 2026-05-07 through 2026-05-14. The window closes with an inactive day on 2026-05-20. The heaviest single-day spend was 2026-05-18 at $5.83, followed by 2026-05-19 at $5.27. Separately, $12.34 across 3 sessions carried in from before the window; the by-day series does not cover it.
- **Model mix**: `claude-sonnet-4-6` appeared across both lines of driftwood work, the line-number and parser-module efforts. `claude-nimbus-2` appeared in a single session (`422fdead`), a parser-module session, and its spend is untracked.
- **Outlier sessions**:

| Session | Repo | Tokens | Spend | What it produced |
|---|---|---|---|---|
| Teach driftwood to report a line number | jrivera/driftwood | 9.7M | $5.83 | Line-number reporting work (title; no summary or outcomes recorded) |
| Split the driftwood parser module | jrivera/driftwood | 9.0M | $5.27 | Parser-module split work (title; no summary or outcomes recorded) |
| Teach driftwood to report a line number | jrivera/driftwood | 8.5M | $4.83 | Line-number reporting work (title; no summary or outcomes recorded) |
| ef35bcda | jrivera/driftwood | 7.6M | $4.56 | Untitled driftwood session (no summary or outcomes recorded) |
| Teach driftwood to report a line number | jrivera/driftwood | 8.0M | $4.49 | Line-number reporting work (title; no summary or outcomes recorded) |
| eec33f5b | jrivera/driftwood | 7.3M | $4.30 | Untitled driftwood session (no summary or outcomes recorded) |
| Teach driftwood to report a line number | jrivera/driftwood | 7.8M | $4.26 | Line-number reporting work (title; no summary or outcomes recorded) |
| 9587c1f7 | jrivera/driftwood | 7.6M | $4.15 | Untitled driftwood session (no summary or outcomes recorded) |
| Split the driftwood parser module | jrivera/driftwood | 7.5M | $4.04 | Parser-module split work (title; no summary or outcomes recorded) |
| Teach driftwood to report a line number | jrivera/driftwood | 7.5M | $4.04 | Line-number reporting work (title; no summary or outcomes recorded) |

## Conclusion

This period's work ran entirely in `jrivera/driftwood`, split by session title between teaching driftwood to report a line number and splitting the driftwood parser module. No output outcomes were recorded, and both lines of work were still in progress in the late-period sessions of 2026-05-18 and 2026-05-19.