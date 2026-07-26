---
title: "Claude Usage Report - Jordan Rivera - 2026-04-01 - 2026-04-30"
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

Work was sustained across 25 of 30 days, spread through the month rather than concentrated in a single spike, with the heaviest single day on 2026-04-23 ($77.16) and the busiest by session count on 2026-04-26 (4 sessions); no sessions carried in from before the window. The largest gaps were single inactive days at 2026-04-10, 2026-04-13, 2026-04-15, 2026-04-25, and the closing 2026-04-30. The most work landed in openpipe-oss/quill and northwind-media/beacon, which together carried the bulk of the period's spend. The period produced 93 commits, 32 PRs opened, and 442 files edited across seven repos, with themes centered on the quill release tooling and plugin docs, beacon ingest reliability, and the halyard rollout plan and rollback trigger. Model mix leaned on Opus for the largest single sessions while Sonnet appeared in the most sessions overall.

## Quantified Output

These are observed tool invocations extracted from session transcripts, not estimates.

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

openpipe-oss/quill carried the most spend and produced 30 commits and 16 PRs opened this period.

| Repo | Spend | Commits | PRs Opened | Files Edited |
|---|---|---|---|---|
| openpipe-oss/quill | $264.78 | 30 | 16 | 161 |
| northwind-media/beacon | $165.84 | 24 | 3 | 67 |
| northwind-media/tideline | $83.29 | 8 | 4 | 56 |
| northwind-media/halyard | $58.42 | 15 | 5 | 71 |
| jrivera/driftwood | $49.28 | 6 | 2 | 55 |
| northwind-media/almanac | $45.49 | 6 | 1 | 22 |
| jrivera/sextant | $4.16 | 4 | 1 | 10 |

The period spent $671.28 and produced 93 commits, a ratio of $7.22 per commit, and 32 PRs opened, a ratio of $20.98 per PR. Against the calendar, the period ran a ratio of $26.85 per active day. Per-session spend sits at a ratio of $15.26 while the median session was $12.71 and the 90th-percentile session $28.43; the mean above the median indicates a few larger sessions carried the period.

## Cost Summary

| Model | Sessions Using | Total Tokens | Spend |
|---|---|---|---|
| claude-opus-4-7 | 18 | 346.4M | $386.51 |
| claude-sonnet-4-6 | 26 | 403.7M | $243.46 |
| claude-haiku-4-5 | 18 | 211.8M | $41.31 |
| **Total** | 44 | 961.9M | $671.28 |

## Reconciliation

This render's modeled total was reconciled against the Claude Enterprise Analytics cost export for this exact window; see the Reconciliation section for the billed figure and the scope note.

| Figure | Amount |
|---|---|
| Billed | $910.78 |
| Modeled | $671.28 |

Unseen account spend is +$239.50 against the anthropic enterprise analytics export for the window 2026-04-01 to 2026-04-30. An Analytics export covers everything the account billed: claude.ai web, other clients, and other hosts. clyde report covers only the Claude Code sessions in one catalog. Billed spend meeting or exceeding modeled spend is the expected relationship here; a positive unseen-account-spend figure means usage clyde does not see, never that clyde miscounted.

| Model | Billed | Modeled | Unseen Account Spend |
|---|---|---|---|
| claude-opus-4-7 | $534.59 | $386.51 | +$148.08 |
| claude-sonnet-4-6 | $229.11 | $243.46 | -$14.35 |
| claude-opus-4-8 | $73.90 | $0.00 | +$73.90 |
| claude-haiku-4-5 | $73.18 | $41.31 | +$31.87 |

## Agent-Type Cost Attribution

The `(main-session)` row carried the most spend; the table below is a true partition of the period spend, with every dollar landing in exactly one row and totaling $671.28.

| Agent Type | Tokens | Spend |
|---|---|---|
| (main-session) | 639.6M | $479.74 |
| phase-implementer | 133.4M | $79.33 |
| doc-writer | 103.5M | $61.78 |
| code-reviewer | 85.4M | $50.42 |

## The Efficiency Story

