# Handoff: the defects v0.20.0 did NOT fix

**Author:** Scott Idler
**Date:** 2026-07-31
**Status:** Open register. Diagnosed AND scoped to a concrete fix. Do not re-derive.
**Audience:** the next agent picking up `tatari-tv/clyde`
**Supersedes:** `docs/design/2026-07-30-open-defects-handoff.md` (C is closed; that file keeps the
original diagnosis and its addendum)

v0.20.0 shipped `docs/design/2026-07-30-archived-session-spend.md` and closed exactly one item from
the previous register: C, the ~30% cost undercount. `clyde cost --no-cache monthly` for June went
`$4,818.54` to `$8,040.64` on desk.lan. Everything else in that register was left alone, and leaving
it alone was the wrong call: the register's own routing already said D, E and F were targeted fixes
needing no design doc, so there was nothing to wait for.

**Every item below carries the actual edit, the test that has to bite, and what it risks.** Line
numbers are against `main` at v0.20.0. The named symbols are the durable anchors.

## Do these first: three targeted fixes, no design doc

### D. Fail-open cost math: an unpriceable model contributes $0 to the catalog

- **Where:** `common/src/metrics.rs:138` translates the `Result` to an `Option`
  (`pricing.calculate_usd(model, usage).ok()`). The fail-open is at the CONSUMER:
  `efficiency/src/metrics.rs:173` does `.unwrap_or_else(|| { warn!(...); 0.0 })` and adds that $0
  into `cost_usd`.
- **Sharper than the old register entry.** The REPORT path already discloses this: an unpriced model
  lands in `Totals::untracked_models` / `SessionEntry::untracked_models`
  (`report/src/report.rs:76,120`), and `report/src/aggregate.rs:330-340` even drops the cache
  counterfactual rather than guessing. The unguarded path is the CATALOG: the `cost_usd` column
  `efficiency` writes has no `untracked_models` equivalent, and nothing downstream can tell a genuine
  $0 from an unpriced one.
- **Why it matters more after v0.20.0.** `report collect` now reads 199 more June rows on desk.lan,
  and each one's dollars come from that column. A host running a model the feed lacks gets a quietly
  low total that looks exactly like the bug we just spent a design doc chasing.
- **The fix:** persist the unpriced-model set alongside the efficiency blob, the way the report
  artifact already does, and stop laundering the failure into a number.
  1. Add `unpriced_models: BTreeSet<String>` to the counters that feed `SessionEfficiency`
     (`efficiency/src/metrics.rs`), populated where the `unwrap_or_else` currently swallows it.
     `RawCounters` already carries `by_model`, so the shape and the kebab-case serde are established.
  2. Surface it in `PersistStats` and in `print_reindex`, exactly as Phase 2 did for
     `unrecoverable`: a count that is always printed, so `cost_usd` is never quietly low.
  3. `report collect` unions it into the artifact's existing `untracked_models` so the two paths
     agree instead of one being blind.
- **The precedent to copy:** `cost/src/oracle.rs:326` handles the same call with an explicit `match`.
  Phase 3 of the archived-session-spend doc is the shape for the disclosure half: split the guard,
  exclude or flag the row, state the count, never ship a silently-wrong total.
- **The test that must bite:** a session whose transcript names a model absent from
  `pricing/data/pricing.json` gets a non-empty `unpriced-models` in its stored blob and a non-zero
  count in the reindex output. Break it by restoring the bare `unwrap_or_else(|| 0.0)` and the count
  goes to zero while `cost_usd` stays low.
- **Risk:** touches the `efficiency_json` shape. Decide whether that needs a `SCHEMA_VERSION` bump;
  the archived-session-spend doc declined one because only WHICH ROWS reached the report changed, and
  this is different: the blob itself grows a field.

### E. The `_variable` lint only walks `*/src/`

- **Where:** `.otto.yml:16`, `grep -rn --include='*.rs' -P '...' */src/`
- **What:** misses `*/tests/` and `*/build.rs`. Exactly the hole the em-dash lint was deliberately
  scoped to avoid (whole tree, `--exclude-dir=target`).
- **The fix:** drop `*/src/` for `.` plus `--exclude-dir=target`, matching the em-dash lint's shape
  two blocks below it in the same file. Also copy that lint's EXPLICIT status check: the `_variable`
  lint still uses `if grep ...`, which reads every non-zero exit (including 127, binary missing, and 2,
  read error) as "clean". That is the same fail-open that made the em-dash lint a no-op in CI from the
  day it landed.
