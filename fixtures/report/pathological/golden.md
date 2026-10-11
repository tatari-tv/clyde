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
**Total Spend:** $49.47
**Pricing Basis:** Total spend is modeled Claude Code catalog spend at published list rates; account-level billed spend comes from Claude Enterprise Analytics.
**Sessions:** 12 across 1 repository
**Active Days:** 9 of 20

> window is session-level (M2): whole sessions whose catalog `modified` falls in [since, until]; not per-record like pre-v2 reports, so a boundary-straddling session's numbers can differ from a v1 report.

---

## Executive Summary

## What the Money Produced

Every session falls in exactly one band, and the first rule that matches wins. Names say what was observed in the transcript, not what the session achieved.

| Band | Sessions | Spend | Share of Spend |
|------|---------:|------:|---------------:|
| `committed` | 0 | $0.00 | 0% |
| `edited` | 0 | $0.00 | 0% |
| `wrote-records` | 0 | $0.00 | 0% |
| `no-observed-outcome` | 12 | $49.47 | 100% |
| **Total** | 12 | $49.47 | |

- `committed`: at least one commit or opened pull request
- `edited`: files edited, no commit or pull request
- `wrote-records`: a Confluence, Jira or Slack write, no file edit
- `no-observed-outcome`: none of the above

`no-observed-outcome` is not "no result": a sample shows it is mostly `claude -p` hook sessions that answered a question, and files written through Bash heredocs or commits made by subprocesses are invisible to `outcome::extract`.

## Cost Summary

| Model | Sessions Using | Total Tokens | Spend |
|-------|---------------:|-------------:|------:|
| `claude-sonnet-4-6` | 11 | 87.6M | $49.47 |
| `claude-nimbus-2` | 1 | 10.3M | (untracked) |
| **Total** | 12 | 97.9M | $49.47 |

**Note: spend for the following models was not computed because they are not in this binary's pricing table: `claude-nimbus-2`. The total above understates actual spend. Update clyde's pricing data to include them.**

## Reconciliation

No Claude Enterprise Analytics cost export was supplied for this render (--reconcile <file>); the total spend above is a modeled figure only and has not been reconciled against this operator's authoritative billed spend.

## Agent-Type Cost Attribution

Every dollar of the period's $49.47 spend lands in exactly one row below; `(main-session)` carried the most. The `(main-session)` row is work a session did itself rather than delegating.

| Agent Type | Tokens | Spend |
|------------|-------:|------:|
| `(main-session)` | 97.9M | $49.47 |

## The Efficiency Story

- Cache read share was 93.3%: most of the context read each turn is re-read from cache at a fraction of the fresh-input rate, which is what makes sustained agentic sessions economical. Fresh input was 1.0M against 91.1M read from cache.
- Tool error rate: 5.2%.
- Share of cache writes paying the 1h premium: 3.0%.
- Interrupts observed: 5.
- Context compactions observed: 2.

| Skill | Calls | Sessions | Tokens | Spend | $/call |
|---|---:|---:|---:|---:|---:|
| `release-notes` | 9 | 3 | 3.3M | $6.13 | $0.68 |
| `schema-review` | 4 | 2 | 1.9M | $3.44 | $0.86 |

Coverage: $9.57 of $49.47 (19.3%), embedded-price basis

| Server | MCP Tool | Calls | Sessions | Time | Avg | Tokens | Spend | $/call |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| `_unknown` | `wiki-write` | 16 | 5 | 13.0s | 0.8s | 6.5M | $11.99 | $0.75 |
| `tracker` | `search` | 3† | 4 | 0.9s | 0.9s | 3.4M | $6.23 | $1.06 |

Coverage: $18.22 of $49.47 (36.8%), embedded-price basis

† includes sessions recorded before call counting

Calls counted on 10 of 12 sessions

## Unobserved and Flagged Spend

Spend worth a second look. These rows overlap and are not a partition of the total.

