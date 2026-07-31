# Handoff: the v0.22.0 scope regression, and everything else still open

**Author:** Scott Idler
**Date:** 2026-07-31
**Status:** Handoff brief. Every item is diagnosed to the file and line, and the headline finding is
REPRODUCED end to end. Do not re-derive any of it.
**Audience:** the next agent picking up `tatari-tv/clyde`

v0.22.0 (PR #82, https://github.com/tatari-tv/clyde/pull/82) is tagged, pushed and installed. It fixed
three real defects and **introduced one security regression**, found by an adversarial review run after
the merge. This brief is the register of what is still open.

**Item 1 is a shipped regression that can send personal session content to the work Anthropic account.
Everything else waits behind it.**

## The register

- [ ] **1. HIGH / security / SHIPPED.** A git-origin re-probe retroactively flips an already-classified
      personal session to `work`. Reproduced. Introduced by v0.22.0.
- [ ] **2. MEDIUM / security.** `parse_slug` never validates the remote's HOST, and v0.22.0 newly feeds
      its output to the routing gate.
- [ ] **3. LOW / safe direction, permanent.** A stale git-origin probe reading `personal` is recorded as
      SETTLED, locking a genuine work session out of enrichment forever.
- [ ] **4. Test integrity.** `git_origin_classifies_every_real_world_layout` asserts an input the
      resolver can never emit, so it does not bite and it overstated the fix's coverage. Patrick's `~`
      layout is NOT fixed.
- [ ] **5. LOW / pre-existing.** The cwd path convention OUTRANKS the remote that v0.22.0 calls
      authoritative. Pick one framing.
- [ ] **6. MINOR.** `sessions/src/enrich.rs:115` swallows an error `RepoSource::from_str` raises loudly
      on purpose.
- [ ] **7. 278 archived rows are never re-parsed**, so `title` and `activity_at` never update on them --
      despite clyde holding a staged copy of every one.
- [ ] **8. `lazy_reindex` ignores the configured `projects-dir`** that explicit `reindex` honors. Two
      reindex paths, two different answers.
- [ ] **9. `marquee mcp register`** refuses to update an existing entry after the binary moves. Different
      repo (`tatari-tv/marquee`), cannot ride a clyde PR.
- [ ] **10. The runbook is stale in the user's favour** and is deliberately NOT updated until item 1
      lands.
- [ ] **11. A stashed doc+test** on the git-origin trust boundary is partly SUPERSEDED by items 1 and 2;
      rework it, do not apply it as-is.

## What is already verified -- do not re-measure

The layout-independence win is real, and items 1-3 do not invalidate it. Measured in an isolated
sandbox (its own `HOME`, no `~/repos` anywhere, no `<org>/<name>` nesting, no `repo-root` configured):

| layout | shape | attributed | routed |
|---|---|---|---|
| `$SB/Projects/philo` | flat, no org level | `tatari-tv/philo` via `git-origin` | work |
| `$SB/code/work/marquee` | nested, no org level | `tatari-tv/marquee` via `git-origin` | work |
| `$SB/git/tatari/clyde` | org level reads `tatari` | `tatari-tv/clyde` via `git-origin` | work |
| `$SB/wt/persona-cli` | bare worktree, https remote | `tatari-tv/persona-cli` via `git-origin` | work |
| `$SB/src/mine/sideproject` | personal | `scottidler/sideproject` | refused |
| `$SB` (bare home) | no repo | none | refused |

v0.21.0 scores **0%** on that same set, proven structurally rather than asserted: no cwd carries a
`repos/` component, and every row's touch set is empty, so both of its signals abstain.

Also settled:

- **Titles:** 61 rows over 200 chars went to **2** on a copy of the live catalog; FTS/column mismatches
  **0**. The 2 survivors are item 7.
- **The `$USER` fix is UNVERIFIABLE on Linux** by construction -- it is macOS Keychain-specific and the
  `getpwuid` fallback makes it a no-op here. It needs a macOS confirmation from Luke, Stephen or Calvin.
  Do not mark it verified on desk.lan evidence.

---

## 1. A git-origin re-probe retroactively flips a personal session to work

**Status: REPRODUCED end to end. This is a regression v0.22.0 introduced. It is the reason nobody should
run `clyde session enrich` on v0.22.0 yet.**

### The mechanism, pinned

The claim v0.22.0 shipped on was: "`git-origin` is not a widening, because it is a statement about the
session's OWN cwd, which is the authoritative form of what the path convention approximates."

That is true about WHAT it describes and false about WHEN. The two signals read different eras:

- Step 1 (`has_work_org`, `session/src/scope.rs:240`) parses the RECORDED `cwd` string, captured when the
  session ran and immutable afterwards.
- Step 3 (`session/src/scope.rs:197`) reads `sessions.repo_source`, written by a LIVE `git` subprocess
  against that path at whatever moment the last reindex ran
  (`common/src/repo.rs:229` `detect_with_blocked_roots`).

The only thing linking them is the path STRING, and the re-probe is not a one-shot. `reindex` calls
`apply_chain` for EVERY parsed session on EVERY pass (`sessions/src/index.rs:77`), and `upsert_repo`
writes on any strict rank improvement (`sessions/src/db/repo.rs:79`). A row that never reached rank 0 is
re-probed indefinitely and flips the first time a probe succeeds.

### The repro, executed

Run in an isolated sandbox against installed v0.22.0:

1. `$SB/side-project` is a git repo with **no** `origin` remote. Its session content is personal
   (seeded with "divorce paperwork" / "eurorack build" so a leak would be unmistakable).
2. `clyde session reindex` -> `repo = NULL`, `repo_source = NULL`, `repo_rank = 99` (the schema default).
   `clyde session enrich --dry-run` -> **`scope=personal`, `would-send=False`.** Correct.
3. Adopt the project into the org. **One command, no directory change, nothing about the session
   touched:**
   ```
   git remote add origin git@github.com:tatari-tv/side-project.git
   ```
4. `clyde session reindex`. The log records the flip verbatim:
   ```
   repo::Resolver::resolve: .../side-project -> tatari-tv/side-project via git-origin
   Db::upsert_repo: session_id=99999999-... repo=tatari-tv/side-project source=git-origin rank=0
   ```
5. `clyde session enrich --dry-run` -> **`scope=work`, `would-send=True`, payload 142 bytes.**

The personal-phase transcript is now queued for the work Anthropic account. `gh repo create
tatari-tv/<x> --source=.` produces the identical state and is an ordinary workflow.

### What LIMITS it, measured -- and why that is not a fix

A row whose `scope_version` is already **2** is excluded by `enrich_candidates`
(`sessions/src/db/enrich.rs:274`), and step 5 above then reports `considered: 4` with the row absent.
**Verified: a settled row does NOT flip.**

The exposure is rows whose `scope_version` is NULL, which is the PROVISIONAL state
(`sessions/src/enrich.rs:142`). That happens whenever the classification was made before a full explicit
reindex reached the row, because `clyde session enrich` refreshes through `lazy_reindex`, which runs the
content reindex only and never `reindex_efficiency` -- the sole writer of `outcome_json`
(documented at `sessions/src/db/enrich.rs:29`).

**That is exactly the state of a teammate host**: the population this change was built for. The runbook's
`reindex`-before-`enrich` ordering narrows it, but a security invariant held up by documented command
ordering is not held up.

### Fix directions (pick one; all keep the layout-independence win)

- **(a) Bound the observation to the session.** Require the git-origin observation to bracket the
  session's activity window. `repo_paths` already carries timestamps (`sessions/src/db/repo.rs:100`
  `record_repo_path`), and `activity_at` now exists on every row, so both sides of the comparison are in
  hand.
