# Design Document: Close the Parked Attribution Items

**Author:** Scott Idler
**Date:** 2026-08-01
**Status:** Partially implemented. Phases 1, 2, 3 and 5 are BUILT, each one commit, `otto ci` green.
**Phase 4 is NOT built and Phase 0 has NOT run**: Phase 0 needs `sessions.db` from Keegan's,
Stephen's and Luke's machines and cannot run on this host, and it gates Phase 4. So rule 5 is neither
built nor struck, and AC4 FAILS pending that measurement -- recorded as a failure rather than amended
away, because the criterion is sound. Everything else closes. Five authoring passes plus three
review-panel rounds (Gemini Architect, Codex Staff Engineer). Every finding folded or pushed back
with rationale; the one pushback (`EXPORT_SCHEMA_VERSION`) was conceded by Codex with git evidence.
Acceptance criteria executed against `main` at v0.24.0 (`2c3bd71`), observed values inline, and
re-executed against this branch -- results in
`2026-08-01-close-the-parked-attribution-items-implementation-notes.md`. Open Questions empty.
**Review Passes Completed:** 5/5 authoring, 3 panel rounds

## Summary

`2026-07-31-attribution-and-routing` shipped in v0.23.0 and closed Keegan's bare-container-root bug
plus the `~/repos/<org>/<repo>` scope gate. It also parked two items and declared a third
"inherent". None of that was requested. This doc closes all three, plus three defects found while
verifying them, and removes the last places where a teammate's directory layout can silently cost
them attribution or scope.

## Problem Statement

### Background

Timeline, all 2026-07-31 to 2026-08-01:

- Keegan reported repo attribution failing at a bare-repo container root
  (`marquee.internal.tatari.dev/p/~keegan/clyde-repo-attribution-fails-at-a-bare-repo-container-root`).
- The Foundry thread (`C039YLDJW5T`, ts `1785523797.559079`) turned up four teammates on four
  layouts at 0% enrich coverage: Stephen `~/code/work/<repo>` and `~/wt/<repo>`, Luke
  `~/Projects/<repo>`, Keegan `~/git/tatari/<repo>`, Patrick `~`.