- Sessions in `no-observed-outcome`: 12, spending $49.47 (100% of session spend).
- Heavy model on a short session: 0 sessions, $0.00. Heavy means a model name containing `opus`, `fable`; short means at most 3 turns.

What `no-observed-outcome` does and does not mean is noted under the band table above.

No session carries an efficiency flag.

## What This Funded

### jrivera

12 sessions across 1 repository, 97.9M tokens, $49.47.

- `jrivera/driftwood` (12 sessions, 97.9M tokens, $49.47 spend)

## Usage Profile

Daily figures are whole UTC days: each session's spend sits on the days its turns ran, not the day it began. The daily series shows $38.06 of the period's $49.47 spend; the other $11.41 was spent on days outside the window by 3 sessions the window includes.

1 session has no per-day breakdown (the transcript was gone when the catalog was rebuilt), so its spend sits whole on the day it began: $4.26.

**Daily spend**

| Day | Spend |
|-----|------:|
| 2026-05-01 | $5.23 |
| 2026-05-02 | $4.83 |
| 2026-05-03 | $0.00 |
| 2026-05-04 | $0.00 |
| 2026-05-05 | $4.56 |
| 2026-05-06 | $3.70 |
| 2026-05-07 | $0.00 |
| 2026-05-08 | $0.00 |
| 2026-05-09 | $0.00 |
| 2026-05-10 | $0.00 |
| 2026-05-11 | $0.00 |
| 2026-05-12 | $0.00 |
| 2026-05-13 | $0.00 |
| 2026-05-14 | $0.00 |
| 2026-05-15 | $4.49 |
| 2026-05-16 | $4.15 |
| 2026-05-17 | $0.00 |
| 2026-05-18 | $5.83 |
| 2026-05-19 | $5.27 |
| 2026-05-20 | $0.00 |

**Daily active sessions**

| Day | Sessions |
|-----|------:|
| 2026-05-01 | 2 |
| 2026-05-02 | 1 |
| 2026-05-03 | 1 |
| 2026-05-04 | 0 |
| 2026-05-05 | 1 |
| 2026-05-06 | 1 |
| 2026-05-07 | 0 |
| 2026-05-08 | 0 |
| 2026-05-09 | 0 |
| 2026-05-10 | 0 |
| 2026-05-11 | 0 |
| 2026-05-12 | 0 |
| 2026-05-13 | 0 |
| 2026-05-14 | 0 |
| 2026-05-15 | 1 |
| 2026-05-16 | 1 |
| 2026-05-17 | 0 |
| 2026-05-18 | 1 |
| 2026-05-19 | 1 |
| 2026-05-20 | 0 |

**Outlier sessions**

| Session | Repo | Tokens | Spend | What it produced |
|---------|------|-------:|------:|------------------|
| Teach driftwood to report a line number | `jrivera/driftwood` | 9.7M | $5.83 | (no recorded output) |
| Split the driftwood parser module | `jrivera/driftwood` | 9.0M | $5.27 | (no recorded output) |
| Teach driftwood to report a line number | `jrivera/driftwood` | 8.5M | $4.83 | (no recorded output) |
| ef35bcda | `jrivera/driftwood` | 7.6M | $4.56 | (no recorded output) |
| Teach driftwood to report a line number | `jrivera/driftwood` | 8.0M | $4.49 | (no recorded output) |
| eec33f5b | `jrivera/driftwood` | 7.3M | $4.30 | (no recorded output) |
| Teach driftwood to report a line number | `jrivera/driftwood` | 7.8M | $4.26 | (no recorded output) |
| 9587c1f7 | `jrivera/driftwood` | 7.6M | $4.15 | (no recorded output) |
| Split the driftwood parser module | `jrivera/driftwood` | 7.5M | $4.04 | (no recorded output) |
| Teach driftwood to report a line number | `jrivera/driftwood` | 7.5M | $4.04 | (no recorded output) |

## Conclusion

