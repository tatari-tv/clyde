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
**Total Spend:** $671.28
**Pricing Basis:** Total spend is modeled Claude Code catalog spend at published list rates; account-level billed spend comes from Claude Enterprise Analytics.
**Sessions:** 44 across 7 repositories
**Active Days:** 25 of 30

---

## Executive Summary

Work ran on 25 of the 30 days in the window and was spread broadly rather than concentrated in a few spikes; the heaviest single day was 2026-04-23 at $77.16, with a secondary run of activity late in the window (2026-04-26 through 2026-04-29 carried $50.05, $15.12, $41.47, and $28.43). The four inactive days were 2026-04-10, 2026-04-13, 2026-04-15, and 2026-04-25, plus a closing gap on 2026-04-30. `openpipe-oss/quill` carried the most work by session count (17 sessions, $264.79), followed by `northwind-media/beacon` ($165.85); across the period the sessions produced 93 commits, 32 pull requests opened, and 442 files edited. 30 of 44 sessions in the window (68.2%) carry an enrich summary; the rest are cited by title only, so part of the narrative below rests on titles rather than summaries.

## Quantified Output

The counts below are observed tool invocations extracted from session transcripts, not estimates.

| Metric | Count |
|---|---|
| Sessions producing commits | 35 |
| Commits | 93 |
| Pull requests opened | 32 |
| Confluence pages written or updated | 8 |
| Jira tickets written | 11 |
| Slack messages | 20 |
| Files edited | 442 |
| Lines of file content written | 34331 |
| Lines of file content replaced | 9402 |

Spend against output, per repo, in the order given:

| Repo | Spend | Commits | PRs Opened | Files Edited |
|---|---|---|---|---|
| openpipe-oss/quill | $264.79 | 30 | 16 | 161 |
| northwind-media/beacon | $165.85 | 24 | 3 | 67 |
| northwind-media/tideline | $83.29 | 8 | 4 | 56 |
| northwind-media/halyard | $58.42 | 15 | 5 | 71 |
| jrivera/driftwood | $49.28 | 6 | 2 | 55 |
| northwind-media/almanac | $45.49 | 6 | 1 | 22 |
| jrivera/sextant | $4.16 | 4 | 1 | 10 |

`openpipe-oss/quill` carried the most spend at $264.79 and produced 30 commits and 16 PRs opened.

The period spent $671.28 and produced 93 commits, a ratio of $7.22 per commit; against 32 PRs opened, a ratio of $20.98 per PR. Across 44 sessions the ratio is $15.26 per session, while `session-spend-p50` is $12.71 and `session-spend-p90` is $28.43; the per-session ratio sitting above the median indicates a few larger sessions carried the period.

## Cost Summary

| Model | Sessions Using | Total Tokens | Spend |
|---|---|---|---|
| claude-opus-4-7 | 18 | 346.4M | $386.51 |
| claude-sonnet-4-6 | 26 | 403.7M | $243.46 |
| claude-haiku-4-5 | 18 | 211.8M | $41.31 |
| **Total** | 44 | 961.9M | $671.28 |

## Reconciliation

This render's modeled total was reconciled against the Claude Enterprise Analytics cost export for jordan.rivera@northwind-media.example over this exact window; see the Reconciliation section for the billed figure and the scope note.

| Figure | Amount |
|---|---|
| Billed | $910.78 |
| Modeled | $671.28 |

The unseen account spend is +$239.50, from the anthropic enterprise analytics cost report for jordan.rivera@northwind-media.example over the window 2026-04-01 to 2026-04-30.

This billed figure is the Claude Enterprise Analytics cost report for jordan.rivera@northwind-media.example alone, covering everything that account was billed across every Claude product: claude.ai web, Cowork, other clients, and other hosts. clyde report covers only the Claude Code sessions in this catalog on this machine. Billed spend meeting or exceeding modeled spend is the expected relationship here; a positive unseen-account-spend figure is the same person's usage that clyde cannot see, never that clyde miscounted.

| Model | Billed | Modeled | Unseen Account Spend |
|---|---|---|---|
| claude-opus-4-7 | $534.59 | $386.51 | +$148.08 |
| claude-sonnet-4-6 | $229.11 | $243.46 | -$14.35 |
| claude-opus-4-8 | $73.90 | $0.00 | +$73.90 |
| claude-haiku-4-5 | $73.18 | $41.31 | +$31.87 |

## Agent-Type Cost Attribution

The `(main-session)` row carried the most spend at $479.74; the table below is a true partition of the period spend, accounting for the whole $671.28.

| Agent Type | Tokens | Spend |
|---|---|---|
| (main-session) | 639.6M | $479.74 |
| phase-implementer | 133.4M | $79.33 |
| doc-writer | 103.5M | $61.78 |
| code-reviewer | 85.4M | $50.43 |

## The Efficiency Story

