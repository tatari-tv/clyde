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

Work ran across 25 of the 30 days in the window, spread broadly rather than concentrated in a few spikes, with only four inactive days (2026-04-10, 2026-04-13, 2026-04-15, 2026-04-25) and the closing day 2026-04-30 idle; the single heaviest day was 2026-04-23 at $77.16 and the busiest by session count was 2026-04-26 with 4 sessions at $50.05. No spend was carried in from before the window. The bulk of the work landed in `openpipe-oss/quill` ($264.78, 17 sessions) and `northwind-media/beacon` ($165.84, 7 sessions), with `northwind-media/tideline` and `northwind-media/halyard` following. Across the period the sessions produced 93 commits, 32 pull requests opened, and edits to 442 files, per observed transcript outcomes. The output mix spans ingest reliability work on beacon, plugin documentation and release hardening on quill, dashboard and query work on tideline, and rollout tooling on halyard.

30 of 44 sessions in the window (68.2%) carry an enrich summary; the rest are cited by title only.

## Quantified Output

The figures below are observed tool invocations extracted from session transcripts, not estimates.

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

`openpipe-oss/quill` carried the most spend and produced 30 commits and 16 PRs opened across its 17 sessions.

| Repo | Spend | Commits | PRs Opened | Files Edited |
|---|---|---|---|---|
| openpipe-oss/quill | $264.78 | 30 | 16 | 161 |
| northwind-media/beacon | $165.84 | 24 | 3 | 67 |
| northwind-media/tideline | $83.29 | 8 | 4 | 56 |
| northwind-media/halyard | $58.42 | 15 | 5 | 71 |
| jrivera/driftwood | $49.28 | 6 | 2 | 55 |
| northwind-media/almanac | $45.49 | 6 | 1 | 22 |
| jrivera/sextant | $4.16 | 4 | 1 | 10 |

The period spent $671.28 and produced 93 commits, a ratio of $7.22 per commit; against 32 PRs opened, a ratio of $20.98 per PR. Across 44 sessions the per-session ratio is $15.26, while the median session spend (`session-spend-p50`) was $12.71 and the 90th percentile (`session-spend-p90`) was $28.43; the mean sitting above the median indicates a few larger sessions carried more of the total. Against 25 active days the ratio is $26.85 per active day.

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

The unseen-account-spend figure is +$239.50, from anthropic enterprise analytics for jordan.rivera@northwind-media.example over the window 2026-04-01 to 2026-04-30.

This billed figure is the Claude Enterprise Analytics cost report for jordan.rivera@northwind-media.example alone, covering everything that account was billed across every Claude product: claude.ai web, Cowork, other clients, and other hosts. clyde report covers only the Claude Code sessions in this catalog on this machine. Billed spend meeting or exceeding modeled spend is the expected relationship here; a positive unseen-account-spend figure is the same person's usage that clyde cannot see, never that clyde miscounted.

| Model | Billed | Modeled | Unseen Account Spend |
|---|---|---|---|
| claude-opus-4-7 | $534.59 | $386.51 | +$148.08 |
| claude-sonnet-4-6 | $229.11 | $243.46 | -$14.35 |
| claude-opus-4-8 | $73.90 | $0.00 | +$73.90 |
| claude-haiku-4-5 | $73.18 | $41.31 | +$31.87 |

## Agent-Type Cost Attribution

The `(main-session)` row carried the most spend; this table is a true partition of the period spend, so its rows account for the whole $671.28.

| Agent Type | Tokens | Spend |
|---|---|---|
| (main-session) | 639.6M | $479.74 |
| phase-implementer | 133.4M | $79.33 |
| doc-writer | 103.5M | $61.78 |
| code-reviewer | 85.4M | $50.43 |

## The Efficiency Story

Cache reads accounted for 92.2% of context read (`cache-read-share`): most of the context the model reads each turn is re-read from cache at a fraction of the fresh-input rate, which is what makes sustained agentic sessions economical. Against 11.6M input tokens, 883.3M tokens were cache reads. The binary reports a list-price equivalent of $3,201.82 and cache savings of $2,530.54, computed from published per-token rates.

Additional signals from the period:

- Tool error rate: 4.2% of tool calls errored.
- Cache 1h write fraction: 7.4% of cache writes paid the 1h premium.
- Interrupts: 29.
- Compactions: 6.

| Skill | Tokens | Spend |
|---|---|---|
| schema-review | 24.1M | $44.59 |
| release-notes | 14.8M | $27.44 |

Skill attribution covers $72.03 of $671.28 (10.7%), embedded-price basis; these are tags, not a partition.

