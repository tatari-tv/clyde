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

Work fell on 9 of the 20 days in the window, with a distinct empty stretch: every day from 2026-05-07 through 2026-05-14 is an `active: false` row in `aggregates.by-day`, splitting the period into an early cluster (2026-05-01 through 2026-05-06) and a later one (2026-05-15 through 2026-05-19), plus $12.34 carried in from 3 sessions that began before the window. All sessions landed in a single repository, `jrivera/driftwood`, which accounts for the full $49.47 of attributed spend. The session titles center on two lines of work: teaching driftwood to report a line number and splitting the driftwood parser module. Note that 0 of 12 sessions in the window (0.0%) carry an enrich summary; the rest are cited by title only, so the narrative below rests on titles rather than digests of the full transcripts.

## Cost Summary

| Model | Sessions Using | Total Tokens | Spend |
|---|---|---|---|
| claude-sonnet-4-6 | 11 | 87.6M | $49.48 |
| claude-nimbus-2 | 1 | 10.3M | (untracked) |
| **Total** | 12 | 97.9M | $49.48 |

The `claude-nimbus-2` row is excluded from the total; its spend was not computed.

> **Note: spend for the following models was not computed because they are not in this binary's pricing table: `claude-nimbus-2`. The total above understates actual spend. Update clyde's pricing data to include them.**

The `unit-costs` block records these ratios for the period: the period spent $49.48 across 9 active days, a ratio of $5.50 per active day, and across 12 sessions, a ratio of $4.12 per session. Session spend has a median (`session-spend-p50`) of $4.30 and a 90th-percentile (`session-spend-p90`) of $5.27; the per-session ratio of $4.12 sitting near the median indicates the sessions were closely grouped in cost rather than carried by a few large ones.

## Reconciliation

No Claude Enterprise Analytics cost export was supplied for this render (--reconcile <file>); the total spend above is a modeled figure only and has not been reconciled against this operator's authoritative billed spend.

## Agent-Type Cost Attribution

| Agent Type | Tokens | Spend |
|---|---|---|
| (main-session) | 97.9M | $49.48 |

All spend is attributed to `(main-session)`, the work the sessions did directly rather than delegating to a subagent. This table is a partition of the period spend and accounts for the whole $49.48.

## The Efficiency Story

Cache reads made up 93.3% of the context the model read (`cache-read-share`), meaning most of the context each turn was re-read from cache at a fraction of the fresh-input rate, which is what makes sustained agentic sessions economical. Input tokens totaled 1.0M against 91.1M cache-read tokens.

Additional signals from `efficiency`:

- Tool error rate: 5.2% of tool calls errored.
- Cache 1h-write fraction: 3.0% of cache writes paid the 1h premium.
- Interrupts: 5 times the user interrupted.
- Compactions: 2 times the context was compacted.

Skill attribution (tags, not a partition):

| Skill | Tokens | Spend |
|---|---|---|
| release-notes | 3.3M | $6.13 |
| schema-review | 1.9M | $3.44 |

Coverage: $9.57 of $49.48 (19.3%), embedded-price basis.

MCP tool attribution (tags, not a partition):

| MCP Tool | Tokens | Spend |
|---|---|---|
| wiki-write | 6.5M | $11.99 |
| tracker-search | 3.4M | $6.23 |

Coverage: $18.22 of $49.48 (36.8%), embedded-price basis.

## What This Funded

All 12 sessions in the window belong to `jrivera`'s personal org, in the repository `jrivera/driftwood`. The persona identifies Northwind Media as the employer org; no sessions in this window touched a Northwind Media repository, so only the personal-org tier is present. `driftwood` is the parser tool being iterated on across the period; the session labels group into two lines of work.

- `jrivera/driftwood` (12 sessions, 97.9M tokens, $49.47 spend): the sessions divide between two title-labeled efforts. Line-number reporting is the dominant thread, carried by sessions labeled "Teach driftwood to report a line number" (for example `2b3bd835`, `3cecc43a`, and `455f349d`). The second thread is parser restructuring, labeled "Split the driftwood parser module" (`1a45219a`, `0e6ec74b`). None of these sessions carry an enrich summary, so these themes rest on session titles rather than transcript digests, and no per-repo `outcomes` were recorded for this repository.

## Usage Profile

**Temporal distribution:** The active days form two clusters, 2026-05-01 through 2026-05-06 and 2026-05-15 through 2026-05-19, separated by a full inactive run: 2026-05-07 through 2026-05-14 are all `active: false` rows in `aggregates.by-day`, and 2026-05-17 and 2026-05-20 are also inactive. The heaviest single-day spend rows are 2026-05-18 ($5.83) and 2026-05-19 ($5.27). Separately, $12.34 across 3 sessions is carried in from sessions that began before 2026-05-01; the by-day series does not cover that spend.

**Model mix:** `claude-sonnet-4-6` appears in 11 of the sessions and covers both threads, the line-number reporting and parser-split work in `jrivera/driftwood`. `claude-nimbus-2` appears in a single session (`422fdead`, labeled "Split the driftwood parser module") and is untracked in this binary's pricing.

**Outlier sessions:**

| Session | Repo | Tokens | Spend | What it produced |
|---|---|---|---|---|
| Teach driftwood to report a line number | jrivera/driftwood | 9.7M | $5.83 | Line-number reporting work (title label; no summary or outcome fields recorded) |
| Split the driftwood parser module | jrivera/driftwood | 9.0M | $5.27 | Parser module split (title label; no summary or outcome fields recorded) |
| Teach driftwood to report a line number | jrivera/driftwood | 8.5M | $4.83 | Line-number reporting work (title label; no summary or outcome fields recorded) |
| ef35bcda | jrivera/driftwood | 7.6M | $4.56 | Untitled session in driftwood; no summary or outcome fields recorded |
| Teach driftwood to report a line number | jrivera/driftwood | 8.0M | $4.49 | Line-number reporting work (title label; no summary or outcome fields recorded) |
| eec33f5b | jrivera/driftwood | 7.3M | $4.30 | Untitled session in driftwood; no summary or outcome fields recorded |
| Teach driftwood to report a line number | jrivera/driftwood | 7.8M | $4.26 | Line-number reporting work (title label; no summary or outcome fields recorded) |
| 9587c1f7 | jrivera/driftwood | 7.6M | $4.15 | Untitled session in driftwood; no summary or outcome fields recorded |
| Split the driftwood parser module | jrivera/driftwood | 7.5M | $4.04 | Parser module split (title label; no summary or outcome fields recorded) |
| Teach driftwood to report a line number | jrivera/driftwood | 7.5M | $4.04 | Line-number reporting work (title label; no summary or outcome fields recorded) |

## Conclusion

Over 2026-05-01 to 2026-05-20, 12 sessions in `jrivera/driftwood` carried $49.48 in modeled spend across two title-labeled threads: line-number reporting and splitting the parser module. Both threads were still being worked in the late-period cluster of 2026-05-18 and 2026-05-19, the two highest-spend active days in the window.

## Methodology

- "window is session-level (M2): whole sessions whose catalog `modified` falls in [since, until]; not per-record like pre-v2 reports, so a boundary-straddling session's numbers can differ from a v1 report."