- **(b) Freeze the decision.** Forbid a later probe from upgrading a recorded `personal` to `work`;
  only a `SCOPE_VERSION` bump or an explicit operator action may.
- **(c) First-sight only.** Accept git-origin only when it was resolved during the pass that first
  indexed the session.

(a) is the most faithful to "what repo was this session actually in", and it is the one to price first.

---

## 2. The remote's HOST is never validated

**Status: confirmed by reading and by executing `parse_slug` against crafted URLs.**

`parse_slug` (`common/src/repo.rs:408`) strips a scheme and discards everything up to the first `/` or
`:` -- every branch is `let (_, path) = ...`. The host is never examined:

```
git@github.com:tatari-tv/philo.git          -> Some("tatari-tv/philo")   work
git@evil.example.com:tatari-tv/x.git        -> Some("tatari-tv/x")       work
https://evil.example.com/tatari-tv/x        -> Some("tatari-tv/x")       work
http://10.0.0.5:8080/tatari-tv/x            -> Some("tatari-tv/x")       work
ssh://git@gitea.local:2222/tatari-tv/x.git  -> Some("tatari-tv/x")       work
```

The `<org>/<repo>` SHAPE guards are sound (`tatari-tv-evil/x` and `foo/tatari-tv` are both refused, an
empty segment is refused, and `repo.contains('/')` at `common/src/repo.rs:439` refuses a third segment).
The host is the only gap.