Cache reads accounted for 92.2% of the context read across the period, meaning most of the context the model reads each turn is re-read from cache at a fraction of the fresh-input rate, which is what makes sustained agentic sessions economical. Against 11.6M fresh input tokens the cache supplied 883.3M read tokens. At published list rates the same token volume without caching would have carried a list-price equivalent of $3,201.82, a cache savings of $2,530.54 (computed from published per-token rates).

Other signals: the tool-error rate was 4.2% of tool calls; 7.4% of cache writes paid the 1h premium; the user interrupted 29 times; the context was compacted 6 times.

Skill attribution (tags, not a partition):

| Skill | Tokens | Spend |
|---|---|---|
| schema-review | 24.1M | $44.59 |
| release-notes | 14.8M | $27.44 |

These skill tags cover $72.03 of $671.28 (10.7%), embedded-price basis.

MCP-tool attribution (tags, not a partition):

| MCP Tool | Tokens | Spend |
|---|---|---|
| tracker-search | 21.9M | $40.51 |
| wiki-write | 9.6M | $17.83 |

These MCP tags cover $58.34 of $671.28 (8.7%), embedded-price basis.

## What This Funded

### Northwind Media (employer org)

Four repos in `northwind-media` drew 20 sessions and $353.05, concentrated on ingest reliability, a staged rollout, and dashboard work.

