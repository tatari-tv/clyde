---
title: "Claude Usage Report - Jordan Rivera - 2026-04-01 to 2026-04-30"
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
**Period:** 2026-04-01 - 2026-04-30
**Total Spend:** $671.26
**Pricing Basis:** Total spend is modeled Claude Code catalog spend at published list rates; account-level billed spend comes from Claude Enterprise Analytics.
**Sessions:** 44 across 7 repositories
**Active Days:** 26 of 30

> window is session-level (M2): whole sessions whose catalog `modified` falls in [since, until]; not per-record like pre-v2 reports, so a boundary-straddling session's numbers can differ from a v1 report.

---

## Executive Summary

## What the Money Produced

Every session falls in exactly one band, and the first rule that matches wins. Names say what was observed in the transcript, not what the session achieved.

| Band | Sessions | Spend | Share of Spend |
|------|---------:|------:|---------------:|
| `committed` | 22 | $409.66 | 61% |
| `edited` | 8 | $92.23 | 14% |
| `wrote-records` | 5 | $40.42 | 6% |
| `no-observed-outcome` | 9 | $128.95 | 19% |
| **Total** | 44 | $671.26 | |

- `committed`: at least one commit or opened pull request
- `edited`: files edited, no commit or pull request
- `wrote-records`: a Confluence, Jira or Slack write, no file edit
- `no-observed-outcome`: none of the above

`no-observed-outcome` is not "no result": a sample shows it is mostly `claude -p` hook sessions that answered a question, and files written through Bash heredocs or commits made by subprocesses are invisible to `outcome::extract`.

## Quantified Output

These are observed tool invocations extracted from session transcripts, not estimates.

| Metric | Count |
|--------|------:|
| Sessions producing commits | 22 |
| Commits | 56 |
| Pull requests opened | 20 |
| Confluence pages written or updated | 11 |
| Jira issues written or updated | 14 |
| Slack messages sent | 20 |
| Files edited | 374 |
| Lines of file content written | 28,321 |
| Lines of file content replaced | 7,500 |

| Repo | Spend | Commits | PRs Opened | Files Edited |
|------|------:|--------:|-----------:|-------------:|
| `openpipe-oss/quill` | $264.78 | 20 | 9 | 146 |
| `northwind-media/beacon` | $165.84 | 12 | 2 | 49 |
| `northwind-media/tideline` | $83.29 | 8 | 4 | 56 |
| `northwind-media/halyard` | $58.42 | 7 | 4 | 71 |
| `jrivera/driftwood` | $49.28 | 3 | 0 | 30 |
| `northwind-media/almanac` | $45.49 | 6 | 1 | 22 |

**Opened pull requests**

