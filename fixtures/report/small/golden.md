---
title: "Claude Usage Report - Jordan Rivera - 2026-03-02 to 2026-03-08"
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
**Period:** 2026-03-02 - 2026-03-08
**Total Spend:** $64.86
**Pricing Basis:** Total spend is modeled Claude Code catalog spend at published list rates; account-level billed spend comes from Claude Enterprise Analytics.
**Sessions:** 9 across 1 repository
**Active Days:** 6 of 7

> window is session-level (M2): whole sessions whose catalog `modified` falls in [since, until]; not per-record like pre-v2 reports, so a boundary-straddling session's numbers can differ from a v1 report.

---

## Executive Summary

## What the Money Produced

Every session falls in exactly one band, and the first rule that matches wins. Names say what was observed in the transcript, not what the session achieved.

| Band | Sessions | Spend | Share of Spend |
|------|---------:|------:|---------------:|
| `committed` | 9 | $64.86 | 100% |
| `edited` | 0 | $0.00 | 0% |
| `wrote-records` | 0 | $0.00 | 0% |
| `no-observed-outcome` | 0 | $0.00 | 0% |
| **Total** | 9 | $64.86 | |

- `committed`: at least one commit or opened pull request
- `edited`: files edited, no commit or pull request
- `wrote-records`: a Confluence, Jira or Slack write, no file edit
- `no-observed-outcome`: none of the above

`no-observed-outcome` is not "no result": a sample shows it is mostly `claude -p` hook sessions that answered a question, and files written through Bash heredocs or commits made by subprocesses are invisible to `outcome::extract`.

## Quantified Output

These are observed tool invocations extracted from session transcripts, not estimates.

| Metric | Count |
|--------|------:|
| Sessions producing commits | 9 |
| Commits | 26 |
| Pull requests opened | 12 |
| Confluence pages written or updated | 3 |
| Jira issues written or updated | 3 |
| Slack messages sent | 8 |
| Files edited | 108 |
| Lines of file content written | 8,455 |
| Lines of file content replaced | 1,717 |

| Repo | Spend | Commits | PRs Opened | Files Edited |
|------|------:|--------:|-----------:|-------------:|
| `northwind-media/beacon` | $64.86 | 26 | 12 | 108 |

**Opened pull requests**

