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
**Total Spend:** $64.85
**Pricing Basis:** Total spend is modeled Claude Code catalog spend at published list rates; account-level billed spend comes from Claude Enterprise Analytics.
**Sessions:** 9 across 1 repository
**Active Days:** 6 of 7

> window is session-level (M2): whole sessions whose catalog `modified` falls in [since, until]; not per-record like pre-v2 reports, so a boundary-straddling session's numbers can differ from a v1 report.

---

## Executive Summary

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
| `northwind-media/beacon` | $64.85 | 26 | 12 | 108 |

A ratio of $2.49 of period spend per observed commit.
A ratio of $5.40 of period spend per pull request opened.
A ratio of $10.81 of period spend per active day.
A ratio of $7.21 of period spend per session (the mean).
Median session spend was $6.39; the 90th percentile was $11.59.

## Cost Summary

| Model | Sessions Using | Total Tokens | Spend |
|-------|---------------:|-------------:|------:|
| `claude-sonnet-4-6` | 6 | 55.4M | $34.56 |
| `claude-opus-4-7` | 3 | 30.0M | $30.29 |
| **Total** | 9 | 85.4M | $64.85 |

## Reconciliation

No Claude Enterprise Analytics cost export was supplied for this render (--reconcile <file>); the total spend above is a modeled figure only and has not been reconciled against this operator's authoritative billed spend.

## Agent-Type Cost Attribution

Every dollar of the period's $64.85 spend lands in exactly one row below; `(main-session)` carried the most. The `(main-session)` row is work a session did itself rather than delegating.

| Agent Type | Tokens | Spend |
|------------|-------:|------:|
| `(main-session)` | 85.4M | $64.85 |

## The Efficiency Story

- Cache read share was 92.3%: most of the context read each turn is re-read from cache at a fraction of the fresh-input rate, which is what makes sustained agentic sessions economical. Fresh input was 1.0M against 78.5M read from cache.
- At full list-price input rates the same tokens would model to $320.69, so cache reuse accounts for $255.84 (computed from published per-token rates).
- Tool error rate: 6.0%.
- Share of cache writes paying the 1h premium: 6.7%.
- Interrupts observed: 4.
- Context compactions observed: 1.

| Skill | Tokens | Spend |
|---|-------:|------:|
| `release-notes` | 1.1M | $2.09 |
| `schema-review` | 1.1M | $1.97 |

Coverage: $4.06 of $64.85 (6.3%), embedded-price basis

| MCP Tool | Tokens | Spend |
|---|-------:|------:|
| `wiki-write` | 3.9M | $7.24 |

Coverage: $7.24 of $64.85 (11.2%), embedded-price basis

## What This Funded

### northwind-media

9 sessions across 1 repository, 85.4M tokens, $64.85.

- `northwind-media/beacon` (9 sessions, 85.4M tokens, $64.85 spend): 26 commits, 12 PRs opened, 108 files edited

## Usage Profile

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

**Daily sessions**

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

