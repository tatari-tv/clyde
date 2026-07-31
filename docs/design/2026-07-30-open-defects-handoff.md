# Handoff: open defects after v0.19.0

**Author:** Scott Idler
**Date:** 2026-07-30
**Status:** Open register. Diagnosed, deliberately unfixed. Do not re-derive.
**Audience:** the next agent picking up `tatari-tv/clyde`

Everything below is diagnosed to file and line and was left alone on purpose: none of it was in
`docs/design/2026-07-30-excise-api-key-followups.md`, and unrequested scope is illegitimate.

Depth for A/B/C: `docs/design/2026-07-30-scope-dormancy-cost-handoff.md`. This is the index.

## A. `scope` classifies off `cwd` alone. BLOCKS the excision's AC6

- **Where:** `sessions/src/enrich.rs:105` (the gate) -> `session/src/scope.rs:59` (`classify`)
- **What:** enrich re-derives scope from `cwd` at send time and consults nothing else
- **The asymmetry, and the lead:** `common/src/repo.rs:37` already attributes a repo FOUR ways
  (`git-origin` / `known-path` / `files-touched` / `path-guess`). `scope` uses none of them, so the
  catalog can know a session edited `tatari-tv/philo` and still call it personal
- **Measured:** 21 sessions on desk.lan have a `tatari-tv/*` repo but `scope='personal'`. Patrick: 0
  of 131 (runs `claude` from `~`). Keegan: repo values "less null and more just wrong"
- **Blast radius:** 0% enrichment coverage, and `report collect`'s Executive Summary, What This
  Funded, and Conclusion render EMPTY
- **NOT a bug in the fail-safe.** `session/src/scope.rs:20` already documents this failure direction
  as acceptable. What was never priced is its cost to a `cwd`-hostile workflow
- **Why a design doc, not a patch:** widening the gate is a SECURITY change. Trusting `files-touched`
  means a `$HOME` session that touched one work file may ship its whole body to the work account
- **Blocks:** `docs/design/2026-07-29-excise-api-key.md:548` (AC6, the 50% enrichment floor). The
  keyless half of AC6 is already confirmed by a teammate; only the percentage is outstanding

## B. Dormancy reads the transcript's filesystem mtime, not activity time

- **Where:** `session/src/parse.rs:353` sets `modified` = MAX filesystem mtime across the session's
  files. Filtered at `sessions/src/db.rs:611` (`enrich_candidates`) **and `:682`
  (`staging_candidates`)** -- two call sites, a fix must cover both
- **What:** anything that rewrites a file under `~/.claude/projects/` resets dormancy: a Syncthing or
  Dropbox sync, a restore, a `cp -r` to a new machine
- **Corrects the thread's guess:** `reindex` does NOT write transcripts, it reads their mtimes, so
  the exposure is far wider than one subcommand
- **Observed:** Patrick, sessions dated Jul 1-30 run on Jul 30 -> `considered: 0` at 7d,
  `considered: 44` at `--dormant-after 1h`
- **Invisible on desk.lan:** 1,843 rows sit past the 7d cutoff here, so sweeps always find work. It
  needs a regression test, not a fix-and-eyeball
- **Fix is nearly free:** `session/src/parse.rs:388` ALREADY parses per-message `timestamp` (MIN into
  `created`). A MAX of the same field is activity time, immune to file touches
- **Two things the doc must settle:** `modified` stays load-bearing elsewhere (grown-since-enrichment
  compares `s.modified > s.enriched_modified`; export duration is mtime minus earliest ts), so add
  alongside, never repurpose. And decide the backfill: NULL means "not dormant" or trigger a re-parse
- **Downstream:** with the runbook's `enrich` -> `collect` order, the last 7 days of a month-to-date
  report can never be enriched

## C. Cost reads ~30% under the `claude.ai` web UI

- **Reported by:** Keegan. `ccu` used to land within 5-10% (always low); after recent fixes "at least
  30% lower than the web UI shows"
- **REFUTED, do not redo:** it is not missing model pricing. Every model in this catalog
  (`opus-4-7/4-8/5`, `sonnet-5/4-6`, `fable-5`, `haiku-4-5`) has an entry in
  `pricing/data/pricing.json`
- **First question, and it decides whether there is a bug at all:** does the web UI count usage
  beyond Claude Code (claude.ai chats, desktop)? If so the gap is EXPECTED. Settle this before
  touching any math
- **Other leads, unverified:** the 5m/1h cache-write split and the above-200k tiers
  (`pricing/src/pricing.rs:165`); whether subagent/sidechain entries are counted once; whether
  `<synthetic>` (8 rows here) is excluded deliberately

## D. Fail-open in cost math: an unpriceable model is silently dropped

- **Where:** `common/src/metrics.rs:138` -- `pricing.calculate_usd(model, usage).ok()`
- **What:** `None` drops the row from the sum. It does `warn!`, so not silent in the log, but the
  TOTAL is quietly low rather than loud
- **Not the cause of C on this machine** (nothing is unpriced here), but on a host using a model the
  feed lacks it would look exactly like a ~30% undercount
- **Contrast:** `cost/src/oracle.rs:326` handles the same call with a `match`. Same class as the
  `if rg ...` lint bug fixed in v0.19.0: a non-happy path converted into an apparent success

## E. The `_variable` lint only walks `*/src/`

- **Where:** `.otto.yml:16` -- `grep -rn --include='*.rs' -P '...' */src/`
- **What:** misses `*/tests/` and `*/build.rs`. Exactly the hole the em-dash lint was deliberately
  scoped to avoid (whole tree, `--exclude-dir=target`)
- **Status:** recorded in the followups doc's Non-Goals as found-while-drafting, nobody asked, its
  own change. Still true

## F. `report/templates/slots/*.pmt` are outside the em-dash lint

- **Where:** 5 templates, compiled in via `include_str!` and sent to the model
- **What:** not `.rs`, so `--include='*.rs'` cannot see them. Currently CLEAN (0 occurrences)
- **Guarded by:** `report/src/render/slots/tests.rs`'s `!prompt.contains('\u{2014}')` assertion, which
  is their ONLY protection. Deleting it opens a hole CI cannot see
- Left as-is: the test covers it, and widening the lint was unrequested

## G. Not a defect, but outstanding: the hook registration is uncommitted

- **Where:** `~/repos/scottidler/claude`, `HOME/.claude/settings.json`
- **What:** the PreToolUse entry for `codex-stdin-guard.sh` is live on disk but NOT committed, because
  that file also carries unrelated in-flight changes (`block-question-picker`, the `ask` list, a
  plugin entry) that were not mine to commit
- **Action:** commit it with your own batch. The hook script itself IS committed and symlinked

## Suggested routing

- **A:** `/create-design-doc`. Security section required. Name the AC6 dependency
- **B:** `/create-design-doc`. Small in code, but a schema addition plus a migration, and the bar is a
  test that fails on a machine where every mtime is fresh
- **C:** probe first, design second. The web-UI comparison question is a measurement, not a design
- **D, E, F:** targeted fixes, no doc. D is the only one with real consequence