- `northwind-media/beacon`: [#100](https://github.com/northwind-media/beacon/pull/100), [#103](https://github.com/northwind-media/beacon/pull/103), [#104](https://github.com/northwind-media/beacon/pull/104), [#106](https://github.com/northwind-media/beacon/pull/106), [#107](https://github.com/northwind-media/beacon/pull/107), [#109](https://github.com/northwind-media/beacon/pull/109), [#110](https://github.com/northwind-media/beacon/pull/110), [#115](https://github.com/northwind-media/beacon/pull/115), [#118](https://github.com/northwind-media/beacon/pull/118), [#121](https://github.com/northwind-media/beacon/pull/121), [#122](https://github.com/northwind-media/beacon/pull/122), [#124](https://github.com/northwind-media/beacon/pull/124)

A ratio of $2.49 of period spend per observed commit.
A ratio of $5.41 of period spend per pull request opened.
A ratio of $10.81 of period spend per active day.
A ratio of $7.21 of period spend per session (the mean).
Median session spend was $6.39; the 90th percentile was $11.59.

## Cost Summary

| Model | Sessions Using | Total Tokens | Spend |
|-------|---------------:|-------------:|------:|
| `claude-sonnet-4-6` | 6 | 55.4M | $34.57 |
| `claude-opus-4-7` | 3 | 30.0M | $30.29 |
| **Total** | 9 | 85.4M | $64.86 |

## Reconciliation

No Claude Enterprise Analytics cost export was supplied for this render (--reconcile <file>); the total spend above is a modeled figure only and has not been reconciled against this operator's authoritative billed spend.

## Agent-Type Cost Attribution

Every dollar of the period's $64.86 spend lands in exactly one row below; `(main-session)` carried the most. The `(main-session)` row is work a session did itself rather than delegating.

| Agent Type | Tokens | Spend |
|------------|-------:|------:|
| `(main-session)` | 85.4M | $64.86 |

## The Efficiency Story

- Cache read share was 92.3%: most of the context read each turn is re-read from cache at a fraction of the fresh-input rate, which is what makes sustained agentic sessions economical. Fresh input was 1.0M against 78.5M read from cache.
- At full list-price input rates the same tokens would model to $320.70, so cache reuse accounts for $255.84 (computed from published per-token rates).
- Tool error rate: 6.0%.
- Share of cache writes paying the 1h premium: 6.7%.
- Interrupts observed: 4.
- Context compactions observed: 1.

| Skill | Calls | Sessions | Tokens | Spend | $/call |
|---|---:|---:|---:|---:|---:|
| `release-notes` | 3 | 1 | 1.1M | $2.09 | $0.70 |
| `schema-review` | 2 | 1 | 1.1M | $1.97 | $0.99 |

Coverage: $4.06 of $64.86 (6.3%), embedded-price basis

| Server | MCP Tool | Calls | Sessions | Time | Avg | Tokens | Spend | $/call |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| `_unknown` | `wiki-write` | 10 | 3 | 7.6s | 0.8s | 3.9M | $7.24 | $0.72 |

Coverage: $7.24 of $64.86 (11.2%), embedded-price basis

Calls counted on 7 of 9 sessions

## Unobserved and Flagged Spend

Spend worth a second look. These rows overlap and are not a partition of the total.

- Sessions in `no-observed-outcome`: 0, spending $0.00 (0% of session spend).
- Heavy model on a short session: 0 sessions, $0.00. Heavy means a model name containing `opus`, `fable`; short means at most 3 turns.

What `no-observed-outcome` does and does not mean is noted under the band table above.

No session carries an efficiency flag.

## What This Funded

### northwind-media

9 sessions across 1 repository, 85.4M tokens, $64.86.

- `northwind-media/beacon` (9 sessions, 85.4M tokens, $64.86 spend): 26 commits, 12 PRs opened, 108 files edited

## Usage Profile

Daily figures are whole UTC days: each session's spend sits on the days its turns ran, not the day it began. The daily series accounts for all $64.86 of the period's spend.

**Daily spend**

| Day | Spend |
|-----|------:|
| 2026-03-02 | $18.70 |
| 2026-03-03 | $12.67 |
| 2026-03-04 | $9.77 |
| 2026-03-05 | $11.59 |
| 2026-03-06 | $7.83 |
| 2026-03-07 | $4.30 |
| 2026-03-08 | $0.00 |

**Daily active sessions**

| Day | Sessions |
|-----|------:|
| 2026-03-02 | 2 |
| 2026-03-03 | 2 |
| 2026-03-04 | 2 |
| 2026-03-05 | 1 |
| 2026-03-06 | 1 |
| 2026-03-07 | 1 |
| 2026-03-08 | 0 |

**Outlier sessions**

| Session | Repo | Tokens | Spend | What it produced |
|---------|------|-------:|------:|------------------|
| Add a dead-letter queue to the ingest path | `northwind-media/beacon` | 11.1M | $11.59 | 4 commits, 2 PRs opened, 3 files edited |
| Wire the ingest retry backoff | `northwind-media/beacon` | 11.0M | $11.41 | 4 commits, 1 PR opened, 15 files edited |
| 3e0cfd4a | `northwind-media/beacon` | 11.2M | $7.83 | 3 commits, 6 files edited |
| ce6093cc | `northwind-media/beacon` | 7.9M | $7.29 | 2 commits, 1 PR opened, 12 files edited |
| Trace a cold start in the ingest worker | `northwind-media/beacon` | 9.6M | $6.39 | 4 commits, 2 PRs opened, 19 files edited |
| Trace a cold start in the ingest worker | `northwind-media/beacon` | 10.3M | $6.28 | 2 commits, 2 PRs opened, 14 files edited |
| f33eb686 | `northwind-media/beacon` | 9.2M | $5.39 | 3 commits, 1 PR opened, 6 files edited |
| Audit the beacon retention policy | `northwind-media/beacon` | 7.9M | $4.38 | 3 commits, 2 PRs opened, 17 files edited |
| Backfill the missing ingest metric labels | `northwind-media/beacon` | 7.3M | $4.30 | 1 commit, 1 PR opened, 16 files edited |

## Conclusion