**The exposure is new to v0.22.0.** Before it, `is_work_slug` (`session/src/scope.rs:268`) only ever saw
`repos_touched` keys, which `slug_under_root` derives from LOCAL paths under `repo_root`. Step 3 newly
feeds it a string derived from a git REMOTE URL, so the module's stated threat model ("the hazard is
ABSENCE, not forgery") no longer covers its input. `.gitmodules` in a third-party clone is
attacker-authored content that reaches this path.

An earlier in-session defense of this ("no worse than `mkdir -p ~/repos/tatari-tv/x`") is recorded here
as REJECTED: the mkdir comparison is true but does not cover the submodule vector, and it reasons about
the wrong trust class.

**Fix:** have `parse_slug` return or validate the host, and require a known GitHub host before a
remote-derived slug may confer work scope. **Do not use a strict `github.com` string test** -- SSH `Host`
aliases (`git@github-work:tatari-tv/x`) are ordinary config and would reintroduce the 0%-coverage bug.
Resolve the alias, or allowlist by configuration.

Zero test coverage today: every `parse_slug` test uses `github.com`
(`common/src/repo/tests.rs:38-71`), and no scope test uses a non-GitHub host.

---

## 3. A stale probe can permanently lock a work session out of enrichment

**Status: confirmed by reading; the mirror of item 1, in the safe direction.**

A session that genuinely ran in a work repo, whose path now holds a personal checkout, classifies
`personal` with `Basis::GitOrigin`. Because `!reads_stored_evidence()` holds for that basis, the caller
writes `scope_version = 2` -- SETTLED (`sessions/src/enrich.rs:142`). The predicate at
`sessions/src/db/enrich.rs:274` then excludes it on all four disjuncts.

Restoring the work checkout does not recover it. Only another `SCOPE_VERSION` bump, `--all`, or
`--only <id>` will. Directionally safe, permanently wrong, and silent.

Whatever fix item 1 takes should settle this at the same time: they are the same
observed-at-the-wrong-time defect pointing in opposite directions.

---

## 4. A test asserts an input production cannot emit

**Status: confirmed. This one is a process failure, not just a bug.**

`session/src/scope/tests.rs:282` asserts that cwd `/home/patrick` classifies work via
`repo_source = "git-origin"` -- Patrick being one of the four measured 0%-coverage teammates.

**The resolver can never produce that input for that path.** `detect_with_blocked_roots`
(`common/src/repo.rs:229`) rejects a toplevel that matches a blocked root, and `blocked` is `[$HOME]`
(`common/src/repo.rs:224` `home_dir_as_blocked`). For cwd `/home/patrick` the toplevel IS `$HOME`, so it
is rejected; and if `$HOME` is not a repo at all, `rev-parse` fails and it is rejected anyway.

Consequences, both of which need fixing:

- **Patrick's `~` layout is NOT fixed by v0.22.0.** Any claim otherwise (including in the PR body) is
  wrong and should be corrected.
- The test hand-built impossible data, so it does not bite and it inflated the measured win. Replace the
  `/home/patrick` row with the true expectation: a `~`-cwd session stays `personal`, and say why.

A `~`-cwd session can only be placed by the touch set, which requires the totality check to pass -- so
for Patrick specifically, the honest answer is that his coverage improves only where every edited file
lands inside a work checkout.

---

## 5. The path convention outranks the "authoritative" remote

**Status: pre-existing, not introduced by v0.22.0. Recorded because the design now contradicts itself.**

Steps 1 and 2 return before step 3 is reached (`session/src/scope.rs:158-196`), so
`~/repos/tatari-tv/<a-personal-clone>` classifies **work** by the directory name even though the remote
says personal. If git-origin is authoritative it should not be outranked by the approximation; if it is
a fallback, v0.22.0's justification needs rewording. Pick one and make the code and the comments agree.

---

## 6. `.parse().ok()` swallows a deliberately loud error

`sessions/src/enrich.rs:115`:

```rust
rec.repo_source.as_deref().and_then(|s| s.parse().ok()),
```

`RepoSource::from_str` raises on an unrecognized value on purpose -- `common/src/repo.rs:81` explains
that "a silently-dropped provenance would let a guess be rendered as an observation." This is the one
call site that undoes that. An unrecognized value becomes `None` and falls through to the touch set:
safe direction, but it should `warn!` at minimum.