- `openpipe-oss/quill`: [#112](https://github.com/openpipe-oss/quill/pull/112), [#113](https://github.com/openpipe-oss/quill/pull/113), [#136](https://github.com/openpipe-oss/quill/pull/136), [#137](https://github.com/openpipe-oss/quill/pull/137), [#169](https://github.com/openpipe-oss/quill/pull/169), [#170](https://github.com/openpipe-oss/quill/pull/170), [#217](https://github.com/openpipe-oss/quill/pull/217), [#218](https://github.com/openpipe-oss/quill/pull/218), [#220](https://github.com/openpipe-oss/quill/pull/220)
- `northwind-media/halyard`: [#124](https://github.com/northwind-media/halyard/pull/124), [#157](https://github.com/northwind-media/halyard/pull/157), [#158](https://github.com/northwind-media/halyard/pull/158), [#178](https://github.com/northwind-media/halyard/pull/178)
- `northwind-media/tideline`: [#166](https://github.com/northwind-media/tideline/pull/166), [#167](https://github.com/northwind-media/tideline/pull/167), [#190](https://github.com/northwind-media/tideline/pull/190), [#205](https://github.com/northwind-media/tideline/pull/205)
- `northwind-media/beacon`: [#118](https://github.com/northwind-media/beacon/pull/118), [#119](https://github.com/northwind-media/beacon/pull/119)
- `northwind-media/almanac`: [#226](https://github.com/northwind-media/almanac/pull/226)

A ratio of $11.99 of period spend per observed commit.
A ratio of $33.56 of period spend per pull request opened.
A ratio of $25.82 of period spend per active day.
A ratio of $15.26 of period spend per session (the mean).
Median session spend was $12.71; the 90th percentile was $28.43.

## Cost Summary

| Model | Sessions Using | Total Tokens | Spend |
|-------|---------------:|-------------:|------:|
| `claude-opus-4-7` | 18 | 346.4M | $386.51 |
| `claude-sonnet-4-6` | 26 | 403.7M | $243.46 |
| `claude-haiku-4-5` | 18 | 211.8M | $41.29 |
| **Total** | 44 | 961.9M | $671.26 |

## Reconciliation

This render's modeled total was reconciled against the Claude Enterprise Analytics cost export for jordan.rivera@northwind-media.example over this exact window; see the Reconciliation section for the billed figure and the scope note.

| Figure | Amount |
|--------|-------:|
| Billed | $910.76 |
| Modeled | $671.26 |

Unseen account spend is +$239.50, from anthropic enterprise analytics for jordan.rivera@northwind-media.example over 2026-04-01 to 2026-04-30.

This billed figure is the Claude Enterprise Analytics cost report for jordan.rivera@northwind-media.example alone, covering everything that account was billed across every Claude product: claude.ai web, Cowork, other clients, and other hosts. clyde report covers only the Claude Code sessions in this catalog on this machine. Billed spend meeting or exceeding modeled spend is the expected relationship here; a positive unseen-account-spend figure is the same person's usage that clyde cannot see, never that clyde miscounted.

| Model | Billed | Modeled | Unseen Account Spend |
|-------|-------:|--------:|---------------------:|
| `claude-opus-4-7` | $534.58 | $386.51 | +$148.07 |
| `claude-sonnet-4-6` | $229.10 | $243.46 | -$14.36 |
| `claude-opus-4-8` | $73.90 | $0.00 | +$73.90 |
| `claude-haiku-4-5` | $73.18 | $41.29 | +$31.89 |

## Agent-Type Cost Attribution

Every dollar of the period's $671.26 spend lands in exactly one row below; `(main-session)` carried the most. The `(main-session)` row is work a session did itself rather than delegating.

| Agent Type | Calls | Sessions | Tokens | Spend | $/call |
|------------|------:|---------:|-------:|------:|-------:|
| `(main-session)` | n/a | 44 | 639.6M | $479.75 | n/a |
| `phase-implementer` | 11 | 11 | 133.4M | $79.31 | $7.21 |
| `doc-writer` | 9 | 9 | 103.5M | $61.78 | $6.86 |
| `code-reviewer` | 7 | 7 | 85.4M | $50.42 | $7.20 |

## The Efficiency Story

- Cache read share was 92.2%: most of the context read each turn is re-read from cache at a fraction of the fresh-input rate, which is what makes sustained agentic sessions economical. Fresh input was 11.6M against 883.3M read from cache.
- At full list-price input rates the same tokens would model to $3,201.81, so cache reuse accounts for $2,530.55 (computed from published per-token rates).
- Tool error rate: 4.2%.
- Share of cache writes paying the 1h premium: 7.4%.
- Interrupts observed: 29.
- Context compactions observed: 6.

| Skill | Calls | Sessions | Tokens | Spend | $/call |
|---|---:|---:|---:|---:|---:|
| `schema-review` | 15† | 9 | 24.1M | $44.59 | $1.54 |
| `release-notes` | 22 | 7 | 14.8M | $27.44 | $1.25 |

Coverage: $72.03 of $671.26 (10.7%), embedded-price basis

| Server | MCP Tool | Calls | Sessions | Time | Avg | Tokens | Spend | $/call |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| `tracker` | `search` | 22† | 11 | 14.1s | 0.9s | 21.9M | $40.51 | $1.32 |
| `_unknown` | `wiki-write` | 18 | 6 | 15.3s | 0.9s | 9.6M | $17.83 | $0.99 |

Coverage: $58.34 of $671.26 (8.7%), embedded-price basis

† includes sessions recorded before call counting

Calls counted on 33 of 44 sessions

## Unobserved and Flagged Spend

Spend worth a second look. These rows overlap and are not a partition of the total.

- Sessions in `no-observed-outcome`: 9, spending $128.95 (19% of session spend).
- Heavy model on a short session: 2 sessions, $50.89. Heavy means a model name containing `opus`, `fable`; short means at most 3 turns.

What `no-observed-outcome` does and does not mean is noted under the band table above.

| Efficiency Flag | Sessions | Spend |
|-----------------|---------:|------:|
| Low cache-read share | 4 | $141.20 |
| High tool-error rate | 6 | $126.91 |

## What This Funded

### northwind-media

20 sessions across 4 repositories, 504.0M tokens, $353.04.

- `northwind-media/beacon` (7 sessions, 208.9M tokens, $165.84 spend): 12 commits, 2 PRs opened, 49 files edited
- `northwind-media/tideline` (4 sessions, 109.3M tokens, $83.29 spend): 8 commits, 4 PRs opened, 56 files edited
- `northwind-media/halyard` (6 sessions, 132.7M tokens, $58.42 spend): 7 commits, 4 PRs opened, 71 files edited
- `northwind-media/almanac` (3 sessions, 53.1M tokens, $45.49 spend): 6 commits, 1 PR opened, 22 files edited

### openpipe-oss

17 sessions across 1 repository, 370.0M tokens, $264.78.

- `openpipe-oss/quill` (17 sessions, 370.0M tokens, $264.78 spend): 20 commits, 9 PRs opened, 146 files edited

### jrivera

7 sessions across 2 repositories, 87.9M tokens, $53.44.

- `jrivera/driftwood` (5 sessions, 65.6M tokens, $49.28 spend): 3 commits, 30 files edited
- `jrivera/sextant` (2 sessions, 22.3M tokens, $4.16 spend)

## Usage Profile

Daily figures are whole UTC days: each session's spend sits on the days its turns ran, not the day it began. The daily series accounts for all $671.26 of the period's spend.

**Daily spend**

| Day | Spend |
|-----|------:|
| 2026-04-01 | $11.80 |
| 2026-04-02 | $34.25 |
| 2026-04-03 | $6.08 |
| 2026-04-04 | $50.94 |
| 2026-04-05 | $15.37 |
| 2026-04-06 | $42.73 |
| 2026-04-07 | $17.51 |
| 2026-04-08 | $10.69 |
| 2026-04-09 | $17.04 |
| 2026-04-10 | $0.00 |
| 2026-04-11 | $20.98 |
| 2026-04-12 | $15.94 |
| 2026-04-13 | $0.00 |
| 2026-04-14 | $4.07 |
| 2026-04-15 | $1.26 |
| 2026-04-16 | $32.27 |
| 2026-04-17 | $44.28 |
| 2026-04-18 | $44.68 |
| 2026-04-19 | $12.12 |
| 2026-04-20 | $15.90 |
| 2026-04-21 | $26.74 |
| 2026-04-22 | $19.99 |
| 2026-04-23 | $78.84 |
| 2026-04-24 | $12.71 |
| 2026-04-25 | $0.00 |
| 2026-04-26 | $34.54 |
| 2026-04-27 | $20.55 |
| 2026-04-28 | $32.13 |
| 2026-04-29 | $47.85 |
| 2026-04-30 | $0.00 |

**Daily active sessions**

| Day | Sessions |
|-----|------:|
| 2026-04-01 | 2 |
| 2026-04-02 | 4 |
| 2026-04-03 | 1 |
| 2026-04-04 | 2 |
| 2026-04-05 | 1 |
| 2026-04-06 | 2 |
| 2026-04-07 | 2 |
| 2026-04-08 | 2 |
| 2026-04-09 | 2 |
| 2026-04-10 | 0 |
| 2026-04-11 | 1 |
| 2026-04-12 | 1 |
| 2026-04-13 | 0 |
| 2026-04-14 | 2 |
| 2026-04-15 | 1 |
| 2026-04-16 | 1 |
| 2026-04-17 | 3 |
| 2026-04-18 | 3 |
| 2026-04-19 | 1 |
| 2026-04-20 | 2 |
| 2026-04-21 | 1 |
| 2026-04-22 | 2 |
| 2026-04-23 | 3 |
| 2026-04-24 | 1 |
| 2026-04-25 | 0 |
| 2026-04-26 | 4 |
| 2026-04-27 | 2 |
| 2026-04-28 | 3 |
| 2026-04-29 | 2 |
| 2026-04-30 | 0 |

**Outlier sessions**

| Session | Repo | Tokens | Spend | What it produced |
|---------|------|-------:|------:|------------------|
| Wire the ingest retry backoff | `northwind-media/beacon` | 83.1M | $72.45 | 4 commits, 5 files edited |
| Document the quill plugin hooks | `openpipe-oss/quill` | 43.8M | $41.12 | 4 commits, 2 PRs opened, 20 files edited |
| 3aed06b6 | `openpipe-oss/quill` | 35.3M | $34.04 | 3 commits, 9 files edited |
| 54f9d4bb | `northwind-media/beacon` | 29.4M | $32.27 | 1 file edited |
| Document the quill plugin hooks | `openpipe-oss/quill` | 27.9M | $28.43 | (no recorded output) |
| 18c8897c | `northwind-media/tideline` | 24.9M | $26.74 | 4 commits, 1 PR opened, 16 files edited |
| Add a rollback trigger to halyard | `northwind-media/halyard` | 66.6M | $26.70 | 3 commits, 1 PR opened, 7 files edited |
| Harden the quill release script | `openpipe-oss/quill` | 27.4M | $26.36 | (no recorded output) |
| Add a dead-letter queue to the ingest path | `northwind-media/beacon` | 25.3M | $25.96 | 12 files edited |
| Repair the nightly almanac backfill | `northwind-media/almanac` | 23.7M | $24.98 | 2 commits, 1 PR opened, 5 files edited |

## Month over Month

| Figure | 2026-03-02 to 2026-03-31 | 2026-04-01 to 2026-04-30 |
|--------|---:|---:|
| Spend | $523.14 | $671.26 |
| Sessions | 31 | 44 |
| Total tokens | 765.8M | 961.9M |

## Conclusion