- **Measured, so this is genuinely trivial:** widening it surfaces **zero** new violations today.
  `grep -rn --include='*.rs' --exclude-dir=target -P '<pattern>' . | grep -v '^\./[a-z-]+/src/'`
  returns 0 lines at v0.20.0. Land the widening now, while it is free.
- **The test that must bite:** add a `let _foo = 1;` under any `*/tests/` file, confirm `otto lint`
  fails, remove it.
- **Risk:** none measured. The fail-open status check is the part with real value.

### F. `report/templates/slots/*.pmt` are outside the em-dash lint

- **Where:** 5 templates, compiled in via `include_str!` and sent to the model. The lint is
  `--include='*.rs'`, so it cannot see them.
- **Currently clean:** 0 em-dash occurrences across all five. Their ONLY protection is the assertion
  at `report/src/render/slots/tests.rs:531` (`!prompt.contains('\u{2014}')`). Delete that test and CI
  cannot see the hole.
- **The fix:** add `--include='*.pmt'` to the em-dash lint's `grep` in `.otto.yml`. Verified it passes
  immediately: no `.pmt`, `.yml` or `.toml` file in the tree carries the character.
- **The test that must bite:** put an em-dash in one `.pmt`, confirm `otto lint` fails.
- **Risk:** none. Keep the `tests.rs:531` assertion too; belt and braces on prompts that go to a model.

## These need design docs

### B. Dormancy reads the transcript's filesystem mtime, not activity time. DO THIS BEFORE A

- **Where:** `session/src/parse.rs:353` sets `modified` to the MAX filesystem mtime across the
  session's files. Filtered at `sessions/src/db.rs:601` (`enrich_candidates`) **and `:692`
  (`staging_candidates`)**. Two call sites; a fix must cover both. (The previous register said 611 and
  682. Both drifted.)
- **What:** anything that rewrites a file under `~/.claude/projects/` resets dormancy: a Syncthing or
  Dropbox sync, a restore, a `cp -r` to a new machine.
- **Why this is now more urgent than when it was registered.** v0.20.0 wired `stage_dormant` into
  every `clyde session reindex` plus a 6h `clyde-reindex` systemd timer. `staging_candidates` filters
  on the same mtime-derived `modified`, so a stray mtime touch now suppresses the AUTOMATIC staging
  sweep. Staging is the only thing standing between a dormant session and permanent unpriceability, so
  this defect silently disables the protection C's fix depends on. It went from an enrichment
  annoyance to a money leak.
- **Observed (Patrick):** sessions dated Jul 1-30 all run on Jul 30 gave `considered: 0` at 7d and
  `considered: 44` at `--dormant-after 1h`.
- **Invisible on desk.lan:** plenty of rows sit past the 7d cutoff here, so sweeps always find work.
  It needs a regression test, not a fix-and-eyeball.
- **The fix is nearly free:** `session/src/parse.rs:388` ALREADY parses per-message `timestamp` (MIN
  into `created`). A MAX of the same field is activity time, immune to file touches.
- **Two things the doc must settle:** `modified` stays load-bearing elsewhere (grown-since-enrichment
  compares `s.modified > s.enriched_modified`; export duration is mtime minus earliest ts), so ADD
  alongside, never repurpose. And decide the backfill: NULL means "not dormant", or trigger a re-parse.
- **Routing:** `/create-design-doc`. Schema addition plus a migration. The bar is a test that fails on
  a machine where every mtime is fresh.

### A. `scope` classifies off `cwd` alone. Still blocks the excision's AC6

- **Where:** `sessions/src/enrich.rs:105` (the gate) calls
  `session::classify(rec.cwd.as_deref().map(Path::new))` and consults nothing else, into
  `session/src/scope.rs:59`.
- **The asymmetry, and the lead:** `common/src/repo.rs:37` already attributes a repo FOUR ways
  (`git-origin` / `known-path` / `files-touched` / `path-guess`). `scope` uses none of them, so the
  catalog can know a session edited `tatari-tv/philo` and still call it personal.
- **Measured:** 21 sessions on desk.lan have a `tatari-tv/*` repo but `scope='personal'`. Patrick: 0
  of 131 (runs `claude` from `~`). Keegan: repo values "less null and more just wrong".
- **Blast radius:** 0% enrichment coverage, and `report collect`'s Executive Summary, What This
  Funded, and Conclusion render EMPTY.
- **NOT a bug in the fail-safe.** `session/src/scope.rs:20` already documents this failure direction
  as acceptable. What was never priced is its cost to a `cwd`-hostile workflow.
- **Why a design doc, not a patch:** widening the gate is a SECURITY change. Trusting `files-touched`
  means a `$HOME` session that touched one work file may ship its whole body to the work account.
