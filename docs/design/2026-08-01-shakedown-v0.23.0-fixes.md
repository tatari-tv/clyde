# Design Document: Shakedown v0.23.0 Fixes

**Author:** Scott Idler
**Date:** 2026-08-01
**Status:** Implemented. All five phases landed, one commit each, `otto ci` green on every one.
Every acceptance criterion re-verified against the live catalog after the final phase; the observed
numbers are recorded inline below. Implementation notes:
`docs/design/2026-08-01-shakedown-v0.23.0-fixes-implementation-notes.md`.
**Review Passes Completed:** 5/5, plus one cross-model review panel (Gemini Architect + Codex Staff
Engineer). Consensus closed on 10 of 11 findings; the 11th (the export contract) was escalated to
Scott, who directed it be folded in WITH the major schema bump.

## Summary

`docs/shakedown-v0.23.0.md` filed three findings against v0.23.0: an operator scope override that
silently no-ops on a normal enrich run (MAJOR), a `doctor` count that reads as a decision count and is
not one (MEDIUM), and an eyre `Location:` footer on every user-facing error (cosmetic). This doc is
the execution plan for all three plus a fourth the review panel surfaced, one commit per phase.

Three of the four are instances of something wider than the line item, and every widening was
measured, not guessed:

- F2 is not specific to `host-refused`. `probe-refused` reads 326 on the live catalog with shipped
  config while the true decision count is 0, and `Basis`, the enum built to prevent exactly this, has
  zero production consumers.
- F3 is not specific to the footer. The binary has three different error renderings, and this repo
  already decided which one is right for four of its subcommands and never brought `main` along.
- **P4, found by the panel:** the export contract re-derives `scope` from the cwd alone
  (`sessions/src/db/query.rs:288`), so no routing decision reaches the wire. 31 rows already export a
  scope contradicting the catalog, and F1's fix would have stopped at that boundary. Folded in at
  Scott's direction, carrying a major `schema-version` bump to 2.

Every fix is the in-house mechanism already in the tree, applied where it was missed.

## Problem Statement

### Background

