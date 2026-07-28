# Handoff: nine releases since Friday 2026-07-24

**Author:** Scott Idler
**Date:** 2026-07-28
**Status:** Handoff brief. Analysis complete, do not re-derive.
**Audience:** the next agent picking up `tatari-tv/clyde`

Read this before opening a new design doc. It records what shipped in the last four days, what each
release was for, how much design-doc machinery each one consumed, and the defect list that came out
of the most recent one and is still open. Every number here was pulled from git and the code, not
from an agent's recollection.

## The scoreboard

Nine releases, `v0.12.2` through `v0.16.0`. Seven landed Friday 2026-07-24, two on Monday
2026-07-27. Saturday and Sunday produced design docs but no tags. Tag dates below are git's
`creatordate`; the working timezone is US Pacific, so a late tag can read a day earlier than the UTC
timestamps in logs.

| version | day | PR | files | insertions | phase trailers | plan size |
|---|---|---|---|---|---|---|
| v0.12.2 | Fri 07-24 | [#54](https://github.com/tatari-tv/clyde/pull/54) | 13 | 279 | 0 | no doc |
| v0.12.3 | Fri 07-24 | [#56](https://github.com/tatari-tv/clyde/pull/56) | 6 | 198 | 1 | of 5 (cross-repo doc) |
| v0.13.0 | Fri 07-24 | [#55](https://github.com/tatari-tv/clyde/pull/55) | 51 | 4,832 | 5 | of 6 |
| v0.13.1 | Fri 07-24 | [#57](https://github.com/tatari-tv/clyde/pull/57) | 11 | 215 | 0 | no doc |
| v0.13.2 | Fri 07-24 | [#58](https://github.com/tatari-tv/clyde/pull/58) | 8 | 763 | 0 | no doc |
| v0.13.3 | Fri 07-24 | [#59](https://github.com/tatari-tv/clyde/pull/59), [#46](https://github.com/tatari-tv/clyde/pull/46) | 11 | 228 | 0 | no doc |
| v0.14.0 | Fri 07-24 | [#60](https://github.com/tatari-tv/clyde/pull/60) | 40 | 6,167 | 8 | of 3 AND of 6 (three docs) |
| v0.15.0 | Mon 07-27 | [#63](https://github.com/tatari-tv/clyde/pull/63), [#62](https://github.com/tatari-tv/clyde/pull/62), [#61](https://github.com/tatari-tv/clyde/pull/61) | 122 | 39,823 | 14 | of 14 (complete) |
| v0.16.0 | Mon 07-27 | [#64](https://github.com/tatari-tv/clyde/pull/64) | 21 | 2,272 | 4 | of 7 (phases 0 through 6) |

Totals: **9 releases, 12 PRs, ~54,800 insertions, 32 phase trailers.**

Thirteen design-doc files were authored in the window: six primary docs, six implementation-notes
companions, one handoff brief.

## Phases per release, and why the average lies

**32 phases / 9 releases = 3.6 average. Ignore that number.** The distribution is bimodal and the
average sits in a gap: the per-release counts are 0, 0, 0, 0, 1, 4, 5, 8, 14, so the median is 1 and
the modal release shipped zero phases.

- **Four releases shipped zero phases**: `v0.12.2`, `v0.13.1`, `v0.13.2`, `v0.13.3`. All targeted
  fixes, no design doc, correctly so.
- **Five releases used the phase machinery**, at 1, 4, 5, 8 and 14 phases. Among those the average is
  **6.4 phases per release**.
- `v0.15.0` is the outlier in every dimension: 14 of 14 phases completed, 122 files, 39,823
  insertions, which is 73% of the window's total. Most of that bulk is fixtures and committed golden
  artifacts, not logic.
- `v0.14.0` carried **three** design docs in one release (`report-render-claude-cli-transport` at 3
  phases, `pricing-feed-staleness-and-index` and `render-output-ceilings-config` sharing a 6-phase
  numbering). One tag, three plans.
- `v0.16.0` has the **worst completion ratio among single-repo plans**: 4 of 7 phases. `v0.12.3`'s
  1-of-5 reads lower on paper, but its doc was cross-repo and the other phases shipped in other
  repos; every other doc-driven release either completed its plan or came within one phase.
  `v0.16.0` is the only one abandoned mid-flight, and the phase count flatters it: Phase 0 was
  zero-code log mining and Phase 4 shipped as a three-word deletion.

## What each release was actually for

There is a single arc through all nine. Reading them as independent releases misses it.

1. **`v0.12.2` (kebab-case JSON)** and **`v0.12.3` (plugin skills)**: consistency and packaging.
   Housekeeping.
2. **`v0.13.0` (collect once, render from data)**: the architectural split. Collection stopped being
   coupled to rendering, so a render became a pure function of a committed JSON artifact. Everything
   after this depends on it.
3. **`v0.13.1`, `v0.13.2`, `v0.13.3`**: cost-accuracy and fail-loudly fixes plus a pricing refresh.
   Reactive, small, correct.
4. **`v0.14.0` (claude CLI transport)**: made renders cheap. Renders could now go through the local
   `claude` CLI on a subscription instead of paid API calls. This is what made high-volume render
   experimentation affordable, which matters for what follows.
5. **`v0.15.0` (report story fidelity)**: the big one. Fourteen phases, and among them the **narrowed
   prose guard** (`report/src/quotable.rs`, `report/src/claim.rs`). It replaced a whitelist that
   pre-approved every numeric token in a ~940KB context block with three derived sets, so a prose
   figure is only accepted if the binary actually computed it. It works: it caught a fabricated
   dollar figure the old whitelist would have passed. A rejection is a hard render failure.
6. **`v0.16.0` (make guard rejections legible)**: remediation of `v0.15.0`'s side effect. The guard
   was rejecting `--prior` renders at a high rate, and this release set out to fix that rate.

**The point of the effort, stated plainly:** `v0.13.0` through `v0.15.0` was a deliberate campaign to
make the report a document that cannot state a number the binary did not compute. That is the
through-line and it largely succeeded. `v0.16.0` was the first attempt to pay down the cost of that
guarantee, and it is where the arc stalls.

## What `v0.16.0` was supposed to be, and what happened

The doc is `docs/design/2026-07-27-month-over-month-deltas.md`. Its thesis: the guard rejects
`--prior` renders because the templates ask the model for a Month over Month comparison, hand it two
absolute totals, and forbid it from subtracting. So the only sentence left to write is an invented
round number. Fix: compute the comparison in Rust as display-rounded strings, license them like every
other figure.

Seven phases were planned. **Phase 0 was a zero-code gate whose only job was to prove the rejected
tokens were comparison figures. They were not.** The plan's own STOP condition fired and phases 3, 5
and 6 were never built.

What Phase 0 established, from the logs plus the literal render invocations:

| configuration | the doc claimed | actual |
|---|---|---|
| with `--prior` | 1 passed, 3 rejected (75%) | 1 passed, 4 rejected (80%) |
| without `--prior` | 3 passed, 2 rejected (40%) | 2 passed, 1 rejected (33%) |
| HTML with `--prior` | 0 passed, 3 rejected | unchanged, 3 of 3 |

The doc also counted a render that tripped the reconcile-identity guard on a deliberately corrupted
export, before the LLM was ever called. It does not belong in the rate.

Five rejections occurred across the window's renders: four with `--prior`, one without. **Zero of
the four `--prior` rejections is a confirmed comparison figure.** Of those four, one is readable in
full and is a threshold about the current period ("above 100 sessions"), not a subtraction; the
other **three are unclassifiable**, and the reason is exactly the two defects this release fixed:
the excerpt was wrong and the artifact was discarded. The fifth rejection fired on a render with no
`--prior` at all, so it cannot be a comparison by construction.

**Unproven is not disproven.** Phase 0's own report said "zero are comparison figures"; that
overstates it. The honest reading across all five: one confirmed-not, one impossible by
construction, three unknown. Do not inherit the stronger claim.

What actually shipped in `v0.16.0`:

- `excerpt_at` quotes the span the guard rejected. The old code scanned the whole document for the
  first substring match, which is why token `500` quoted a line containing the licensed `$1,500.08`.
- Citations grouped per token and capped. One real rejection shape went from a 6,667-character error
  to 266.
- A rejected render persists to `xdg_data_dir()/clyde/rejected/` and the error names the path.
- `or KPI deltas` deleted from `report-html.pmt`. A delta is a subtraction and the same clause forbids
  subtracting, so it contradicted itself regardless of the rejection data.
- `permit` settings discovery bounded (see defect 5 below for the residual).

**The rejection rate is unchanged and was never measured after the change.**

### The instruments were validated live

The first `--prior` render on the installed `v0.16.0` was rejected, and the diagnostics worked:

```
report failed: the rejected html render was written to
~/.local/share/clyde/rejected/2026-07-28-060821-html.html for inspection:
html rendering wrote `class="stroke2"` on <polyline> inside an <svg> chart subtree,
and that value is not one the binary computed
```

Error names the path, artifact is on disk, nothing was written to the output path, and it fired
through the **geometry** guard rather than the prose guard, which is the "covers all three HTML
guards" requirement.

It also surfaced a rejection class neither theory predicted. The template says, in one sentence, that
no attribute inside the `<svg>` may contain a digit and that this "covers `class` names (use
digit-free ones)". The model obeyed the hard half (it moved stroke-width into a CSS rule,
`.trend .stroke2{stroke-width:2.4}`) and broke the easy half in the same breath. **Not a false
positive.** The guard has now been correct on every observed firing, and no false positive has been
demonstrated.

## Open defects

When this doc was first drafted, none of these had a GitHub issue -- they lived in doc prose, which
is where work goes to be forgotten. **All seven are now filed, in row order:
[#65](https://github.com/tatari-tv/clyde/issues/65) through
[#71](https://github.com/tatari-tv/clyde/issues/71).** Fixes for rows 1 through 5 are open in
[#73](https://github.com/tatari-tv/clyde/pull/73); the sweep for row 6 rides this doc's own PR
([#72](https://github.com/tatari-tv/clyde/pull/72)); row 7 is the measurement, which runs on the
binary that ships the fixes.

Every line below was verified against the code on 2026-07-28.

| # | Defect | Location | Why it matters | Shape |
|---|---|---|---|---|
| 1 | `--prior` and `--llm` absent from `render::run`'s INFO line | `report/src/render.rs:43` | This line is the only record of a render's configuration. Establishing which of nine renders used `--prior` required digging literal shell commands out of an agent transcript. Violates the function-level logging rule: a load-bearing parameter is unlogged | targeted fix |
| 2 | `otto ci` test task is fail-fast | `.otto.yml:72` (`cargo test --workspace`) | Cargo runs crates in order and stops at the first failure, so an early-crate failure silently skips every later crate. One broken `permit` test meant the `report` crate's ~500 tests never executed under `otto ci` at all, and the run read as "one known failure" rather than "500 tests never ran" | targeted fix (`--no-fail-fast`) |
| 3 | Three `round_cents` copies, already diverged | `report/src/report.rs:243`, `report/src/reconcile.rs:151`, `report/src/merge.rs:274` | Not cosmetic. The `merge.rs` copy omits the negative-zero normalization the other two have, so a merged artifact can serialize `"spend-usd": -0.0` where the other paths cannot. `report.rs`'s own comment documents that as a real shipped bug. A `cents` module already exists to own it | targeted fix |
| 4 | Digit-free chart-class rule is a parenthetical | `report/templates/report-html.pmt:121-123` | Confirmed live cause of a `v0.16.0` rejection. The constraint is real and correct but buried mid-bullet in a dense list; naming an explicit licensed class token would be harder to miss than "use digit-free ones" | prompt edit, measure after |
| 5 | `permit` boundary uses exact `PathBuf` equality | `permit/src/settings/parser.rs:138` | Correct where `temp_dir()` is `/tmp` and the walk hits `/tmp` exactly. A symlinked or trailing-slash `$TMPDIR` naming the same directory would not match, and the boundary silently stops applying | targeted fix (canonicalize) |
| 6 | Operator-local paths in older docs | `docs/shakedown-*.md`, several `docs/design/*.md`, fixtures in `clyde/tests/` | This repo is PUBLIC and a real data leak was purged from history at cost during the `v0.15.0` build. CodeRabbit flagged the pattern on [#64](https://github.com/tatari-tv/clyde/pull/64); only the two files that PR touched were redacted. Absolute `/home/user` paths, project slugs and session UUIDs remain elsewhere | mechanical sweep |
| 7 | The `--prior` rejection rate, ~80% | `report/src/quotable.rs`, `report/src/claim.rs`, both templates | The actual problem. Untouched by `v0.16.0` and unmeasured after it. The evidence points at round-number and threshold invention in the CURRENT-period narrative (daily session-count claims, repo-spend superlatives), occurring with and without `--prior`, which is a different defect from the one the doc was written for | **needs a design doc** |

Also parked, from the `v0.16.0` doc's own Non-Goals and worth knowing before you re-litigate any of
them:

- **Loosening the guard for bare small integers.** Permanently excluded. At real scale `14` is
  already licensed several times over, and "above 100 sessions" is precisely the bare small integer
  such an exemption would legalize.
- **Auto-retry on rejection.** Each retry is a full paid call over ~940KB and it hides the rate.
- **A targeted repair turn (option E).** Disqualified on fail-closed grounds: telling the model to
  rewrite a sentence "using only licensed figures" points it at the guard's weakest axis, since
  `foreign_figures` checks token membership and not claim semantics. Its likeliest output is a figure
  that is globally licensed and semantically wrong, which PASSES. That trades a loud correct
  rejection for a silent wrong number in a finance-facing document.
- **A mechanical backstop for word multipliers** ("nearly triple"). `multiplier_pattern` requires a
  digit-led token ending in `x`.
- **No transport seam** in `markdown_from_context` / `html_from_context`. They resolve their transport
  internally, so a true end-to-end guard-rejection test is unreachable without refactoring the hot
  render path. `v0.16.0` covered the contract by composition instead: one test proves a rejection is
  an `Err`, another proves an `Err` from generation writes nothing, a third proves `run` is still
  wired to the helper that guarantees the ordering.

## Endgame: how this arc ends

The arc ends in two PRs, one measurement session, and at most one short doc. It does not end in
another seven-phase plan; the process notes below explain why.

"Done" for the whole arc: the guard's guarantee intact (nothing loosened), all seven defects closed
as GitHub issues, and the `--prior` rejection rate measured on the shipped binary and either reduced
by evidence-driven fixes or accepted with the number written into this doc. The purpose of the
campaign, a report that cannot state a number the binary did not compute, already holds. What
remains is closing the cost ledger and the defect table.

### Step 1: file the seven issues

Mechanical. Each defect-table row becomes an issue linking back to this doc. This is what makes the
ending visible: the arc is over when the issues are closed, not when a doc says so.

Done: [#65](https://github.com/tatari-tv/clyde/issues/65) through
[#71](https://github.com/tatari-tv/clyde/issues/71), one per defect-table row in order.

### Step 2: redaction sweep PR (defect 6)

Highest urgency because the repo is public and independent of everything else, so it goes first.
The sweep as measured on 2026-07-28: 14 files carry operator-local strings (11 under `docs/`,
3 fixtures in `clyde/tests/`: `collect.rs`, `export.rs`, `search.rs`), 29 occurrences of the
username plus assorted absolute paths, project slugs and session UUIDs. Replace with neutral
placeholders; the test fixtures will need their assertions updated in the same PR. One mechanical
PR, no design doc.

Done: this doc rides that PR ([#72](https://github.com/tatari-tv/clyde/pull/72)).

### Step 3: cleanup PR, no design doc (defects 1, 2, 3, 4, 5)

All five are targeted fixes under the triage rule. One PR, one patch release.

- **#1** add `prior` and `llm` to the `render::run` INFO line (`report/src/render.rs:43`). This is
  also the instrument for Step 4: the rate measurement needs each render's configuration in the log
  instead of in an agent transcript.
- **#2** `cargo test --workspace --no-fail-fast` in `.otto.yml:72`.
- **#3** one `round_cents` (with the negative-zero normalization) in the existing `cents` module,
  all three call sites pointed at it. `reconcile.rs:149`'s comment documents the divergence as
  deliberate policy ("every dollar choke point re-normalizes independently rather than sharing a
  public helper"); that policy is what let `merge.rs` drift, so the comment dies with the fix.
  Regression test: a merged zero-spend artifact serializes `"spend-usd": 0.0`, not `-0.0`.
- **#4** promote the digit-free chart-class rule out of the parenthetical in
  `report/templates/report-html.pmt:121-123` into its own bullet, and name a licensed example class
  token. Measured in Step 4, not on faith.
- **#5** canonicalize both sides of the boundary comparison in `discover_settings_local`
  (`permit/src/settings/parser.rs:120-138`).

Open: [#73](https://github.com/tatari-tv/clyde/pull/73), carrying the patch bump.

### Step 4: measure (zero code; the Phase 0 lesson applied as method)

On the released binary, run a fixed batch of renders per configuration: with and without `--prior`,
markdown and HTML, through the claude CLI transport (cheap by design; that was `v0.14.0`'s point).
Every rejection now persists to `~/.local/share/clyde/rejected/` with an accurate excerpt, so each
one can be classified from its artifact into a claim shape: threshold ("above 100 sessions"), daily
session count, date-range superlative, chart geometry. Output: the real rate per configuration and
a taxonomy with counts. Today the evidence base is one persisted artifact; this step builds the
evidence the `v0.16.0` doc never had. It also delivers #4's "measure after" for free.

### Step 5: pick the ending the evidence names (defect 7)

Three admissible endings, decision rule attached:

- **(a) The rate is already tolerable.** `v0.16.0`'s legibility work and Step 3's prompt edit were
  never measured; the 80% figure predates both. If the measured rate is low, write it here, close
  #7, arc over.
- **(b) One or two claim shapes dominate AND their figures are computable from data already in the
  artifact.** Then the fix is the `v0.15.0` pattern in miniature: compute those figures in Rust and
  license them, or delete the template demand that elicits them (the same shape as the shipped
  `or KPI deltas` deletion). The template currently demands exactly the observed elicitors:
  "sustained daily work or a few expensive spikes" (`report.pmt:253`) and "which date ranges carried
  the most work" (`report.pmt:254-257`), while `aggregates.by-day` already holds the data to license
  busiest-range and per-day figures. Targeted fix if it is one template edit plus one licensed set;
  a short doc capped at two or three phases if it is more. Remeasure after, then close.
- **(c) Invention is diffuse across many shapes.** Then the honest ending is accepting the rate as
  the price of the guarantee: every observed firing has been correct, rejection is loud, the
  artifact persists, and a re-run costs subscription tokens, not API dollars. Write the accepted
  number here, close #7 as by-design.

Not admissible: loosening the guard for bare small integers, auto-retry, the repair turn. The
Non-Goals above parked each with reasoning; they stay parked.

### Step 6: close the books

Write the measured rate and the chosen ending into this doc, flip any doc statuses that shipped,
confirm all seven issues are closed and no branch is dangling. The arc is over when this doc's
defect table has zero open rows.

## Process notes for whoever picks this up

- **`v0.16.0` was over-scoped and the numbers show it.** A seven-phase scaffold was built around a
  hypothesis that an hour of log reading falsified. Phase 0 existed to test it and did its job, but
  it should have been the whole doc: spike first, then decide what to write. The cheap version was
  read the logs, fix `excerpt`, add persistence because both are right regardless, ship. Two phases,
  no design doc.
- **A five-pass design doc and a two-model review panel both converged on the wrong root cause.**
  Nine log lines corrected them. When a doc's premise is an empirical claim about observed behavior,
  test the claim before scoping phases around it. This is the single most transferable lesson in the
  window.
- **Do not trust an agent's measurement without checking the tool it used.** A 41-occurrence token
  count in this session was wrong because it came from a naive `rg '[0-9]+'` rather than the crate's
  real `numeric_pattern()` (`report/src/quotable.rs:513`), whose FIRST alternative is
  `\d{4}-\d{2}-\d{2}`, so a full ISO date is consumed whole as one token. The true count was 1.
- **`bump --tag-only` fails on this machine** with "requires a clean working tree" because of
  sandbox-phantom untracked entries, even with no tracked modifications. Never clean those. Verify
  `HEAD == origin/main`, then create the annotated tag by hand and push it by explicit name.
- **`gh pr create` is hook-denied on this repo** without a release-intent line in the body:
  `Release: rides this PR (vX.Y.Z)` or `Release: none -- <why>`. Decide before composing the body.
- **The branch name is the source of truth for the PR title**, enforced by a hook. `v0.16.0`'s branch
  started as `compute-month-over-month-deltas` and had to be renamed to
  `make-guard-rejections-legible`, because the original described work that was deliberately not
  done. Rename before a PR exists; never after.

## Where the artifacts are

- `docs/design/2026-07-27-month-over-month-deltas.md` (status, criteria verdicts, the Phase 0 STOP
  entry, and the post-STOP re-scope decision)
- `docs/design/2026-07-27-month-over-month-deltas-implementation-notes.md` (per-phase notes plus an
  orchestration section recording deviations)
- `docs/design/2026-07-27-render-guard-rejection-rate.md` (the original handoff brief)
- `docs/design/2026-07-26-report-story-fidelity.md` (the guard itself: Phase 8 `--prior`, Phase 10
  quotable facts, Phase 13 render eval)
- `~/.local/share/clyde/rejected/` (persisted rejected renders; outside the repo, gitignored)