- `2026-07-31-attribution-and-routing` (v0.23.0, PR #84) fixed the container root, made rule 1
  layout-independent, made rule 3 layout-independent, and added the `git-origin` branch to the scope
  gate.
- `2026-08-01-shakedown-v0.23.0-fixes` (v0.24.0, PR #85) fixed the shakedown findings.

### Problem

Three items were parked or declared out of reach without the owner asking for it:

- **P1. Multi-root `repo-root`.** Non-Goals, line 210: "Parked. Revisit if a layout survives Phases
  6 and 7 at zero coverage." Stephen runs TWO roots. The config holds one `PathBuf`.
- **P2. Rule 4 layout independence.** Non-Goals line 207 and Resolved Decisions line 962: "Inherent,
  not deferred: rule 4 runs when the cwd is GONE, so there is nothing to probe." The premise is
  wrong. There is nothing to probe, but there is something to LOOK UP: the catalog already knows 67
  slugs.
- **P3. The scope anchor still hardcodes the literal component `repos`.** Not in any doc. Found
  2026-08-01 reviewing v0.24.0 against the thread.

Three more found while verifying those:

- **P4.** No matrix row composes an off-layout root WITH a missing work tree, which is Keegan's
  actual case (`~/git/tatari/airflow-dags` container root).
- **P5.** An orphaned linked worktree is diagnosed as a `safe.directory` problem.
- **P6.** Four doc comments and one function parameter still describe rule 3 as the path parse it
  stopped being in v0.23.0. One of them names a caller that no longer exists.

P3 is the one that bites silently, so it gets stated in full.

`has_work_org` and `has_repos_anchor` (`session/src/scope.rs:425-438`) walk path components looking
for the literal string `repos` (`REPOS_ANCHOR`, `:72`). Neither reads the configured `repo-root`.
Two consequences:

- Setting `repo-roots: ~/code/work` buys nothing at the gate. The anchor still hunts for `repos`.
- A cwd with a literal `repos` component NOT followed by a work org returns
  `Decision { scope: Personal, basis: CwdAnchor, settled: true }`, and the `git-origin` branch below
  it never runs. `~/repos/clyde` (a flat clone, no org level) is work, reads personal, and the
  `settled: true` excludes it from `enrich_candidates` until the next `SCOPE_VERSION` bump.

That is the same failure as the original bug, in the same file, one branch earlier: a path
convention deciding a routing question it cannot answer, and failing closed so quietly nobody sees
it. `has_repos_anchor_detects_any_org_slot_not_just_work`
(`session/src/scope/tests.rs:332-342`) asserts `/home/saidler/repos/whoever/x` is anchored. It never
asks about `/home/saidler/repos/clyde`.

### Goals

- Every rule that reads a path reads the CONFIGURED roots, and there can be more than one.
- No layout can produce a settled-personal decision that the remote could have answered.
- Rule 4 stops being layout-dependent, or the claim that it must be gets retired with a measurement.
- Every shape in this doc is a permanent matrix row, not a hand-verification.
- No comment or signature in the attribution path describes behavior that shipped away.

### Non-Goals

- **`WORK_ORGS` as config.** Hardcoded `["tatari-tv"]` (`session/src/scope.rs:70`). Nobody asked.
  One org, one company. Excluded, not parked.
- **Register item 7, the archived rows never re-parsed.** Swept and still open: its revisit condition
  was "it is the next doc after this one" (attribution doc line 200) and the next doc was the
  shakedown fixes, so the condition fired and nothing happened. Different subsystem (it changes what
  a reindex scans and touches `reconcile_archived`). Named here so it is not lost a second time.
  Scott's call whether it is the next doc.
- **Register item 9, `marquee mcp register`.** Different repo. Unchanged.

## Proposed Solution

### Overview

One theme: **the roots are configuration, plural, and every path rule reads them.** Then one lookup
rule that needs no path convention at all, the honest disclosure of what it recovers, and the two
matrix rows plus the four stale comments that verifying the rest turned up.

Six items, five phases. P4 and P5 share the last phase because both are matrix rows.

### Architecture

#### P1. `repo-root` -> `repo-roots`, a list

`repo_root: PathBuf` becomes `repo_roots: Vec<PathBuf>` (`common/src/config.rs:345-346`).

- Default: `[<home>/repos]`. Same behavior as today for anyone who never set it.
- Each entry validated as it is today: absolute AND an existing directory, or the config fails to
  load naming the key (`de_repo_root`, `:471-490`). An empty list is rejected: `repo-roots: []` is
  indistinguishable in effect from "no attribution" and saying so at load beats discovering it.
- **Overlapping roots are rejected at load, and the check runs on the CANONICAL paths.** A root that
  is a prefix of another is a config error naming both. Longest-prefix-wins was the first draft and
  it is WRONG: with roots `~/repos` and `~/repos/tatari-tv`, the cwd `~/repos/tatari-tv/clyde/src`
  takes the longer root and yields the slug `clyde/src`. There is no tie-break that makes a nested
  pair mean one thing, so the pair is refused. Gemini's finding is that checking the RAW strings is
  not enough: `repo-roots: [~/repos, /data/code]` does not overlap textually, but if `/data/code` is
  a symlink to `~/repos/tatari-tv` the canonical pair does, and rule 4 matching both spellings
  resurrects the exact ambiguity the rejection exists to prevent. Overlap is tested after
  canonicalization.
- **Roots are canonicalized at load, and a cwd matches either spelling.** `slug_under_root` is
  lexical by design (`common/src/repo.rs:833`, pure path parsing, no filesystem). A root reached
  through a symlink therefore never matches a cwd recorded through the real path, and rule 4 silently
  stops firing: matrix row 24's defect in rule 4's clothing. The config validator already stats each
  root, so canonicalizing there is free. Both spellings are matched because rule 4 runs when the cwd
  is GONE and the recorded cwd cannot be canonicalized retroactively.

**The rename is not aliased.** `deny_unknown_fields` already makes an old `repo-root:` a hard error,
but the generic message names an unknown key without saying what replaced it. A dedicated
`repo_root` field that errors with the migration ("`repo-root` is now `repo-roots`, a list") is one
loud signal instead of two ways to spell one thing. Two keys meaning the same value is the thing the
house rules forbid.

#### P2. Rule 5, the learned name map

Rule 4 guesses `<root>/<org>/<repo>` from the path. It cannot fire on a layout with no org level, and
when the cwd is gone there is no git to ask. Both true. Neither means the ANSWER is unavailable.

`repo_paths` (`path`, `repo`, `first_seen`, `last_seen`) already holds the slugs clyde has resolved,
and it is written ONLY on a rule-1 success (`sessions/src/db/repo.rs:94`). Every entry is a git
observation, never a guess, so rule 5 cannot compound one guess into another. A vanished
`~/Projects/philo` is answerable from it: basename `philo`, one known slug ending `/philo`,
therefore `tatari-tv/philo`.

**It fires only on a UNIQUE basename.** Measured on the desk.lan catalog, 2026-08-01:

```
$ sqlite3 sessions.db "select substr(repo,instr(repo,'/')+1) n, group_concat(distinct repo)
                       from repo_paths group by 1 having count(distinct repo)>1;"
drata-cli|tatari-tv/drata-cli,yorkeccak/drata-cli
github-actions|tatari-tv/github-actions,scottidler/github-actions
ralph-wiggum-loop|tatari-tv/ralph-wiggum-loop,scottidler/ralph-wiggum-loop
```

3 of 67 slugs collide, and every one collides ACROSS the work/personal boundary. A "pick the first"
rule would attribute a personal session to a work repo in one of three cases. Colliding basenames
decline, which is the fail-closed direction and costs nothing the code has today.

**Rule 5 fires ONLY on an inconclusive probe.** Found in pass 4: without this, a live `/tmp/philo`
that is conclusively not a repo would be attributed to `tatari-tv/philo` on its directory name alone,
and that guess would land in the by-repo tables of every report. The `ProbeOutcome` machinery
v0.23.0 shipped already draws exactly the right line: `NotARepo` and `NoOrigin` are CONCLUSIVE and
refuse rule 5; only `Indeterminate` (the vanished cwd rule 5 exists for) reaches it. Reusing the
enum rather than inventing a second notion of "gone" is also what keeps the two from drifting.

**Rule 5's population is unproven, so Phase 0 measures it before Phase 4 builds it.** Gemini's
hardest question is the one that lands: a teammate whose only `clyde` checkout is `tatari-tv/clyde`,
running a vanished scratchpad at `/tmp/clyde`, gets that session attributed to the work repo. No
leak (the gate reads `GitOrigin` only, both reviewers verified it), but it pollutes the report's
by-repo tables with a guess. Nothing in the data distinguishes a never-indexed checkout from a
scratchpad that shared a directory name, so the false positive is inherent to the rule.

Against that: zero measured recoveries here. A rule with an unproven population and a proven
false-positive mode does not get built on assertion, and it does not get parked either. It gets the
house treatment for a design resting on an unproven environmental assumption: **Phase 0, a zero-code
spike.** Run the recovery query on Keegan's, Stephen's and Luke's catalogs. The decision rule is
written down now so it is not re-litigated later:

- Any catalog recovers more than zero -> Phase 4 builds rule 5 as specified.
- Every catalog recovers zero -> rule 5 has no population, Phase 4 is struck, and P2 is answered by
  MEASUREMENT rather than by the predecessor's assertion that layout independence is "inherent".

Either outcome closes P2. The parked item was the unexamined claim, not the feature.

**Rule 5 reads the cwd's own basename and nothing else.** It does not walk up to a parent, so a
vanished bare-container CHILD (`<repo>/main`) is not recovered: basename `main`. Stated as a
limitation rather than fixed, because guessing through a parent directory is a second guess stacked
on the first.

**Rule 5 confers no work scope, by construction.** The gate's `git-origin` branch is gated on
`RepoSource::GitOrigin` alone (`session/src/scope.rs`), so `KnownPath`, `FilesTouched`, `PathGuess`
and the new `NameGuess` never reach `is_work_slug`. Rule 5 changes by-repo attribution in reports. It
cannot change what leaves the machine. That property is why a guessed org is tolerable here and
would not be one branch up.

`RepoSource::NameGuess`, token `name-guess`, rank 4, below `PathGuess`. A guess that infers the org
ranks below one that read it off the path.

**Measured recovery on desk.lan: zero.**

```
$ sqlite3 sessions.db "
with known as (select substr(repo,instr(repo,'/')+1) n, count(distinct repo) c
               from repo_paths group by 1),
     u as (select replace(cwd, rtrim(cwd, replace(cwd,'/','')), '') base
           from sessions where repo is null and cwd is not null)
select count(*) from u join known on known.n = u.base where known.c = 1;"
0
```

The 311 unresolved rows are 198 at `/home/saidler`, then `/tmp`, `/home/saidler/.claude`, agent
scratchpads, and 20 at `/home/saidler/repos/tatari-tv` (the ORG dir, which has no repo component to
match). They are sessions that ran outside any repo, which is the marquee report's own "cwd is not a
repo at all" bucket. Rule 5 is INERT on this host.

That is a null result and it is recorded, not spun. It ships anyway, and the reason is the exact
mistake this doc exists to stop repeating: desk.lan is the host where every layout assumption looks
fine. The attribution doc's own addendum says it plainly ("Problem 4 is invisible on desk.lan, and
rule 4 is why"). The population rule 5 serves is on Stephen's, Luke's and Keegan's machines. Doctor
discloses the count so the null result is visible wherever it holds.

#### P3. The anchor reads the roots

`has_work_org` / `has_repos_anchor` stop matching a literal `repos` and start matching the configured
roots. The first component under a matched root is the org slot, and the rule reads:

- **Work, settled** iff that component is a work org AND a component follows it.
- **Ambiguous** iff it is a work org with NO following component. `<root>/tatari-tv` has at least
  four possible occupants and the path separates none of them. It anchors Work ONLY on a conclusive
  `ProbeOutcome::NotARepo`, which means a plain directory. Every other outcome defers.
- **Personal, settled** iff it is not a work org AND a component follows it.
- **Unanchored** otherwise, which is exactly one shape: a single non-work-org component under a root.

**The "a following component is required" formulation was WRONG and the panel caught it.** Gemini
walked `~/repos/tatari-tv`, the org directory itself: today `windows(2)` matches
`["repos", "tatari-tv"]` and yields Work, and under the first formulation it would have had no
following component, gone unanchored, failed git-origin (an org dir is not a repo), and landed
Personal. Measured on the live catalog:

```
$ sqlite3 sessions.db "select cwd, count(*) from sessions where cwd glob '*/repos/*'
                       and cwd not glob '*/repos/*/*' group by 1;"
/home/saidler/repos/tatari-tv|21
```

21 sessions on this machine alone, silently demoted by a fix meant to stop silent demotions.

**Then the naive repair of that regression opened a leak, and Gemini found that too (round 2).**
Granting Work to any work-org component with no following component means `~/code/tatari-tv`, a FLAT
REPO literally named `tatari-tv` under a configured root, reads as the work org and is sent to the
work account without the remote ever being consulted. That is the mirror of
`~/repos/scottidler/tatari-tv`, which the predecessor deliberately made Personal for exactly this
reason. Two fixes in a row, each reopening the other's hole, because the path shape genuinely does
not carry the answer.

The answer comes from git, not from the path. But "defer when a slug was resolved, anchor Work when
it was not" is STILL wrong, and I found the third occupant myself walking the shape after round 2
rather than waiting for the panel to walk it for me. Measured against git 2.53.0:

| occupant of `<root>/tatari-tv` | probe | slug? | correct verdict |
|---|---|---|---|
| org DIRECTORY (the 21 sessions) | `NotARepo` | no | Work by anchor |
| flat repo named `tatari-tv`, personal origin | `Resolved` | yes | Personal, by the remote |
| bare CONTAINER named `tatari-tv`, personal origin | `Resolved` | yes | Personal, by the remote |
| **EMPTY repo named `tatari-tv`, no origin** | **`NoOrigin`, conclusive** | **no** | **NOT Work** |

The fourth row is the hole. An empty personal repo named `tatari-tv` under a configured root
resolves no slug, so "no slug means anchor Work" hands it Work scope and ships its content to the
work account on a directory-name coincidence. Verified live:

```
$ git init -q -b main tatari-tv && git -C tatari-tv config --local --get remote.origin.url; echo $?
1                       # NoOrigin: conclusive, and NOT a plain directory
$ git -C tatari-tv rev-parse --show-toplevel
/tmp/.../q3/code/tatari-tv    # it IS a work tree, unlike the org dir
```

So the rule is `NotARepo` specifically, not "no slug". Round 3 made both reviewers enumerate the
whole enum, and the rule is stated exhaustively rather than by exception, because two rounds of
"defer unless X" already produced two holes:

| `ProbeOutcome` | means | bare `<root>/<work-org>` verdict |
|---|---|---|
| `NotARepo` | observed, and it is a plain directory | **Work by anchor.** The org dir |
| `Resolved` | it is a checkout with a parseable origin | defer to git-origin, guards apply |
| `NoOrigin` | it IS a repo, just has no remote | fail closed, Personal |
| `Indeterminate` | vanished cwd, or git could not answer | fail closed, Personal |
| `OutsideRoot` | the repo boundary is not at or above the cwd | fail closed, Personal |
| `Blocked` | the nearest boundary is a blocked root (`$HOME`) | fail closed, Personal |

One sentence: **anchor Work only when the probe positively observed a non-repository.** Everything
else either has a remote to ask or is an absence of evidence, and absence of evidence has never
granted Work in this codebase.

**The panel split on this and I am taking Codex's side.** Gemini's replacement rule was "defer when
`repo.is_some()`, anchor Work when `repo.is_none()`", which contradicts its own answer to the same
question one paragraph earlier: it correctly identified that a vanished (`Indeterminate`) and a
no-remote (`NoOrigin`) flat repo named `tatari-tv` both resolve no slug, then proposed a rule that
hands both of them Work. Codex's enumeration is the one that holds.

**`Blocked` is my call, against Gemini, and it costs coverage.** Gemini argues a git-managed `$HOME`
makes an org dir probe `Blocked` and that the anchor should grant Work to preserve those sessions. It
is probably right that `Blocked` implies the cwd is not its own checkout: if it had its own `.git`,
`--show-toplevel` would return the cwd rather than `$HOME`. I am still failing it closed. That
reasoning is a Work-granting branch resting on an inference I would be making alone at the end of
three rounds, the affected population is teammates with a git-managed `$HOME` (nobody on the thread
that we know of), and the loss is recoverable by the gate on the next pass or by an operator
override. A leak is not recoverable. Disclosed rather than quietly chosen.

Verified against the real org dir on this machine, which is the case that must survive:

```
$ git -C /home/saidler/repos/tatari-tv rev-parse --show-toplevel
fatal: not a git repository (or any of the parent directories): .git
```

No `.git` marker at or above it (`/home/saidler/.git` is a directory with no `HEAD`, so
`is_git_marker` correctly rejects it), therefore `NotARepo`, therefore Work. The 21 hold.

The complete table. `<root>` is any configured root; `~/repos` is the default one, `~/code` stands
for a configured off-layout root. Rows marked NEW came from Gemini's round-2 exhaustive walk, which
is why this table and not the first one is the contract for what P3 changes.

| cwd | first component under root | today | after | direction |
|---|---|---|---|---|
| `~/repos/tatari-tv/clyde` | `tatari-tv`, work org, +following | Work | Work | none |
| `~/repos/tatari-tv` (org dir, no origin) | `tatari-tv`, work org, bare | Work | Work | none, preserved |
| `~/code/tatari-tv` (FLAT REPO named tatari-tv) | `tatari-tv`, work org, bare | falls through | git-origin decides | NEW, leak closed |
| `~/repos/scottidler/x` | `scottidler`, +following | Personal, settled | Personal, settled | none |
| `~/repos/scottidler/tatari-tv` | `scottidler`, +following | Personal, settled | Personal, settled | none |
| `~/repos/clyde` (flat repo, no org level) | `clyde`, bare | **Personal, settled** | git-origin decides | widening, the defect |
| `~/repos/scottidler` (personal org dir) | `scottidler`, bare | Personal, settled | falls through, Personal, unsettled | basis only |
| `~/code/tatari-tv/clyde` | `tatari-tv`, work org, +following | falls through | Work | NEW, widening |
| `~/code/scottidler/x` | `scottidler`, +following | falls through | Personal, settled | NEW, narrowing |
| `~/code/clyde` (flat repo) | `clyde`, bare | falls through | falls through | none |
| `~/code/repos/tatari-tv/x` | `repos`, +following | Work | Personal, settled | NEW, narrowing |
| `~/repos/scottidler/repos/tatari-tv/x` | `scottidler`, +following | **Work** (inner `repos`) | Personal, settled | NEW, narrowing, a FIX |
| `/tmp/repos/scottidler/x` | no matched root | Personal, settled | falls through, Personal | NEW, basis only |
| `/elsewhere/repos/tatari-tv/x` | no matched root | **Work** | git-origin decides | narrowing |
| `~/code/work/philo` | no matched root | falls through | falls through | none |

Nine rows change, not three. The two that matter most:

- **`~/code/tatari-tv/clyde` is a widening in the LEAK direction and it is intended.** A session under
  an operator-declared root with a work-org slot gains Work scope where today it depends on the
  remote. That is the same trust model `~/repos/tatari-tv/*` runs on, extended to the roots the
  operator named. The operator declaring a root IS the authorization. Called out here because a
  widening toward the work account never rides undisclosed.
- **`~/repos/scottidler/repos/tatari-tv/x` reads Work today** off the inner literal `repos`, which is
  the "contains a work org somewhere in the path" bug the predecessor's org-slot rule was written to
  avoid and missed one level down. Reading the configured root fixes it.

- **`<root>/<repo>`** is the defect. It stops being settled-personal.
- **`/elsewhere/repos/tatari-tv/x`** is a `repos/<work-org>` adjacency OUTSIDE every configured root.
  Today it reads Work off the literal `repos`. After, the remote answers. For a live checkout that is
  the same verdict by a better route. For a VANISHED cwd there is no remote to ask, so it moves
  Work -> Personal. Fail-safe direction, one line of config to restore.
- **`<root>/<personal-org>`** reaches the same Personal by a different route, unsettled instead of
  settled. Costs one predicate evaluation per pass, changes no verdict.

`classify_with_evidence` is pure and takes no config. The roots arrive as an `Anchors` value built
**exactly once, immediately after `Config::load()` in `clyde/src/main.rs`**, and passed by reference
from there. Never constructed inside a row loop: canonicalizing roots stats the disk, and building
it inside `db.routing_summary()`'s iteration would put a `stat` per row per pass. Gemini's finding.

`sessions::routing::classify_row` is the single call site shared by the enrich gate and doctor, so
that is where it threads.

**Export is a SECOND classifier and it has to be threaded too.** Codex's finding, and it is the one
I missed with the widest blast radius. `sessions/src/db/query.rs:343` falls back to
`session::classify(cwd_path)` when a row carries neither an override nor a stored scope, and
`classify` calls `has_work_org` directly (`session/src/scope.rs:169`). Change the helper without
changing that call and export silently keeps the literal-`repos` rule while the gate uses configured
roots: two answers to one question, which is the defect this doc exists to remove.

**Export stops running a second classifier entirely. This reverses what I wrote in round 2 and both
reviewers are why.** I had said the fallback stays `session::classify` (cwd-only) and just reads the
same `Anchors`, and that adding `repo_source` to the SELECT would be enough. Codex showed it is not:
the bare-work-org branch needs the probe outcome, the slug, and the host allowlist, which is the
entire git-origin branch, so "add one column" reconstructs the gate badly inside export.

The fallback calls `sessions::routing::classify_row`, the same seam the gate uses, with the same
`Anchors` and host policy. `EXPORT_COLS` / `ExportRaw` gain the routing evidence columns
(`repo_source`, `repo_probe`, `repo_host`) alongside the `repo` they already carry.

**My round-2 objection to this was wrong and it is worth saying why, so nobody restores it.** I
argued that an evidence-based fallback changes emitted values for rows that have evidence. It cannot:
the fallback only executes when `scope_override` AND stored `scope` are both absent
(`sessions/src/db/query.rs:330-343`), which is a row the gate has never decided. Gated rows read the
stored decision and never reach this line.

Gemini proposed the alternative: have export's cwd-only `classify` return Personal for the
bare-work-org shape and accept a temporary divergence until the gate runs. Rejected. It is fail-safe,
but it knowingly ships two answers to one question on exactly the 21 org-dir rows, and removing that
class of divergence is what this doc is for. Codex's own words: this code already uses the stored
decision specifically so export cannot drift from the gate.

**`SCOPE_VERSION` 3 -> 4.** The classifier's answer changes for both rows above, in opposite
directions, which is exactly what the version exists to re-offer.

#### P5. Orphaned worktree, diagnosed as itself

Live, 2026-08-01: delete a main checkout, leave its linked worktree.

```
$ git -C E-ft rev-parse --show-toplevel
fatal: not a git repository: /tmp/.../E/.git/worktrees/E-ft
```

Every probe returns 128. `no_work_tree_root` reaches the `Refused` + `has_git_marker` arm (the `.git`
FILE counts) and returns `Indeterminate`, which is the correct OUTCOME: `git worktree repair` or
restoring the main checkout recovers the row, so nothing conclusive may be recorded. The warning it
prints is wrong:

```
carries a git marker git could not use (check `safe.directory` and .git permissions)
```

Neither remedy applies. A `.git` FILE whose `gitdir:` target does not exist is an orphaned worktree
and gets its own warning naming that. Outcome unchanged.

### Data Model

- `sessions.repo_source` gains the token `name-guess`, rank 4. Existing tokens unchanged. Ranks are
  "lower is better" and the catalog writes only on a strict improvement
  (`common/src/repo.rs:51-61`), so a `name-guess` can never overwrite an existing attribution.
- **Rule 5 rows are not readable by an older binary, and that is a real cost.**
  `RepoSource::from_str` enumerates the legal set, so a v0.24.0 clyde reading a `name-guess` row hits
  `parse_repo_source` (`sessions/src/routing.rs:31-44`), which WARNS and returns `None`. It does not
  hard-fail: the row classifies without the remote signal, which is the fail-safe direction, but it
  emits one warning per affected row per pass. On a mixed-version fleet that is log noise, not a
  correctness problem. Named because it is the kind of thing that reads as a crash in a teammate's
  log. `from_str`'s error message and its legal-set enumeration both get the new token.
- No schema migration. `repo_paths` is read as it stands.
- `scope_version` rows at 3 become re-offer candidates at 4. Same mechanism as the 2 -> 3 bump.

### API Design

```rust
// common/src/config.rs
pub fn repo_roots(&self) -> &[PathBuf];          // was repo_root(&self) -> &Path

// common/src/repo.rs
pub fn from_path_guess(cwd: &Path, roots: &[PathBuf]) -> Option<Resolved>;
pub fn slug_under_roots(path: &Path, roots: &[PathBuf]) -> Option<String>;   // longest prefix wins
// Rule 5 takes the TYPED probe outcome, not a collapsed Option. Codex's finding: `resolve`
// currently funnels the probe through `detect()` into `Option<String>` (repo.rs:214-219), so the
// Indeterminate gate is unstateable in the proposed shape and an implementer would invent a second
// notion of "gone". `apply_chain` already holds the typed outcome (sessions/src/index.rs:143).
pub fn from_name_guess<M: NameMap>(cwd: &Path, probe: &ProbeOutcome, names: &M) -> Option<Resolved>;
impl Resolver { pub fn resolve<M: PathMap, N: NameMap>(
    &mut self, cwd: &Path, paths: &M, repos_touched: &BTreeMap<String,u64>,
    roots: &[PathBuf], names: &N) -> Option<Resolved>; }   // retains probe(cwd), does not collapse it

// session/src/scope.rs
pub struct Anchors { roots: Vec<PathBuf> }
pub fn classify_with_evidence(..., anchors: &Anchors, facts: &RoutingFacts<'_>) -> Decision;

// efficiency/src/collect.rs
pub fn collect_layouts(candidates, config, work_remote_hosts) -> Result<Collected>;  // repo_root dropped
```

`NameMap` mirrors the existing `PathMap` trait: one lookup method, a real implementation over
`repo_paths` and a test double, so rule 5 is unit-testable without a database.

### Implementation Plan

#### Phase 0: Prove rule 5 has a population
**Model:** sonnet
- Zero code. Run the unique-basename recovery query (recorded above) against Keegan's, Stephen's and
  Luke's catalogs, plus `select count(*) from repo_paths` on each so a zero is distinguishable from
  an empty map.
- **Success criteria:** three numbers recorded in this doc. Any non-zero -> Phase 4 proceeds. All
  zero -> Phase 4 is struck and the Resolved Decision records the measurement.

#### Phase 1: Stop the code lying about rule 3
**Model:** sonnet
- `efficiency/src/collect.rs:121-122`: docstring says `repo_root` buckets edited paths for rule 3.
  False since v0.23.0 Phase 7.
- `efficiency/src/outcome.rs:27`: module doc says the shape comes via `slug_under_root`, "pure path
  parsing". False.
- `session/src/scope.rs:479`: calls `slug_under_root` "the only writer" of `repos_touched` keys.
  False.
- `common/src/repo.rs:834`: says `slug_under_root` is "shared by rule 4 and by
  `efficiency::outcome::union`". False, and the worst of the four: it names a caller that does not
  exist (`rg -n 'slug_under_root' efficiency/` finds only two comments and one test name), so it
  invents a coupling an implementer would work to preserve.
- Drop the dead `repo_root` parameter from `collect_layouts` (used only in its own `debug!`), and
  update `efficiency::reindex_efficiency` and `clyde/src/main.rs:793`.
- **Success criteria:** `rg -c 'repo_root' efficiency/src/collect.rs` returns no match; `otto ci`
  green.

#### Phase 2: `repo-roots`, a list
**Model:** opus
- Config field, plural, validated per entry, canonicalized at load. Empty list rejected. Overlapping
  roots rejected, naming both.
- Dedicated `repo_root` field that errors naming the rename.
- `slug_under_roots`, matching each root in both its configured and canonical spelling; rule 4 and
  every caller threaded.
- `clyde doctor` prints each root with its own existence and `<org>/<repo>` presence note.
- **Success criteria:** a config with two roots resolves a rule-4 cwd under EACH; `repo-root:` errors
  naming `repo-roots`; `repo-roots: []` and a nested pair each error at load.

#### Phase 3: The anchor reads the roots
**Model:** opus

**This is the highest-value phase in the doc.** P1 and P2 widen what can be attributed; P3 is the
only one that stops a silent, settled loss of coverage that is happening today. It is third only
because it depends on Phase 2's roots.
- `Anchors` built once after `Config::load()` in `clyde/src/main.rs`, threaded through
  `routing::classify_row` to `classify_with_evidence`.
- `RoutingFacts.repo_probe` carries the parsed `ProbeOutcome`, not a presence flag.
- Export's fallback at `query.rs:343` calls `routing::classify_row` instead of `session::classify`;
  `EXPORT_COLS` / `ExportRaw` gain `repo_source`, `repo_probe`, `repo_host`. Export gains a top-level
  `scope-version` field (additive, no schema bump).
- `has_work_org` / `has_repos_anchor` match configured roots. Work org anchors Work with or without a
  following component; a non-work-org component anchors Personal only when a component follows.
- `SCOPE_VERSION` 3 -> 4.
- **Success criteria:**
  - `<root>/<repo>` with a work origin classifies Work via `GitOrigin`, not Personal via `CwdAnchor`.
  - `<root>/tatari-tv` as an org DIR with no origin still classifies Work. Deleting the
    work-org-bare branch flips it, which is the 21-session regression Gemini found.
  - `<root>/tatari-tv` as a FLAT REPO with a personal origin classifies Personal. Deleting the
    deferral flips it to Work, which is the leak Gemini found in round 2.
  - `<root>/tatari-tv` as an EMPTY repo with NO origin classifies Personal. Widening the anchor
    condition from `NotARepo` to "no slug resolved" flips it to Work, which is the hole found in
    round 3.
  - One test per `ProbeOutcome` variant at this shape, six in total. The rule is stated as an
    exhaustive table and it is asserted as one: a seventh variant must be a compile error, matching
    how `Basis` is handled in `doctor` today. All six share one path shape and one rule.
  - `<root>/scottidler/repos/tatari-tv/x` classifies Personal, not Work off the inner `repos`.
  - `<root>/scottidler/tatari-tv` still Personal.
  - **Guard paths, which the first draft failed to assert (Codex):** a `<root>/<repo>` work slug from
    a non-allowlisted host stays Personal via `HostRefused`, and one with a conclusive negative probe
    stays Personal via `ProbeRefused`. These are the two guards the Security section leans on and
    nothing was falsifying a regression in them.
  - The disclosed narrowing: a vanished `/elsewhere/repos/tatari-tv/x` classifies Personal.
  - Export's fallback and the gate return the same scope for the same cwd.

#### Phase 4: Rule 5, the learned name map
**Model:** opus
- `NameMap` trait, `repo_paths`-backed impl, `RepoSource::NameGuess` at rank 4.
- `as_str`, `from_str` and `from_str`'s legal-set error message all learn the new token.
- Unique-basename-only; a colliding basename declines.
- `clyde doctor` counts `name-guess` alongside the other four.
- Gated on an INCONCLUSIVE probe: `NotARepo` / `NoOrigin` refuse it.
- **Success criteria:** a vanished `<anywhere>/philo` resolves to `tatari-tv/philo` when that
  basename is unique; a vanished `<anywhere>/github-actions` declines on the collision; a LIVE
  non-repo `/tmp/philo` declines on the conclusive probe; a `NameGuess` row never reaches
  `is_work_slug`.

#### Phase 5: The two missing matrix rows
**Model:** sonnet
- Row 33: bare container ROOT under an off-layout root (`<home>/git/tatari/dags`), asserted through
  the real gate to Work. The composition of rows 6 and 16, which is Keegan's actual case.
- Row 34: orphaned linked worktree. Assert `Indeterminate` AND the warning names the orphan, not
  `safe.directory`.
- **Success criteria:** each row names the deletion that breaks it, per Codex. Row 33: delete the
  `--git-common-dir` fallback in `no_work_tree_root` and it declines. Row 34: delete the orphan
  branch and the assertion on the warning text fails.

## Acceptance Criteria

- [x] AC1: `rg -c 'repo_root' efficiency/src/collect.rs` returns no match.
      *Observed on main:* `8`
      *After Phase 1:* no match (exit 1). PASS.
- [x] AC2: `rg -n 'pub const SCOPE_VERSION' session/src/scope.rs` shows `= 4`.
      *Observed on main:* `66:pub const SCOPE_VERSION: i64 = 3;`
      *After Phase 3:* `71:pub const SCOPE_VERSION: i64 = 4;`. PASS.
- [x] AC3: `rg -c 'repo_roots' common/src/config.rs` is non-zero, and a `clyde.yml` carrying the old
      `repo-root:` key exits non-zero with a message naming `repo-roots`.
      *Observed on main:* `rg -c 'repo_roots' common/src/config.rs` = `0` (no match)
      *After Phase 2:* `9`, and the old key exits 1 with ``` `repo-root` is now `repo-roots`, a list:
      replace `repo-root: /path` with `repo-roots: [/path]` ```. PASS.
- [ ] AC4: `clyde doctor` prints a `name-guess` line under `resolved by:`.
      *Observed on main:* absent; `resolved by:` lists git-origin/known-path/files-touched/path-guess only
      *After Phase 5:* still absent. **FAILS.** Phase 4 is unbuilt because Phase 0 could not run, so
      nothing writes the source. The criterion is sound and is deliberately NOT amended: it stays
      failing until the population is measured and rule 5 is built or struck.
- [x] AC5: the matrix carries a named row for the off-layout container root and one for the orphaned
      worktree, and each has a test that fails when its production branch is deleted. Asserted by
      name, not by counting struct fields: Codex's point that a `PathBuf` field count is too indirect
      to prove a row does anything.
      *Observed on main:* neither row exists; `rg -c 'pub [a-z_]*: PathBuf,' common/src/checkout.rs` = `23`
      *After Phase 5:* `Matrix::offlayout_container_root` and `Matrix::orphaned_worktree` exist by
      name, and each test was RUN with its production branch deleted and failed. PASS.
- [x] AC6: export's cwd fallback and the enrich gate return the same scope for the same cwd under the
      same config, asserted by a test that drives both.
      *Observed on main:* no such test; `sessions/src/db/query.rs:343` calls `session::classify`
      while the gate calls `classify_with_evidence`, so they can already disagree today
      *After Phase 3:* `export_scope_fallback_agrees_with_the_enrich_gate_for_every_cwd_shape` drives
      both over three cwd shapes; `session::classify` is deleted. Proven to fail against a restored
      cwd-only fallback. PASS.

## Resolved Decisions

- **2026-08-01, nothing in this doc is parked.** The owner never approved the three parks it closes.
  Recorded so a future reviewer does not re-park them on cost grounds.
- **2026-08-01, rule 5 ships despite recovering zero sessions here.** Null result measured and
  recorded. desk.lan is the host where every layout assumption looks fine; it is not the population.
- **2026-08-01, rule 5 declines on a colliding basename.** 3 of 67 slugs collide and all three cross
  the work/personal boundary.
- **2026-08-01, `repo-root` is renamed, not aliased.** One key, one meaning, loud migration error.
- **2026-08-01, `WORK_ORGS` stays hardcoded.** Unrequested.
- **2026-08-01, overlapping roots are refused, not tie-broken.** Longest-prefix-wins yields the slug
  `clyde/src` for `~/repos/tatari-tv/clyde/src` under a nested pair. No tie-break is correct, so the
  config is.
- **2026-08-01, rule 5 is gated on `ProbeOutcome::Indeterminate`.** A conclusive `NotARepo` means the
  directory was observed and is not a repo. Guessing past an observation is the failure mode the
  `ProbeOutcome` enum was added to prevent.
- **2026-08-01, panel round 1: the anchor rule is work-org-first, not following-component-first.**
  Gemini. The first formulation silently demoted 21 measured sessions at `~/repos/tatari-tv`.
- **2026-08-01, panel round 1: export is threaded, not left legacy.** Codex. `query.rs:343`'s
  `session::classify` fallback is a second classifier; it reads the same `Anchors`. It does NOT
  become evidence-based, which would be a separate semantic change.
- **2026-08-01, `EXPORT_SCHEMA_VERSION` stays 2; export exposes `scope-version` additively.**
  Pushback sent to Codex, and CODEX CONCEDED with the git history: `077d3d0` moved `SCOPE_VERSION`
  1 -> 2 and `21c3a32` moved it 2 -> 3, both with `EXPORT_SCHEMA_VERSION` unchanged; `2c3bd71` bumped
  the export schema for a derivation meaning change, not a classifier revision. P3 changes classifier
  accuracy, not the contract meaning of `scope`. Phase 3 bumps `SCOPE_VERSION` 3 -> 4 and adds a
  top-level `scope-version` to `ExportEnvelope`, which is additive under the contract's own rule for
  new envelope fields (`docs/session-export-contract.md:252`) and has precedent in `prompt-version`.
  Codex verified `SCOPE_VERSION` is exposed nowhere today. A future change to `scope`'s type,
  vocabulary, or meaning still requires a major bump.
- **2026-08-01, panel round 2: `<root>/<work-org>` with nothing under it defers to git-origin.**
  Gemini. Granting it Work outright fixed the 21-session org-dir regression and simultaneously
  reopened the `~/repos/scottidler/tatari-tv` leak one level up, for a flat repo literally named
  `tatari-tv`. The path does not carry the answer; rule 1 does. Org dirs have no origin and keep
  Work; a flat repo's remote decides.
- **2026-08-01, panel round 3: the bare-work-org rule is stated as an exhaustive `ProbeOutcome`
  table, and the panel SPLIT.** Codex's enumeration is taken. Gemini's `repo.is_none()` replacement
  contradicted its own answer to the same question (it hands Work to the vanished and no-remote cases
  it had just identified as leaks). `Blocked` fails closed against Gemini's advice: its inference is
  probably right but it is a Work-granting branch resting on one reviewer's reasoning, the loss is
  recoverable and the leak would not be.
- **2026-08-01, panel round 3: export calls `classify_row`, reversing my round-2 position.** Both
  reviewers found the fallback structurally cannot evaluate a probe-dependent branch. My objection to
  an evidence-based fallback was wrong: it only fires for rows with no stored decision, so no gated
  row's value changes. Gemini's "return Personal and accept divergence" rejected for shipping two
  answers to one question on the 21 org-dir rows.
- **2026-08-01, panel round 3: `RoutingFacts.repo_probe` carries the parsed outcome, not presence.**
  Codex. It is `Option<&str>` today and documents PRESENCE as the signal
  (`session/src/scope.rs:383-390`), which cannot express `NotARepo` vs `NoOrigin`.
- **2026-08-01, round 3, author-found: the bare-work-org anchor keys on `NotARepo`, not on "no slug".**
  The panel did not find this one; I walked the shape's occupants after round 2 and measured a
  fourth. An EMPTY repo named `tatari-tv` with no origin resolves no slug and is not a plain
  directory, so "no slug means Work" ships a personal repo's content to the work account on a
  directory-name coincidence. `NotARepo` is the only outcome that means "this is an org dir".
  `NoOrigin` and `Indeterminate` both defer, failing closed.
- **2026-08-01, round 3: export's SELECT gains `repo_source`.** The bare-work-org branch is the only
  non-pure part of the anchor, and export has no probe. It does not need one: `repo_source =
  'git-origin'` is the persisted answer to the same question, so export evaluates the identical rule
  with no git subprocess.
- **2026-08-01, panel round 2: the anchor change touches nine rows, not three.** Gemini's exhaustive
  walk. Full table in P3, including the intended leak-direction widening at
  `<off-layout-root>/<work-org>/<repo>` and the pre-existing inner-`repos` bug it fixes.
- **2026-08-01, panel round 1: rule 5 gets a Phase 0 spike.** Gemini named a real false-positive mode
  (`/tmp/clyde` -> `tatari-tv/clyde`) with zero measured recoveries against it. Measure the
  population on three teammate catalogs, decision rule recorded, either outcome closes P2.
- **2026-08-01, panel round 1: overlap is checked after canonicalization** (Gemini), **rule 5 carries
  the typed `ProbeOutcome`** (Codex), **the basename map is built once per pass** (Codex), and **the
  P3 guard paths get their own assertions** (Codex).
- **2026-08-01, the anchor change narrows one case as well as widening one.** A vanished
  `repos/<work-org>` cwd outside every configured root moves Work -> Personal. Fail-safe direction,
  one line of config to restore, disclosed rather than discovered.

## Alternatives Considered

### Alternative 1: Derive the anchor from `repo-roots` but keep `repos` as an implicit extra root
- **Description:** Match the configured roots OR the literal `repos`, so nobody's current behavior
  changes.
- **Pros:** No behavior change for anyone.
- **Cons:** Preserves the exact defect. `~/repos/clyde` stays settled-personal.
- **Why not chosen:** The defect IS the current behavior.

### Alternative 2: Rule 5 picks the most-recently-seen slug on a collision
- **Description:** Break basename ties with `repo_paths.last_seen`.
- **Pros:** Recovers three more basenames.
- **Cons:** Every collision measured here crosses work/personal. Recency is not evidence of which
  one this session was.
- **Why not chosen:** Fails open on the one axis that matters.

### Alternative 3: Leave rule 4 alone, document it as inherent
- **Description:** Keep the v0.23.0 Resolved Decision.
- **Pros:** No work.
- **Cons:** The premise is false. `repo_paths` holds the answer.
- **Why not chosen:** The owner did not approve the deferral, and the justification does not hold.

## Technical Considerations

### Dependencies
None added. `repo_paths` and `SharedResolver` already exist.

### Performance
**The first draft said rule 5 is "one indexed read per unresolved cwd". That is false and Codex
caught it.** `repo_paths` is `path TEXT PRIMARY KEY` with no index on `repo` and none on a basename
(`sessions/src/db/migrate.rs:360`); the existing DB-backed `PathMap` is an exact-path point lookup
(`sessions/src/db/repo.rs:300`), which a `substr(repo, ...)` search is not.

Rule 5 therefore builds its basename map ONCE per pass from a single full `repo_paths` read and
answers from memory. 67 slugs today, so the read is trivial, but the shape matters more than the
number: a per-row `substr` scan would be a full table scan per unresolved cwd.

`SharedResolver`'s memo is unaffected.

### Security
- Rule 5 cannot confer work scope: the gate reads `GitOrigin` only. Stated in the code, asserted by a
  test that fails if a new source is added to that branch.
- The anchor change makes MORE sessions reach the `git-origin` branch. That branch is the v3-narrowed
  one (probe refusal, host allowlist), so the widening inherits every guard rather than bypassing
  them.
- `repo-roots` is operator config, not remote input. Each entry must be absolute and exist, so a
  relative or typo'd root fails at load rather than silently matching nothing.

### Testing Strategy
Every shape goes in `common/src/checkout.rs` as a real-`git` fixture row and is asserted through the
real gate, not a mock. Each phase names the deletion that must break its test. Mutation threshold
stays zero, per the v0.23.0 decision.

### Rollout Plan
One branch, five phases, one commit each, `otto ci` green per phase. Ships as one release.

**Blast radius:** clyde only. No cross-repo dependency, no ship-order constraint.

**Operator steps after upgrade:**
- `repo-root:` in an existing `clyde.yml` must become `repo-roots:`. Loud error until it does.
- `clyde session reindex --reresolve-repo` to pick up rule 5 (attribution writes are upgrade-only).
- Enrichment re-offer is automatic off the `SCOPE_VERSION` bump.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| The `repo-root` rename breaks a teammate's config | High | Low | Deliberate. Loud load error naming the new key |
| The anchor change moves a session from Personal to Work that should not move | Low | High | Only the `<root>/<repo>` row changes, and only into the `git-origin` branch with all its guards |
| Rule 5 attributes a personal session to a work repo | Low | Medium | Unique basename only; measured collision set is 3 of 67 and all decline. Confers no work scope: the gate reads `GitOrigin` only |
| Rule 5 guesses past a live directory that is simply not a repo | Low | Medium | Gated on `ProbeOutcome::Indeterminate`; a conclusive `NotARepo` refuses |
| A vanished `repos/<work-org>` cwd outside every root loses work scope | Low | Low | Fail-safe direction, disclosed in the anchor table, fixed by adding the root |
| Overlapping roots produce a nonsense slug | Low | High | Refused at load, naming both roots |
| `SCOPE_VERSION` 4 re-offers a large row set at once | Medium | Low | Same mechanism as 2 -> 3, which ran on this catalog. The gate records the skip before the transport, so a re-offer spends no tokens |

## Open Questions

None.

## References

- Keegan's report: `marquee.internal.tatari.dev/p/~keegan/clyde-repo-attribution-fails-at-a-bare-repo-container-root`
- Foundry thread: `tatari.slack.com/archives/C039YLDJW5T/p1785533980229709`
- `docs/design/2026-07-31-attribution-and-routing.md` (Non-Goals 200-210, Resolved Decisions 962)
- `docs/design/2026-08-01-shakedown-v0.23.0-fixes.md`
- `common/src/repo.rs`, `common/src/checkout.rs`, `session/src/scope.rs`, `common/src/config.rs`