- v0.23.0 (PR #84, https://github.com/tatari-tv/clyde/pull/84) shipped
  `docs/design/2026-07-31-attribution-and-routing.md`: git-origin attribution, the `work-remote-hosts`
  allowlist, the recorded conclusive-negative probe, and `clyde session scope` as the operator escape
  hatch when a routing decision is wrong.
- The shakedown exercised 24 commands against a freshly installed `v0.23.0` (`21c3a32`). 21 passed.
- The runbook is a marquee post and it is already published:
  <https://marquee.internal.tatari.dev/p/~scott-idler/claude-usage-report-pipeline-runbook>. Its
  operator section advertises `clyde session scope --set work` as the remedy for a wrong routing
  decision. That advice does not work.
- `main` is `21c3a32`. No code has landed since the tag.

### Problem

**P1. An operator override does not re-offer the row it exists to rescue.**

`Db::set_scope_override` (`sessions/src/db/routing.rs:158`) writes `scope_override`,
`scope_override_reason`, `scope_override_by`, `scope_override_at`, and nothing else.
`Db::enrich_candidates` (`sessions/src/db/enrich.rs:345`) excludes any row matching
`enrich_status = 'skipped-personal' AND scope_version >= 3`. Every wrongly-personal session carries
exactly that state, because the gate that skipped it wrote both columns.

So the classifier honors the override (`session/src/scope.rs:193`, step 0, beats every rule) and the
row never reaches the classifier.

Reproduced first-hand on a copy of the live catalog, session `00849874`:

```
$ clyde session scope --session 00849874 --set work --reason "design-doc F1 repro"
✓ 00849874 scope override set to work by saidler@desk

$ clyde session enrich --dry-run --dormant-after 1h            # normal run
  00849874 -> ABSENT from details

$ clyde session enrich --dry-run --all --dormant-after 1h
  {"session-id":"00849874...","scope":"work","would-send":true,"status":"would-enrich"}
```

Forcing `personal` is unaffected: that row stays a candidate and the gate then skips it. Only the
direction an operator needs is broken.

`--clear` has the same blockage in mirror image, and the shakedown did not find it. Force a row
`personal` -> a normal pass records `skipped-personal` + `scope_version = 3` -> `--clear` restores
rule-based classification, which may now say work, and the row is excluded from ever being asked.

`--all` is the only workaround today, and it sets `force` (`sessions/src/enrich.rs:101`,
`force = opts.all || opts.only.is_some()`), which makes `overwrite_tags = true` unconditionally
(`sessions/src/enrich.rs:305`). The workaround for one wrong row re-enriches the whole catalog and
clobbers every manual tag.

**P2. `doctor`'s routing lines count SQL conditions and are read as decisions.**

`Db::routing_summary` (`sessions/src/db/routing.rs:293`) answers each routing line with its own
`COUNT(*)` over a single column. The classifier does not work that way: `classify_with_evidence`
returns at the cwd work-anchor (`session/src/scope.rs:213`) or the non-work repos-anchor
(`session/src/scope.rs:225`) BEFORE the git-origin branch (`session/src/scope.rs:278`) that is the
only place `repo_probe` or `host_confers_work` is ever read. A row can satisfy the SQL condition
while the condition decided nothing.

Measured on the live catalog, `main`, real config, right now:

```
doctor:  probe-refused  326
reality: of those 326, rows attributed by git-origin at all:                 0
         ... carrying a work slug:                                           0
         ... whose cwd also lacks a repos/<org> anchor (the only reachable)  0
```

`probe-refused` reads 326 and the number of decisions a probe refusal made is 0. Not one of those
rows is even git-origin attributed, so the classifier never reaches the branch. The shakedown filed
this against `host-refused` and had to plant a bogus allowlist to see 1451; `probe-refused` shows the
same defect at 326 on the maintainer's own catalog with the shipped config.

The root cause is not the SQL. `Basis` (`session/src/scope.rs:83`) exists for exactly this, and its
own doc comment says so:

> Its own variant rather than a `GitOrigin` personal, because Phase 8 has to count these separately:
> at 3am an operator must be able to tell "the remote says personal" from "clyde refused to trust the
> remote", and one timestamp cannot.

Phase 8 is `doctor`. `Basis` has **zero production consumers**: every read of `Decision::basis` in the
tree is one of ten assertions in `session/src/scope/tests.rs`. (`rg '\.basis'` also hits
`report/src/render/document.rs:143`, which is an unrelated pricing-basis field.) The enum that was
supposed to make these counts decision-accurate was built, documented, tested, and never wired to the
thing it was built for. `routing_summary` has no tests at all.

**P3. `clyde` has three error renderings, and the default one leaks source locations.**

```
$ clyde session scope --session deadbeef-0000 --set work
Error: no session matches "deadbeef-0000"

Location:
    clyde/src/main.rs:828:15
```

The messages are good. The source location is noise for a CLI user, it appears on every error path
including pure argument-validation ones, and the line number moves with unrelated edits.

The real shape is wider than the footer, and this repo already decided the question once:

| path | rendering | reaches |
|---|---|---|
| `dispatch_tool` (`clyde/src/main.rs:162`) | `{e:#}`, or `{e:?}` under `--log-level debug\|trace` | `report`, `cost`, `permit`, `efficiency` |
| the `update` arm (`clyde/src/main.rs:119`) | `error: {e}` | `update check\|install\|revert` |
| `fn main() -> Result<()>` (`clyde/src/main.rs:43`) | eyre's own `Error: {e:?}`, WITH `Location:` | every other subcommand |

`dispatch_tool`'s doc comment is explicit that its form is the intended one:

> At the default (info or lower verbosity) we print `{e:#}` -- the full eyre **cause chain** with NO
> `Location:`/backtrace -- so a normal failure reads as a clean, chained message instead of leaking
> an internal `report/src/config.rs:NNN` source location. Only when `--log-level debug` (or trace) is
> set do we print `{e:?}` (Debug, with the location capture) for diagnosis.

So the fix is not to invent a suppression mechanism. It is that the decision was implemented on four
subcommands and `main` was never brought along.

### Goals

- `scope --set` and `scope --clear` take effect on the next ORDINARY `clyde session enrich`, with no
  `--all` and no manual-tag loss.
- Every `doctor` routing line that names a refusal counts decisions the classifier actually made,
  by construction, so the two can never drift again.
- No `Location:` block on clyde's DEFAULT error rendering, and one renderer for the whole binary.
  `--log-level debug|trace` keeps the location capture, which is the point of the escape hatch.
- `clyde session export` emits the scope that was actually decided, so an override reaches downstream
  consumers instead of dying at the wire boundary.
- The published runbook stops advertising a remedy that does not work.

### Non-Goals

- **Renaming `clyde doctor` vs `clyde session doctor`.** The shakedown raises it as an observation,
  not a finding. It is a user-visible rename of two shipped commands and it is not what was asked for.
  Revisit condition: a second operator confuses them in a real incident.
- **An `enrich` window flag.** Same reason: filed as an observation. `--dormant-after 1h` is the
  documented lever and it works. Revisit condition: someone needs coverage that `--dormant-after`
  cannot express.
- **Publishing the runbook.** The marquee post is outward-facing and is Scott's to send. This doc
  updates the diff file in the repo; the publish is a rollout step, not a phase.
- Re-litigating the cwd-anchor-outranks-the-remote precedence. That was decided in
  `2026-07-31-attribution-and-routing.md` and `cwd_anchor_outranks_the_remote_in_both_directions`
  asserts it. P2 fixes the REPORTING of that precedence, not the precedence.

**P4. The export contract re-derives `scope` from the cwd alone, so no routing decision reaches it.**

Found by the review panel, verified here. `build_export_record` (`sessions/src/db/query.rs:288`)
computes:

```rust
let scope = session::classify(cwd_path).as_str().to_string();
```

`session::classify` (`session/src/scope.rs:148`) is the LEGACY cwd-only rule: work iff the cwd has a
`repos/<work-org>` anchor, personal otherwise. It ignores `scope_override`, git-origin attribution,
and the touch set. The inline comment records this as deliberate ("never the stored NULLable column,
finding S1"), and `docs/session-export-contract.md:104` documents it as contract behavior.

Measured on the live catalog:

```
stored scope='work'                                      897
stored scope='personal'                                 1064
stored scope NULL (never processed by the gate)          223
DIVERGENT: stored 'work', exported 'personal'             31
```

31 rows already export a scope that contradicts the catalog. P1 makes it strictly worse: every
session an operator forces to `work` and enriches would export `personal`, the exact opposite of what
the operator asked for. P1's effect would stop at the export boundary.

## Proposed Solution

### Overview

| Problem | Fix | Blast radius |
|---|---|---|
| P1 | the override write also sets `scope_version = NULL` | 2 SQL statements |
| P2 | `routing_summary` tallies `Basis` from the classifier, `doctor` prints decisions and conditions separately | `RoutingSummary`, `doctor.rs` |
| P3 | one shared error renderer, `dispatch_tool`'s existing form, used by `main` too | `clyde/src/main.rs` |
| P4 | export reads `scope_override` -> stored `scope` -> `classify(cwd)`, and `schema-version` goes to 2 | export contract, 5 goldens, external consumers |

### Architecture

**P1 copies the mechanism this codebase already documents.** `Db::record_enrich_skip`
(`sessions/src/db/enrich.rs:256`) writes `scope_version = NULL` for exactly this purpose, and says so:

> Leaving the column NULL is what keeps such a row a candidate for the next pass (see
> `Db::enrich_candidates`'s predicate).

An operator override IS new evidence, arriving after the recorded decision was made. NULLing
`scope_version` states the truth: this row's stored scope decision no longer describes it. No new
predicate, no new flag, no new column.

Proven before writing it down. On the same catalog copy, with the row's `scope_version` set to NULL
and nothing else changed, a NORMAL dry-run and an `--all` dry-run return byte-identical verdicts:

```
normal:  {"session-id":"00849874...","scope":"work","would-send":true,"redaction-count":1,"payload-bytes":195022,"status":"would-enrich"}
--all:   {"session-id":"00849874...","scope":"work","would-send":true,"redaction-count":1,"payload-bytes":195022,"status":"would-enrich"}
```

An already-enriched row is NOT re-offered by this change, which is the property that keeps it from
becoming a re-enrich storm. The second candidacy clause
(`enriched_at IS NULL OR modified > enriched_modified OR prompt_version < ?`) still excludes it, and
`scope_version` is not one of its disjuncts.

**`--clear` NULLs `scope_version` only when an override was actually present.** The panel caught a
real hole in the first draft: `clear_scope_override` (`sessions/src/db/routing.rs:171`) updates any
existing session, override or not, and its `Ok(n > 0)` means "the session exists". Adding an
unconditional `scope_version = NULL` would turn `scope --clear` on a session with NO override into a
hidden "re-offer this row" command. There are 1018 `skipped-personal` rows with `scope_version >= 3`
on the live catalog and 0 overrides, so every one of them was reachable by a nominal no-op.

The write becomes conditional in SQL, which preserves the existing row-existence return semantics:

```sql
UPDATE sessions SET scope_override = NULL, scope_override_reason = NULL,
       scope_override_by = NULL, scope_override_at = NULL,
       scope_version = CASE WHEN scope_override IS NOT NULL THEN NULL ELSE scope_version END
 WHERE session_id = ?1
```

Adding `AND scope_override IS NOT NULL` to the `WHERE` instead was considered and rejected: it would
flip `Ok(n > 0)` from "session exists" to "an override existed", changing what the CLI's
"no session matches" path means.

Known limitation, deliberately not fixed: a row at the `attempts` cap stays excluded by
`s.attempts < ?1` no matter what an operator overrides it to. That is a different guard with a
different purpose (stop retrying a session that keeps failing), and a row at the cap already reached
the transport, so it was already classified work. It is not the F1 population. Overriding scope is
not the right lever for a stuck retry budget, and quietly resetting `attempts` from a scope command
would be the kind of hidden side effect this fix exists to remove.

**P2 counts what the classifier decided, not what SQL can see.** `routing_summary` reads every row in
one pass, runs `session::scope::classify_with_evidence` per row, and tallies the returned `Basis`.
Each count is then a decision count by construction and cannot drift from the classifier, because it
IS the classifier.

Doctor prints two groups, because they are two different kinds of claim:

```
  routing decisions:   (what decided each row; sums to the row count)
    override        <n>  operator-set; read them with `session scope --list`
    cwd-anchor      <n>  the cwd's repos/<org> anchor decided
    git-origin      <n>  the remote's slug decided
    touch-set       <n>  the set of repos the session edited decided
    host-refused    <n>  a work slug REFUSED by a non-allowlisted host; add it to work-remote-hosts
    probe-refused   <n>  a work slug REFUSED by a conclusive negative; `session reindex --clear-probe --session <id>`
  routing conditions:  (facts present on rows; these did NOT decide anything on their own)
    probe-recorded  <n>  rows carrying a conclusive negative; clear a stale one with `session reindex --clear-probe --session <id>`
    host-unknown    <n>  indexed before v13; keeps pre-v13 authority until a reprobe records a host
    anchor/remote   <n>  cwd and remote disagree: an ordinary fork, or a personal clone under the work org
    blocked         <n>  cwd resolves to a blocked root ($HOME); correct, and never attributed
    outside-root    <n>  git found a repo that does not contain the cwd; nothing is recorded for these
    indeterminate   <n>  git answered NOTHING; check `safe.directory` and that git is installed
```

The live-probe trio (`blocked`, `outside-root`, `indeterminate`) moves under `conditions` unchanged,
including its sampling notice. That is where it always belonged: it is a statement about the machine
now, not about what decided a row, and it is measured by re-probing rather than read from the
catalog.

`resolved by:` (`by_source`) stays exactly as-is and is neither group. It answers "which rule
ATTRIBUTED the repo", a different question from "which rule set the scope".

Print order follows the classifier's own precedence, so the list reads top-down the way a decision is
actually made: override, cwd-anchor, git-origin, touch-set, then the two refusals with `host-refused`
before `probe-refused` because the host check (`session/src/scope.rs:288`) runs before the probe
check (`session/src/scope.rs:296`).

The decisions group summing to the catalog row count is the invariant that makes this
self-checking, and it holds because `classify_with_evidence` returns exactly one `Basis` on every
path (the tail returns `Basis::TouchSet`, `session/src/scope.rs:342`).

Tally into a fixed-order array rather than a `HashMap`, so print order is stable. **The
variant-to-index and variant-to-label mappings must both be exhaustive `match`es, never
`basis as usize`.** The panel caught this: `Basis` is a plain fieldless enum
(`session/src/scope.rs:82`), so an `as usize` cast keeps compiling when a seventh variant is added
and then either panics on index or silently drops that variant from the printed list. An exhaustive
`match` is what actually makes a new variant a compile error, and the array length is a consequence
of that match, not the guard itself.

Free cross-check available on day one: the `override` basis count must equal the old
`scope_override IS NOT NULL` SQL count. `Override` is step 0 and beats every rule, so that one
condition WAS decision-accurate. If the two disagree, the tally is wrong.

**Doctor uses the REAL `SshResolver`, and the old "doctor must not spawn ssh" constraint is dropped.**
This was drafted the other way (a null resolver, literal comparison only) and the review panel killed
it: a null resolver is NOT the gate's input, so a row with `repo_host = 'github-work'` resolving to
`github.com` would read `GitOrigin` at the gate and `HostRefused` in doctor. That reintroduces the
exact defect P2 exists to remove, one layer down.

The constraint it was protecting against does not exist. `HostPolicy::confers_work`
(`common/src/repo/host.rs:126`) short-circuits on a literal allowlist match before resolving, and
`resolve` memoizes per host, caching failures too. So the spawn bound is **distinct non-literal
hosts per run**, not sessions. Measured on the live catalog: exactly **one** distinct `repo_host`
value, `github.com` (1283 rows), which is a literal match and spawns nothing. `ssh -G` touches no
network. Doctor already spawns up to 64 `git` subprocesses per run, so even a machine with a handful
of aliases pays less here than it already pays there.

Result: `doctor` reports the decisions the real enrich gate would make on this machine, with no
approximation and no caveat. The `host-refused` remedy line drops its ssh-alias disclaimer, because
aliases are now resolved in doctor exactly as they are at the gate.

Cost: 2184 rows, 379 KB of `outcome_json` total. Doctor already spawns up to `REPROBE_SAMPLE_MAX`
(64, `clyde/src/doctor.rs:219`) `git` subprocesses on every run. One table scan and 380 KB of JSON is
not the expensive part of this command.

A malformed `outcome_json` on one row must WARN and count that row as evidence-absent, never abort
the scan. `scope_evidence` already takes that position (`sessions/src/db/enrich.rs:85`, "a malformed
blob is warned about rather than propagated, mirroring `Db::repos_touched`, so one corrupt row cannot
abort an enrich pass") and the batch form inherits it. `doctor` is the command an operator runs when
something is already broken; it is the last place that may die on one bad row.

**P4 exports the decision that was actually made, in the classifier's own precedence.** The derived
field becomes, first match wins:

```
scope_override  ??  stored `scope` column  ??  classify(cwd)
```

Each step is the classifier's own order truncated to what a cheap paged endpoint can know. Step 1 is
`classify_with_evidence`'s step 0 verbatim (`session/src/scope.rs:193`), so an override reaches the
wire even on a row the gate has not processed yet. Step 2 is the decision the gate recorded. Step 3
preserves the contract's "never null" guarantee for the 223 rows that have neither.

`EXPORT_COLS` (`sessions/src/db/query.rs:29`) does NOT currently select `scope`. Phase 4 appends
`s.scope` and `s.scope_override` to the END of the list, so no existing `map_export_raw` index shifts.

Rejected: running the full `classify_with_evidence` at export time. It needs five more columns plus
an `outcome_json` parse per row on the bulk paged endpoint whose whole point is being cheap, and it
would make export a THIRD site re-implementing the routing decision. Reading what the gate already
decided is the decomposition that cannot drift.

**The bump to `schema-version: 2` is Scott's explicit call, taken 2026-08-01.** My recommendation was
that no bump was required: the field's type and vocabulary are unchanged (`"work" | "personal"`,
never null), so under `docs/session-export-contract.md:251-259` this reads as a within-major change.
Scott overrode that and directed the major bump. It is the safer reading: the field's MEANING changes
for 31 rows today and for every overridden row after, and a consumer pinned to v1 that silently
starts receiving differently-derived values has no way to notice. The bump is what tells them.

**P3 extends the renderer that already exists, to one renderer for the whole binary.** Extract
`dispatch_tool`'s two-branch rendering into a shared `render_error(&eyre::Report, debug: bool)`, and
route all three paths through it:

- `dispatch_tool` calls it instead of inlining the branch. No behavior change.
- the `update` arm calls it, replacing `error: {e}`.
- `main` becomes `fn main() -> ExitCode`, catching what it used to return: on `Err`, call
  `render_error` and return `ExitCode::FAILURE`.

`debug` comes from `is_debug_level` (`clyde/src/main.rs:174`), which `run` already computes the same
way. Errors raised before the log level is known (a clap `from_arg_matches` failure) render with
`debug = false`, which is the correct default for a usage error.

Result: one renderer, one meaning, and the `--log-level debug` escape hatch keeps working on every
subcommand instead of four. `run_resume_action`'s red `✗` lines
(`clyde/src/main.rs:515`) stay as they are: those are deliberate user-facing messages on a non-error
control path, not propagated `Report`s.

The output change is real and intended: `Error: <msg>\n\nLocation:\n    <file>:<line>` becomes
`<msg>`, matching what `report`/`cost`/`permit` have always printed. Dropping eyre's `Error: `
prefix is the parity choice; keeping it would leave `clyde session ...` and `clyde report ...`
rendering the same failure two different ways, which is the defect being fixed.

### Data Model

No schema change. `scope_version` is already nullable (schema v12) and NULL already means
"provisional, re-offer this row".

`RoutingSummary` (`sessions/src/db/routing.rs:236`) changes shape:

- REMOVED: `host_refused: usize` (the SQL count that lies, and has no honest condition reading:
  "rows whose host is not allowlisted" is only interesting when it refused something)
- RENAMED: `probe_refused` -> `probe_recorded`. Same query (`repo_probe IS NOT NULL`), same number,
  a name that says what it counts. It moves to the conditions group.
- ADDED: `by_basis: [usize; 6]` indexed by `Basis`, plus the accessor that pairs each count with its
  variant for printing
- UNCHANGED: `host_unknown`, `anchor_remote_disagreement`, `reprobe_candidates`, `by_source`

Only consumer is `clyde/src/doctor.rs:71`. `Basis` is already re-exported as `session::Basis`
(`session/src/lib.rs:24`), so no new export is needed.

The single scan needs, per row: `cwd`, `repo`, `repo_source`, `outcome_json`, `repo_probe`,
`repo_host`, `scope_override`. That is `scope_evidence`'s four columns
(`sessions/src/db/enrich.rs:100`) plus the three the classifier takes positionally. `scope_evidence`
is per-session by design; Phase 3 adds the batch form rather than calling it 2184 times.

### API Design

No CLI surface changes. Same flags, same exit codes. `doctor`'s stdout gains one group header and
regroups existing lines; it stays TTY-independent and pipe-clean.

### Implementation Plan

#### Phase 1: One error renderer for the whole binary
**Model:** sonnet

- Extract `render_error(e: &eyre::Report, debug: bool)` in `clyde/src/main.rs`, carrying
  `dispatch_tool`'s existing doc-comment rationale onto it.
- `dispatch_tool` and the `update` arm call it. `main` becomes `-> ExitCode` and calls it on `Err`.
- Tests in `clyde/src/tests.rs`:
  1. `render_error` at `debug = false` on a `.context()`-chained report emits the full cause chain
     and no `"Location:"`
  2. at `debug = true` it emits the Debug form (the escape hatch still works)
- Break it to prove it bites: flip the `debug` branch and watch test 1 fail
- **Success criteria:**
  - `clyde session scope --session deadbeef-0000 --set work` prints `no session matches
    "deadbeef-0000"`, no `Location:` line, exit 1
  - the same command with `--log-level debug` still shows the Debug form
  - `otto ci` green, one commit

#### Phase 2: An override re-offers its row
**Model:** opus

- `Db::set_scope_override` adds `scope_version = NULL` to its existing UPDATE.
  `Db::clear_scope_override` adds the CONDITIONAL form above (`CASE WHEN scope_override IS NOT NULL`).
  Doc-comment both with why, citing `record_enrich_skip` as the mechanism being copied.
- `clyde/src/main.rs`: `--set personal` on a row that is ALREADY enriched warns before the write,
  naming that the transcript has already been sent and the override cannot un-send it. Same shape as
  the existing conclusive-negative warning: printed before the write, does not block it.
- Tests in `sessions/src/db/routing/tests.rs`, all five directions:
  1. `--set work` on a `skipped-personal` + `scope_version = 3` row -> it IS in `enrich_candidates`
     without `all`
  2. `--clear` on the same shape, WITH an override present -> it IS in `enrich_candidates` (the
     mirror case the shakedown missed)
  3. `--clear` on the same shape with NO override present -> `scope_version` is UNCHANGED and the row
     is NOT re-offered (the hole the panel found)
  4. `--set personal` on a plain candidate -> still a candidate, no regression
  5. an ALREADY-ENRICHED row (`enriched_at` set, current `prompt_version`) -> `--set` and `--clear`
     do NOT re-offer it
- Break it to prove it bites: drop the `scope_version = NULL` clause and watch 1 and 2 fail; make
  `--clear`'s write unconditional and watch 3 fail
- **Success criteria:**
  - the four tests above pass, and 1 and 2 fail with the clause removed
  - `otto ci` green, one commit

#### Phase 3: `doctor` counts decisions, not conditions
**Model:** opus

- `Db::routing_summary` reads every row in one scan and classifies it through
  `session::scope::classify_with_evidence`, assembling `RoutingFacts` the same way
  `sessions/src/enrich.rs:150` does. `host_confers_work` comes from `HostPolicy::new` with the real
  `SshResolver`, the same construction the gate uses, so doctor and the gate cannot disagree on an
  alias.
- Degradation rules, both mirroring what the enrich path already does, so `doctor` is never LESS
  operable than `enrich` on a damaged catalog:
  - malformed `outcome_json` -> warn, count the row as evidence-absent, keep scanning
    (`sessions/src/db/enrich.rs:85`)
  - unparseable `repo_source` -> warn and classify without the remote signal, exactly as
    `sessions/src/enrich.rs:131` does. `RepoSource::from_str` is deliberately loud
    (`common/src/repo.rs:80`), and a hand-edited or future-version value must not abort the one
    command an operator runs when things are already broken.
- Drop `host_refused`; rename `probe_refused` -> `probe_recorded` (same query, honest name); add the
  `Basis` tally. Keep `host_unknown`, `anchor_remote_disagreement`, `reprobe_candidates`, `by_source`.
- `clyde/src/doctor.rs` prints `routing decisions:` and `routing conditions:` as two groups, each line
  keeping its own remedy, decisions in classifier-precedence order. The host-refused line DROPS its
  ssh-alias disclaimer, which is no longer true. The live-probe trio moves under conditions with its
  sampling notice intact.
- Variant-to-index and variant-to-label are exhaustive `match`es, never `basis as usize`.
- Tests in `sessions/src/db/routing/tests.rs` (`routing_summary` currently has none):
  1. the basis tally sums to the catalog row count, on a seeded multi-shape catalog
  2. a row with a recorded `repo_probe` whose cwd carries a `repos/<org>` work anchor counts as
     `CwdAnchor`, NOT `ProbeRefused` (this is the defect, asserted directly)
  3. a row that genuinely reaches the refusal (git-origin, work slug, unanchored cwd, recorded probe)
     counts as `ProbeRefused`, and the same row with an allowlisted-vs-refused host flips between
     `GitOrigin` and `HostRefused`
  4. a row carrying BOTH a non-allowlisted host and a recorded probe counts as `HostRefused`, not
     `ProbeRefused`, pinning the classifier's precedence
  5. a row whose `repo_host` is an SSH alias resolving to an allowlisted host counts as `GitOrigin`,
     using an injected `HostResolver` fake (`common/src/repo/host/tests.rs:28` is the pattern). This
     is the test that would have caught the null-resolver draft.
- Break it to prove it bites: make case 2's row count as `ProbeRefused` and watch test 2 fail;
  swap the real resolver for a null one and watch test 5 fail
- **Success criteria:**
  - the basis `probe-refused` count equals the SQL-measured true count on the live catalog (both 0
    today, where `main`'s line reads 326), and `probe-recorded` still reports 326
  - the decisions group sums to the `sessions` row count
  - the basis `override` count equals `SELECT COUNT(*) FROM sessions WHERE scope_override IS NOT NULL`
  - `otto ci` green, one commit

#### Phase 4: Export honors the real classification, `schema-version` -> 2
**Model:** opus

Ordered BEFORE the docs phase so the runbook and shakedown addenda describe the finished behavior.

- `sessions/src/db/query.rs`: append `s.scope, s.scope_override` to `EXPORT_COLS` (at the END, so no
  existing index shifts), carry both onto `ExportRaw`, and replace `build_export_record`'s
  `session::classify(cwd_path)` with the three-step precedence. Rewrite the `finding S1` comment:
  the reason it avoided the stored column was NULLability, and the fallback answers that.
- `sessions/src/export.rs:75`: `EXPORT_SCHEMA_VERSION` 1 -> 2. Update the `EXPORT_SCHEMA_VERSION 1`
  reference in the `efficiency` field doc comment (`sessions/src/export.rs:181`).
- Goldens: all five `sessions/tests/fixtures/export/*.json` (`with-body`, `with-efficiency`,
  `never-enriched`, `staged-archived`, `enriched`) go to `"schema-version": 2`. Any fixture whose
  `scope` changes under the new derivation gets re-baselined, and the diff is inspected row by row
  rather than blanket-accepted.
- `sessions/tests/export.rs`: the inline envelope literal at `:119`, and the test at `:104`
  (`export_schema_version_stays_one_after_efficiency_block`). That test asserted a real historical
  fact, that the efficiency block rode additively. Rename it and keep the fact in its doc comment;
  do not delete the reasoning.
- `sessions/src/export/tests.rs:143`: keep a `schema-version: 1` parse case (it proves the
  deserializer is not version-gated and still tolerates `future-key`) and ADD a v2 case. Emitting 2
  and refusing to parse 1 are different promises.
- `docs/session-export-contract.md`: the envelope example (`:77`), the field table (`:87`), the
  `scope` row (`:104`) restated as the three-step derivation, the additive-within-major note
  (`:209`), and the compat-promise section (`:248`, `:258`). Add a short "what changed in v2" entry
  naming the scope derivation as the single breaking change.
- **Success criteria:**
  - `clyde session export --id <an overridden work session>` emits `"scope":"work"`; on `main` the
    same session emits `"scope":"personal"`
  - every emitted envelope carries `"schema-version": 2`, and no fixture still contains
    `"schema-version": 1` except the deliberate backward-parse case
  - `rg -c '"schema-version": 1' sessions/tests/fixtures/export/` returns no matching files
  - `otto ci` green, one commit

#### Phase 5: Correct the runbook diff
**Model:** sonnet

- `docs/design/2026-07-31-runbook-update-draft.md` section 3: `--set`/`--clear` now take effect on the
  next ordinary `enrich`. State it, and drop any implication that `--all` is needed.
- Section 4: replace the routing table with the decisions/conditions split, and say plainly that a
  refusal count is a count of decisions, not of rows carrying the condition.
- `docs/shakedown-v0.23.0.md`: append a resolution line per finding pointing at this doc. Do NOT
  rewrite its findings; it is a point-in-time record.
- Add a runbook section for the export contract: `schema-version` is now `2`, `scope` is the decided
  scope rather than a cwd guess, and any consumer pinned to `1` must be updated before the release
  reaches it. This is the operator-facing half of the cross-repo blast radius above.
- Swept for every place the broken remedy is advertised. Corrected by the panel, which found three
  hits the first sweep missed. Exactly one needs action:
  - `docs/design/2026-07-31-runbook-update-draft.md:59` -> CORRECT IT. It is the source of the
    published marquee post.
  - `docs/design/2026-07-31-attribution-and-routing.md:435`, `:690`, `:1100` and
    `docs/design/2026-07-31-attribution-and-routing-implementation-notes.md:271` -> LEAVE THEM ALL.
    Design docs and implementation notes are point-in-time; they described what shipped, and
    rewriting shipped history to match a later fix is how the record stops being trustworthy.
  - `clyde/src/cli.rs:138` ("Force this session's scope, beating every classification rule") -> no
    change needed. Inaccurate today, accurate the moment Phase 2 lands.
  - `README.md` does not mention `scope`. There is no `CLAUDE.md` in this repo, so there is no living
    doc carrying the claim.
- **Success criteria:**
  - the runbook draft's operator section names no `--all` workaround
  - the runbook draft's routing table is the decisions/conditions split, not the old five-row table
  - the runbook draft names `schema-version: 2` and the consumer action it forces
  - `docs/shakedown-v0.23.0.md` carries a resolution line for each of F1, F2, F3
  - `otto ci` green, one commit

## Acceptance Criteria

- [ ] **AC1.** `clyde session scope --session deadbeef-0000 --set work` prints
  `no session matches "deadbeef-0000"` with no `Location:` line and exits 1; the same command with
  `--log-level debug` still prints the Debug form including the location.
  `Observed on main:` prints `Error: no session matches "deadbeef-0000"`, then a blank line, then
  `Location:` / `    clyde/src/main.rs:828:15`. Exit 1. FAILS today, which is the point. The
  `--log-level debug` half cannot be distinguished on `main` because both paths render identically
  there; it becomes meaningful only after Phase 1.
  **PASS (verified after Phase 5).** Prints exactly `no session matches "deadbeef-0000"`, no
  `Location:` line, no `Error: ` prefix, exit 1. With `--log-level debug` the `Location:` block is
  present again, so the escape hatch works on this subcommand for the first time.
- [ ] **AC2.** On a catalog copy: `scope --set work` on a `skipped-personal` row, then
  `clyde session enrich --dry-run --dormant-after 1h` (NO `--all`) lists that session with
  `"scope":"work","would-send":true`.
  `Observed on main:` session `00849874`, override set successfully, `matched=0` in `.details` on the
  normal run; `--all` returns `"scope":"work","would-send":true`. FAILS today.
  **PASS (verified after Phase 5).** On a fresh catalog copy, `scope --set work` then
  `enrich --dry-run --dormant-after 1h` with NO `--all` returns
  `{"session-id":"00849874-...","scope":"work","would-send":true,"redaction-count":1,"payload-bytes":195022,"status":"would-enrich"}`
  -- byte-identical to the `--all` verdict the doc recorded.
- [ ] **AC3.** `clyde doctor`'s `probe-refused` line equals the number of rows that can actually
  reach the refusal branch, measured independently in SQL:

  ```sql
  SELECT COUNT(*) FROM sessions
   WHERE repo_probe IS NOT NULL AND repo_source='git-origin'
     AND repo LIKE 'tatari-tv/_%' AND repo NOT LIKE 'tatari-tv/%/%'
     AND (cwd IS NULL OR cwd NOT LIKE '%/repos/%')
     AND (repo_host IS NULL OR LOWER(repo_host) IN ('github.com'));
  ```

  Stated as an equality, not a fixed number, so the criterion survives the catalog growing a row that
  genuinely IS refused. Three couplings to code, all of which must move together or the criterion
  goes stale:
  - the `LIKE` pair mirrors `is_work_slug` (`session/src/scope.rs:462`): non-empty repo segment, no
    second slash
  - `'tatari-tv'` hardcodes `WORK_ORGS` (`session/src/scope.rs:69`), a one-entry const today
  - the `repo_host` clause encodes that a HOST refusal precedes a PROBE refusal
    (`session/src/scope.rs:288` before `:296`), so a non-allowlisted host with a probe stamp is
    `HostRefused` and must NOT be counted here. The panel caught this omission; without the clause
    the criterion double-counts the moment that shape appears. The host list must match the
    `work-remote-hosts` config in effect when the criterion is run.
  `Observed on main:` doctor's line reads `326`; the SQL measures `0`. Positive control on the same
  catalog: `1073` rows DO carry a well-formed work slug, so the `0` is a real absence and not an
  empty query. They disagree by 326, which is the finding. FAILS today.
  **PASS (verified after Phase 5).** doctor's `probe-refused` DECISION count `0` == the independent
  SQL `0`. Positive control on the same catalog: `1079` rows carry a well-formed work slug, so the
  `0` is a real absence. `probe-recorded` still reports the condition, at `345`. The catalog has grown
  since the doc was written (2184 -> 2190 rows, 326 -> 345 stamps, 1073 -> 1079 slugs), which is
  exactly why this criterion was written as an EQUALITY rather than a fixed number.
- [ ] **AC4.** `clyde doctor`'s routing-decisions counts sum to the `sessions` row count.
  `Observed on main:` NOT OBSERVABLE, and this is a forward criterion rather than evidence. There are
  no basis counts on `main` at all, so there is nothing to run; it depends on Phase 3. Asserted in
  Phase 3's test 1 against a seeded catalog, and checkable by hand after release against
  `SELECT COUNT(*) FROM sessions` (2184 today).
  **PASS (verified after Phase 5).** doctor's `(total)` line reads `2190` and
  `SELECT COUNT(*) FROM sessions` reads `2190`. The free cross-check also holds: the `override` basis
  count `1` equals `SELECT COUNT(*) FROM sessions WHERE scope_override IS NOT NULL` `1`.
- [ ] **AC5.** `clyde session export` emits `"scope":"work"` for a session whose stored `scope` is
  `work`, and every envelope carries `"schema-version": 2`. Falsifiable in SQL against the live
  catalog: the count of rows where the exported scope disagrees with
  `COALESCE(scope_override, scope, <cwd rule>)` is exactly zero.
  `Observed on main:` 31 rows have stored `scope='work'` and a cwd with no `repos/tatari-tv/` anchor,
  so `build_export_record`'s `classify(cwd)` emits `personal` for every one of them. Envelopes carry
  `"schema-version": 1` (`sessions/src/export.rs:75`). FAILS today on both halves.
  **PASS (verified after Phase 5).** The 31-row divergent population still measures 31; a sampled
  member (`03640da6`, cwd `/home/saidler`) now exports `"scope":"work"` where `main` emitted
  `"personal"`. Whole-catalog export: `1912` records, `"schema-version": 2`, and the count of rows
  whose exported scope disagrees with `COALESCE(scope_override, scope, <cwd rule>)` is **0**.
  `rg -c '"schema-version": 1' sessions/tests/fixtures/export/` returns no matching files.
- [ ] **AC6.** `otto ci` exits 0 on each of the five phase commits, and each phase is exactly one
  commit.
  `Observed on main:` `otto ci` green on `21c3a32`. Re-run per phase.
  **PASS.** Five commits, one per phase, `otto ci` exit 0 on each:
  `7d32053` (Phase 1), `b754764` (Phase 2), `d1b34d2` (Phase 3), `d80a2f2` (Phase 4), and the Phase 5
  docs commit this status flip rides in.

## Resolved Decisions

- **2026-08-01. P2 fixes the class, not just `host-refused`.** The shakedown filed `host-refused`.
  While verifying it, `probe-refused` measured 326-vs-0 on the live catalog with shipped config,
  which is the same defect at larger real magnitude. Fixing one count and leaving the other lying is
  not defensible, and the spot-fix would have to be undone by the class fix later. Traceability:
  `host-refused` is the filed finding, `probe-refused` is mine, found while confirming it. Narrowing
  to `host-refused` alone is Scott's call and is recorded as Alternative 2.
- **2026-08-01. `--clear` is in scope for P1.** Not in the shakedown. Same predicate, same blockage,
  mirror direction, and shipping the fix for one direction only would leave a second silent no-op
  behind a command the runbook advertises on the same line.
- **2026-08-01. P3 unifies the renderer instead of disabling eyre's `track-caller`.** The feature
  flag was the drafted fix and is recorded as Alternative 3. Withdrawn on review pass 4: it kills the
  location capture under `--log-level debug` too, which silently guts the escape hatch
  `dispatch_tool` documents at `clyde/src/main.rs:157`. A custom `EyreHandler` via `set_hook` was also
  considered and rejected: it duplicates `DefaultHandler`'s chain rendering and can diverge on an
  eyre upgrade. Extending the renderer this repo already wrote beats both.
- **2026-08-01. P3 drops eyre's `Error: ` prefix rather than adding it to `dispatch_tool`.** Parity
  with the four subcommands that have shipped the bare form since they were absorbed. Flipping the
  choice is a one-word change in `render_error` if Scott wants the prefix kept everywhere instead.
- **2026-08-01 (Scott, override). P4 rides in this doc as Phase 4, WITH the major `schema-version`
  bump to 2.** I escalated two questions: whether the export defect belongs here or in its own doc,
  and whether it needs a major bump. My recommendation was Phase 5 here, no bump, on the reading that
  the field's type and vocabulary are unchanged so `docs/session-export-contract.md:251` treats it as
  within-major. Scott directed both: fold it in, take the bump. Settled, not to be relitigated. His
  reading is the safer one: the field's MEANING changes for 31 rows today and for every overridden
  row after, and a v1-pinned consumer receiving differently-derived values with no version change has
  no way to detect it.
- **2026-08-01 (panel). `doctor` uses the real `SshResolver`, not a null one.** The first draft had
  doctor do a literal-only host comparison to avoid spawning `ssh`. Codex showed that breaks P2's
  central claim: an alias row would read `GitOrigin` at the gate and `HostRefused` in doctor.
  Measured the constraint it was protecting: one distinct `repo_host` on the live catalog
  (`github.com`, 1283 rows), which literal-matches and spawns nothing, and `confers_work` memoizes
  per host anyway. The constraint was imaginary; the divergence was real.
- **2026-08-01 (panel). `--clear` NULLs `scope_version` only when an override was present.** Codex
  found that the unconditional form turns a no-op clear into a hidden re-offer, reachable against
  1018 rows on the live catalog. Fixed with a `CASE` in the UPDATE, preserving the existing
  row-existence return semantics.
- **2026-08-01 (panel). The `Basis` tally uses exhaustive `match`, not `basis as usize`.** Codex was
  right that `[usize; 6]` alone guarantees nothing: a fieldless enum casts cleanly and a seventh
  variant would compile, then panic or silently vanish from the output.
- **2026-08-01 (panel). AC3's SQL gained a `repo_host` clause.** Host refusal precedes probe refusal
  in the classifier, so a non-allowlisted host with a probe stamp is `HostRefused`. Without the
  clause the criterion double-counts as soon as that shape exists. Both readings are 0 today, so the
  recorded observation does not change; the criterion was still wrong.
- **2026-08-01 (panel). `--set personal` on an already-enriched row warns.** Raised by Gemini: the
  transcript has already been sent, and an override cannot un-send it. A warning before the write,
  matching the existing conclusive-negative warning's shape. Not a hard failure: the operator may
  legitimately want the catalog to record the correct scope going forward.
- **2026-08-01. `doctor` keeps a `probe-recorded` line under conditions.** Found on review pass 4:
  once `probe-refused` becomes a decision count it reads 0 on this catalog, and the 326 rows that DO
  carry a stale conclusive negative would vanish from `doctor` entirely. `--clear-probe` is the
  remedy for those rows and an operator has no other way to find them.

## Alternatives Considered

### Alternative 1: P1 clears `enrich_status` instead of NULLing `scope_version`
- **Description:** the override write sets `enrich_status = NULL`, satisfying the predicate's first
  disjunct.
- **Pros:** also re-offers the row.
- **Cons:** destroys the record of WHY the row was skipped, which `session doctor` and the export
  contract both read. `scope_version` is the column whose documented meaning is exactly "is this
  decision still current".
- **Why not chosen:** it throws away observability to achieve what a NULL in the right column already
  achieves, and it does not copy the in-house mechanism.

### Alternative 2: P2 corrects the `host-refused` remedy string only
- **Description:** leave both counts as SQL conditions; extend the remedy text to disclose that the
  cwd anchor decides first.
- **Pros:** one-line change, zero risk.
- **Cons:** `probe-refused 326` still reads as 326 refusals when it is 0. A count whose label needs a
  paragraph of disclaimer to not mislead is the wrong count. And it leaves `Basis` unwired, so the
  next reader re-derives the same wrong thing in SQL.
- **Why not chosen:** names must tell the truth. This is available if Scott wants the smaller change.

### Alternative 3: P3 disables eyre's `track-caller` feature
- **Description:** `eyre = { version = "0.6.12", default-features = false, features = ["auto-install"] }`
  in the root `Cargo.toml`. `DefaultHandler::debug` gates the `Location:` block on
  `#[cfg(all(track_caller, feature = "track-caller"))]` (eyre-0.6.12 `src/lib.rs:817`), the only site
  in the crate reading that feature. Verified reachable: `cargo tree -i eyre -e features` shows
  `track-caller` reached only through `eyre feature "default"`, and only the eight workspace crates
  pull it. No third-party crate in the tree depends on eyre.
- **Pros:** one line. Genuinely works.
- **Cons:** it kills the location capture GLOBALLY, including under `--log-level debug`. That
  silently guts `dispatch_tool`'s deliberate debug branch: `{e:?}` would stop differing from `{e:#}`
  and the escape hatch documented at `clyde/src/main.rs:157` becomes a dead branch that still looks
  alive. It also leaves the binary with three error renderings, fixing the footer and not the
  inconsistency underneath it.
- **Why not chosen:** it treats the symptom and destroys an in-house decision to do it. This was the
  drafted fix and it was withdrawn in review pass 4, after `rg 'Location:'` surfaced `dispatch_tool`.

### Alternative 4: P2 mirrors the anchor predicate in SQL
- **Description:** add `cwd NOT LIKE '%/repos/%'` style guards to the existing COUNT queries, the way
  `anchor_remote_disagreement` already mirrors `has_work_org`.
- **Pros:** no per-row classification, stays one SQL pass.
- **Cons:** a second implementation of the classifier, in a language that cannot express it. It
  already drifted once, which is this finding. `anchor_remote_disagreement` is the precedent AND the
  warning.
- **Why not chosen:** two signals encoding the same meaning is the defect, not the fix.

## Technical Considerations

### Dependencies
- No new crates, no dependency or feature changes at all. The rejected Alternative 3 was the only
  part of this design that touched `Cargo.toml`.
- `sessions` already depends on `session` (`session::SCOPE_VERSION` at `sessions/src/db/enrich.rs:352`)
  and on `common` (`common::repo::RepoSource` at `sessions/src/enrich.rs:166`), so Phase 3 adds no
  new crate edge.

### Performance
- `doctor`: one full scan of `sessions` plus JSON parse of `outcome_json` where the classifier reaches
  the touch-set branch. 2184 rows / 379 KB today. Doctor already spawns up to 64 `git` subprocesses
  per run; this is not the cost.
- `doctor`'s `ssh -G` spawns are bounded by DISTINCT non-literal hosts per run, not by rows, because
  `HostPolicy::resolve` memoizes and caches failures. Live catalog: one distinct host, `github.com`,
  which literal-matches and spawns nothing. `ssh -G` prints effective config without connecting, so
  even the non-zero case touches no network.
- `enrich`: unchanged. Phase 2 adds one column to two UPDATEs that fire only on an operator command.
- Phase 2 does bump the v5 export revision on an override write, as it already does. Operator
  commands are rare; this is not the per-pass churn `record_enrich_skip` was hardened against.

### Security
- P1 widens what gets SENT: a row an operator forced to `work` now actually ships. That is the
  requested behavior and it is gated on an explicit `--set work` plus a required `--reason`, with the
  conclusive-negative warning still firing before the write.
- The reverse direction stays safe: `--set personal` already worked, and nothing here weakens the
  routing gate.
- P2 is read-only and touches no send path.
- P4 does NOT change what leaves the machine. The routing gate (`sessions/src/enrich.rs`) decides
  what gets transmitted to the LLM backend; `export` is a read-only local contract that reports what
  was decided. A row exported as `work` was already sent as `work` (or was never sent at all). P4
  makes the report agree with the decision; it cannot cause a send.
- P3 removes output. It cannot widen anything.

### Testing Strategy
- Every phase carries a test that bites, verified by breaking the code and watching the test fail.
  Named per phase above.
- **Why F1 shipped with a test that looks like it covers it.**
  `a_scope_override_beats_a_refusal_in_both_directions` (`sessions/src/enrich/tests.rs:976`) sets the
  override on a FRESH row: never enriched, never skipped, `scope_version` already NULL. So it
  exercises the classifier's override branch and never the candidacy predicate that blocks in
  production. It passes today, it passes after the fix, and it was never going to catch this. Phase
  2's tests seed the `skipped-personal` + `scope_version = 3` state explicitly, because that state is
  the bug. No existing test changes behavior under this fix.
- `routing_summary` currently has ZERO tests. Phase 3 is where it gets them.
- `clyde/src/doctor/tests.rs` covers installation health only; the routing block is untested there
  and stays that way (the assertions belong at the `routing_summary` layer, which is where the logic
  is).

### Cross-repo blast radius

- Phases 1, 2, 3 and 5 are clyde-internal. Nothing outside this repo observes them.
- **Phase 4 is the only phase that leaves the repo.** `schema-version` 1 -> 2 is a breaking change
  under this contract's own promise (`docs/session-export-contract.md:255`), and its whole purpose is
  that a v1-pinned consumer notices.
- Nothing in this tree consumes the envelope: `clyde` only ever CONSTRUCTS `ExportEnvelope`
  (`clyde/src/main.rs:405`), and no crate deserializes one outside `sessions`' own tests. So the
  affected consumers are external to this repo and are not enumerable from it. Naming them and
  sequencing their update is Scott's, and it is a release-gating question, not a build-gating one.
- Ship order this forces: land all five phases -> release -> update whatever consumes
  `clyde session export` to accept `schema-version: 2` -> republish the runbook. A consumer that
  hard-fails on an unrecognized `schema-version` will break at the release, by design; one that
  tolerates it keeps working with the old (wrong) scope semantics until updated.

### Rollout Plan
- Five phases, independently committable, each `otto ci` green. NOT independently releasable: AC3
  requires Phase 3, AC2 requires Phase 2, AC5 requires Phase 4, and the runbook correction in Phase 5
  describes all of them.
- Ship order: land all five -> release -> `cargo install` -> re-run AC1/AC2/AC3/AC5 against the live
  catalog and record the numbers here -> Scott republishes the runbook via `marquee:replace`.
- No migration. No reindex required. Existing `skipped-personal` rows are unaffected until an
  operator touches them.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| P1 re-offers already-enriched rows, causing a re-enrich storm and spend | Low | High | the second candidacy clause excludes them; Phase 2 test 4 asserts it directly |
| P2's per-row classification is slower than the SQL counts on a much larger catalog | Med | Low | one scan + 380 KB parse against 64 existing git subprocesses; if it ever bites, `doctor` is a diagnostic, not a hot path |
| P2 changes numbers operators have already screenshotted | High | Low | intended: the old numbers were wrong. Phase 5 says so in the runbook |
| P3 changes stderr text that a script or systemd unit greps for | Low | Med | verified: `clyde-enrich.service` and `clyde-reindex.service` are bare `ExecStart=` lines with no shell, pipe, or grep, so systemd judges them by exit code; both run `--log-level info`, so they get the clean form. `rg 'Location:'` finds no assertion anywhere in the tree |
| a future subcommand propagates `Err` past the new renderer and re-leaks a location | Med | Low | there is one renderer and `main` is the only propagation sink; a new arm gets it for free |
| Basis tally and `by_source` get read as duplicates | Med | Low | separate group headers; the doc and the remedy lines name the difference |
| an external consumer hard-fails on `schema-version: 2` at release | High | Med | that is the bump working as designed, not a defect. Sequenced in the ship order above; the consumer update precedes republishing the runbook |
| a golden fixture's `scope` changes and gets blanket-accepted as noise | Med | Med | Phase 4 requires the re-baseline diff be inspected row by row, and AC5 states the divergence count must reach exactly zero |

## Open Questions

None.

## References

- `docs/shakedown-v0.23.0.md` (the three findings)
- `docs/design/2026-07-31-attribution-and-routing.md` (what v0.23.0 shipped)
- `docs/design/2026-07-31-runbook-update-draft.md` (the diff Phase 5 corrects)
- PR #84, https://github.com/tatari-tv/clyde/pull/84
- Runbook: <https://marquee.internal.tatari.dev/p/~scott-idler/claude-usage-report-pipeline-runbook>
- eyre 0.6.12 `DefaultHandler::debug`, the `Location:` gate (background for Alternative 3, which was
  rejected): `~/.cargo/registry/src/index.crates.io-*/eyre-0.6.12/src/lib.rs:817`
- `clyde/src/main.rs:153-177` (`dispatch_tool` + `is_debug_level`), the in-house rendering decision
  Phase 1 extends