**Ingest reliability on beacon.** `northwind-media/beacon` (7 sessions, 208.9M tokens, $165.85 spend) was the busiest employer repo. Session `44d4dfde` measured what the slowest healthy boot needs and cut the startup probe timeout to it so a restarting pod stops being marked ready before its cache is warm; `4b1d68f6` traced a cold start in the ingest worker to a synchronous hostname lookup on the request path and moved it behind a warmed cache with a regression test; `14fabbe1` added a dead-letter queue so a record the parser rejects is parked with its failure reason instead of being retried forever (PR #211); and `54f9d4bb` audited the retention policy against what the storage layer actually deletes and wired two unswept buckets into the scheduled job. The repo landed 24 commits and 3 PRs opened.

**Staged rollout on halyard.** `northwind-media/halyard` (6 sessions, 132.7M tokens, $58.42 spend) built out a cutover plan and its safety machinery: sessions `07dce623` and `dadb0a1e` drafted the rollout plan (the staged traffic shift, the rollback trigger, and the two dashboards that gate each stage), and `2ad5c741`, `4e15c3a5`, and `88ac1250` added the rollback trigger the plan called for, wiring it to the same health check the staged shift reads and rehearsing it against the staging fleet. The repo landed 15 commits and 5 PRs opened.

**Dashboard and query work on tideline.** `northwind-media/tideline` (4 sessions, 109.3M tokens, $83.29 spend): session `18c8897c` ported the dashboard to the new theme tokens and verified contrast on the dense table view, and `f01b33b8` profiled the slow dashboard query, found a missing composite index behind a filter the UI always sends, and recorded before-and-after latency in the ticket (PR #205). The repo landed 8 commits and 4 PRs opened.

**Backfill and API work on almanac.** `northwind-media/almanac` (3 sessions, 53.1M tokens, $45.49 spend): session `f9e95abd` repaired the nightly backfill so it resumes from the last completed partition instead of restarting the whole range when a single shard times out (PR #226), and `519daf85` added cursor pagination to the list endpoint while keeping the old offset parameter working for existing callers. The repo landed 6 commits and 1 PR opened.

### openpipe-oss

`openpipe-oss/quill` (17 sessions, 370.0M tokens, $264.79 spend) is an open-source project and the single busiest repo of the period. Work clustered on two themes: hardening the release script (session `05981637` made it refuse to publish on a dirty working tree, an existing tag, or a missing changelog entry; `e3d9d683` and `93c4b9e8` continued that hardening) and documenting the plugin hooks against the code rather than the wiki, dropping two hooks the loader no longer calls (sessions `6bb58fd4`, `3aed06b6`, `2dc73697`, `6dadddb2`, `f146c6ed`). The repo landed 30 commits and 16 PRs opened.

### jrivera (personal org)

Two repos in the user's personal org drew 7 sessions and $53.44; both are developer tooling.

- `jrivera/driftwood` (5 sessions, 65.6M tokens, $49.28 spend): a parser library. Sessions `86b0fd93`, `89cf1386`, `eaffd2a6`, and `e4ed747c` taught the parser to carry a line and column through to its error type so a malformed document is reported where it broke, and `93071145` split the parser into a tokenizer and a shape builder so the error path can name the offending construct. The repo landed 6 commits and 2 PRs opened.
- `jrivera/sextant` (2 sessions, 22.3M tokens, $4.16 spend): sessions `4b1d68f6`-adjacent test tooling; `b1cb8b81` and `4bf28f14` rewrote the snapshot test harness to compare fixtures structurally rather than by rendered string, removing an ordering flake that had been retried rather than fixed. The repo landed 4 commits and 1 PR opened.

## Usage Profile

**Temporal distribution:** Activity was spread across the window rather than clustered, with 25 active days out of 30. The heaviest spend day was 2026-04-23 at $77.16, and a late run of days 2026-04-26 through 2026-04-29 carried $50.05, $15.12, $41.47, and $28.43. The inactive days were 2026-04-10, 2026-04-13, 2026-04-15, 2026-04-25, and the closing 2026-04-30; no run of inactive days exceeded a single date. Carried-in spend was $0.00 across 0 sessions, so the by-day series covers the whole window.

**Model mix:**
- `claude-opus-4-7` appeared across quill documentation, beacon retention work (`54f9d4bb`), tideline query profiling (`f01b33b8`), and driftwood parser refactors.
- `claude-sonnet-4-6` was the most widely used model by session count, appearing alongside Opus and Haiku on ingest, release-script, and rollout work.
- `claude-haiku-4-5` appeared on lighter-weight sessions: rollout-plan drafts (`07dce623`, `dadb0a1e`), metric backfills (`213c5f24`), and the sextant test harness.

**Outlier sessions:**

| Session | Repo | Tokens | Spend | What it produced |
|---|---|---|---|---|
| Wire the ingest retry backoff | northwind-media/beacon | 83.1M | $72.45 | 4 commits, 1 Confluence write, 3 Slack messages, 5 files edited |
| Document the quill plugin hooks | openpipe-oss/quill | 43.8M | $41.12 | 4 commits, PRs #136 and #137, 20 files edited |
| 3aed06b6 | openpipe-oss/quill | 35.3M | $34.04 | Documented plugin hooks against code, dropped two unused hooks; 3 commits, 9 files edited |
| 54f9d4bb | northwind-media/beacon | 29.4M | $32.27 | Audited retention policy, wired two unswept buckets into the scheduled job; 3 commits |
| Document the quill plugin hooks | openpipe-oss/quill | 27.9M | $28.43 | Documented plugin hooks against code rather than the wiki |
| 18c8897c | northwind-media/tideline | 24.9M | $26.74 | Ported dashboard to new theme tokens, verified contrast; 4 commits, PR #190 |
| Add a rollback trigger to halyard | northwind-media/halyard | 66.6M | $26.70 | Added rollback trigger wired to the staged-shift health check; 3 commits, PR #178 |
| Harden the quill release script | openpipe-oss/quill | 27.4M | $26.36 | Hardened the release script against dirty tree, existing tag, missing changelog entry |
| Add a dead-letter queue to the ingest path | northwind-media/beacon | 25.3M | $25.96 | Added dead-letter queue for rejected records; 3 commits, PR #211 |
| Repair the nightly almanac backfill | northwind-media/almanac | 23.7M | $24.98 | Repaired backfill to resume from last completed partition; 2 commits, PR #226 |

## Month over Month

- The prior period (2026-03-02 to 2026-03-31, 30 days) and this period (2026-04-01 to 2026-04-30, 30 days) are the same length. Spend was $523.17 across 31 sessions in the prior period against $671.28 across 44 sessions this period.
- Both periods touched 7 repos. `openpipe-oss/quill` moved to the top by spend ($106.62 prior, $264.79 this period), and its session count rose from 9 to 17.
- `northwind-media/beacon` rose from $75.46 to $165.85; `northwind-media/halyard` rose from $42.84 to $58.42. `northwind-media/tideline` was $84.13 prior and $83.29 this period.
- Model mix: `claude-opus-4-7` was $322.31 prior and $386.51 this period; `claude-sonnet-4-6` was $161.96 prior and $243.46 this period; `claude-haiku-4-5` was $38.90 prior and $41.31 this period.

## Forward-Looking

- The halyard cutover is past its rollout-plan and rollback-trigger stages: session `2ad5c741` (2026-04-28) added the rollback trigger the plan called for and rehearsed it against the staging fleet, leaving the staged traffic shift as the remaining gated work.
- The quill plugin-hook documentation is being maintained against the code rather than the wiki, with the most recent session `45f45aca` on 2026-04-29 continuing that reference cleanup.
- The tideline dashboard query work has an index in place and before-and-after latency recorded in the ticket (session `f01b33b8`, 2026-04-26).

## Conclusion

The period shipped 93 commits and 32 PRs across 7 repositories, concentrated on beacon ingest reliability, the halyard rollout and rollback machinery, tideline dashboard work, and quill release and documentation hardening. The halyard cutover is entering its staged traffic shift and the quill hook reference is being kept current against the code.

## Methodology

- window is session-level (M2): whole sessions whose catalog `modified` falls in [since, until]; not per-record like pre-v2 reports, so a boundary-straddling session's numbers can differ from a v1 report.