---

## 7. 278 archived rows are never re-parsed

**Status: measured on the live catalog. Pre-existing, not a v0.22.0 regression.**

Every archived row keeps `parse_version = NULL` forever:

```
archived=0  parse_version=2   1862 rows
archived=1  parse_version=-1   278 rows   (NULL)
```

Their live transcripts are reaped from `~/.claude/projects`, so `scan::find_session_files` never finds
them, `parse_sessions` never parses them, and the narrow backfill never sees them -- **even though
`staged_path IS NOT NULL` for all 278.** clyde has a durable copy of every one and does not read it.

Consequences: their titles keep the old raw-prompt form (2 of the catalog's remaining long titles are
here), and `activity_at` stays NULL so `dormancy_at()` falls back to `modified` for them, which is the
mtime defect v0.21.0 fixed for everyone else.

This is the same category error v0.20.0 fixed for COST ("archived means the transcript moved, not that
the session did not happen"), still present in the PARSE path. Teaching `reindex` to parse from staged
copies changes what `scanned` means and interacts with `reconcile_archived`, so it wants its own change.

---

## 8. `lazy_reindex` ignores the configured `projects-dir`

**Status: confirmed by reading, and it corrupted a sandbox during this session's testing.**

- `cmd_reindex` resolves the projects dir from config (`cfg.projects_dir()`).
- `lazy_reindex` uses the hardcoded platform default instead:
  `clyde/src/main.rs:883`, `session::paths::claude_projects_dir()`.

So `clyde session reindex` honors a configured `projects-dir` and `clyde session enrich` silently does
not. Setting the key and then running `enrich` pulls in the real `~/.claude/projects` regardless.

The config field's own doc scopes it to `clyde mcp serve`, so this may be intentional drift rather than
an oversight -- but two reindex paths resolving the same input differently is a trap either way, and it
makes isolated testing of `enrich` impossible without overriding `HOME`.

---

## 9. `marquee mcp register` will not update a moved binary

Reported by Luke on 2026-07-31: after the `marquee` binary moves, `marquee mcp register` refuses to
update the existing entry; `claude mcp remove marquee -s user` followed by a re-register works.

Different repo (`tatari-tv/marquee`), so it cannot ride a clyde PR. Tracked here so it is not lost.

---

## 10. The runbook is stale, deliberately

https://marquee.internal.tatari.dev/p/~scott-idler/claude-usage-report-pipeline-runbook

It currently documents the scope fix as partial and tells readers to expect low coverage, which was true
at v0.21.0 and understates v0.22.0. It is deliberately NOT updated yet: telling teammates to re-run
`enrich` is the exact action item 1 makes unsafe. Update it in the same arc that closes item 1, and fold
in the item 4 correction about the `~` layout.

---

## 11. A stashed doc+test, partly superseded

`git stash` on this machine holds a doc comment plus
`scope_pins_the_git_origin_trust_boundary` covering the unvalidated host. It is CI-green but its
reasoning is weaker than item 2's and its "no worse than mkdir" framing is wrong. **Rework it against
item 2 rather than applying it.** If the stash is gone, nothing is lost -- item 2 has the full analysis.

---

## Suggested route

- **Items 1 + 2 + 3 together: `/create-design-doc`.** They are one change to the routing invariant that
  decides whether a transcript leaves the machine, and item 1 is a shipped regression, so it gets the
  full funnel and a security section. Name the v0.22.0 dependency explicitly.
- **Item 4: targeted fix**, and it rides that doc's phase that touches scope tests. Correct the PR
  body's coverage claim at the same time.
- **Items 5, 6, 8: targeted fixes.** Small, independent, no doc.
- **Item 7: `/create-design-doc`.** It changes what a reindex scans.
- **Items 9, 10: not clyde changes.** 9 is a marquee issue; 10 is a publish, gated on item 1.
- **Run `/review-panel` BEFORE merging the item 1-3 fix, not after.** The regression in item 1 shipped
  because the panel was named as a plan and then skipped, and a "green CI" report went out ahead of the
  review that would have caught it. That sequencing error is the actual root cause of this brief
  existing.

## Interim guidance

Until item 1 lands: **do not run `clyde session enrich` on v0.22.0**, and do not ask teammates to. A full
explicit `clyde session reindex` settles rows at `scope_version = 2` and closes the window for the rows
it reaches, but that is a mitigation, not a fix, and it does nothing for rows added afterwards.

`clyde report collect` / `render` are unaffected -- they never consult scope.