Cache read share was 92.2%: most of the context the model reads each turn is re-read from cache at a fraction of the fresh-input rate, which is what makes sustained agentic sessions economical. Against 11.6M input tokens, 883.3M tokens were read from cache. The list-price equivalent of that read volume is $3,201.82 and the modeled cache savings is $2,530.54, both computed from published per-token rates.

Other observed signals:

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

Enrichment coverage: 30 of 44 sessions in the window (68.2%) carry an enrich summary; the rest are cited by title only. Where a session below carries no summary, its theme rests on its title.

### Northwind Media (employer org)

Four repos, 20 sessions, 504.0M tokens, $353.04 spend. The employer work clustered around ingest reliability in beacon, the halyard rollout, tideline dashboard performance, and the almanac API.

**Ingest reliability (beacon)**

- `northwind-media/beacon` (7 sessions, 208.9M tokens, $165.84 spend): 24 commits and 3 PRs opened. Session ccf3945e wired the ingest retry backoff (title); 14fabbe1 added a dead-letter queue so a record the parser rejects is parked with its failure reason instead of being retried forever behind the live stream (PR #211); 4b1d68f6 traced a cold start to a synchronous hostname lookup on the request path and added a regression test. 54f9d4bb audited the retention policy against what the storage layer actually deletes and wired two missed buckets into the scheduled sweep.

**Rollout and rollback (halyard)**

- `northwind-media/halyard` (6 sessions, 132.7M tokens, $58.42 spend): 15 commits and 5 PRs opened. dadb0a1e drafted the rollout plan for the cutover (staged traffic shift, rollback trigger, and the two dashboards that must be green); 2ad5c741 added the rollback trigger the plan called for, wired to the same health check the staged shift reads, and rehearsed against staging; 88ac1250 continued the trigger work (PRs #157, #158).

**Dashboard performance (tideline)**

- `northwind-media/tideline` (4 sessions, 109.3M tokens, $83.29 spend): 8 commits and 4 PRs opened. f01b33b8 profiled the slow dashboard query, found a missing composite index behind a filter the UI always sends, and recorded before/after latency (PR #205); 18c8897c ported the dashboard to the new theme tokens and verified contrast on the dense table view; cfd90771 continued the theme port (title).

**Almanac API and backfill**

- `northwind-media/almanac` (3 sessions, 53.1M tokens, $45.49 spend): 6 commits and 1 PR opened. f9e95abd repaired the nightly backfill so it resumes from the last completed partition instead of restarting the whole range on a single shard timeout (PR #226); 519daf85 added cursor pagination to the list endpoint while keeping the old offset parameter working and documenting the deprecation window; f4271278 added pagination to the API (title).

### jrivera (personal org)

Two repos, 7 sessions, 87.9M tokens, $53.44 spend. driftwood is a document parser and sextant is a tooling project with a snapshot-test harness; both are engineering-productivity work maintained alongside the employer repos.

- `jrivera/driftwood` (5 sessions, 65.6M tokens, $49.28 spend): 6 commits and 2 PRs opened. eaffd2a6 and 89cf1386 taught the parser to carry a line and column through to its error type so a malformed document is reported where it broke; 93071145 split the parser into a tokenizer and a shape builder so the error path can name the offending construct.
- `jrivera/sextant` (2 sessions, 22.3M tokens, $4.16 spend): 4 commits and 1 PR opened. b1cb8b81 rewrote the snapshot test harness to compare fixtures structurally rather than by rendered string, removing an ordering flake that had been retried rather than fixed.

### openpipe-oss (open source)

One repo, 17 sessions, 370.0M tokens, $264.78 spend. quill is an open-source project whose release tooling and plugin documentation dominated this month's open-source work.

**Release script hardening and plugin docs (quill)**

- `openpipe-oss/quill` (17 sessions, 370.0M tokens, $264.78 spend): 30 commits and 16 PRs opened. 05981637, e3d9d683, and 93c4b9e8 hardened the release script so it refuses to publish on a dirty tree, an existing tag, or a changelog with no entry for the version being cut (PRs #193, #194, #217, #218); 6bb58fd4, 3aed06b6, and 45f45aca documented the plugin hooks against the code rather than the wiki and dropped two hooks the loader has not called since the rewrite (PRs #136, #137).

## Usage Profile

- **Temporal distribution**: Work was spread across the month with no run of consecutive inactive days; the single inactive days were 2026-04-10, 2026-04-13, 2026-04-15, 2026-04-25, and 2026-04-30. Spend peaked on 2026-04-23 ($77.16) and 2026-04-04 ($50.94), with 2026-04-26 carrying the most sessions (4). No spend carried in from before the window ($0.00 across 0 sessions), so the by-day series covers the whole period.
- **Model mix**: claude-opus-4-7 appeared in the largest single sessions across quill docs, beacon retention, tideline profiling, and driftwood parser work. claude-sonnet-4-6 appeared in the most sessions, spanning the quill release script, beacon backfill, and halyard rollout. claude-haiku-4-5 appeared in the shorter, cheaper sessions such as the halyard rollout plan drafts and driftwood parser splits.
- **Outlier sessions**:

| Session | Repo | Tokens | Spend | What it produced |
|---|---|---|---|---|
| Wire the ingest retry backoff | northwind-media/beacon | 83.1M | $72.45 | 4 commits, 1 Confluence write, 3 Slack messages, 5 files edited |
| Document the quill plugin hooks | openpipe-oss/quill | 43.8M | $41.12 | 4 commits, PRs #136 and #137, 20 files edited |
| 3aed06b6 | openpipe-oss/quill | 35.3M | $34.04 | Documented plugin hooks against the code, dropped two unused hooks; 3 commits, 9 files edited |
| 54f9d4bb | northwind-media/beacon | 29.4M | $32.27 | Audited retention policy, wired two missed buckets into the sweeper; 3 commits |
| Document the quill plugin hooks | openpipe-oss/quill | 27.9M | $28.43 | Documented plugin hooks against the code, dropped two unused hooks |
| 18c8897c | northwind-media/tideline | 24.9M | $26.74 | Ported dashboard to new theme tokens, verified contrast; 4 commits, PR #190 |
| Add a rollback trigger to halyard | northwind-media/halyard | 66.6M | $26.70 | Added rollback trigger wired to the staged-shift health check; 3 commits, PR #178 |
| Harden the quill release script | openpipe-oss/quill | 27.4M | $26.36 | Hardened release script to refuse a dirty tree, existing tag, or missing changelog entry |
| Add a dead-letter queue to the ingest path | northwind-media/beacon | 25.3M | $25.96 | Added dead-letter queue for rejected records; 3 commits, PR #211 |
| Repair the nightly almanac backfill | northwind-media/almanac | 23.7M | $24.98 | Backfill resumes from last completed partition; 2 commits, PR #226 |

## Month over Month

- Both periods cover 30 days. Spend was $671.28 this period against $523.17 prior; sessions were 44 against 31; tokens 961.9M against 765.8M. Repo count held at 7 in both.
- The org mix shifted: openpipe-oss/quill rose from $106.61 (9 sessions) prior to $264.78 (17 sessions) this period and moved to the top of the by-org table, while northwind-media stayed the largest org tier ($353.04 against $282.71 prior).
- Output rose across the board: 93 commits against 67, 32 PRs opened against 26, and 442 files edited against 250. Confluence writes went from 2 to 8.
- Model mix held its shape: claude-opus-4-7 led spend in both periods ($386.51 against $322.31), with claude-sonnet-4-6 second and claude-haiku-4-5 third.

## Forward-Looking

- The halyard cutover is past the rollout-plan stage (dadb0a1e) and into implementation, with the rollback trigger added and rehearsed against staging (2ad5c741, 88ac1250) late in the period.
- quill release tooling is stabilizing: the release script now refuses unsafe publishes (e3d9d683, 93c4b9e8) and the plugin-hook reference has been aligned to the code and pruned of dead hooks (45f45aca on 2026-04-29).
- driftwood's parser error reporting is mid-refactor, with the tokenizer/shape-builder split landed (93071145) and line/column carried into the error type (89cf1386).

## Conclusion

This period shipped 93 commits and 32 PRs across quill release tooling, beacon ingest reliability, tideline dashboard performance, the almanac API, and the driftwood parser, with the largest spend in openpipe-oss/quill and northwind-media/beacon. In flight into next month are the halyard cutover and rollback trigger, the quill release-script and plugin-doc hardening, and the driftwood parser error-reporting refactor.