| MCP Tool | Tokens | Spend |
|---|---|---|
| tracker-search | 21.9M | $40.51 |
| wiki-write | 9.6M | $17.83 |

MCP attribution covers $58.34 of $671.28 (8.7%), embedded-price basis; these are tags, not a partition.

## What This Funded

### Northwind Media (employer org)

Four repos, 20 sessions, 504.0M tokens, $353.04 spend. The employer work split across ingest reliability on beacon, dashboard and query work on tideline, rollout tooling on halyard, and API/backfill work on almanac.

**Ingest reliability (beacon)**

- `northwind-media/beacon` (7 sessions, 208.9M tokens, $165.84 spend): the reliability spine of the period. Session `14fabbe1` added a dead-letter queue so a record the parser rejects is parked with its failure reason instead of being retried forever behind the live stream (PR #211). Session `4b1d68f6` traced a cold start in the ingest worker to a synchronous hostname lookup on the request path, moved it behind a warmed cache, and added a regression test. Session `54f9d4bb` audited the retention policy against what the storage layer actually deletes, found two buckets the sweeper never visited, and wired them into the same scheduled job. The repo recorded 24 commits and 3 PRs opened.

**Dashboard and query work (tideline)**

- `northwind-media/tideline` (4 sessions, 109.3M tokens, $83.29 spend): session `18c8897c` ported the dashboard to the new theme tokens, removed the last of the hardcoded colors, and verified contrast on the dense table view where the old palette failed (PR #190). Session `f01b33b8` profiled the slow dashboard query, found a missing composite index behind a filter the UI always sends, added it, and recorded the before/after latency in the ticket (PR #205). The repo recorded 8 commits and 4 PRs opened.

**Rollout tooling (halyard)**

- `northwind-media/halyard` (6 sessions, 132.7M tokens, $58.42 spend): sessions `07dce623` and `dadb0a1e` drafted the rollout plan for the cutover, covering the staged traffic shift, the rollback trigger, and the two dashboards that have to be green before each stage advances; sessions `2ad5c741` and `4e15c3a5` then added the rollback trigger that plan called for, wired to the same health check the staged shift reads, and rehearsed it against the staging fleet. The repo recorded 15 commits and 5 PRs opened.

**API and backfill work (almanac)**

- `northwind-media/almanac` (3 sessions, 53.1M tokens, $45.49 spend): session `519daf85` added cursor pagination to the list endpoint, kept the old offset parameter working for existing callers, and documented the deprecation window; session `f9e95abd` repaired the nightly backfill so it resumes from the last completed partition instead of restarting the whole range when a single shard times out (PR #226). The repo recorded 6 commits and 1 PR opened.

### OpenPipe OSS (open-source org)

- `openpipe-oss/quill` (17 sessions, 370.0M tokens, $264.78 spend): the single largest repo of the period, an open-source project centered on plugin documentation and release hardening. Multiple sessions (`3aed06b6`, `6bb58fd4`, `2dc73697`) documented the plugin hooks against the code rather than the wiki and dropped two hooks the loader has not called since the rewrite. Release-hardening sessions (`05981637`, `e3d9d683`, `93c4b9e8`) taught the release script to refuse publishing when the working tree is dirty, when the tag already exists, or when the changelog has no entry for the version being cut. The repo recorded 30 commits and 16 PRs opened.

### jrivera (personal org)

Two repos, 7 sessions, 87.9M tokens, $53.44 spend. These are engineering-productivity tools maintained under the user's personal org.

- `jrivera/driftwood` (5 sessions, 65.6M tokens, $49.28 spend): a parser project. Sessions `86b0fd93`, `89cf1386`, and `eaffd2a6` taught the parser to carry a line and a column through to its error type so a malformed document is reported where it broke rather than at the end of the file; session `93071145` split the parser into a tokenizer and shape builder so the error path can name the offending construct. The repo recorded 6 commits and 2 PRs opened.
- `jrivera/sextant` (2 sessions, 22.3M tokens, $4.16 spend): sessions `4bf28f14` and `b1cb8b81` rewrote the snapshot test harness so fixtures are compared structurally rather than by rendered string, removing the ordering flake that had been retried rather than fixed. The repo recorded 4 commits and 1 PR opened.

## Usage Profile

**Temporal distribution:** Work was spread fairly evenly across the window with no long dormant stretch; the four inactive days (2026-04-10, 2026-04-13, 2026-04-15, 2026-04-25) are isolated rather than consecutive, and the window closed with 2026-04-30 idle. The heaviest single day by spend was 2026-04-23 ($77.16, 2 sessions), followed by 2026-04-04 ($50.94) and 2026-04-26 ($50.05, the busiest by session count at 4). No spend was carried in from before the window; the by-day series covers the whole period.

**Model mix:**

- claude-opus-4-7 appeared on the heavier design and refactor work, including the beacon retention audit (`54f9d4bb`), the tideline theme port (`18c8897c`), the almanac pagination (`519daf85`), and driftwood parser work.
- claude-sonnet-4-6 was the most widely used model by session count and appeared across quill release hardening, beacon fixes, and halyard rollout work, frequently paired with the other models in the same session.
- claude-haiku-4-5 appeared on lighter, shorter sessions such as the halyard rollout-plan drafts (`07dce623`, `dadb0a1e`), the sextant snapshot-test fixes, and the almanac metric-label backfill.

**Outlier sessions:**

| Session | Repo | Tokens | Spend | What it produced |
|---|---|---|---|---|
| Wire the ingest retry backoff | northwind-media/beacon | 83.1M | $72.45 | 4 commits, 1 Confluence write, 3 Slack messages, 5 files edited |
| Document the quill plugin hooks | openpipe-oss/quill | 43.8M | $41.12 | Documented plugin hooks against the code; 4 commits, PRs #136 and #137, 20 files edited |
| 3aed06b6 | openpipe-oss/quill | 35.3M | $34.04 | Documented plugin hooks and dropped two unused hooks from the reference; 3 commits, 9 files edited |
| 54f9d4bb | northwind-media/beacon | 29.4M | $32.27 | Audited the retention policy, found two buckets the sweeper never visited, wired them into the scheduled job; 3 commits |
| Document the quill plugin hooks | openpipe-oss/quill | 27.9M | $28.43 | Plugin-hook documentation (per session title) |
| 18c8897c | northwind-media/tideline | 24.9M | $26.74 | Ported dashboard to new theme tokens, verified contrast on dense table view; 4 commits, PR #190 |
| Add a rollback trigger to halyard | northwind-media/halyard | 66.6M | $26.70 | Added rollback trigger wired to the staged-shift health check, rehearsed against staging; 3 commits, PR #178 |
| Harden the quill release script | openpipe-oss/quill | 27.4M | $26.36 | Hardened the release script against dirty tree, existing tag, and missing changelog entry (per summary) |
| Add a dead-letter queue to the ingest path | northwind-media/beacon | 25.3M | $25.96 | Added dead-letter queue for parser-rejected records; 3 commits, PR #211 |
| Repair the nightly almanac backfill | northwind-media/almanac | 23.7M | $24.98 | Repaired the nightly backfill to resume from the last completed partition; 2 commits, PR #226 |

## Month over Month

- The two periods are the same length: prior covered 30 days (2026-03-02 to 2026-03-31) against this period's 30 days.
- Spend rose from $523.17 to $671.28; sessions rose from 31 to 44; tokens from 765.8M to 961.9M.
- Both periods spanned 7 repositories. `openpipe-oss/quill` was the top repo in both, at $106.61 (9 sessions) prior versus $264.78 (17 sessions) this period. `northwind-media/beacon` moved from $75.46 to $165.84, and `northwind-media/halyard` from $42.84 (2 sessions) to $58.42 (6 sessions).
- Model mix held the same three models with opus-4-7 leading spend in both periods; opus-4-7 spend moved from $322.31 to $386.51 and sonnet-4-6 from $161.96 to $243.46.
- Output counts rose: 67 commits and 26 PRs opened prior against 93 commits and 32 PRs opened this period.

## Forward-Looking

- The halyard cutover has moved from planning to execution: the rollout plan was drafted (`07dce623`, `dadb0a1e`) and the rollback trigger it called for was added and rehearsed against the staging fleet (`2ad5c741`, `4e15c3a5`), leaving the staged traffic shift ahead.
- Quill plugin-hook documentation and release-script hardening are both in an active pass, with documentation rewritten against the code and the release script now gating on tree, tag, and changelog state.
- Beacon ingest reliability work is ongoing, from the dead-letter queue and cold-start fix to the retention-policy audit.

## Conclusion

The period shipped 93 commits and 32 pull requests across 7 repositories, concentrated in quill documentation and release hardening, beacon ingest reliability, and the halyard rollout tooling. In flight at the window's close are the halyard staged cutover, continued quill release work, and further beacon reliability changes.

## Methodology note

window is session-level (M2): whole sessions whose catalog `modified` falls in [since, until]; not per-record like pre-v2 reports, so a boundary-straddling session's numbers can differ from a v1 report.