- **Routing:** `/create-design-doc`. Security section required. Name the AC6 dependency
  (`docs/design/2026-07-29-excise-api-key.md:548`, the 50% enrichment floor; the keyless half is
  already confirmed by a teammate, only the percentage is outstanding).

## Housekeeping

### The XDG data-dir helper now exists in five copies

- **Where:** `session/src/paths.rs`, `permit/src/config.rs`, `report/src/config.rs`,
  `cost/src/config.rs`, and `common/src/scan.rs`. The fifth was added by v0.20.0 Phase 1, because
  `common` had none and cannot depend on `session` (the edge runs `session -> common`).
- **Not a bug:** none of them uses the banned `dirs::data_local_dir()`, so the macOS trap is closed
  everywhere. It is five copies of ten lines that must agree.
- **The fix:** promote one into `common` (it is already the lowest crate in the graph) and have the
  other four delegate, the way `session::paths::staged_dir` now delegates to
  `common::scan::default_staged_dir`. Targeted, no doc, but it touches five crates so it wants its own
  commit and nothing else riding along.

### G. The hook registration is still uncommitted

- **Where:** `~/repos/scottidler/claude`, `HOME/.claude/settings.json`
- **What:** the PreToolUse entry for `codex-stdin-guard.sh` is live on disk but NOT committed, because
  that file also carries unrelated in-flight changes (`block-question-picker`, the `ask` list, a
  plugin entry) that were not mine to commit.
- **Action:** commit it with your own batch. The hook script itself IS committed and symlinked.
  Different repo; nothing in `clyde` blocks on it.

## Acceptance criteria: DONE, all six pass

Run on desk.lan 2026-07-31 against the installed v0.20.0, after `clyde session reindex`. Numbers are
recorded per criterion in `docs/design/2026-07-30-archived-session-spend.md`; summary here so this
file stands alone.

| | result |
|---|---|
| **A1** everything with bytes gets priced | PASS. Run 1 `304 = 240 + 64`, run 2 `66 = 2 + 64`, `unrecoverable` stable at 64. DB remainder is exactly 64 rows, all `archived=1 AND staged_path IS NULL` |
| **A2** June accounts for every row | PASS. `.totals.sessions` 558, 0 disclosed, SQL 558 |
| **A3** June clears the bound on both pipelines | PASS. report `$7,689.04` (bound 7300), cost `$8,040.64` (bound 7700) |
| **A4** May stops reading as zero-usage | PASS. Exit 0, 15 sessions, `$170.39`, notes names 64 unrecoverable |
| **A5** staging backlog drains | PASS. `0`, from a `1496` baseline. First sweep staged 1498 sessions / 2584 files; second reported `staged=0 up-to-date=1590` |
| **A6** `otto ci` | PASS. Green on all six phase commits and on PR #79's six required checks |

Two things that came out of running them, both already folded into the design doc:

- **A1's second-run wording was a doc defect and was amended.** It pinned
  `.efficiency.computed == 0`, which cannot hold on a machine where Claude Code is running:
  `upsert_session` NULLs efficiency on a content change, so a session that grows between the two
  passes is legitimately a new candidate. Two did. The criterion now asserts the invariant that
  carries the meaning (`unrecoverable` stable, full set accounted for, DB remainder equal to it)
  rather than a count the environment must change.
- **The two pricing pipelines land 4.4% apart** (`$7,689.04` report vs `$8,040.64` cost), inside the
  1-9% band the investigation established, so the independent cross-check is still usable. Against
  June ground truth of `$9,110.96` that is -15.6% and -11.8%; the remainder is multi-host, which
  `clyde report merge` covers.

**The 64 unrecoverable rows are now a fixed, closed set**, not a leading edge: the staging backlog is
0 and the sweep runs inside every reindex. It only stays closed if the `clyde-reindex` timer is
installed (`clyde doctor` reports it absent on this host as of 2026-07-31) and if B is fixed, since B
can suppress the sweep that keeps it closed.

## Suggested order

1. **E and F.** Both measured at zero new violations. One commit each, or one commit for both since
   they edit adjacent blocks of `.otto.yml`. Do them before anything else touches the tree.
2. **D.** Real consequence, and the only one of the three targeted fixes with a schema question.
3. **B.** Design doc. Ahead of A because it now silently disables v0.20.0's staging protection.
4. **A.** Design doc, security section required.
5. **The XDG consolidation**, whenever the tree is otherwise quiet.
6. **G**, in the other repo, with your own settings.json batch.
