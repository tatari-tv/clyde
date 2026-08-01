# Design Document: Repo Attribution and the Routing Gate

**Author:** Scott Idler
**Date:** 2026-07-31
**Status:** Implemented (2026-07-31, branch `attribution-and-routing`)
**Review Passes Completed:** 5/5 (draft, correctness, clarity, edge cases, excellence).
**Review panel:** rounds 1 through 4 complete, 2026-07-31, Architect (Gemini) + Staff Engineer
(Codex) in parallel. 36 findings, all dispositioned in "Review Panel: findings and disposition". 33
folded in, 1 partially folded with pushback, 2 rejected with rationale. Every round split on the
verdict (Gemini READY each time, Codex NOT READY in rounds 2, 3 and 4); all nine of Codex's blockers
reproduced against `main` and are folded, two were escalated past the proposed remedy after the fix
proved incomplete, and one corrected a measurement error of the author's.

**Round 5 closed it.** Codex: READY, with executed evidence for every cell of the per-vector defense
table. Gemini: ABSTAINED, stating plainly that its persona cannot execute shell commands, which is
the right call and more useful than a fifth unevidenced clear. No new blockers from either.
**Supersedes:** the separate `2026-07-31-layout-independent-attribution.md` draft (folded in here on
the owner's call, 2026-07-31: "solve everything in this doc")

## Summary

clyde answers two questions about every session: what repo was this in, and may its transcript leave
the machine. Both are broken. The routing gate has a shipped, reproduced regression that flips a
personal session to work on a later reindex. The attribution chain reads as four rules and behaves as
one, with a hole at the root of a bare-repo container. This doc closes both, and it closes them
behind test machinery that makes this class of defect fail CI instead of failing review.

## Problem Statement

### Background

v0.22.0 (PR #82, https://github.com/tatari-tv/clyde/pull/82) fixed three defects that blocked every
teammate on the runbook thread: `USER` dropped from the child env, scope keyed on the
`~/repos/<org>/<repo>` layout, and a 2,000-char prompt rendered as a title. The layout-independence
win is real and measured, and nothing here rolls it back.

It also shipped a HIGH security regression, found by an adversarial review run after the merge and
registered in `docs/design/2026-07-31-scope-regression-handoff.md` (branch
`origin/scope-regression-handoff`, commit `ad27fb9`, not on `main`). That register carries eleven
items. This doc owns items 1, 2, 3, 4, 5, 6, 8 and 10, plus Keegan's bare-container attribution bug,
which the register does not cover.

Two claims made in the PR #82 body are wrong and are corrected here rather than left standing:
Patrick's `~` layout is NOT fixed (register item 4), and the `$USER` fix is unverified on macOS
because the Linux `getpwuid` fallback makes it a no-op on every host we can test from.

### Problem 1: a later probe retroactively flips personal to work (register item 1, SHIPPED, HIGH)

`classify_with_evidence` step 3 (`session/src/scope.rs:197`) reads `sessions.repo_source`, written by
a LIVE `git` subprocess at whatever moment the last reindex ran. Step 1 (`has_work_org`,
`session/src/scope.rs:240`) parses the RECORDED `cwd`, immutable since the session ran. The two read
different eras and only the path string links them.

`reindex` calls `apply_chain` for every session on every pass (`sessions/src/index.rs:77`), and
`upsert_repo` writes on any strict rank improvement (`sessions/src/db/repo.rs:85-89`,
`WHERE session_id = ?1 AND ?2 < repo_rank`). A row that never reached rank 0 is re-probed
indefinitely and flips the first time a probe succeeds.

Reproduced end to end in an isolated sandbox against installed v0.22.0. A personal project with no
`origin`, classified `scope=personal, would-send=False`. Then one command, nothing about the session
touched:

```
git remote add origin git@github.com:tatari-tv/side-project.git
clyde session reindex
```

Log records the flip: `upsert_repo: ... source=git-origin rank=0`. Next
`clyde session enrich --dry-run` reports `scope=work, would-send=True`. The personal transcript is
queued for the work Anthropic account. `gh repo create tatari-tv/<x> --source=.` produces the
identical state and is an ordinary workflow.

What limits it: a row whose `scope_version` is already 2 is excluded by `enrich_candidates`
(`sessions/src/db/enrich.rs:273-275`). The exposed population is rows with `scope_version` NULL, the
PROVISIONAL state written at `sessions/src/enrich.rs:141-142`. That is exactly the state of a
teammate host, which is the population the git-origin branch was built for.

### Problem 2: the remote's HOST is never validated (register item 2)

`parse_slug` (`common/src/repo.rs:408`) strips a scheme and discards everything up to the first `/`
or `:`. Every branch is `let (_, path) = ...`. Executed against crafted URLs, all five confer work
scope:

```
git@github.com:tatari-tv/philo.git          -> tatari-tv/philo
git@evil.example.com:tatari-tv/x.git        -> tatari-tv/x
https://evil.example.com/tatari-tv/x        -> tatari-tv/x
http://10.0.0.5:8080/tatari-tv/x            -> tatari-tv/x
ssh://git@gitea.local:2222/tatari-tv/x.git  -> tatari-tv/x
```

The `<org>/<repo>` shape guards are sound. The host is the only gap, and the exposure is NEW to
v0.22.0: before it, `is_work_slug` only ever saw `repos_touched` keys derived from LOCAL paths.
v0.22.0 newly feeds it a string derived from a remote URL, so the module's stated threat model
("the hazard is ABSENCE, not forgery") no longer covers its input. A `.gitmodules` in a third-party
clone is attacker-authored content that reaches this path.

### Problem 3: a stale probe permanently locks a work session out (register item 3)

The mirror of Problem 1, in the safe direction. A session that genuinely ran in a work repo, whose
path now holds a personal checkout, classifies `personal` with `Basis::GitOrigin`. That basis is
`!reads_stored_evidence()`, so the caller records `scope_version = 2`, SETTLED, and
`enrich_candidates` excludes it on all four disjuncts. Restoring the work checkout does not recover
it. Directionally safe, permanently wrong, silent.

### Problem 4: rule 1 dies at a bare-repo container root (Keegan, not in the register)

`detect_with_blocked_roots` (`common/src/repo.rs:237`) probes `git rev-parse --show-toplevel` and
`?`-returns when it fails. At the root of a bare-repo container there is no work tree, so it gives up
before line 257, the call that answers. Reproduced locally, 2026-07-31:

```
$ git -C <container>/airflow-dags rev-parse --show-toplevel
fatal: this operation must be run in a work tree        (rc=128)
$ git -C <container>/airflow-dags rev-parse --git-common-dir
<container>/airflow-dags/.bare                          (rc=0)
$ git -C <container>/airflow-dags remote get-url origin
git@github.com:tatari-tv/airflow-dags.git               (rc=0)
```

Not exotic: it is what `git init --bare` plus branch directories produces, and clyde's own
`clyde/build.rs:3-7` already resolves it for `cargo:rerun-if-changed`. Believed covered because
`docs/design/2026-07-26-report-story-fidelity.md:326-336` asserts "Rule 1 is already layout-agnostic,
verified 2026-07-26" over a four-row table whose every row is a cwd inside a work tree. The
bare-container row is the worktree CHILD, not the container root.

### Problem 5: rules 3 and 4 need an org level most layouts do not have

Both terminate in `slug_under_root` (`common/src/repo.rs:368`), which strips `repo_root` and demands
two normal components. `~/code/work/philo` has one. `~/git/tatari/philo` has two but the org slot
reads `tatari`. Rule 3's input is built the same way (`efficiency/src/outcome.rs:221`), so off-layout
the bucket map is always empty and rule 3 abstains on every session.

### Problem 6: the tests could not see any of this

Three separate failures of test machinery, and they are the reason this register exists at all.

- **A test that asserts an input production cannot emit** (register item 4).
  `session/src/scope/tests.rs:277-289` asserts cwd `/home/patrick` classifies work via
  `repo_source = "git-origin"`. `detect_with_blocked_roots` can never emit that: `blocked` is
  `[$HOME]` (`common/src/repo.rs:224`), so a toplevel equal to `$HOME` is rejected, and if `$HOME` is
  not a repo the probe fails anyway. The test hand-builds the row through a `with_repo(..)` helper,
  bypassing the resolver. It does not bite, and it inflated the measured win.
- **No test crosses the time dimension.** Every scope test is a single classification of a fixed
  input. Problem 1 is a SEQUENCE (classify, mutate the world, reindex, reclassify), and no test in
  the tree has that shape, so nothing could have caught it.
- **`enrich` cannot be tested in isolation** (register item 8, and the register understates it).
  There are THREE resolution paths and three different answers:

  | path | resolves `projects-dir` from | honors config? |
  |---|---|---|
  | `cmd_reindex` (`clyde/src/main.rs:653-657`) | `--projects-dir` flag, else `session::paths::claude_projects_dir()` | **no** |
  | `lazy_reindex` (`clyde/src/main.rs:883`) | `session::paths::claude_projects_dir()` only | **no** |
  | `mcp serve` (`clyde/src/main.rs:90`) | `cfg.projects_dir()` | yes |

  **The register says `cmd_reindex` resolves from config. It does not**, and this doc said so too
  until the review panel checked it. Only MCP serve reads the key. So setting `projects-dir` and
  running either `reindex` or `enrich` pulls in the real `~/.claude/projects`, which is why it
  corrupted a sandbox during the register's own testing. The fix is three paths agreeing, not two.

The register names the process cause too: the review panel was named as a plan and then skipped, and
a green-CI report went out ahead of it. "Run the panel next time" is not a fix. Phase 5 is the fix.

### Measured

Resolution counts by rule, two catalogs, 2026-07-31:

| rule | `repo_source` | desk.lan (`~/repos/<org>/<repo>`) | Keegan (`~/git/tatari/<repo>`) |
|---|---|---|---|
| 1 | `git-origin` | 1311 | 68 |
| 2 | `known-path` | 243 | 1 |
| 3 | `files-touched` | 196 | 0 |
| 4 | `path-guess` | 84 | 0 |
| | (unresolved) | 309 | 53 |

Keegan's cost of Problem 4: 12 sessions, $326.87 of July spend, coverage 49% -> 72%.
`tatari-tv/airflow-dags` is absent from every by-repo table in his month's report.

**Problem 4 is invisible on desk.lan, and rule 4 is why.** Three bare containers exist here
(`~/repos/okta/okta-cli-client`, `~/repos/nvidia/skillspector`, `~/repos/qdrant/qdrant`), each under
`<repo-root>/<org>/<repo>`, so a session run at one gets a correct answer from rule 4 and lands as
`path-guess`. The maintainer's layout masks the defect with a lowest-confidence guess. Of 38 distinct
unresolved cwds on desk.lan, 21 still exist and 0 are container roots, so desk.lan cannot validate
Problem 4 end to end. Same constraint PR #82 hit; same answer: constructed fixture plus a teammate
re-run.

### Goals

- No sequence of reindexes can upgrade a recorded `personal` decision to `work`.
- A remote-derived slug confers work scope only from an allowlisted host, with SSH `Host` aliases
  resolved rather than rejected.
- A `personal` git-origin decision is recoverable, not permanent.
- Rule 1 resolves at a bare-repo container root, with the `$HOME` guard intact.
- Rule 3 stops depending on the `<repo-root>/<org>/<repo>` shape.
- Every scope test drives the real resolver. A test that does not bite fails CI.
- `clyde doctor` states the effective `repo-root` and the per-rule resolution counts.

### Non-Goals

- **Register item 7, the 278 archived rows never re-parsed.** Own design doc: it changes what a
  reindex scans and interacts with `reconcile_archived`. Parked, revisit condition: it is the next
  doc after this one.
- **Register item 9, `marquee mcp register`.** Different repo (`tatari-tv/marquee`), cannot ride a
  clyde PR.
- **Register item 11, the stashed doc+test.** Resolved: discard, do not apply. Its "no worse than
  mkdir" framing is wrong and Phase 3 supersedes it. Recorded so it is not rediscovered and applied.
- **Making rule 4 layout-independent.** Inherent, not deferred: rule 4 runs when the cwd is GONE, so
  there is nothing to probe, and `~/code/work/philo` carries no org. It stops being load-bearing and
  Phase 8 discloses when it is inert.
- **Multi-root `repo-root`.** Parked. Revisit if a layout survives Phases 6 and 7 at zero coverage.

## Proposed Solution

### Overview

Bound every live observation to the moment it was valid, validate what the observation says before
trusting it, and make the test harness able to observe a sequence rather than a snapshot.

### Architecture

#### The routing fix: recorded negative evidence, revisable refusal

**The register's recommended fix (a) does not survive contact, and this is the reason the design
departs from it.** Fix (a) is "require the git-origin observation to bracket the session's activity
window", comparing `repo_paths.first_seen` against `activity_at`. Walk the two cases:

| case | session ran | first clyde probe | `first_seen <= activity_at`? |
|---|---|---|---|
| the leak (Problem 1) | T0, no origin | T3, origin now present | false. Refused, correct |
| an ordinary teammate | T0, origin present all along | T1 (first ever index) | **false. Refused, WRONG** |

`first_seen` records when clyde first LOOKED, not when the remote was added, and clyde always looks
after the session ran. Fix (a) refuses every legitimate first index and reinstates the 0%-coverage
bug it was meant to preserve against. Time alone cannot separate the two cases, because the
successful observation is after T0 in both.

**The only thing that separates them is the earlier FAILED observation.** So record the negative.

**Four persisted facts, not one.** The review panel's hardest question was: what exactly makes a
`git-origin` work decision trusted after v13? The honest answer is four things, and an earlier version
of this design added a column for one of them and hand-waved the rest. All four are now explicit:

| fact | where it lives | why it is needed |
|---|---|---|
| the slug | `sessions.repo` (exists) | what repo this is |
| the remote HOST it came from | `sessions.repo_host` (NEW) | Problem 2. A slug alone cannot be host-checked later |
| whether a conclusive no-origin probe preceded it | `sessions.repo_probe` (NEW) | Problem 1. The negative evidence |
| an operator override | `sessions.scope_override` (NEW) | recovery when any of the above is wrong |

**`None` is not evidence. Only a conclusive negative stamps.** `detect_with_blocked_roots` returns
`Option<String>`, collapsing at least seven distinct outcomes: cwd missing, cwd not a git repo, git
absent, `safe.directory` refusal, blocked root, no origin configured, and origin present but
unparseable. Stamping on all of them turns a transient environment failure into a permanent lockout,
which is the panel's severest finding and it is correct.

Rule 1 returns a typed `ProbeOutcome` instead:

```rust
enum ProbeOutcome {
    Resolved { slug: String, host: String },
    NoOrigin,        // cwd exists, IS a git repo, git answered, no origin. CONCLUSIVE -> records
    NotARepo,        // cwd exists, not a work tree and not a git dir. CONCLUSIVE -> records
    Blocked,         // resolved to a blocked root. Not evidence about a remote -> no record
    OutsideRoot,     // containment check rejected the toplevel -> no record (see below)
    Indeterminate,   // cwd missing, git absent, safe.directory, unparseable -> NO RECORD, warn
}
```

**`run_git` must control its own environment before any of this is trustworthy.** Round 2 found that
`Resolved` is forgeable today. `run_git` (`common/src/repo.rs:400-406`) spawns `git` with the
caller's environment intact, so an inherited `GIT_DIR` redirects both probes at an unrelated repo
while the containment check passes. Executed, 2026-07-31:

```
$ env GIT_DIR=<clyde>/.git git -C /tmp rev-parse --show-toplevel
/tmp                                          # toplevel == cwd, containment PASSES
$ env GIT_DIR=<clyde>/.git git -C /tmp remote get-url origin
ssh://git@github.com/tatari-tv/clyde          # an unrelated repo's origin
```

So a session run from `/tmp` with `GIT_DIR` exported attributes to `tatari-tv/clyde` and, after
Phase 2, is routed as WORK on that basis. The containment check does NOT catch it, and an earlier
version of this doc claimed it did.

**The fix is an ALLOWLIST, not a scrub list, and the primitive changes too.** Round 3 found a second
channel and round 3's own remedy was still a denylist. Executed, 2026-07-31, against a repo whose
real origin is `git@github.com:scottidler/sideproject.git`:

```
$ GIT_CONFIG_COUNT=1 \
  GIT_CONFIG_KEY_0='url.git@github.com:tatari-tv/.insteadOf' \
  GIT_CONFIG_VALUE_0='git@github.com:scottidler/' \
  git remote get-url origin
git@github.com:tatari-tv/sideproject.git      # a PERSONAL repo now reads as WORK

$ GIT_CONFIG_GLOBAL=/tmp/evil.cfg git remote get-url origin
git@github.com:tatari-tv/sideproject.git      # same forge, a variable round 3 did not name
```

Note the direction. Round 3 demonstrated work reading as personal, which is the safe direction. The
leak direction works identically and it is the one that matters.

**A third channel survives both of round 3's remedies, and it is the one that decides the design.**
Found while folding round 3, before the panel raised it. A hostile `~/.gitconfig` needs no
environment variable at all:

```
$ printf '[remote "origin"]\n\turl = git@github.com:tatari-tv/forged.git\n' > $FAKEHOME/.gitconfig
$ env -i PATH=/usr/bin:/bin HOME=$FAKEHOME git -C <repo-with-NO-origin> config --get remote.origin.url
git@github.com:tatari-tv/forged.git           # rc=0
```

That is the worst shape in the whole doc: a repo with NO origin, which must produce the conclusive
`NoOrigin` negative, instead produces a forged `Resolved` pointing at a work repo. It survives
`env_clear` for as long as `HOME` is forwarded, and it needs no `GIT_*` variable.

**Three changes. The scope qualifier is the load-bearing one:**

1. **`--local`.** `git config --local --get remote.origin.url` reads ONLY the repo's own config, so
   system and global config cannot contribute. Verified against the hostile `~/.gitconfig` above: the
   no-origin repo returns rc=1 (correctly conclusive) and a real repo returns its true origin. This
   is also the honest primitive: the question is what remote THIS repo recorded, not what this
   machine's config would display.
2. **Not `git remote get-url`.** That form APPLIES `insteadOf` rewriting by design. `git config` does
   not. Verified: both `GIT_CONFIG_*` forges above return the true `scottidler/sideproject` under
   `git config`.
3. **`env_clear()` plus an allowlist of exactly `PATH`, and no `HOME`.** Enumerating dangerous `GIT_*`
   variables is a losing game: a denylist was proposed twice and missed a channel both times.

**Each defense has a distinct job, and an earlier version of this section got that wrong.** It
claimed `GIT_CONFIG_COUNT` defeats `--local`. It does not, and the error came from testing plain
`--get` and then writing the conclusion about `--local`. Corrected by measurement, 2026-07-31:

| vector | `env_clear` | `--local` | `git config` not `remote get-url` |
|---|---|---|---|
| `GIT_DIR` redirect | **the only defense** | no (returns the redirected repo) | no |
| `GIT_CONFIG_*` injecting `remote.origin.url` | yes | **yes** | no |
| `GIT_CONFIG_*` `insteadOf` rewrite | yes | yes | **the only defense against the display form** |
| hostile `~/.gitconfig`, no-origin repo | yes (no `HOME`) | **yes** (rc=1) | no |

So all three are load-bearing and none is redundant: `env_clear` is the only thing that stops
`GIT_DIR`, `--local` is the only thing that stops config-scope forgery if a future caller inherits an
environment, and the primitive change is what stops `insteadOf`. Dropping `HOME` overlaps with
`--local` on the config channel and is kept as posture, not as an independent defense. AC11 states
which deletion bites which vector rather than asserting all three bite everything.

Verified with `HOME` entirely absent, across every checkout shape in the matrix: bare container root,
container child, plain bare mirror, and a normal clone subdirectory all return the correct origin,
`rev-parse --show-toplevel` and `--git-common-dir` both work, and the no-origin repo still returns
rc=1. Nothing in rule 1 needs `HOME`.

**Harvest the PATTERN from `child_env`, never its allowlist.** Both reviewers converged on this
independently in round 4. `common/src/llm/cli.rs` `child_env` forwards `HOME`, `USER`, `PATH`,
`NO_UPDATE_NOTIFIER` and the proxy vars because `claude` needs Keychain access and network egress. A
git probe is a local filesystem read and needs none of it; forwarding `HOME` would reopen the
`~/.gitconfig` channel. `XDG_CONFIG_HOME` is excluded for the same reason: it is another global
config path. `PATH` only, because `run_git` invokes `git` by name rather than absolute path.

The pattern itself is proven in-house: `child_env` is `env_clear()` then an explicit allowlist, and
PR #82 fixed a bug in that very allowlist, so both the shape and its failure mode are understood.
`run_git` gets the same mechanism with its own, much shorter, list.

Matrix rows 28, 31 and 32, and all of it lands in Phase 2 because `ProbeOutcome`'s arms are not
truthful without it.

`OutsideRoot` exists because of a second bug found while writing this section, and it is a
PRE-EXISTING defect that this design would otherwise convert into a lockout. Measured 2026-07-31:

```
$ ln -s <real>/proj <link>
$ git -C <link> rev-parse --show-toplevel
<real>/proj                      # canonical, NOT <link>
```

`common/src/repo.rs:240` then evaluates `toplevel == cwd || cwd.starts_with(&toplevel)` with
`toplevel = <real>/proj` and `cwd = <link>`. Neither holds, so **rule 1 declines for every session
whose cwd was reached through a symlink**, today, silently. Same for a subdirectory under the
symlink. Two consequences:

- Fix the underlying bug: canonicalize both sides before comparing. That is a Phase 6 change to the
  containment check, and it widens attribution.
- Until then, and after, a containment rejection is NOT evidence about a remote, so it must never
  record a conclusive negative.

Measured on desk.lan: 0 of the 21 still-on-disk unresolved cwds are symlink-reached, so this explains
none of the local 309. The 123 symlinks under `~/repos` are manifest-managed dotfiles, not checkouts.
Recorded as a null result on this host, not as "no impact": a teammate who symlinks a checkout hits
it on every session.

**The arms map to exit codes, measured so the implementer does not have to guess.** Rule 1 already
establishes that the cwd is a repo via `rev-parse` BEFORE reading the origin, so by the time the
origin read runs, "not a repo" has been ruled out. Against git 2.53.0, with
`git config --local --get remote.origin.url`:

| cwd | rc | arm |
|---|---|---|
| repo with an origin (flat clone, bare container root, plain bare mirror all verified) | 0 | `Resolved` |
| repo with NO origin | 1 | `NoOrigin`, CONCLUSIVE |
| not a git repo (`fatal: --local can only be used inside a git repository`) | 128 | `NotARepo`, CONCLUSIVE, and only reachable via the `rev-parse` stage |
| anything else at the origin-read stage | non-0/1 | `Indeterminate`, records nothing |

The last row is the load-bearing one: `rc=1` means git answered and there is no origin key, which is
evidence. Any other failure at that point means git did not answer the question asked, which is not.
Do NOT collapse `128` into `NotARepo` at the origin-read stage: the repo was already established, so
a fatal there is an anomaly, not a finding.

`sessions.repo_probe TEXT` stores the last conclusive outcome and the timestamp. `Indeterminate`
never writes, and it `warn!`s with the reason so the operator can see a host where every probe is
failing. That single change answers the transient-failure objection: a `safe.directory` error is
`Indeterminate`, so nothing is recorded and nothing is locked out.

- A `git-origin` attribution confers **Work** only when no conclusive negative precedes it AND the
  host is allowlisted AND no operator override says otherwise.
- Problem 1's repro: reindex #1 records `NoOrigin` (conclusive), `git remote add`, reindex #2 resolves
  but the prior conclusive negative refuses work scope. Flip dead.
- Ordinary teammate: the first index resolves, nothing is ever stamped, work scope is conferred. The
  v0.22.0 win is preserved intact, which is the whole constraint.

**Recovery is a real command, because the panel proved my first answer was false.** This doc
previously said recovery from a wrong stamp is `enrich --only <id>`. It is not: `--only` sets
`force` for candidate SELECTION (`sessions/src/enrich.rs:92`) and then runs the same classifier and
the same gate, so a wrong stamp still refuses. Verified against `main`. Two real mechanisms instead:

- `clyde session reindex --clear-probe --session <id>` clears the stamp for named sessions only.
  Narrow, explicit, and it re-stamps on the next pass if the cwd still declines conclusively.
- `sessions.scope_override TEXT` (`work` | `personal`, NULL default) wins over every rule. It is the
  escape hatch for a decision the rules get wrong in either direction, and it is what makes register
  item 3 recoverable without a `SCOPE_VERSION` bump.

  **An override with no setter and no audit trail is a hole, not a hatch.** Round 2 was right that
  the earlier draft said "operator-set" and specified nothing, which in practice means hand-editing
  SQLite. It gets a real surface:

  ```
  clyde session scope --session <id> --set work|personal --reason <text>
  clyde session scope --session <id> --clear
  clyde session scope --list
  ```

  `--reason` is REQUIRED, and it is stored with the actor and the timestamp
  (`scope_override_reason`, `scope_override_by` as `$USER@host`, `scope_override_at`). Validation:
  the id must resolve to exactly one
  session, the same rule `--reresolve-repo --session` already enforces. `--list` is the audit
  surface; `doctor`'s count is the signal to go read it. Setting `work` on a row carrying a
  conclusive negative is the one case that also prints a warning naming what is being overridden, so
  the operator cannot do it without seeing what the evidence said.

**Never cleared by `--reresolve-repo`.** That flag re-derives attribution, which is computed; the
probe record is an observation. Clearing it on the command Phase 6 tells every operator to run would
re-open the flip window on a routine invocation.

**Recording the negative rather than the first sight is what makes this migratable.** An earlier draft
of this design used a `repo_first_sight` column requiring work authority to be positively proven. It
is wrong, and the reason is worth keeping: `SCOPE_VERSION` bumps 2 -> 3, which re-offers and
re-decides EVERY stored row, so a column that is NULL on every existing row and reads as "unproven"
would strip work authority from all 1311 `git-origin` rows on desk.lan at once. Backfilling it from a
live probe would be the exact retro-observation defect being fixed. A negative-evidence column has no
such problem: NULL means "no evidence of a flip", which is the correct reading for a row nobody has
caught flipping, and there is no coverage change on upgrade at all.

**Residual gap, named.** A flip that completes entirely before clyde ever indexes the session (the
session runs with no origin, the remote is added, and only then does clyde first look) leaves no
failure to stamp, so it confers work. The 6h `clyde-reindex` timer narrows the window to the gap
between a session ending and the next sweep. This is strictly better than today, where the flip works
at any distance, and it is the honest limit of what negative evidence can prove.

**Problem 3 is the same defect pointing the other way, and it is fixed at the same time.** A
`git-origin` decision that yields **Personal** is recorded PROVISIONAL, never settled, so a later
correct observation recovers it. The asymmetry is deliberate and it is the fail-safe direction: work
requires first-sight authority, personal is always revisable.

That reintroduces the export-revision churn the code comments at `sessions/src/enrich.rs:127-131`
worry about, because `record_enrich_skip` (`sessions/src/db/enrich.rs:210-213`) is a bare UPDATE that
writes on every pass whether or not anything changed. Fix it at the source: guard the WHERE clause so
a no-change update touches nothing.

```sql
UPDATE sessions SET scope=?2, enrich_status=?3, scope_version=?4
WHERE session_id=?1
  AND (scope IS NOT ?2 OR enrich_status IS NOT ?3 OR scope_version IS NOT ?4)
```

`IS NOT` rather than `!=` so a NULL `scope_version` compares correctly. Re-offering a personal row
already spends no tokens (the gate records the skip before the transport), so with the churn closed
the cost of provisional-personal is one predicate evaluation per pass.

#### The host fix

`parse_slug` returns the host alongside the slug. A remote-derived slug confers work scope only when
the host is allowlisted.

- New config key `work-remote-hosts`, default `["github.com"]`. Config drives behavior, and a shop
  with an internal GitHub Enterprise needs to say so without a code change.
- **Do NOT use a strict `github.com` string test.** SSH `Host` aliases (`git@github-work:tatari-tv/x`)
  are ordinary config and a literal test reintroduces the 0%-coverage bug for everyone who uses one.
  Resolve the alias with `ssh -G <host>` and read the resulting `hostname`, memoized per alias, and
  fall back to the literal string when `ssh` is unavailable.
- Fails CLOSED: an unresolvable or non-allowlisted host yields a slug that can attribute a repo but
  can never confer Work scope.

#### The precedence fix (register item 5): comments change, code does not

Today steps 1 and 2 return before step 3 (`session/src/scope.rs:158-178`), so
`~/repos/tatari-tv/<a-personal-clone>` classifies **work** by directory name while its remote says
personal. The code contradicts its own comments, which call the remote authoritative.

**This doc previously resolved that by making a personal remote refuse ahead of the cwd anchor. Both
reviewers independently rejected it and they are right, so it is withdrawn.** The breaking case is a
personal FORK of a work repo, checked out in a work directory:
`~/repos/tatari-tv/clyde-fork` with origin `git@github.com:scottidler/clyde-fork.git`. That is
ordinary work, the cwd anchor reads it correctly today, and the change would silently drop it from
enrichment. `session/src/scope/tests.rs:356`
(`cwd_anchor_outranks_the_remote_in_both_directions`) asserts exactly that case as Work, and I had
written a phase step to invert a test that is currently correct. My "zero coverage cost" claim was
also unmeasured, which the staff engineer flagged separately.

Resolution: **keep the precedence, fix the comments.** The remote is authoritative for sessions the
path convention CANNOT place; it does not outrank a positive path signal. The module docs say that
instead of claiming general authority, so code and comments agree, which is all register item 5 asked
for.

Disclosure instead of a behavior change: when the cwd anchor and a trusted remote DISAGREE, `warn!`
with both values and surface a count in `clyde doctor` (Phase 8). A fork in a work directory is
legitimate and a personal clone parked under the work org is a smell, and clyde cannot tell them
apart from the slug. Making the disagreement visible is the honest move; guessing is not.

#### The attribution fix: `--git-common-dir` when there is no work tree

Do NOT simply reorder the two calls. Reading `origin` first drops the blocked-root guard on
`repo.rs:249`, which is what stops a session run from a git-tracked `$HOME` being attributed to the
dotfiles repo.

```rust
let root = match run_git(cwd, &["rev-parse", "--show-toplevel"]) {
    Some(tl) => {
        let tl = PathBuf::from(tl.trim());
        if !(tl == cwd || cwd.starts_with(&tl)) { return None; }   // unchanged
        tl
    }
    None => {
        // No work tree: a bare repo, or the root of a bare-repo container.
        let common = cwd.join(run_git(cwd, &["rev-parse", "--git-common-dir"])?.trim());
        // `.` means cwd IS the bare repo; anything else is `<root>/.bare` or `<root>/.git`.
        let root = if common == cwd { cwd.to_path_buf() } else { common.parent()?.to_path_buf() };
        if !(root == cwd || cwd.starts_with(&root)) { return None; }
        root
    }
};
if blocked.iter().any(|b| b == &root) { return None; }              // guard preserved
let origin = run_git(cwd, &["remote", "get-url", "origin"])?;
parse_slug(origin.trim())
```

**The `common == cwd` branch is a correction to the reported fix and it is load-bearing.** The
marquee report's snippet was `cwd.join(common).parent()?`. Measured against real git output:

| shape | `--git-common-dir` | `cwd.join(..).parent()` | correct root |
|---|---|---|---|
| container root | `<cwd>/.bare` | `<cwd>` | `<cwd>` |
| container root, relative form | `.bare` | `<cwd>` | `<cwd>` |
| normal clone at toplevel | `.git` | `<cwd>` | `<cwd>` |
| **plain bare repo** | **`.`** | **parent of `<cwd>`** | **`<cwd>`** |

For a plain bare repo the unguarded expression walks one level too high, so a bare repo at `$HOME`
computes a root of `$HOME`'s parent and bypasses the blocked-root guard. `Path` equality compares
components, so `cwd.join(".") == cwd` is `true` and the guard is one comparison. Both forms were run
through `rustc` to confirm, 2026-07-31.

The containment check is mirrored into the new branch because `git -C X rev-parse` can find a repo
that does not contain `X` (via `$GIT_DIR`, or a cwd inside a `.git` directory).

**Rule 2 improves for free.** Every rule 1 success writes its `(cwd, repo)` pair to the learned path
map, so a container root that resolves once stays resolvable after deletion.

#### Rule 3 stops reading the layout

Replace the `slug_under_root` call at `efficiency/src/outcome.rs:230` with the rule 1 probe against
each edited file's parent directory, memoized through `Resolver`'s existing per-path cache. One `git`
invocation per distinct directory, not per file. `repo_root` stays for rule 4.

### Data Model

One migration, `user_version` 12 -> 13, following the existing pattern in `sessions/src/db/migrate.rs`
(idempotent DDL plus the version bump in one transaction, `snapshot_before` for a populated catalog):

```sql
ALTER TABLE sessions ADD COLUMN repo_host      TEXT;  -- host the origin URL came from, or NULL
ALTER TABLE sessions ADD COLUMN repo_probe     TEXT;  -- '<outcome>@<rfc3339>' last CONCLUSIVE probe, or NULL
ALTER TABLE sessions ADD COLUMN scope_override TEXT;  -- 'work' | 'personal', operator-set, or NULL
ALTER TABLE sessions ADD COLUMN scope_override_reason TEXT;  -- required when scope_override is set
ALTER TABLE sessions ADD COLUMN scope_override_by     TEXT;  -- $USER@host that set it
ALTER TABLE sessions ADD COLUMN scope_override_at     TEXT;  -- RFC3339, when it was set
```

Six columns. The `_by` column exists because round 3 caught the doc promising "stored with the setter
and the timestamp" while the migration had no actor column: the text described an audit trail the
schema could not hold. `sessions` already carries a `host`, and catalogs get merged across machines,
so the actor is `$USER@host` rather than a bare username.

No backfill for any of the three. A backfill would have to probe live, which is the
retro-observation defect they exist to close.

**The release is NOT scope-neutral, and the earlier draft's claim that it was is withdrawn.** Both
reviewers caught this independently and it was the clearest error in the doc. Phase 2 alone preserves
every no-stamp row's scope, but `SCOPE_VERSION` 2 -> 3 re-decides EVERY row, and Phase 3 lands in the
same release. Two populations change:

- **Rows whose remote is on a non-allowlisted host.** `repo_host` is NULL on every pre-v13 row, so
  the host cannot be checked without a live reprobe, and a live reprobe is the retro-observation
  defect. **Resolved in round 2 with a one-directional rule: a live-populated `repo_host` on a
  pre-v13 row may only REMOVE Work authority, never confer it.** If the probe finds a
  non-allowlisted host, the row is stripped, which fails closed. If it finds an allowlisted host or
  cannot probe at all, the row keeps exactly the authority it already had under v0.22.0. That is the
  distinction the earlier draft asserted but did not make executable: "attribution only" was not a
  rule, "strip-only" is. Pre-v13 rows therefore carry pre-v13 trust, which is honest, because the
  evidence needed to do better was never collected. Problem 2 is closed for rows indexed under v13
  and later; for older rows it can only be enforced downward. `doctor` counts the rows still carrying
  a NULL `repo_host`.
- **Rows where the cwd anchor and the remote disagree.** Unchanged in behavior after the precedence
  withdrawal above, but now counted and warned.

**Phase 2 therefore ships a measured delta, not a claim.** Before/after `scope` counts across the
whole catalog, in the PR body. The acceptance criterion is that the delta is EXPLAINED (every changed
row falls into one of the two populations above), not that it is zero.

### API Design

- `common::repo::parse_slug` returns `Option<RemoteSlug { host, slug }>`. **Every call site is updated
  in the same commit**, which the reviewers noted the earlier draft did not say: `session/src/scope.rs`,
  `efficiency/src/outcome.rs`, `common/src/repo.rs`'s own rule 1, and
  `common/src/repo/tests.rs:38-71`. Phase 3 is not independently committable otherwise, it just breaks
  the build. Confirm the full set with `rg -n 'parse_slug' --type rust` before starting.
- `common::repo::detect_with_blocked_roots` returns `ProbeOutcome`, not `Option<String>`. This is the
  change that makes "transient failure" distinguishable from "conclusively no origin". Callers that
  only want the slug get `.resolved_slug()`.
- `session::classify_with_evidence` gains the probe record, the host, the override, and the allowlist.
- `efficiency::outcome` gains a `&mut Resolver` on the `repos_touched` path.
- `clyde session reindex` gains `--clear-probe` (requires `--session`, like `--reresolve-repo` does).
- `clyde doctor` gains report lines. One new flag, no new subcommand.

### Implementation Plan

Ordering is security first, and test machinery before the fixes it must catch.

#### Phase 0: Prove the git probes. DONE 2026-07-31
**Model:** sonnet
- Zero code. `git init --bare` a container, a plain bare mirror, a worktree child; run all three
  probes. Run both root expressions through `rustc`.
- **Success criteria:** `--show-toplevel` fails at a container root while `origin` succeeds; a plain
  bare repo returns `.`. **BOTH OBSERVED.** The second changed the design.

#### Phase 1: Make the harness able to see a sequence (register item 8)
**Model:** opus
- Make all THREE projects-dir paths agree on one precedence: `--projects-dir` flag, then
  `cfg.projects_dir()`, then `session::paths::claude_projects_dir()`. Today `cmd_reindex` skips
  config entirely, `lazy_reindex` reads neither flag nor config, and only `mcp serve` honors the key.
  Extract one resolver function and call it from all three, so a fourth caller cannot diverge.
- Add an end-to-end sandbox fixture: its own `HOME`, its own `projects-dir`, real `git init` repos in
  a `TempDir`, driving real `reindex` -> `enrich --dry-run`. Not a manual procedure, a test. Never
  against `~/repos`.
- Build the full checkout matrix from the Testing Strategy section as that fixture, all 22 rows. The
  matrix is the deliverable of this phase; later phases assert against it rather than each inventing
  their own fixtures.
- Rows the current code already handles correctly are asserted green here, which is what proves the
  harness works. Rows 6, 10, 13, 19 and 21 are the known-broken ones and are NOT asserted yet: they
  land with their fixes in Phases 2, 3 and 6, so every phase stays green.
- **Success criteria:** `cargo test -p clyde matrix_` runs green over every row not listed as
  known-broken. A test drives EACH of the three commands (`session reindex`, `session enrich`,
  `mcp serve`) against a sandbox `projects-dir` set only in config, and all three read it. The
  earlier draft's criterion here was `rg -c 'cfg.projects_dir()' == 2`, which the staff engineer
  correctly called misleading: it is satisfiable by "MCP plus lazy_reindex" while explicit reindex
  stays divergent. Behavior per command path, not a grep count. Rows 9 and 10 executed by hand in the
  harness reproduce `scope=work, would-send=True`, recorded in the implementation notes as proof the
  harness sees Problem 1 before Phase 2 fixes it. `otto ci` green.

#### Phase 2: Close the retro-flip and the permanent lockout (register items 1 and 3)
**Model:** opus
- Migration v13: `repo_host`, `repo_probe`, `scope_override`, `scope_override_reason`,
  `scope_override_by`, `scope_override_at`.
- **`run_git` gets `env_clear()` plus an allowlist of exactly `PATH`** (no `HOME`), harvested from
  the `common/src/llm/cli.rs` `child_env` pattern, and the origin read becomes
  `git config --local --get remote.origin.url`. Without all three, `Resolved` is forgeable and every
  arm below is a lie. This lands FIRST in the phase.
- `detect_with_blocked_roots` returns `ProbeOutcome`. Only `NoOrigin` and `NotARepo` are recorded;
  `Indeterminate`, `Blocked` and `OutsideRoot` warn and record nothing.
- Work gate refuses a git-origin work slug preceded by a conclusive negative. Personal git-origin
  decisions never settle. `scope_override` wins over every rule.
- A live-populated `repo_host` on a pre-v13 row may only STRIP Work, never confer it.
- `clyde session reindex --clear-probe --session <id>`, and
  `clyde session scope --session <id> --set|--clear|--list` with `--reason` required on `--set`.
- Guard `record_enrich_skip`'s UPDATE against no-change writes.
- `SCOPE_VERSION` 2 -> 3.
- Matrix rows 9, 10, 23, 26, 28, 29 and 30 turn on, rows 9+10 as one SEQUENCE test, all landing in
  this commit with the fix so the phase stays green.
- **Success criteria:** `scope_never_upgrades_personal_to_work_on_a_later_probe` passes and reverting
  only the probe gate makes it fail. `a_transient_git_failure_never_stamps` passes and reverting the
  `Indeterminate` arm makes it fail: this is the panel's severest finding and it gets its own biting
  test. A third test proves a personal git-origin row is re-offered rather than excluded. A fourth
  proves no-change `record_enrich_skip` leaves the export revision untouched. A fifth proves
  `scope_override` beats a refusal in both directions. A sixth,
  `git_dir_in_the_environment_cannot_forge_an_attribution`, passes and deleting the env scrub makes
  it fail: run the binary with `GIT_DIR` exported and a cwd unrelated to it, and assert the session
  resolves to nothing rather than to the `GIT_DIR` repo.
  **Measured, not asserted:** run the v12 -> v13 migration against a copy of the live catalog and
  record before/after `scope` counts. The criterion is that every changed row is explained by one of
  the two populations named in Data Model, and the number goes in the PR body. `otto ci` green.

#### Phase 3: Validate the remote's host (register items 2 and 11)
**Model:** opus
- `parse_slug` returns host plus slug, and **every call site is updated in this commit** so the phase
  builds standalone (`rg -n 'parse_slug' --type rust` for the set).
- Persist the host to `sessions.repo_host` so a later host policy change can be applied without a
  live reprobe. Storing only the slug is what makes pre-v13 rows uncheckable.
- `work-remote-hosts` config key, default `["github.com"]`.
- `ssh -G` alias resolution, memoized, literal fallback.
- The register's five crafted URLs become a table test, plus an `ssh`-absent case.
- Turn on matrix rows 19, 20 and 21.
- Discard the stash named in item 11.
- **Success criteria:** `cargo test -p common parse_slug_refuses_a_non_allowlisted_host` passes; row
  20 (`git@github-work:tatari-tv/x` via an alias resolving to `github.com`) still confers work, which
  is the check that this fix did not reintroduce the 0%-coverage bug; row 21's submodule remote
  attributes a repo but confers no scope. `otto ci` green.

#### Phase 4: The honesty batch (register items 4, 5, 6)
**Model:** sonnet
- Item 4: replace the impossible `/home/patrick` row with the true expectation (a `~`-cwd session
  stays personal, and why), and convert the scope tests to drive the real resolver over matrix
  fixtures instead of `with_repo(..)`.
- Item 5: **comments only, no behavior change.** The module docs stop claiming the remote is
  generally authoritative and say it places sessions the path convention cannot.
  `cwd_anchor_outranks_the_remote_in_both_directions` (`session/src/scope/tests.rs:356`) STAYS and
  keeps asserting what it asserts today. Add the disagreement `warn!`.
- Item 6: `sessions/src/enrich.rs:115` `warn!`s instead of swallowing what `RepoSource::from_str`
  raises loudly on purpose.
- **Success criteria:** `rg -c 'with_repo' session/src/scope/tests.rs` exits 1 with no output; the
  fork case (matrix row 18: work path, personal remote) still classifies Work and emits the
  disagreement warning; deleting the `warn!` in item 6 fails a test that asserts it. `otto ci` green.

#### Phase 5: Make a non-biting test fail CI
**Model:** sonnet
- `cargo install cargo-mutants --version <pinned>`, pinned and recorded, so a toolchain refresh
  cannot silently change the gate.
- Scope covers the whole routing path, not two files. Both reviewers said two is too narrow:
  `session/src/scope.rs`, `common/src/repo.rs`, `sessions/src/enrich.rs`, `sessions/src/db/enrich.rs`,
  `efficiency/src/outcome.rs`, and the v13 migration.
- **The threshold is ZERO. Not "whatever was measured".** The earlier draft set it to the measured
  survivor count, and both reviewers independently showed why that is a hole: with a budget of N, a
  new surviving mutant can be masked by fixing an old one and the count never moves. Unavoidable
  survivors get a `// mutants:skip` at the site with a one-line rationale, which is reviewable in a
  diff. A budget number is not.
- Measure first anyway, to size the work: run it, record the count, then drive it to zero by writing
  tests or annotating. If the count is large enough to swamp this phase, say so and split the
  remainder into a named follow-on rather than raising the threshold.
- Wire into `otto ci` if the runtime is tolerable; otherwise a separate task required before merge.
  Decide by measurement.
- **Success criteria:** `otto mutants; echo $?` returns 0 with ZERO unannotated survivors across all
  six paths. Deleting the body of a scope guard produces a surviving mutant that fails the task.
  Every `mutants:skip` in the tree carries a rationale comment.

#### Phase 6: Rule 1 resolves at a bare-repo container root, and backfill
**Model:** opus
- Implement the `--git-common-dir` fallback exactly as written, including the `common == cwd` guard
  and the mirrored containment check.
- Add the missing fifth row to `docs/design/2026-07-26-report-story-fidelity.md:326-336` and correct
  the "verified layout-agnostic" sentence to name the shape that was never tested.
- Canonicalize both sides of the containment check at `common/src/repo.rs:240`. This is a separate,
  confirmed, pre-existing bug: `--show-toplevel` returns the canonical path, so a symlink-reached cwd
  fails the lexical `starts_with` and rule 1 declines. Details and the measurement are under "The
  routing fix".
- Turn on matrix rows 6, 7, 8, 13 and 24:
  `detect_resolves_at_a_bare_repo_container_root`, `detect_resolves_at_a_plain_bare_repo`,
  `detect_declines_a_repo_found_above_the_cwd`, `detect_still_blocks_a_bare_repo_at_a_blocked_root`,
  `detect_resolves_a_symlinked_cwd`.
- Run `clyde session reindex --reresolve-repo` and record before/after `repo_source` counts.
- **Success criteria:** the three tests pass; deleting the fallback fails the first two and deleting
  the `common == cwd` guard fails the third. On desk.lan exactly zero sessions move from unresolved
  to `git-origin` via a container root (measured expectation: 0 of 21 still-on-disk unresolved cwds
  are container roots), recorded as a known-null result. Do NOT assert the totals are unchanged:
  `--reresolve-repo` re-runs the whole chain and directories legitimately come and go. `otto ci` green.

#### Phase 7: Rule 3 stops reading the layout
**Model:** opus
- Replace the `slug_under_root` call at `efficiency/src/outcome.rs:230` with the memoized rule 1 probe.
- Rename `union_repos_touched_is_empty_off_the_configured_root` to
  `union_repos_touched_resolves_off_the_configured_root` and invert it, so the old assumption cannot
  quietly return.
- Add `union_repos_touched_declines_a_non_repo_directory` covering `$HOME` and a temp dir.
- **Success criteria:** matrix rows 14, 15 and 16 (the three no-org-level teammate layouts, real
  checkouts with `tatari-tv` origins, none under `repo_root`) each yield
  `repos_touched == {"tatari-tv/<repo>": N}`; row 11's non-git edits and row 12's `$HOME` edits stay
  unbucketed; row 17 stays personal. Reindex wall time on the ~1,800-row desk.lan catalog recorded
  before and after. `otto ci` green.

#### Phase 8: Disclose, and update the runbook (register item 10)
**Model:** sonnet
- `clyde doctor` prints the resolved config path, the effective `repo-root`, whether it exists, the
  per-rule resolution counts, and a line when `repo-root` contains zero `<org>/<repo>` pairs (rule 4
  is inert on this host).
- **Routing observability, which the staff engineer flagged as entirely missing.** At 3am the
  operator has to tell four refusals apart, and one timestamp cannot. `doctor` reports counts for:
  rows refused by a conclusive prior probe, rows refused by a non-allowlisted host, rows with a NULL
  `repo_host` awaiting reprobe, rows carrying a `scope_override`, and rows where the cwd anchor and
  the remote disagree. Each with the command that inspects or clears it.
- `Indeterminate` probe outcomes get a counter too: a host where every probe is indeterminate has a
  `safe.directory` or a missing-git problem, and today that is invisible.
- Update the runbook, fold in the item 4 correction about the `~` layout, and lift the interim
  "do not run enrich" guidance now that Phase 2 has landed.
- **Success criteria:** `clyde doctor | rg 'repo-root'` prints one line naming an absolute path;
  `clyde doctor | rg 'path-guess'` prints a count. `otto ci` green.

## Acceptance Criteria

Every criterion was executed against `main` at v0.22.0 on desk.lan, 2026-07-31, and its output
recorded. All are expected to FAIL on main.

**A named test passing is not an acceptance criterion.** The staff engineer pointed out that AC1-AC3
as first written were satisfiable by empty functions with the right names, which is the same class of
defect as register item 4. Each now pairs the test with the deletion that must break it, and AC6 is
the backstop that catches the whole class mechanically.

- [ ] **AC1.** `cargo test -p sessions scope_never_upgrades_personal_to_work_on_a_later_probe` reports
      `1 passed`, AND reverting the probe gate in `session/src/scope.rs` makes it fail.
      `Observed on main:` `0 passed; ... 224 filtered out` (does not exist).
- [ ] **AC2.** `cargo test -p sessions a_transient_git_failure_never_stamps` reports `1 passed`, AND
      deleting the `ProbeOutcome::Indeterminate` arm makes it fail.
      `Observed on main:` `0 passed` (does not exist; `ProbeOutcome` does not exist).
- [ ] **AC3.** `cargo test -p common parse_slug_refuses_a_non_allowlisted_host` reports `1 passed`,
      AND matrix row 20 (ssh alias to `github.com`) still confers Work.
      `Observed on main:` `0 passed; ... 213 filtered out` (does not exist).
- [ ] **AC4.** `cargo test -p common detect_resolves_at_a_bare_repo_container_root` reports `1 passed`,
      AND deleting the `--git-common-dir` fallback makes it fail.
      `Observed on main:` `0 passed; ... 213 filtered out` (does not exist).
- [ ] **AC5.** `rg -c 'with_repo' session/src/scope/tests.rs` exits 1 with no output.
      `Observed on main:` `8`, exit 0.
- [ ] **AC6.** `otto mutants; echo $?` returns `0` with **zero unannotated survivors** across
      `session/src/scope.rs`, `common/src/repo.rs`, `sessions/src/enrich.rs`,
      `sessions/src/db/enrich.rs`, `efficiency/src/outcome.rs` and the v13 migration. Every
      `mutants:skip` carries a rationale.
      `Observed on main:` task does not exist (`rg -c 'mutants' .otto.yml` exits 1); `cargo-mutants`
      is not installed on this host.
- [ ] **AC7.** A sandbox with `projects-dir` set ONLY in config is read by all three of
      `session reindex`, `session enrich` and `mcp serve`, asserted per command path.
      `Observed on main:` only `mcp serve` reads it (`clyde/src/main.rs:90`); `cmd_reindex` uses the
      flag or the platform default (`:653-657`); `lazy_reindex` uses the platform default (`:883`).
- [ ] **AC8.** `clyde doctor | rg -i 'repo-root'` prints exactly one line naming an absolute path and
      exits 0, and `clyde doctor` reports counts for probe-refused, host-refused, NULL-`repo_host`,
      override, indeterminate-probe, and anchor/remote-disagreement rows.
      Note: this is `clyde doctor` (`clyde/src/doctor.rs`), which has NO TTY branch, so piping is
      safe. It is a different command from `clyde session doctor`, whose `print_doctor`
      (`clyde/src/main.rs:1145`) does switch to JSON when piped. Round 2 conflated the two.
      `Observed on main:` no output, exit 1.
- [ ] **AC11. AMENDED 2026-07-31 during implementation; the original is preserved below.** Five
      named tests in `common::repo::tests`, and each defense shown to bite on the vector it actually
      owns. `cargo test -p common -- git_dir_in_the_environment git_config_in_the_environment
      a_hostile_home_gitconfig the_origin_primitive` reports `5 passed`, and the deletion matrix is:

      | deletion | which test fails |
      |---|---|
      | `env_clear()` from `run_git` | `git_dir_in_the_environment_cannot_forge_an_attribution` |
      | `--local` from `ORIGIN_ARGS` | `the_origin_primitive_reads_only_the_repos_own_config` |
      | `git config` reverted to `git remote get-url` | `the_origin_primitive_does_not_apply_insteadof_rewriting` (+2) |
      | forward `HOME` (with `--local` intact) | **nothing**, as finding 33 predicted |
      | forward `HOME` AND drop `--local` | `a_hostile_home_gitconfig_cannot_forge_an_attribution` |

      **Why the original could not be met, measured rather than argued.** It named ONE test and
      required that deleting `--local` fail the `GIT_CONFIG_*`-injection and hostile-`~/.gitconfig`
      cases. With `env_clear` in place the child never sees a `GIT_CONFIG_*` variable OR `HOME`, so
      deleting `--local` breaks neither: executed, both still passed. The defenses are LAYERED, not
      parallel, and a test routed through the module can only ever observe the outermost one. So
      `--local` is pinned against `ORIGIN_ARGS` directly (the scope property it owns) and the
      primitive change is pinned by an `insteadOf` rule in the repo's OWN config, which no
      environment scrub can reach.

      This is the same class of error as finding 33, which corrected the `HOME` row of the defense
      table and left the identical over-claim standing here.

      **A FOURTH vector was found while building these proofs and is now closed by the same primitive
      change**: an `insteadOf` rule in a repo's own `--local` config rewrites a personal origin to a
      work one with NO environment variable and NO hostile `~/.gitconfig`. It is the only vector where
      the `git config` choice is the sole defense, and the vector table above credits `--local` and
      `env_clear` with covering that row.

      *Original text:* `cargo test -p common git_env_cannot_forge_an_attribution` reports `1 passed`
      over all THREE vectors, deleting `env_clear` fails the `GIT_DIR` case, deleting `--local` fails
      the `GIT_CONFIG_*`-injection case and the hostile-`~/.gitconfig` case, and reverting to
      `git remote get-url` fails the `insteadOf` case.
      `Observed on main:` does not exist. `run_git` (`common/src/repo.rs:400-406`) inherits the
      environment and rule 1 uses `git remote get-url origin` (`:257`). Measured, all three forges
      succeed on `main`:
      `env GIT_DIR=<clyde>/.git git -C /tmp remote get-url origin` -> `ssh://git@github.com/tatari-tv/clyde`;
      `GIT_CONFIG_COUNT=1 ... insteadOf` turns `scottidler/sideproject` into `tatari-tv/sideproject`;
      a `~/.gitconfig` with `remote.origin.url` makes a no-origin repo return
      `git@github.com:tatari-tv/forged.git` at rc=0.
- [ ] **AC9.** The v12 -> v13 scope delta on a copy of the live catalog is recorded in the PR body and
      every changed row falls into one of the two populations named in Data Model.
      `Observed on main:` not applicable; v13 does not exist.
- [ ] **AC10.** `otto ci; echo $?` returns `0` on every phase commit.
      `Observed on main:` green at v0.22.0 per PR #82's required checks; not re-run for this doc.

Two criteria cannot run here and are Phase criteria rather than doc-level ACs: Keegan's `git-origin`
count rising by 12 (his machine), and a macOS confirmation of the v0.22.0 `$USER` fix (Luke, Stephen
or Calvin). Neither gates the merge; both gate the claim.

## Resolved Decisions

- **2026-07-31, the register's fix (a) is rejected on measurement, fix (c) plus a non-settling
  refusal is adopted.** `first_seen` records when clyde first looked, not when the remote appeared,
  and clyde always looks after the session ran, so (a) refuses every legitimate first index. Walked
  in the table under "The routing fix". Fix (b) was also rejected: forbidding any personal-to-work
  upgrade makes register item 3 permanent by design.
- **2026-07-31, negative evidence rather than positive proof, and no backfill.** A `repo_first_sight`
  column requiring work authority to be positively proven was drafted and rejected: `SCOPE_VERSION`
  2 -> 3 re-decides every row, so a NULL-on-every-existing-row column reading as "unproven" would
  strip work authority from all 1311 `git-origin` rows on desk.lan in one upgrade, and backfilling it
  from a live probe would repeat the exact defect being fixed. `repo_probe` has neither
  problem, and Phase 2 in isolation changes no row's scope. The RELEASE is not scope-neutral, because
  Phase 3 lands with it; see Data Model. Its residual gap (a flip completing before clyde's first
  index) is named in the design rather than left for someone to find.
- **2026-07-31, the host allowlist is config, not a literal, and the default is `["github.com"]`.** A
  strict `github.com` test breaks SSH `Host` aliases, which are ordinary config, and would reintroduce
  the exact 0%-coverage bug v0.22.0 fixed. So: config key, `ssh -G` resolution, fail closed. The
  default was measured rather than assumed: all 59 `origin` remotes across `~/repos/tatari-tv/*` on
  desk.lan resolve to `github.com`, zero to anything else.
- **2026-07-31, item 5 resolved as comments-only. REVERSED after review round 1.** The doc first made
  a personal remote refuse ahead of the cwd anchor, calling it safe-direction with zero coverage cost.
  Both reviewers rejected it: it silently drops a personal FORK of a work repo checked out in a work
  directory, which is ordinary work, and the "zero cost" claim was never measured. The precedence
  stays as it is, the comments stop overclaiming, and the disagreement is warned and counted.
- **2026-07-31, the probe record is typed, not boolean. Added after review round 1.** `None` from
  rule 1 collapses at least seven outcomes, and stamping on all of them makes a `safe.directory` error
  a permanent lockout. Only `NoOrigin` and `NotARepo` are conclusive.
- **2026-07-31, four persisted facts, not one.** Slug, host, conclusive-negative record, and operator
  override. The panel's hardest question was what makes a git-origin work decision trusted; the honest
  answer needed all four, and the earlier design added a column for one.
- **2026-07-31, the mutation threshold is zero, not the measured count.** A survivor budget lets a new
  survivor hide behind a fixed old one. Annotated skips with rationale are reviewable; a number is not.
- **2026-07-31, the reported container-root fix is amended before implementation.** The marquee
  report's snippet mis-roots a plain bare repo by one level and bypasses the `$HOME` guard. Found by
  running it. Credit to the report for the root cause, the `--git-common-dir` approach, the
  `build.rs` precedent, and the never-tested fifth row: all its findings, all held up.
- **2026-07-31, rule 4 stays layout-dependent.** Not a deferral. There is no git to ask when the cwd
  is gone. Disclosure beats fabrication.
- **2026-07-31, the two drafts are merged.** On the owner's call. The routing gate and the attribution
  chain are one trust boundary and shipping them separately creates the race the earlier draft's gate
  section was written to warn about.
- **2026-07-31, register item 11's stash is discarded, not applied.** Its reasoning is weaker than
  item 2's and its framing is wrong.

## Addendum: corrections found during implementation

Recorded here rather than by editing the claims in place, so the road not taken stays visible. Every
one was found by RUNNING something, and each is measured in
`docs/design/2026-07-31-attribution-and-routing-implementation-notes.md`.

- **AC11's deletion pairings were unmeetable.** Amended above, with the matrix that replaces them.
  Same class as the panel's finding 33, in a spot finding 33 did not sweep.
- **A FOURTH forgery vector.** An `insteadOf` rule in a repo's OWN `--local` config rewrites a
  personal origin to a work one with no environment variable and no hostile `~/.gitconfig`. Closed by
  the `git config` primitive, which is its only defense.
- **The `common == cwd` guard needed `--is-bare-repository`.** For a cwd inside a NON-bare repo's
  `.git`, git also reports `.`, so rooting at the cwd unconditionally puts the root at `<repo>/.git`
  and the blocked check misses `$HOME`. This design's fallback would have INTRODUCED that hole; the
  old code could not reach it.
- **`has_git_marker` must validate the marker, not just its existence.** `/home/saidler/.git` is a
  plain directory holding only `info/`, so git correctly reports "not a git repository" beneath it. A
  bare `.exists()` downgraded 21 conclusive answers to `Indeterminate` and made `doctor` blame
  `safe.directory`. A `.git` directory now has to carry `HEAD`.
- **"Problem 4 is invisible on desk.lan" is too strong.** The three containers this doc counts carry
  0, 1 and 1 sessions. Seven OTHERS carry 101 between them, two of those under `tatari-tv`. The Phase
  6 criterion still holds exactly as written (0 sessions move from UNRESOLVED, because rule 4 masks a
  container under `<repo-root>/<org>/<repo>`): what was lost was PROVENANCE, not coverage.
- **The blast radius of the `parse_slug` signature change is smaller than stated.** `rg -n 'parse_slug'`
  finds one production call site, not the four the API Design section lists; `session/src/scope.rs`
  and `efficiency/src/outcome.rs` consume a stored slug and never call it.
- **Rule 3 now needs the edited file's parent directory to still exist.** Not named as a cost in
  Phase 7. The path parse it replaces would bucket a checkout deleted years ago; a slug from a
  vanished path is a guess, which is rule 4's job.
- **`Resolver`'s per-path cache is not reachable from Phase 7's caller.** `efficiency::collect` uses
  rayon's `par_iter`, so a `Sync` sibling (`SharedResolver`) was needed. Its memo collapses per
  REPOSITORY rather than per directory, which is this doc's own "cache key moves to the git common
  dir" remedy reached without an extra `git` call.

## Review Panel: findings and disposition

Round 1, 2026-07-31. Architect (Gemini) and Staff Engineer (Codex), in parallel, Design Review mode.
Raw output: `/tmp/review-panel/kBxrijjH/{arch.out,staff.out}`. Every finding is dispositioned. Nothing
is dropped.

| # | finding | raised by | disposition |
|---|---|---|---|
| 1 | The stamp turns a transient git failure (`safe.directory`, unmounted drive, locked index) into a permanent lockout | both, SEVERE | **FOLDED.** `ProbeOutcome` enum; only `NoOrigin`/`NotARepo` are conclusive. `Indeterminate` warns and records nothing. Matrix row 23, its own biting test in Phase 2 |
| 2 | The stated recovery path `enrich --only <id>` is FALSE; `--only` still runs the gate | Codex | **FOLDED, and verified against `main` before folding** (`sessions/src/enrich.rs:92`). Replaced with `--clear-probe --session <id>` and a `scope_override` column |
| 3 | "v13 is scope-neutral" is untrue for the release | both, HIGH | **FOLDED.** Claim withdrawn. Two changing populations named in Data Model, delta MEASURED and recorded in the PR body (AC9) |
| 4 | Phase 3 cannot host-check pre-v13 rows: the catalog stores no host | Codex | **FOLDED**, then AMENDED in round 2. `sessions.repo_host` added. The round-1 fold said NULL "does not confer Work"; that was superseded by the strip-only rule in finding 20, which is the executable version. See Data Model |
| 5 | The precedence change breaks personal forks of work repos in work directories | both | **FOLDED, decision reversed.** Item 5 is now comments-only. `cwd_anchor_outranks_the_remote_in_both_directions` stays. Disagreement is warned and counted instead |
| 6 | "Zero coverage cost" for the precedence change was unmeasured | Codex | **FOLDED** by withdrawing the change. The claim is gone with it |
| 7 | The projects-dir premise is wrong: `cmd_reindex` does not read config | Codex | **FOLDED, and it was worse than stated.** Three paths, three answers. Verified. AC7 now asserts behavior per command path instead of a grep count |
| 8 | Phase 3 changes `parse_slug`'s signature without updating callers, so it breaks the build | Gemini | **FOLDED.** Call-site update named in Phase 3 and in API Design |
| 9 | Interim guidance lifts at Phase 2, but Phase 2 does not fix hosts | Codex | **FOLDED.** The branch ships whole; phases are committable, not releasable |
| 10 | Mutation threshold "whatever was measured" institutionalizes survivors | both | **FOLDED.** Threshold is zero; unavoidable survivors get `// mutants:skip` plus a rationale, reviewable in a diff |
| 11 | Mutation scope of two files is too narrow | Codex | **FOLDED.** Six paths |
| 12 | `cargo install cargo-mutants` needs a pinned version | Codex | **FOLDED** |
| 13 | AC1-AC3 satisfiable by empty tests with the right names | Codex | **FOLDED.** Each AC now pairs the test with the deletion that must break it |
| 14 | Matrix missing: fork, transient failure, symlinked cwd, reused cwd, archived cwd, pre-v13 host | both | **FOLDED.** Rows 23-27, and row 18 relabeled as the fork case |
| 15 | No rollback or observability for a wrong stamp | Codex | **FOLDED.** Phase 8 reports five counts, each with the command that inspects or clears it |
| 16 | Phase 6's desk.lan AC is not reproducible in CI | Gemini | **PARTIALLY FOLDED, pushback recorded.** It is a measurement, not an AC, and it is labelled as one. It stays because a null result on the only catalog available is worth recording. The CI-reproducible half is matrix row 6 |
| 17 | `report merge` and multi-host stamp synchronization | Gemini raised, Codex answered | **CLOSED by the panel itself.** Report JSON carries only `repo`/`repo_source` (`report/src/report.rs:94`, `report/src/merge.rs:169`), so merge is not a routing bypass. Real consequence is observability only, covered by finding 15 |
| 18 | Phase 5 (mutation testing) is unrequested scope | Gemini | **REJECTED, with rationale.** It is the most directly requested thing in the doc. The owner's words: "if we are causing regressions, then the testing needs to be better to catch that and be more robust." Gemini also concedes it is "structurally sound"; its real objection was the non-zero threshold, which is finding 10 and is folded |

Two reviewers, one disagreement in round 1: **finding 18.** Gemini called mutation testing unrequested
process machinery; Codex treated it as correct but under-scoped. Resolved toward Codex.

### Round 2, 2026-07-31

Raw output: `/tmp/review-panel/nqkWdnDD/{arch.out,staff.out}`.

**The two reviewers split on the verdict.** Gemini: READY, all findings closed, "proceed to
implementation." Codex: NOT READY, four blockers. **Codex is right and I sided with it.** Every one of
its four blockers reproduced against `main`, and Gemini missed all four while explicitly certifying
three of them as closed. Round 1 taught the same lesson in the other direction, so the standing rule
for this doc: a READY verdict is not evidence; a reproduced blocker is.

Gemini did withdraw finding 18 on the traceability, which closes round 1's only disagreement.

| # | round 2 finding | disposition |
|---|---|---|
| 19 | `run_git` inherits `GIT_DIR`, so `Resolved` is forgeable and the containment check does not catch it | **FOLDED. Confirmed live**, executed above. Env scrub lands first in Phase 2, matrix row 28, AC11. Gemini asserted the containment guards catch this; it does not, and the reproduction is in the doc |
| 20 | Pre-v13 `repo_host` live population is retro-observation if it can confer Work | **FOLDED.** Strip-only: a live-populated host may only REMOVE Work authority, never grant it. The earlier "attribution only" phrasing was not an executable rule |
| 21 | `scope_override` has no setter, no validation, no audit; "operator-set" means hand-editing SQLite | **FOLDED.** `clyde session scope --set/--clear/--list`, `--reason` required and stored with setter and timestamp, warning when overriding a conclusive negative |
| 22 | Stale scope-neutral and item-5 claims survive in Resolved Decisions and Security | **FOLDED.** All three sites corrected. The doc is the source of truth and it was contradicting itself in the security section, which is the worst place for it |
| 23 | Missing matrix rows: `GIT_DIR`, broken submodule, unreadable `.git/config`, empty repo | **FOLDED.** Rows 28, 29, 30 |
| 24 | AC8 `clyde doctor \| rg` tests the JSON surface, since `print_doctor` switches on TTY | **REJECTED, with rationale.** Two different commands. `clyde doctor` is `clyde/src/doctor.rs` and has no TTY branch (`rg -n 'is_terminal' clyde/src/doctor.rs` returns nothing). `print_doctor` at `clyde/src/main.rs:1145` belongs to `clyde session doctor`. The AC now names which one, because the ambiguity is real even though the finding is not |
| 25 | `mutants:skip` annotations could become broad enough to make the gate theater | **FOLDED.** Smallest-site skips only, each with a rationale, per Codex's own remedy |

### Round 3, 2026-07-31

Raw output: `/tmp/review-panel/r3-MQAJ8QfC/{arch.out,staff.out}`. **Split again: Gemini READY, Codex
NOT READY with three blockers. All three reproduced.** That is three rounds running where the READY
verdict was the wrong one.

Gemini answered the method question directly and the answer is worth keeping: it reasoned statically
about `GIT_DIR` instead of executing it, assuming `--show-toplevel` would return the redirected
repo's root and fail containment. In fact git treats the `-C` path as the work tree when `GIT_DIR` is
set without `GIT_WORK_TREE`, so containment passes trivially. Its own guidance: discount its claims
about environment edge cases, filesystem boundaries, and external binary behavior unless it shows the
terminal output. Adopted.

| # | round 3 finding | disposition |
|---|---|---|
| 26 | The env scrub is a denylist and misses git's config channel: `GIT_CONFIG_COUNT`/`KEY_*`/`VALUE_*` rewrites the origin via `insteadOf` | **FOLDED, and escalated past the proposed remedy.** Reproduced, and two things round 3 missed: it also works via `GIT_CONFIG_GLOBAL`, and it works in the LEAK direction (a personal origin reading as work), which round 3 did not test. A denylist cannot close this. Then, while folding, I found a THIRD channel needing no environment variable at all (finding 32), which settled the design: `env_clear()` with an allowlist of exactly `PATH` and no `HOME`, plus `git config --local --get remote.origin.url`. Matrix rows 31 and 32, AC11 |
| 32 | **Found by the author while folding 26, not by the panel.** A hostile `~/.gitconfig` with `remote.origin.url` turns a no-origin repo into a forged work `Resolved`, with no env vars set | **FOLDED.** This is why the primitive carries `--local` and why `HOME` is not forwarded. Verified across every checkout shape with `HOME` entirely absent: all resolve correctly, and the no-origin repo stays conclusive at rc=1 |
| 27 | `scope_override` has no actor column; the doc promises a setter the schema cannot store | **FOLDED.** `scope_override_by` (`$USER@host`), six columns |
| 28 | The pre-v13 `repo_host` rule contradicts itself: Data Model says strip-only, the round-1 disposition and the risk table say rows lose work scope | **FOLDED.** Both stale sites corrected, and see the process note below |
| 29 | AC8 TTY rejection is correct | **CONFIRMED by both reviewers.** Gemini independently verified `clyde/src/doctor.rs` has no `is_terminal` branch. The pushback stands |
| 30 | Scrubbing `GIT_DIR` does not break a legitimate `run_git` caller | **CONFIRMED by both.** `run_git` is private to rule 1 and only probes recorded session cwds. Gemini added the sharper point: in a hook or CI context an inherited `GIT_DIR` would make every reindexed path resolve against the hook's repo, so the scrub is a correctness fix as much as a security one |

### Round 4, 2026-07-31

Raw output: `/tmp/review-panel/r4-Z0oiNxcP/{arch.out,staff.out}`. Gemini READY, Codex NOT READY with
two findings. Both folded.

**Gemini's READY is weak evidence and it said so itself.** It reported that its persona runs in a
mode with shell execution disabled, so it could not run a single command, and it labelled its own
substantive finding UNREPRODUCED and declined to block on it under the rule I gave it. A verdict
reached without executing anything is not a verdict. What Gemini DID contribute was real: reasoning
alone, it independently derived the hostile-`~/.gitconfig` channel I had reproduced while folding
round 3, and reached the same remedy (`--local`, and do not harvest `child_env`'s allowlist).
Independent convergence on a channel neither of us was looking for is the strongest signal in four
rounds that the analysis is now complete.

| # | round 4 finding | disposition |
|---|---|---|
| 33 | AC11 demands a failure that will not happen: with `--local` present, dropping `HOME` does not independently bite | **FOLDED, and it corrects an over-claim of mine.** I tested `GIT_CONFIG_COUNT` against plain `--get`, then wrote the conclusion about `--local`. Measured: `--local` DOES resist that injection. Each defense now has its own row in a table naming which vector it owns, and AC11 pairs each deletion with the specific case it must break |
| 34 | The risk table still says the mutation threshold is the measured number, contradicting the folded zero decision | **FOLDED.** Swept every mention of threshold and survivor; all now say zero |
| 35 | Do not harvest `child_env`'s allowlist verbatim, only its pattern | **FOLDED.** Both reviewers independently. `child_env` forwards `HOME`/`USER`/proxy vars for `claude`'s Keychain and egress needs; a git probe needs `PATH` and nothing else. `XDG_CONFIG_HOME` excluded as another global config path |
| 36 | Minimum allowlist is empty with an absolute git path, `PATH` only otherwise | **CONFIRMED by both, folded as `PATH`-only** since `run_git` invokes `git` by name |

**Process note, because this is the pattern that bit me four times.** Findings 22, 28 and 34 are the same
failure, three times, plus a fourth caught by my own sweep before the panel saw it: I corrected the
primary statement and left the derived statements stale, in the Security and Risks sections every
time. A design doc that contradicts itself in its security section is worse than one that is simply
wrong, because a reader can cite either half.

The remedy is not "be careful". When a rule changes, grep every mention of the thing it governs and
read each hit, rather than editing the section the finding pointed at. The sweeps that closed 28 and
34 were `rg -n 'repo_host'` and `rg -n 'threshold|survivor'` across the whole file, and the second
one found a stale row the reviewer had not cited.

Finding 33 is a different and more serious error of mine: I ran a test against one form of a command
(`git config --get`), drew a conclusion about a different form (`--local`), and wrote it into the doc
as measured. That is the exact failure this doc criticizes the v0.22.0 verification table for. The
remedy is the same as everywhere else here: the command you ran is the only claim you own.

## Alternatives Considered

### Alternative 1: Reorder rule 1 to read `origin` first
- **Description:** Drop the `--show-toplevel` probe, read `origin` directly.
- **Pros:** Two lines; fixes every no-work-tree shape.
- **Cons:** Deletes the blocked-root guard, so a session run from a git-tracked `$HOME` attributes to
  the dotfiles repo, and scope now consumes that.
- **Why not chosen:** Trades a coverage bug for a routing bug.

### Alternative 2: Freeze the decision (register fix b)
- **Description:** Forbid a later probe from upgrading a recorded `personal` to `work`.
- **Pros:** Simplest possible fix for Problem 1; no schema change.
- **Cons:** Makes Problem 3 permanent by design. A genuine work session misread once is locked out
  forever with no path back.
- **Why not chosen:** Fixes one of a mirrored pair by cementing the other.

### Alternative 3: Bracket the observation to the activity window (register fix a)
- **Description:** Require `repo_paths.first_seen <= activity_at`.
- **Pros:** The most faithful statement of "what repo was this session actually in".
- **Cons:** Refuses every legitimate first index, because clyde always looks after the session ran.
  Reinstates 0% coverage.
- **Why not chosen:** Measured and walked in the table above. It is the right intent with an
  unimplementable predicate.

### Alternative 4: Make `repo-root` a list and tell teammates to configure it
- **Description:** `repo-root: [~/code/work, ~/git/tatari]`.
- **Cons:** Does not work. `~/code/work/philo` has no org component, so `slug_under_root` declines
  regardless of root. Also puts discovery on every operator, which is Problem 5 restated as a feature.
- **Why not chosen:** Fixes nothing for the layouts that motivated the work. Parked as a Non-Goal.

## Technical Considerations

### Dependencies

`cargo-mutants` as a dev/CI tool (Phase 5), installed via `cargo install`, not vendored. `ssh` as a
runtime dependency for alias resolution, with a literal fallback when absent, so it is not hard.
`git` was already hard.

Cross-**repo** blast radius: **none for code.** Every change is inside `tatari-tv/clyde`. Register
item 9 lives in `tatari-tv/marquee` and is explicitly out of scope. Phase 8 publishes a runbook update
to marquee, which is content, not a code dependency.

Ship order this doc forces: Phases 1 through 5 before 6 and 7. Phase 6 manufactures newly-succeeding
git-origin probes, which is the exact input Problem 1's flip consumes, and it routes new remotes into
`parse_slug`, which is Problem 2's gap. Phase 7 enriches the touch set the same way. Fixing coverage
before fixing the gate makes a live leak bigger in exchange for a number.

### Performance

Phase 7 turns one path-parse per edited file into one `git` invocation per distinct parent directory,
memoized. A session editing 200 files across 12 directories goes from 200 string operations to 12
spawns; reindex already spawns `git` once per session cwd. Measured before and after on the ~1,800-row
catalog in Phase 7; if a full reindex regresses meaningfully the cache key moves to the git common dir.

Phase 3's `ssh -G` is one spawn per distinct alias per process, memoized, and only for non-literal
hosts.

Phase 5's mutation run is slow by nature. Phase 5 decides `otto ci` versus a separate pre-merge task
on the measured runtime, not on a guess.

### Security

This doc is mostly a security fix, so the analysis is per-change rather than a single section.

- **Phase 2 narrows the gate. It does not widen anything, and in isolation it changes no row's
  scope.** A git-origin work slug is refused once clyde has recorded that the same cwd previously
  existed with no resolvable origin. **The RELEASE is not scope-neutral**, because Phase 3's host
  gate ships with it and strips pre-v13 rows on non-allowlisted hosts; the delta is measured by AC9
  and the populations are named in Data Model. Residual gap named in "The routing fix": a flip
  completing before clyde's first index leaves no record. The 6h reindex timer bounds that window.
- **Phase 2 also closes a forged-`Resolved` hole.** `run_git` inherits `GIT_DIR`, so before this
  branch an exported `GIT_DIR` attributes any cwd to an unrelated repo, and Phase 2's gate would
  route on that. Scrubbing the git environment is part of making `ProbeOutcome` truthful.
- **Phase 2's provisional-personal is a re-offer, not a re-send.** The gate records a skip before the
  transport, so a re-offered personal row spends no tokens and ships no bytes.
- **Phase 3 narrows further.** A remote-derived slug from an unrecognized host can attribute a repo
  but cannot confer work scope. Fails closed on an unresolvable host.
- **Phase 4's item 5 change is comments-only and moves no classification.** The precedence reversal
  the earlier draft proposed was withdrawn after review round 1: it would have dropped personal forks
  of work repos checked out in work directories. The cwd anchor still outranks the remote; the
  disagreement is warned and counted instead.
- **Phase 6 does not widen the gate by itself**, and it enlarges the input surface of Problems 1 and
  2. Both are closed by Phases 2 and 3, which is why the ship order is fixed.
- **Phase 7 widens the touch-set branch, and it is the only widening in the doc.** A session whose
  work-repo edits currently fail to resolve because the checkout is off-layout will, afterwards,
  produce a unanimous and total work touch set and classify as work. Unanimity and totality are both
  unchanged; a mixed touch set is still personal, an empty one is still personal, and a cwd anchored
  to a personal org is still personal first. The safer half is that a personal checkout's edits now
  resolve to a personal slug and break unanimity DIRECTLY, where today they are dropped and break
  totality by accident.
- **Residual risk, named:** a session run in a personal directory that edited only work-repo files
  ships to the work account. Already true today for on-layout machines; Phase 7 extends it to
  off-layout machines.
- `repos_touched` remains clyde's own parse of its own transcripts. After Phase 3 the remote-derived
  input to `is_work_slug` is host-validated, which is what restores the module's stated threat model.

### Testing Strategy

The register's root cause is that the tests could not see the defect. Four structural changes, in
order of how much they buy:

1. **Mutation testing as a CI gate** (Phase 5) on `scope.rs` and `repo.rs`. A test that does not bite
   is a surviving mutant. This is the only one of the four that catches register item 4's defect
   class automatically, forever, without anyone remembering to look.
2. **Tests drive the real resolver** (Phases 1 and 4). Deleting the `with_repo(..)` helper makes it
   impossible to assert on a row production cannot emit, which is how `/home/patrick` got asserted as
   work.
3. **Sequence tests, not snapshot tests** (Phases 1 and 2). The harness runs reindex, mutates the
   world, reindexes again, and asserts the invariant across the pair. Problem 1 is invisible to any
   single-classification test.
4. **Real `git init` fixtures, not mocked `run_git`** (Phases 1, 6, 7). The defects are in what git
   actually returns; a fake encodes the same wrong assumption the design doc did.

Every test named in a phase must be shown to BITE: delete the code it covers, watch it fail, restore.
Each phase names the exact deletion.

#### The checkout matrix

One shared fixture, built in a `TempDir` with its OWN `HOME` and its own `projects-dir`, never
against `~/repos`. Every row is a real `git init`. Phases 1, 3, 6 and 7 all assert against it, so a
new checkout shape is added in one place and every phase inherits it.

The matrix exists because this whole doc is a catalogue of shapes nobody tested. Rule 1 was declared
"layout-agnostic" over four rows that were all the same shape.

| # | shape | why it is in the matrix |
|---|---|---|
| 1 | flat clone, ssh remote | the baseline that already works |
| 2 | flat clone, https remote | `parse_slug`'s other main branch |
| 3 | subdir of a flat clone | `--show-toplevel` above cwd, containment check |
| 4 | sibling worktree at org level | the v0.21.0 shape |
| 5 | bare container CHILD (`<repo>/main`) | the row the old table DID test |
| 6 | **bare container ROOT** | **Problem 4. The row it did not test** |
| 7 | plain bare mirror (`mirror.git`) | `--git-common-dir` returns `.`, the `common == cwd` guard |
| 8 | cwd inside `.bare/refs` | git finds a repo above cwd; containment |
| 9 | git repo with NO origin | Problem 1's seed state |
| 10 | shape 9, then `git remote add origin` | **Problem 1's flip. The sequence test** |
| 11 | non-git directory | rule 1 declines, nothing stamped wrongly |
| 12 | `$HOME` itself a git repo | the blocked-root guard |
| 13 | bare repo AT `$HOME` | the guard bypass the `common == cwd` branch closes |
| 14 | `<tmp>/code/work/philo` (no org level) | Stephen's layout |
| 15 | `<tmp>/Projects/philo` (no org level) | Luke's layout |
| 16 | `<tmp>/git/tatari/philo` (org slot reads `tatari`) | Keegan's layout |
| 17 | `<tmp>` bare home, no repo | Patrick's layout; must stay personal (register item 4) |
| 18 | **personal FORK of a work repo, in a work directory** | register item 5. The case that killed the precedence change |
| 19 | remote on a non-allowlisted host | Problem 2 |
| 20 | remote via an ssh `Host` alias resolving to `github.com` | Problem 2's fix must not break this |
| 21 | a submodule whose `.gitmodules` names a hostile remote | Problem 2's attacker-authored vector |
| 22 | a checkout deleted after indexing | rule 2's learned path map |
| 23 | **`safe.directory` / dubious-ownership refusal** | **`Indeterminate` must NOT stamp. The panel's severest finding** |
| 24 | symlinked cwd whose `--show-toplevel` returns the canonical path | **CONFIRMED live bug.** Lexical `starts_with` fails; rule 1 declines every symlink-reached session today |
| 25 | same cwd reused by a DIFFERENT repo after delete and reclone | the probe record and the path map both key on the path string |
| 26 | archived session whose cwd no longer exists | must be `Indeterminate`, never a conclusive negative |
| 27 | pre-v13 `git-origin` row with NULL `repo_host` | the migration population named in Data Model; strip-only |
| 28 | **`GIT_DIR` exported, cwd unrelated to it** | **CONFIRMED live bug. Forges `Resolved` and passes containment** |
| 31 | **`GIT_CONFIG_COUNT`/`KEY`/`VALUE` and `GIT_CONFIG_GLOBAL` rewriting a personal origin to a work one** | **CONFIRMED live bug, leak direction. Why the fix is an allowlist, not a scrub list** |
| 32 | **hostile `~/.gitconfig` setting `remote.origin.url`, no env vars at all** | **CONFIRMED. Turns a conclusive `NoOrigin` into a forged work `Resolved`. Why the primitive is `--local` and why `HOME` is not forwarded** |
| 29 | broken submodule; `.git/config` unreadable | must be `Indeterminate`, never `NotARepo` |
| 30 | empty repo, no commits, no origin | must be `NoOrigin` (conclusive), distinguished from repo-discovery failure |

Rows 14 through 17 are the four teammate layouts the register verified once by hand. In the matrix
they become permanent regression tests, which is the difference between a measurement and a guarantee.

Rows 9 and 10 are one sequence, not two states. That pair is Problem 1, and it is the shape no
existing test has.

Rows 23 through 27 all came from the review panel. Every one of them is a way the probe record can
lie, and none of them was in the matrix I wrote before the panel ran.

Row 24 was raised as a hypothetical and turned out to be a confirmed live bug when I ran it. Details
and the measurement are under "The routing fix". It is fixed in Phase 6 by canonicalizing both sides
of the containment check, and `ProbeOutcome::OutsideRoot` keeps it from recording a conclusive
negative in the meantime.

### Rollout Plan

Phases land as separate commits on one branch, each `otto ci` green.

**Run `/review-panel` BEFORE merging, not after.** The register records that naming the panel and then
skipping it is the direct cause of the v0.22.0 regression, and this branch touches the same boundary.

Ship as one minor version: schema v13, `SCOPE_VERSION` 3, and a changed `parse_slug` signature. The
PR body must carry the required `clyde session reindex --reresolve-repo`, and the
lifted interim guidance.

**The branch ships whole. No phase is released on its own.** The staff engineer caught that the
earlier draft lifted the interim guidance at Phase 2, but Phase 2 does not close Problem 2: between
Phase 2 and Phase 3 a `git@evil.example.com:tatari-tv/x` remote still confers work scope. Phases are
independently COMMITTABLE, for bisect and review; they are not independently RELEASABLE.

The register's interim guidance therefore stands until the whole branch is released: **do not run
`clyde session enrich` on v0.22.0**, and do not ask teammates to. `report collect` and `render` never
consult scope, so the reporting half of the runbook is safe to keep using.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Phase 6 or 7 ships before Phase 2 and 3, enlarging a live leak | Low if ship order is honored | **High** | The ship order is stated in Dependencies and in every phase's position; Phase 2's test is the gate |
| A flip completes before clyde's first index, so no negative is recorded and it confers work | Low | High | Named in "The routing fix". Bounded by the 6h reindex timer. Strictly better than today, where distance does not matter. Revisit if a real instance is observed |
| A transient git failure is recorded as conclusive and locks a session out | Low after the fix | High | `ProbeOutcome::Indeterminate` records nothing; matrix row 23; `a_transient_git_failure_never_stamps` bites. Round 1's severest finding |
| Inherited `GIT_DIR`, `GIT_CONFIG_*`, or a hostile `~/.gitconfig` forges a `Resolved` attribution and routes it as work | **Live today, all three vectors reproduced** | **High** | `env_clear()` with a `PATH`-only allowlist, no `HOME`, and `git config --local --get remote.origin.url`; first step of Phase 2; matrix rows 28, 31, 32; AC11 |
| A future `GIT_*` variable opens a third forge channel | Medium over time | High | This is why the fix is an allowlist. A denylist was proposed twice and missed a channel both times |
| `scope_override` is used to paper over a classifier bug | Low | Medium | `--reason` required and stored, `--list` audits, `doctor` counts, warning when overriding a conclusive negative |
| The v13 scope delta is larger than the two named populations explain | Medium | Medium | AC9 measures it on a copy of the live catalog before merge; an unexplained row blocks the phase |
| Pre-v13 rows keep v0.22.0-era work authority until a live probe strips it | **Certain** | Medium | Strip-only by design: the probe can only remove authority, never grant it. Problem 2 is enforceable downward on old rows and fully closed on rows indexed at v13+. Counted by `doctor`, carried in the PR body and the runbook update |
| `scope_override` becomes a habit that hides a real classifier bug | Low | Medium | `doctor` counts override rows; a rising count is the signal to fix the rule instead |
| `ssh -G` alias resolution is slow or absent on a teammate host | Medium | Low | Memoized per alias; literal fallback; fails closed on scope, never on attribution |
| Mutation testing runtime makes `otto ci` unusable | Medium | Low | Phase 5 decides placement on the measured runtime; a separate pre-merge task is an acceptable outcome |
| Phase 5 finds a large survivor count and the phase balloons | Medium | Medium | Threshold stays ZERO. Measure first to size the work, then drive to zero by writing tests or annotating smallest-site `mutants:skip` with rationale. If the remainder is too large for one phase, split it into a named follow-on; do NOT raise the threshold |
| Phase 1 attributes a session via a `$GIT_DIR`-found repo | Low | High | Containment check mirrored into the new branch; test with a cwd inside `.bare/refs` |
| A bare repo at `$HOME` bypasses the blocked-root guard | Low | High | The `common == cwd` branch; `detect_still_blocks_a_bare_repo_at_a_blocked_root` |
| Phase 7 widens what ships to the work account | Certain, by design | Medium | Priced in Security; unanimity and totality unchanged |
| desk.lan cannot validate Phase 6 live | Certain | Medium | Constructed fixture for the test, Keegan's catalog for the number, null result stated not hidden |
| `safe.directory` / dubious ownership silently fails every rule 1 probe | Low | Medium | Pre-existing and out of scope. Phase 8's per-rule counts make it visible: `git-origin` at zero on a populated catalog is this, not a layout problem |

## Open Questions

None. Both questions carried by the previous draft are closed above: Phase 7 stays in this doc, and
register item 1's fix is decided here rather than deferred to another doc.

### Post-ship verification, NOT merge gates

Two facts cannot be established before shipping, and waiting on them would block the fix behind
people who need the fix in order to answer. They are verified after release, not before merge:

- **macOS confirmation of the v0.22.0 `$USER` fix.** A no-op on Linux by construction, so no host we
  can test from can confirm it. Confirm on the first macOS run after release.
- **Keegan's 12-session / $326.87 recovery.** desk.lan has three bare containers and zero session
  cwds at one, so it cannot produce the number. Confirm from his catalog after release.

Neither is unverified logic. Both are covered by the checkout matrix in Testing Strategy, against
real git. What the two humans add is a real-world instance of a shape the matrix already asserts, and
that is a post-ship confirmation of reach, not of correctness.

A third question was closed by measurement rather than asked: all 59 `origin` remotes across
`~/repos/tatari-tv/*` are `github.com`, so the `work-remote-hosts` default needs no input.

## References

- `docs/design/2026-07-31-scope-regression-handoff.md` (branch `origin/scope-regression-handoff`,
  commit `ad27fb9`, not on `main`): the v0.22.0 register. This doc owns items 1, 2, 3, 4, 5, 6, 8, 10
  and 11; item 7 is the next doc; item 9 is a `tatari-tv/marquee` issue.
- Keegan's bug report: `~keegan/clyde-repo-attribution-fails-at-a-bare-repo-container-root`
- Thread: https://tatari.slack.com/archives/C039YLDJW5T/p1785523797559079
- PR #82, the three teammate-blocking fixes: https://github.com/tatari-tv/clyde/pull/82
- PR #63, the last change to `common/src/repo.rs`: https://github.com/tatari-tv/clyde/pull/63
- `docs/design/2026-07-26-report-story-fidelity.md:326-336`, the verification table missing its row
- `docs/design/2026-07-31-open-defects-handoff.md`, the earlier register this one does not overlap
- `common/src/repo.rs:228-259` rule 1, `:368` `slug_under_root`, `:408` `parse_slug`
- `session/src/scope.rs:151-235` the classifier, `:240` `has_work_org`, `:268` `is_work_slug`
- `sessions/src/enrich.rs:105-160` the routing gate, `:115` the swallowed parse error
- `sessions/src/db/enrich.rs:210-215` `record_enrich_skip`, `:273-275` the candidate predicate
- `sessions/src/db/repo.rs:79-91` `upsert_repo`, `:100-109` `record_repo_path`
- `sessions/src/index.rs:77` `apply_chain` per session per pass
- `clyde/src/main.rs:883` `lazy_reindex`'s hardcoded projects dir
- `efficiency/src/outcome.rs:221-241` rule 3's bucketing
- `clyde/build.rs:3-7` the in-house precedent for resolving a bare container
