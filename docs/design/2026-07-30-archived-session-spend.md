# Design Document: Archived Session Spend

**Author:** Scott Idler
**Date:** 2026-07-30
**Status:** Implemented (branch `archived-session-spend`, 2026-07-31). PR #78's gate was satisfied
before Phase 1: it merged as `main` @ `0bb133f` (v0.19.0). Implementation notes:
`docs/design/2026-07-30-archived-session-spend-implementation-notes.md`
**Review Passes Completed:** 5/5 authoring passes + 1 cross-model review panel round (all findings
dispositioned, no open pushbacks)

## Summary

clyde's cost surfaces read the `archived` flag as "this spend did not happen." It actually means
"the transcript is no longer at `~/.claude/projects`." Every session whose live JSONL has aged off
disk is silently dropped from `clyde report collect`, `clyde cost`, and `clyde efficiency`, even
though clyde already holds a durable staged copy of its transcript. This doc fixes the three sites
that embed that category error, and makes the residue that is genuinely unrecoverable disclose
itself instead of reading as zero.

## Problem Statement

### Background

- Slack thread `C039YLDJW5T` (`p1785432360721679`): Keegan, Patrick Shelby, Stephen, escote report
  clyde v0.18.0's cost numbers running "at least 30% lower than the web UI shows."
- Scott's ask, verbatim: "how do we ensure this is not the case." Root-cause it, then fix it.
- Requirement traceability: Phases 1-4 are the fix for the reported symptom. Phase 5 (prevention) is
  read off the "ensure this is not the case" half of that ask rather than a separate request, and it
  is called out here so Scott can cut it if he reads the ask narrower than that. Phase 6 is the house
  convention that shipped behavior lands in the living docs.
- Ground truth for this doc is the Claude Enterprise Analytics API, `claude_code` product, **June
  2026** (a settled month). Same-day pulls understate by up to ~30 days of revision, which produced
  one false conclusion earlier in the investigation before it was redone against settled data.
  - scott.idler@tatari.tv: **$9,110.96**
  - patrick.shelby@tatari.tv: $908.47
  - keegan@tatari.tv: $940.59
  - stephen@tatari.tv: $3,801.43

### Problem

`sessions.archived` is a **transcript-availability** flag. `reconcile_archived`
(`sessions/src/db.rs:690`) sets it when `transcript_path` no longer exists on disk and clears it if
the file reappears. It says nothing about whether the session happened or what it cost.

Three sites read it as a spend-existence flag:

1. **`sessions/src/db.rs:379`** -- the efficiency/cost backfill predicate is
   `WHERE efficiency_json IS NULL AND archived = 0`. An archived row can therefore never be priced,
   staged copy or not. Its `cost_usd` and `efficiency_json` stay NULL forever.
2. **`report/src/lib.rs:209`** -- `report collect` builds its window with `include_archived: false`.
   Even a priced archived row would not be counted.
3. **`cost/src/lib.rs:275`** -- `clyde cost` scans only `~/.claude/projects`. A reaped session's
   bytes are at `~/.local/share/clyde/staged/<id>/`, which nothing scans.
   `efficiency::collect_all` / `collect_matching` have the same blind spot, so
   `clyde efficiency daily|weekly|--worst` undercounts too.

And one downstream disclosure defect, found by the review panel: **`report/src/merge.rs:179`**
constructs a merged report with `notes: vec![WINDOW_NOTE.to_string()]`, discarding every input
report's notes. So a per-host report that correctly states "64 sessions excluded, unrecoverable"
merges into a multi-host report with the partial spend and no disclosure at all. That defeats the
`notes` channel this doc leans on, so it is in scope (Phase 3) rather than parked.

The report's own fail-closed guard (`report/src/lib.rs:244`) cannot catch any of this: it counts
windowed rows with a NULL `efficiency_json`, and the archived rows are filtered out one statement
earlier, before the guard ever sees them.

Measured on `desk.lan` against the settled June window, on `main` @ `4b8eec7`:

| Surface | June 2026 | vs ground truth $9,110.96 |
|---------|-----------|---------------------------|
| `clyde report collect` | $4,393.83 (359 sessions) | -51.8% |
| `clyde cost monthly` | $4,818.54 | -47.1% |

The catalog holds **558** sessions in that window. **199** are archived (35.7%), every one of them
has `cost_usd IS NULL` and `efficiency_json IS NULL`, and every one of them **has a staged copy on
disk**. 558 - 199 = 359, exactly what the report counted.

The May 2026 window is the same failure with the volume turned up: 79 catalog rows, all 79 archived,
and `clyde report collect --since 2026-05-01 --until 2026-06-01` returns
`{"sessions": 0, "spend-usd": 0.0}` **and exits 0**. A month with 79 real sessions renders as a
zero-usage month, with nothing in `notes` saying otherwise. The comment above that code path states
the intent it fails to deliver: "An empty window on an EMPTY catalog is NOT a zero-usage month."

**The gap grows with window age.** July on the same host has 0% archived sessions and a ~15% gap.
June has 35.7% archived and a ~50% gap. May is 100% archived and a 100% gap. Every user's history
ages into this, not just multi-host users.

**And there is a second, worse leak behind the first.** 64 rows on `desk.lan` are archived with NO
staged copy: their transcripts aged off disk before any staging sweep ran, and their dollars are
gone permanently, all of them May 2026. That is not a historical accident, it is the leading edge.
Staging is a manual command today, and right now:

```
SELECT COUNT(*) FROM sessions WHERE archived=0 AND staged_path IS NULL
  AND modified <= strftime('%Y-%m-%dT%H:%M:%S','now','-7 days');
-> 1471
```

(`strftime` with the `T`, not `datetime()`: `modified` is stored RFC 3339, and `datetime()`'s
space separator makes the lexical comparison silently mean "on an earlier date" instead of
"before this instant." Both forms happen to return 1471 today, which is exactly how a query that is
right by luck survives review.)

1,471 live-but-dormant sessions have no durable copy. Every one is on track to become another
permanently unpriceable row. Phase 2 recovers what is still on disk; without Phase 5 the same hole
keeps swallowing sessions.

### Phase 0 spike (complete, zero code): the money is in the staged copies

The fix is worth nothing if the staged copies are empty or unparseable. Measured, not assumed:

- 199 staged dirs for the June archived rows, 371 JSONL files total (199 parents + 172 subagents).
- Symlinked those 199 staged dirs into a scratch tree and pointed `clyde cost` at it:
  `clyde cost --path <tree> --no-cache monthly` -> **`2026-06: $2,924.42`** (plus $119.23 dated
  2026-05, since the window is by per-entry timestamp and a session whose catalog `modified` lands
  in June can hold May-dated records).
- That figure is a **lower bound**. The scratch layout puts each session's staged dir where a
  project dir goes, so `find_session_files` matched the 199 parent transcripts and skipped the 172
  `subagents/*.jsonl` files. Real recovery is higher.
- $4,393.83 + $2,924.42 = $7,318.25, closing 62% of the $4,717 report gap from parent transcripts
  alone. $4,818.54 + $2,924.42 = $7,742.96, closing 74% of the `clyde cost` gap.
- One staged file logged a JSON parse warning on one line. `claude_pricing::parse` warn-and-skips
  it, which is the existing contract. A staged copy is a snapshot of an appending file, so a torn
  final line is expected and costs at most that session's last record.

Conclusion: the staged copies hold the missing money and the existing parser reads them.

### Ship order: PR #78 lands first

Not negotiable, and it is the reason no code should start today. PR #78
(https://github.com/tatari-tv/clyde/pull/78, branch `excise-api-key-followups`) is open, CI green,
`MERGEABLE`, and touches 95 files including **every file this doc modifies**:
`common/src/scan.rs`, `session/src/paths.rs`, `session/src/stage.rs`, `sessions/src/db.rs`,
`sessions/src/index.rs`, `sessions/src/stage.rs`, `efficiency/src/persist.rs`, `report/src/lib.rs`,
`report/src/merge.rs`, `report/src/report.rs`, `cost/src/lib.rs`, `clyde/src/main.rs`,
`clyde/src/bootstrap.rs`, `clyde/src/doctor.rs`, `README.md`.

What that costs if ignored:

- **Every `file.rs:line` in this doc is against `main` @ `4b8eec7` and will shift.** `sessions/src/db.rs`
  alone moves 54 lines. Re-resolve each reference against the post-merge tree before Phase 1; the
  named symbols are the durable anchors, the line numbers are not.
- **Phase 5's harvest target is renamed by #78.** `install_clyde_timer` on `main` becomes
  `ensure_enrich_unit` (`clyde/src/bootstrap.rs:928` on that branch) behind a new `Systemd` trait
  with a `start_enrich_timer` method (`:134,:145`). The trait is the better harvest anyway: it makes
  the timer install testable without touching real systemd. Phase 5 is authored against the
  POST-#78 shape, not main's.
- `clyde/src/bootstrap.rs` is a net -443-line rewrite in #78 and `clyde/src/doctor.rs` gains 75
  lines. Phase 5 edits both. A merge conflict there is not a rebase, it is a re-read.

What #78 does NOT do, verified by diffing it rather than assuming: it changes nothing in the
`archived` / `include_archived` / `notes` logic (`git diff origin/main...excise-api-key-followups`
over `report/src/merge.rs`, `report/src/lib.rs`, `sessions/src/db.rs` filtered for those tokens is
empty), and it leaves the per-file dedup in `efficiency/src/extract.rs` and `fold.rs` untouched. So
none of this doc's findings are pre-empted or already fixed; the collision is textual, not
semantic. Most of those small cross-tree diffs are the 2026-07-30 em-dash purge.

### Goals

- Every catalog session with readable bytes anywhere (live or staged) is priced.
- `report collect` counts spend for the window regardless of transcript availability.
- `clyde cost` and `clyde efficiency` see reaped-but-staged sessions, exactly once each.
- A session with no bytes anywhere is **stated**, never rendered as $0.
- The reap-before-stage race that created the unrecoverable residue is closed.

### Non-Goals

- **Cross-file subagent double-billing, on the `efficiency`/`report` path only.** The same assistant
  `message.id` appears in both a parent transcript and its `subagents/agent-*.jsonl`;
  `efficiency::extract`'s `seen_usage_msg_ids` dedup is per-file
  (`efficiency/src/extract.rs:359`) and `fold` adds no cross-file check
  (`efficiency/src/fold.rs:85`). Confirmed real: 161 instances, $7.61 of $9,130 in July (0.08%).
  **`clyde cost` is NOT affected**: it dedups globally across files on `(message_id, request_id)`
  (`cost/src/lib.rs:414-430`). The previous draft stated this as a whole-product bug; the review
  panel corrected the scope. Parked. Revisit condition: once this doc ships, if
  `report render --reconcile` still shows an overcount above 1%.
- **Making `--reconcile` self-serve.** `report/src/reconcile.rs` needs an
  `ANTHROPIC_ENTERPRISE_SPEND_REPORTING_API_KEY`-produced export, which individual engineers cannot
  generate. Real problem, separate doc.
- **The two Slack narrative bugs** (dormancy sweep counting 0 at 7d but 44 at 1h; `repo:null` /
  `skipped-personal` for sessions launched from `~`). Neither moves a dollar. The dormancy symptom
  is an external mtime touch, not a clyde bug; the repo-classification fail-safe is deliberate.
- **Multi-host aggregation.** `clyde report merge` already covers it.
- **Unifying the two `SessionFile` types** (`common::scan::SessionFile` with mtime/size vs
  `session::model::SessionFile` without). Pre-existing duplication, named here so the next reader
  is not surprised, not touched by this work.

## Proposed Solution

### Overview

Stop asking "is this row archived?" and start asking "where are this session's bytes?" There is
already exactly one answer to that question in the tree:
`sessions::transcript::transcript_layout_parts` resolves a record to a `(parent, subagents_dir)`
pair, preferring the live transcript and falling back to the staged copy, by the presence of a
regular `.jsonl` file. Three consumers already route through it: `enrich`
(`sessions/src/enrich.rs:123`), `export` (`sessions/src/db/query.rs:149`), and the MCP content tools
(`sessions/src/mcp.rs:331,390`). The cost path is the one consumer that does not, and that is the
whole bug.

Five changes plus a docs pass, in dependency order. The numbers are the phase numbers.

1. `common::scan` learns to enumerate an explicit layout, and to union a projects scan with a staged
   scan under live-then-staged precedence.
2. The backfill prices every un-annotated row from wherever its bytes are, and counts the ones with
   no bytes anywhere.
3. `report collect` includes archived rows in the window, and splits its fail-closed guard into
   "not yet indexed" (fail, remedy) and "gone forever" (proceed, disclose).
4. `clyde cost` and `clyde efficiency` scan the staged root for sessions with no live transcript.
5. Prevention: staging runs inside `session reindex`, so a durable copy exists before the TTL can
   win. Without this, the unrecoverable count keeps growing and the fix is not causal closure.
6. README and the implementation notes state the new semantics.

Phases 2 and 3 both depend on 1. Phase 3 depends on 2 (an included-but-unpriced archived row would
trip the fail-closed guard). Phase 4 depends only on 1. Phase 5 is independent of 2-4.

### Architecture

```
                    sessions::transcript::transcript_layout_parts
                    (live-then-staged, by regular-file presence)
                                     |
        +----------------------------+----------------------------+
        |                            |                            |
   enrich, export,          efficiency backfill            report collect
   MCP content tools        (Phase 2, new consumer)        (Phase 3, new consumer)
                                     |
                       common::scan::layout_files
                       (explicit layout -> Vec<SessionFile>)

   clyde cost, clyde efficiency  ->  common::scan::find_session_files_with_staged
                                     (live scan + staged-only sessions)
```

Two discovery shapes, because the two sides know different things:

- **Catalog-driven (report, backfill):** the row carries `transcript_path`, `project_dir`,
  `staged_path`. Resolve the layout per row. No tree walk, and the resolution is per-session exact.
- **Filesystem-driven (`clyde cost`, `clyde efficiency`):** no catalog, so precedence is computed
  from the two roots: scan live, collect the live `group_id` set, then admit a staged session only
  if its id is absent from that set.

The precedence rule is the same rule in both shapes, which is what keeps the two independent
pricing pipelines agreeing (they currently agree within 1-9%, so they are one shared upstream gap,
not two divergent code paths).

### One definition of "recoverable"

The review panel's hardest question: what exactly makes a session recoverable, given that the DB
predicate, the resolver, and the acceptance SQL could each answer differently? Answering it three
ways is how a design passes its own SQL checks while the Rust path excludes different rows. So there
is exactly one answer, and it is a function, not a column:

> **A session is recoverable iff `layout_files` returns a NON-EMPTY vec for it**, live layout first,
> staged layout second. Empty means no readable bytes anywhere, which is the only unrecoverable
> state.

Three things this deliberately is NOT:

- **Not `staged_path IS NOT NULL`.** `stage_dormant` records `staged_path` whenever
  `files_total > 0` (`sessions/src/stage.rs:37-44`), and `stage_session` will stage subagents even
  with no live parent (`session/src/stage.rs:26-30`). So the column can be set for a session with no
  staged parent transcript. The column is a proxy, and this doc does not branch on proxies.
- **Not `transcript_layout_parts` returning `Some`.** That resolver requires a regular
  `<staged>/<session-id>.jsonl` parent (`sessions/src/transcript.rs:48-52`), because its job is
  "where do I parse this session's BODY from" for enrich/export/FTS, and a body needs the parent.
  Pricing asks a different question: "which files hold this session's usage records," and a subagent
  file holds real ones. Using the body resolver for pricing would discard a subagent-only session's
  spend even though its bytes are right there. The two questions stay two functions, with this
  paragraph as the reason, so nobody later collapses them.
- **Not a new column.** Same reason as the rejected Alternative 3: a stored copy of a filesystem
  fact can go stale.

**How far apart are the two predicates in practice? Measured: zero rows.** All 214
archived-with-`staged_path` rows on `desk.lan` resolve a real staged parent (June 199/199, May
15/15). That is not luck, it is structural: `reconcile_archived` flags `archived` off the PARENT
transcript's existence (`sessions/src/db.rs:702`), and `staging_candidates` only offers
non-archived rows, so any session reaching `stage_session` still had its live parent to copy. The
subagent-only case can therefore only arise in the narrow window where the parent is reaped between
the reconcile pass and the staging copy of the same run. Accepting subagent bytes closes even that
window, and costs nothing to do.

Consequence for the acceptance criteria: they must not use `staged_path IS NOT NULL` as a stand-in
for recoverability. A1 and A4 below read the counts clyde itself reports, and the SQL appears only
where it measures something the resolver does not define.

### Edge cases, decided

- **A staged dir whose name is not a UUID-v4.** `find_session_files` BAILS on a non-UUID name in the
  projects tree, because a misnamed file there could be misclassified as a parent or a subagent. The
  staged walk WARNS and skips instead. The names differ in kind: the staged filename is *derived*
  from the directory name (`<dir>/<dir>.jsonl`), so a wrong name finds nothing rather than
  misclassifying something, and bailing would let one stray directory in a clyde-owned cache brick
  every `clyde cost` invocation. Deliberate asymmetry, with this as the reason.
- **`staged-dir` configured equal to `projects-dir`.** Safe by construction: every id is already in
  the live set, so the staged pass admits nothing. No guard needed.
- **A live parent that is zero bytes with a non-empty staged copy.** `make_parent` skips zero-byte
  files, so the group enters the live set only if some other live file for it survived. If a live
  subagent did survive, the staged parent is skipped and that session undercounts, exactly as it
  does today. Accepted: it requires a live file truncated after staging, the blast radius is one
  session, and the failure direction is never a double count.
- **A truncated live transcript alongside a complete staged copy.** `transcript_layout_parts` prefers
  live unconditionally. Pre-existing behavior shared with `enrich`, `export`, and the MCP content
  tools; changing it here would fork the precedence rule for one consumer, which is the class of
  divergence this doc is removing.
- **Reports generated before this ships.** They silently omit archived sessions and carry no marker
  saying so, so a `report merge` mixing pre- and post-fix artifacts understates by whatever the old
  one dropped. No schema-version bump: `SCHEMA_VERSION` gates the merge coverage rules and bumping
  it for a collect-side fix would invalidate artifacts whose shape did not change. The remedy is to
  re-run `report collect` for the window, which reads only the catalog and no JSONL, so it is cheap.
  Called out in Phase 6's README bullet.

### Data Model

No schema migration. The classification is a filesystem fact, derived at read time by the one
existing resolver, so there is no new column that can diverge from the bytes on disk.

One new type in `sessions`, carrying what the backfill needs to resolve a layout:

```rust
/// One un-annotated catalog row plus the three path fields `transcript_layout_parts` needs.
/// Returned by `Db::sessions_missing_efficiency` so the backfill resolves live-or-staged per row
/// instead of walking the projects tree and filtering.
pub struct EfficiencyCandidate {
    pub session_id: String,
    pub transcript_path: PathBuf,
    pub project_dir: String,
    pub staged_path: Option<PathBuf>,
}
```

`PersistStats` gains one field:

```rust
pub struct PersistStats {
    pub candidates: usize,
    pub computed: usize,
    pub written: usize,
    /// Candidates with NO readable transcript, live or staged: nothing left to price, ever.
    /// Reported so `computed < candidates` is never a silent delta.
    pub unrecoverable: usize,
}
```

`Report::notes` (already `Vec<String>`, already the channel for "stated, never silently zeroed")
carries the disclosure line. No new report field.

### API Design

```rust
// common/src/scan.rs

/// Every JSONL in one session's explicit layout, as `SessionFile`s carrying the mtime/size
/// `filter_by_date_range` and cost's cache hash need. Mirrors `session::parse`'s
/// `discover_layout_files` for the `common` type, and reuses the SAME `make_parent`/`make_subagent`
/// constructors as `find_session_files`, so the empty-file skip and the single-stat rule are shared
/// rather than reimplemented. No UUID-v4 guard: the id comes from a catalog row that was indexed
/// through the guarded scanner, so re-validating it here would reject nothing.
pub fn layout_files(session_id: &str, parent: &Path, subagents_dir: &Path) -> Vec<SessionFile>;

/// `~/.local/share/clyde/staged`. THE definition; `session::paths::staged_dir` delegates here so
/// the two can never name different directories.
pub fn default_staged_dir() -> Option<PathBuf>;

/// THE recoverability predicate for pricing: every readable JSONL for one session, live layout
/// first, staged layout second, empty when there are no bytes anywhere. Both Phase 2 (backfill) and
/// Phase 3 (report) branch on `is_empty()` so "unrecoverable" means one thing in both.
///
/// Deliberately NOT `sessions::transcript_layout_parts`: that resolver requires a regular parent
/// `.jsonl` because it answers "where is this session's BODY" for enrich/export/FTS. Pricing asks
/// "which files hold usage records", and a subagent file holds real ones, so a session whose parent
/// was reaped before staging still prices from its subagents here. See the doc's "One definition of
/// recoverable".
pub fn pricing_files(
    session_id: &str,
    live_parent: &Path,
    live_project_dir: &Path,
    staged_dir: Option<&Path>,
) -> Vec<SessionFile>;

/// The live scan, plus every staged session whose id is absent from it. Live-then-staged
/// precedence, the same rule `sessions::transcript_layout_parts` applies per row, so a session
/// staged while still live is counted ONCE (from the live root). A staged root that does not exist
/// yields the live scan unchanged.
pub fn find_session_files_with_staged(projects_dir: &Path, staged_root: &Path) -> Result<Vec<SessionFile>>;

// sessions/src/db.rs
/// Un-annotated rows (`efficiency_json IS NULL`), archived or not, each with its path fields.
/// The `archived = 0` clause is GONE: archived means the live transcript is reaped, not that the
/// session cost nothing, and the staged copy is exactly what it is for.
pub fn sessions_missing_efficiency(&self) -> Result<Vec<EfficiencyCandidate>>;

// efficiency/src/collect.rs
/// What one backfill pass found: the sessions it could compute, and the ones with no bytes left.
/// A named struct rather than a tuple so neither half can be read as the other at a call site.
pub struct Collected {
    pub sessions: Vec<CollectedSession>,
    /// Session ids that resolved to NO readable transcript, live or staged. Nothing to price, ever.
    pub unrecoverable: Vec<String>,
}

/// Compute the listed candidates, resolving each one's bytes through `common::scan::pricing_files`
/// (live-then-staged, subagent-only accepted). Replaces `collect_ids` (deleted: the reindex path was
/// its only caller), and does no tree walk at all: it stats only the candidates' own paths.
pub fn collect_layouts(
    candidates: &[EfficiencyCandidate],
    config: &EfficiencyConfig,
    repo_root: &Path,
) -> Result<Collected>;
```

### Implementation Plan

#### Phase 1: Layout-explicit and staged-union discovery in `common::scan`
**Model:** sonnet

- Add `layout_files(session_id, parent, subagents_dir) -> Vec<SessionFile>`. One `fs::metadata` per
  file for mtime + size, matching `find_session_files`'s existing single-stat pattern.
- Add `default_staged_dir()` beside the existing `default_projects_dir()`. Add a `common` dependency
  to `session` (verified acyclic: `common` does not depend on `session`) and make
  `session::paths::staged_dir()` delegate to it. One definition of that path, two callers.
- Add `find_session_files_with_staged(projects_dir, staged_root)`:
  1. `find_session_files(projects_dir)` for the live files.
  2. Collect their `group_id`s into a `BTreeSet<String>`.
  3. For each `staged_root/<dir>/`, treat the directory name as the session id and skip it if the
     set already holds it; otherwise append `layout_files(id, dir/<id>.jsonl, dir/subagents)`.
  4. Sort the union by path, matching `find_session_files`'s existing stable-order contract (cost's
     equal-cost dedup tie-break depends on it).
- Add `pricing_files(...)`, THE recoverability predicate, built on `layout_files` with the
  live-then-staged precedence stated in exactly one place.
- **Success criteria:**
  - `pricing_files` returns the live files when both roots hold the session, the staged files when
    only the staged root does, a NON-EMPTY vec for a staged session with subagents but no parent
    (the case `transcript_layout_parts` rejects), and empty when neither root has bytes.
  - A session present in BOTH roots yields exactly one group, and every path in it is under the live
    root.
  - A staged-only session yields one group holding its parent and every subagent file.
  - `find_session_files_with_staged(live, nonexistent_staged)` equals `find_session_files(live)`.

#### Phase 2: Price every un-annotated row from wherever its bytes are
**Model:** opus

- `Db::sessions_missing_efficiency` drops `AND archived = 0` and returns
  `Vec<EfficiencyCandidate>`.
- Add `efficiency::collect_layouts`; resolve each candidate with `common::scan::pricing_files`,
  compute in parallel exactly as `collect_ids` did, and treat an empty vec as unrecoverable. Delete
  `collect_ids` and its tests (reindex was its only caller; `dead_code = "deny"` is on at the
  workspace root).
- `reindex_efficiency` calls it; `PersistStats.unrecoverable` carries the no-bytes count;
  `print_reindex` (`clyde/src/main.rs:1009`) prints it in both the TTY and JSON shapes.
- `WARN` per unrecoverable candidate, and **name the reason in the message**, not just the id:
  `"<id>: no transcript on disk (live reaped, no staged copy); spend for this session is
  unrecoverable"`. The first run will emit 64 of these on `desk.lan`, so each line has to read as a
  historical fact rather than a live failure or the operator reads a wall of WARNs as a crash.
- Update the `sessions_missing_efficiency` (`sessions/src/db.rs:369-374`) and `PersistStats`
  (`efficiency/src/persist.rs:31-40`) doc comments. Both currently assert that archived rows have
  "nothing on disk to recompute from," which is the false premise this doc exists to kill; a stale
  comment asserting it invites the predicate straight back.
- Callers of the changed signature that must be updated in the same commit:
  `efficiency/src/persist.rs:109`, `efficiency/src/persist/tests.rs:116,140`,
  `sessions/src/db/tests/efficiency.rs:133,146,183`.
- **Success criteria:**
  - New test: an archived row with a staged copy gets a non-NULL `cost_usd` and `efficiency_json`
    after `reindex_efficiency`; the existing
    `v6_sessions_missing_efficiency_excludes_annotated_and_archived`
    (`sessions/src/db/tests/efficiency.rs:155`) is rewritten to assert the new contract
    (archived-with-staged included, archived-with-nothing counted `unrecoverable`), and renamed so
    the name stops asserting the old behavior.
  - New test: an archived row with no staged copy stays NULL, is not computed, and increments
    `unrecoverable`.
  - `otto ci` green.

#### Phase 3: The report window counts archived spend and discloses the residue
**Model:** opus

- `report/src/lib.rs:209` -> `include_archived: true`.
- Split the NULL-`efficiency_json` guard by resolving each such row through
  `common::scan::pricing_files` (the same predicate Phase 2 branches on, never the `staged_path`
  column):
  - resolves to bytes -> **recoverable**: keep today's behavior exactly (stderr remedy, no
    artifact, non-zero exit, `clyde session reindex` named).
  - resolves to nothing -> **unrecoverable**: exclude the row from the collected set, push a
    `notes` line naming the count, and `eprintln!` the same sentence. Exit 0.
- Same split for the `outcome_json` guard, so `--no-outcomes` is not required to report an old
  window.
- Excluded, not zero-filled: a row with no readable transcript has no tokens, no models, and no
  efficiency, so an all-zero entry would corrupt every ratio-of-sums total. Its existence is
  disclosed in `notes`; its dollars are unknowable and are not invented.
- `jsonl_paths` (`report/src/lib.rs:429`) is today `vec![rec.transcript_path.clone()]`, which for an
  archived row names a file that does not exist. It carries the paths `pricing_files` actually read
  instead, so the artifact's paths always point at readable bytes and anyone auditing a session's
  number can open the file it came from.
- **Fix the merge disclosure loss.** `report/src/merge.rs:179` hardcodes
  `notes: vec![WINDOW_NOTE.to_string()]`, throwing away every input's notes, so a merged multi-host
  report loses the unrecoverable disclosure entirely while keeping the partial spend. Merge instead
  UNIONS the input notes (dedup by string, `WINDOW_NOTE` once), prefixing each host-specific line
  with its source host so a reader can tell which machine lost sessions. Without this, Phase 3's
  whole "stated, never silently zeroed" contract evaporates on exactly the multi-host path the team
  uses. Found by the review panel.
- **Success criteria:**
  - New test: a window holding an archived-but-priced session includes its spend in
    `totals.spend-usd` and counts it in `totals.sessions`.
  - New test: a window holding an unrecoverable row exits 0, excludes the row, and emits a `notes`
    line naming the count; a window holding a recoverable-but-unpriced row still exits non-zero
    naming `clyde session reindex`.
  - New test: merging two reports where one carries an unrecoverable-disclosure note yields a merged
    report whose `notes` still contains that line, host-attributed, plus exactly one `WINDOW_NOTE`.
  - `otto ci` green.

#### Phase 4: `clyde cost` and `clyde efficiency` see the staged root
**Model:** sonnet

- `cost::config::Config` gains `staged_dir: Option<PathBuf>`. The struct already carries
  `#[serde(default, rename_all = "kebab-case")]` (`cost/src/config.rs:33-34`), so the config key is
  `staged-dir` with no per-field attribute, and no snake_case alias is needed (new field, no legacy
  spelling to keep loading). Default: `common::scan::default_staged_dir()`.
  `cost/src/lib.rs:275` calls `find_session_files_with_staged`.
- `efficiency::collect_all` / `collect_matching` take the staged root and call the same function, so
  `clyde efficiency session|daily|weekly|--worst` stop undercounting. Their signature change ripples
  to `efficiency/src/lib.rs:112,146,156,164` and from there to the `clyde efficiency` call sites,
  which must thread the staged root through in the same commit.
- Date bucketing is safe without preserving staged mtimes: `filter_by_date_range`
  (`common/src/scan.rs:197`) is a lower-bound-only prefilter (`mtime_date >= start`), and a staged
  copy's mtime is its staging time, which is strictly LATER than the original. It can only make the
  prefilter more permissive, never drop an in-window file. The real windowing is the per-entry
  timestamp check in the consumer.
- `cost`'s cache-invalidation hash keys on mtime + size, so the first run after this ships
  recomputes once. Expected, not a defect.
- Double-count exposure differs per surface, and the panel established which is which:
  `cost` already dedups message ids GLOBALLY across files (`cost/src/lib.rs:414-430`, keyed on
  `(message_id, request_id)`), so the union scan cannot double-bill there even if precedence failed.
  `efficiency::extract`'s dedup is per-file (`efficiency/src/extract.rs:359`) and `fold` adds no
  cross-file check (`efficiency/src/fold.rs:85`), so on the efficiency surfaces the group-id
  precedence in `find_session_files_with_staged` is the ONLY thing preventing a double count. Test it
  there specifically, not just on `cost`.
- **Success criteria:**
  - New test: a session present in both roots is counted once by `clyde cost`; a staged-only
    session's spend appears.
  - New tests covering the efficiency surfaces this phase changes, which the previous draft's
    criteria did not prove (panel finding): `clyde efficiency session <id>` resolves a staged-only
    session; `daily`, `weekly`, and `--worst` each include a staged-only session and count a
    both-roots session exactly once.
  - `clyde cost --no-cache monthly` June total rises to at least $7,700 on `desk.lan`.
  - `otto ci` green.

#### Phase 5: Close the reap-before-stage race
**Model:** sonnet

- `cmd_reindex` (`clyde/src/main.rs`) calls `sessions::stage_dormant` after `sessions::reindex` and
  before `efficiency::reindex_efficiency`, with the same default dormancy cutoff as
  `clyde session stage` (7d) and `session::paths::staged_dir()` as the root.
- **The hook is `cmd_reindex`, NOT `sessions::index::reindex`.** `lazy_reindex`
  (defined `clyde/src/main.rs:855`) calls `sessions::reindex` from six sites covering search, list,
  export, resume, stage, and enrich (`clyde/src/main.rs:264,272,344,486,785,799`); putting a sweep
  there would make an incidental `clyde session ls` copy the 1,471-file backlog.
  This is the same call that `efficiency::reindex_efficiency` already makes, for the same stated
  reason (`clyde/src/main.rs:679-681`: wired to the explicit reindex "so a query's cheap incremental
  refresh never pays the transcript re-read"). Follow the precedent, do not invent a second policy.
- Ordering inside `cmd_reindex` does not affect recovery (a session already reaped has nothing left
  to copy regardless), but staging before the efficiency pass keeps the sequence readable:
  index -> durable copy -> price.
- `ReindexStats` gains the `StageStats` counts, printed by `print_reindex` in both shapes, so the
  first run's 1,471-file backlog copy is visible rather than a mysterious pause.
- `clyde session stage` stays the explicit, tunable entry point (`--dormant-after`, `--all`). The
  in-reindex sweep is the safety net. Both are idempotent: `copy_if_newer` compares mtimes, so a
  second sweep is a stat per candidate.
- **Ship the timer; do not delegate the race to the operator's memory.** The previous draft called
  scheduling "operator responsibility, out of scope." Both reviewers rejected that independently, and
  they were right: clyde ALREADY installs systemd user units. `bootstrap` has an `--install-timer`
  flag and a unit installer, today wired only to an ENRICH oneshot. So the in-house pattern exists
  and README prose is not causal closure. Harvest it for a `clyde session reindex` service + timer,
  and add a `clyde doctor` check reporting whether that timer is installed and enabled (doctor
  already models exactly this: `timer`, `timer_unit`, `timer_execstart`, and a content-based
  `timer_state`, `clyde/src/doctor.rs:55-59,102`).
- Harvest against the POST-#78 shape, per the Ship Order section: the entry point is
  `ensure_enrich_unit` behind the `Systemd` trait (`start_enrich_timer`), not `main`'s
  `install_clyde_timer`. Going through the trait is what makes the new timer testable without
  touching real systemd, so extend the trait rather than reaching for `systemctl` directly.
- Residual exposure after that, stated honestly: the timer only helps on a machine where `bootstrap`
  ran, so the `doctor` check is what makes its absence visible rather than silent. That is the
  strongest guarantee available without clyde becoming a daemon, which is out of scope.
- **Success criteria:**
  - New test: a reindex over a projects tree holding a dormant session leaves a staged copy, and
    that session's row is priced by the efficiency pass even after its live file is removed.
  - `clyde session reindex` reports a non-zero staged count on first run against a tree with dormant
    sessions, and zero staged / non-zero up-to-date on the second run.
  - `clyde doctor` names the reindex timer, and reports it as missing on a host where it was never
    installed.
  - `otto ci` green.

#### Phase 6: Truth-up the docs
**Model:** sonnet

- README: state that cost and report surfaces price reaped sessions from staged copies, that
  archived means "transcript reaped" and never "no spend," and that a session with no bytes
  anywhere is disclosed in the report's `notes`. Add two operational notes: run
  `clyde session reindex` on a timer so staging beats Claude Code's TTL, and re-run
  `report collect` for any window whose artifact predates this change.
- **No `CLAUDE.md` bullet.** The previous draft said to record the semantics there; this repo has no
  root `CLAUDE.md` (`ls CLAUDE.md` -> no such file), so that bullet was scope creep inventing a new
  file. Dropped. The durable guard against re-introducing an `archived = 0` money predicate is
  Phase 2's regression test, not prose, which is the house preference anyway.
- Write `docs/design/2026-07-30-archived-session-spend-implementation-notes.md`.
- **Success criteria:**
  - `rg -n "archived" README.md` returns the new semantics statement, and no CODE still claims
    archived sessions have nothing to recompute from: `rg -n "nothing on disk to recompute"
    --type rust` returns zero hits.
    - **Amended during implementation (2026-07-31), doc defect.** As authored this criterion greped
      `'*.rs' '*.md'` and asserted ZERO hits. It can never pass: this design doc is itself a `.md`
      that must quote the phrase to explain the false premise it exists to kill, so the criterion
      matched its own text plus the Phase 2 bullet's quotation. Verified: `grep -rn "nothing on disk
      to recompute" --include='*.md' .` returns exactly two lines, both in THIS file (the Phase 2
      bullet and this criterion), and `--include='*.rs'` returns zero. Scoped to Rust, which is where
      the premise was load-bearing (the `sessions_missing_efficiency` doc comment that invited the
      `archived = 0` predicate straight back).
  - `docs/design/2026-07-30-archived-session-spend-implementation-notes.md` exists and walks the
    plan phase by phase.
  - `otto ci` green.

## Acceptance Criteria

- [x] **A1.** Everything with readable bytes gets priced, measured by clyde's OWN resolver rather
      than the `staged_path` proxy (panel finding: the column and the predicate are not the same
      question). Run `clyde session reindex` twice, piped so it emits JSON:
      - First run: `.efficiency.candidates == .efficiency.computed + .efficiency.unrecoverable`
        (every candidate is accounted for, none silently dropped).
      - Second run, immediately after: `.efficiency.candidates == .efficiency.computed +
        .efficiency.unrecoverable` still holds, `.efficiency.unrecoverable` is UNCHANGED from the
        first run, and the cross-check below equals it. Nothing resolvable is left unpriced; what
        remains is exactly the no-bytes-anywhere set.
        - **Amended 2026-07-31, doc defect.** As authored this read
          `.efficiency.candidates == .efficiency.unrecoverable` and `.efficiency.computed == 0`. That
          cannot hold on a machine where Claude Code is running: `upsert_session` NULLs efficiency on
          a content change, so any session that grew between the two passes is legitimately a new
          candidate and is legitimately computed. Measured on desk.lan, run 2 reported
          `candidates=66 computed=2 unrecoverable=64` because two live sessions appended while the
          first pass ran. The criterion pinned a count that the environment must change, so it was
          rewritten to the invariant that actually carries the meaning (`unrecoverable` stable, the
          full set accounted for, the DB remainder equal to it) rather than relaxed to fit the code.
      - Cross-check that the remainder really is the archived residue:
        `sqlite3 ~/.local/share/clyde/sessions.db "SELECT COUNT(*) FROM sessions WHERE efficiency_json IS NULL;"`
        equals the second run's `unrecoverable`.
      *Observed on `main`:* the `unrecoverable` field does not exist yet; `clyde session reindex`
      reports only `candidates`, `computed`, `written`. `SELECT COUNT(*) FROM sessions WHERE
      archived=1 AND efficiency_json IS NULL AND staged_path IS NOT NULL` returns `214`, and all 214
      resolve a real staged parent, so on this host the proxy and the resolver agree today. A1 is
      written against the resolver so it stays true on a host where they do not.
      *Observed after v0.20.0 (2026-07-31, desk.lan):* **PASS.** Run 1
      `candidates=304 computed=240 unrecoverable=64` (304 == 240 + 64). Run 2
      `candidates=66 computed=2 unrecoverable=64` (66 == 2 + 64; `unrecoverable` unchanged). The
      cross-check returns `64`, and all 64 are `archived=1 AND staged_path IS NULL`, so the remainder
      is exactly the no-bytes-anywhere residue and nothing resolvable is left unpriced.
- [x] **A2.** The June report accounts for every row in the window: nothing is dropped without being
      counted somewhere. The artifact must balance against the catalog, and the balance is checked
      WITHOUT the `staged_path` proxy, so it holds on a host where the proxy and the resolver differ.
      - `clyde report collect --since 2026-06-01 --until 2026-07-01 | jq '.totals.sessions'` plus the
        unrecoverable count the artifact discloses in `.notes` equals
        `sqlite3 ~/.local/share/clyde/sessions.db "SELECT COUNT(*) FROM sessions WHERE modified >= '2026-06-01T00:00:00+00:00' AND modified <= '2026-07-01T00:00:00+00:00';"`.
      - The June window's disclosed unrecoverable count is `0`, so `.totals.sessions` is `558`.
      The SQL bounds are the byte-exact strings `append_filters` binds (`since.to_rfc3339()`, and an
      INCLUSIVE `until`), so the two windows differ only for a row whose `modified` is
      nanosecond-exact midnight.
      *Observed on `main`:* report `359`, `notes` discloses nothing, SQL `558`. 199 rows vanish with
      no accounting anywhere, which is the defect in one line.
      *Observed after v0.20.0 (2026-07-31, desk.lan):* **PASS.** `.totals.sessions` is `558`, `notes`
      carries only the standard window note (so the disclosed unrecoverable count is `0`), and the SQL
      returns `558`. 558 + 0 == 558: every row in the window is now accounted for.
- [x] **A3.** June spend clears the spike's measured lower bound on BOTH pricing pipelines. The bound
      is today's number plus the `2924.42` the Phase 0 spike measured from parent transcripts alone;
      subagents can only push it higher, so these are `>=` and not equalities.
      - `clyde report collect --since 2026-06-01 --until 2026-07-01 | jq '.totals["spend-usd"]'`
        returns at least `7300`. *Observed on `main`:* `4393.83`.
      - `clyde cost --no-cache monthly | jq '.months[] | select(.month=="2026-06") | .cost'` returns
        at least `7700`. *Observed on `main`:* `4818.54`.
      *Observed after v0.20.0 (2026-07-31, desk.lan):* **PASS on both.** report `7689.04` (bound
      `7300`), cost `8040.64` (bound `7700`). The two independent pipelines land 4.4% apart, inside
      the 1-9% band the investigation established, so they remain a usable cross-check. Against June
      ground truth of `$9,110.96` the report is -15.6% and cost is -11.8%; the remainder is the
      multi-host factor scoped to `report merge`.
- [x] **A4.** The May window stops rendering as a zero-usage month.
      `clyde report collect --since 2026-05-01 --until 2026-06-01` exits 0, reports
      `.totals.sessions` of `15` (the archived-with-staged rows SQL finds in that window), and
      carries a `.notes` entry naming `64` unrecoverable sessions.
      *Observed on `main`:* `{"sessions": 0, "spend-usd": 0.0}`, exit 0, `notes` holds only the
      standard window note. SQL over the same bounds:
      `SELECT SUM(archived=1 AND staged_path IS NOT NULL), SUM(archived=1 AND staged_path IS NULL), COUNT(*) FROM sessions WHERE modified >= '2026-05-01T00:00:00+00:00' AND modified <= '2026-06-01T00:00:00+00:00';`
      returns `15|64|79`, and `15 + 64 == 79` balances. Pinning exact numbers is safe here where A2
      and A3 use bounds, for two measured reasons: those transcripts are off disk so no reindex can
      change either count, and all 15 were confirmed to resolve a real staged parent `.jsonl`, so the
      SQL count and the resolver's answer are the same 15 rather than coincidentally equal totals.
      *Observed after v0.20.0 (2026-07-31, desk.lan):* **PASS.** Exit 0, `.totals.sessions` is `15`,
      spend `$170.39`, and `.notes` carries the line naming `64 session(s) ... permanently
      unrecoverable ... EXCLUDED ... PARTIAL total`. A month that rendered as
      `{"sessions": 0, "spend-usd": 0.0}` now reports its recoverable spend and states what is gone.
- [x] **A5.** The staging backlog drains. Run immediately after `clyde session reindex` (the cutoff
      is relative to `now`):
      `sqlite3 ~/.local/share/clyde/sessions.db "SELECT COUNT(*) FROM sessions WHERE archived=0 AND staged_path IS NULL AND modified <= strftime('%Y-%m-%dT%H:%M:%S','now','-7 days');"`
      returns `0`.
      *Observed on `main`:* `1471`.
      *Observed after v0.20.0 (2026-07-31, desk.lan):* **PASS.** `0`, down from `1496` (the baseline
      had drifted from the recorded `1471` because the predicate is `now`-relative). The sweep inside
      `clyde session reindex` staged `1498` sessions and copied `2584` files on its first run; the
      second run reported `staged=0 up-to-date=1590 files-copied=0`, proving idempotence.
- [x] **A6.** `otto ci` exits 0 on the final commit.
      *Observed on `main`:* not yet run for this branch. `main` @ `4b8eec7` is the merge commit of
      PR #77 (https://github.com/tatari-tv/clyde/pull/77), which shipped CI green.
      *Observed after v0.20.0:* **PASS.** Green on every one of the six phase commits locally, and
      green on PR #79 (https://github.com/tatari-tv/clyde/pull/79) across all six required checks.

## Resolved Decisions

- **2026-07-30: `archived` is not a spend filter.** It records transcript availability. Any money
  path that reads it is wrong by construction. Fixed at all three sites rather than special-cased at
  one.
- **2026-07-30: no new schema column for "unrecoverable."** It is a filesystem fact, derived at read
  time by the one existing resolver (`transcript_layout_parts`). A stored copy could diverge from
  the bytes on disk, and derived fields that can diverge do.
- **2026-07-30: unrecoverable rows are excluded and stated, not zero-filled.** No tokens, no models,
  no efficiency; an all-zero entry would corrupt every ratio-of-sums in the report. `Report::notes`
  is the existing channel for exactly this ("stated, never silently zeroed").
- **2026-07-30: no mtime preservation on staged copies.** `filter_by_date_range` is lower-bound-only
  and a staged mtime is strictly later than the original, so it cannot drop an in-window file. Real
  windowing is per-entry timestamps. Verified at `common/src/scan.rs:182-215`.
- **2026-07-30: `collect_ids` is deleted, not kept alongside `collect_layouts`.** The reindex path
  was its only caller and `dead_code = "deny"` is on at the workspace root.
- **2026-07-30: `staging_candidates`' `include_archived: false` (`sessions/src/db.rs:677`) stays.**
  An archived session has no live file to copy, so including it would stage nothing.
- **2026-07-30: Phase 5's staging hook is `cmd_reindex`, not `lazy_reindex`.** `lazy_reindex` runs on
  nearly every clyde command (six call sites); `reindex_efficiency` already made exactly this call for
  exactly this reason. Following the existing precedent beats inventing a second cost policy.
- **2026-07-30: no `SCHEMA_VERSION` bump.** The report's shape does not change, only which rows reach
  it. Bumping would invalidate merge coverage for artifacts that are structurally identical.

### Review panel round 1 (2026-07-30): Architect (Gemini) + Staff Engineer (Codex)

Both ran read-only against this worktree. Architect cleared the design ("no other `archived`
money-path filters exist, cleared to proceed") with one finding; Staff Engineer raised five. Every
finding was verified in the code before disposition, and all eight were FOLDED IN. No pushbacks were
sent, because none were warranted.

| Finding | Raised by | Verified | Disposition |
|---|---|---|---|
| `report merge` discards input `notes`, so a merged multi-host report keeps partial spend and loses the unrecoverable disclosure entirely | Staff (High) | Confirmed: `report/src/merge.rs:179` hardcodes `notes: vec![WINDOW_NOTE.to_string()]` | FOLDED: new Phase 3 bullet (union + host-attribute the notes) plus a merge test. This was a genuine fourth defect the author missed |
| "Recoverable" was defined three ways (`staged_path IS NOT NULL` in the SQL, resolver-returns-`Some` in Phase 3, bytes-exist in prose), so the doc could pass its own SQL while the Rust path excluded different rows | Staff (High) | Confirmed: `stage_dormant` sets `staged_path` on `files_total > 0` (`sessions/src/stage.rs:37-44`) while `transcript_layout_parts` demands a regular parent (`sessions/src/transcript.rs:48-52`) | FOLDED, and taken further than asked: added the "One definition of recoverable" section, a single `pricing_files` predicate that ACCEPTS subagent-only bytes (recovers strictly more than the resolver would), and rewrote A1/A2/A4 off clyde's own counts instead of the proxy. Measured the real divergence at zero rows (214/214) and named the structural reason |
| Phase 4 changes four `clyde efficiency` surfaces but its criteria only proved `clyde cost` | Staff (Med) | Confirmed: `collect_all`/`collect_matching` feed `session`, `daily`, `weekly`, `--worst` | FOLDED: per-surface staged-only and both-roots criteria added |
| Phase 5 claimed to "close" the race while delegating scheduling to README prose, when `bootstrap` already installs systemd timers | Staff (Med) + Architect's hardest question, independently | Confirmed: `--install-timer` and a unit installer exist, wired only to enrich | FOLDED: Phase 5 now ships a reindex timer harvested from that installer plus a `clyde doctor` check, and states the honest residual (only helps where `bootstrap` ran) |
| "11 `lazy_reindex` call sites" was wrong | Staff (Med) | Confirmed: 6 call sites plus the definition. The author's `rg -c` counted matching LINES, the exact units error the skill warns about | FOLDED: corrected to six, each site cited |
| Cross-file message-id double-billing was stated as a product-wide non-goal, but `clyde cost` dedups globally | Staff | Confirmed: `cost/src/lib.rs:414-430` keys on `(message_id, request_id)` across files | FOLDED: non-goal rescoped to `efficiency`/`report`, and Phase 4 now says precedence is the ONLY protection on the efficiency surfaces specifically |
| Phases 5 and 6 omitted per-phase `otto ci` while the rollout claimed every phase commit is green; Phase 6 never verified the notes file; no root `CLAUDE.md` exists | Staff | Confirmed: `ls CLAUDE.md` -> absent | FOLDED: criteria added, and the `CLAUDE.md` bullet dropped as scope creep inventing a file |
| 64 WARN lines in one backfill could read as a live crash | Architect | Accepted on its face | FOLDED: the WARN must name the reason, with the wording specified |

**Author-found during the same round, not raised by either reviewer:** PR #78
(https://github.com/tatari-tv/clyde/pull/78) is open and touches every file this doc modifies,
including a net -443-line rewrite of `clyde/src/bootstrap.rs` that renames Phase 5's harvest target.
Both reviewers read the worktree at `main` and had no reason to look at open PRs. See Ship Order.

## Alternatives Considered

### Alternative 1: point `clyde cost` at the catalog instead of the filesystem
- **Description:** Have `clyde cost` read `cost_usd` from `sessions.db` like `report` does, deleting
  its independent JSONL pipeline.
- **Pros:** one pricing path, one place to fix.
- **Cons:** destroys the independent cross-check. The two pipelines currently agree within 1-9%,
  which is how the investigation established this is one shared upstream gap rather than one buggy
  code path; `cost/src/oracle.rs` exists specifically to exploit that independence.
- **Why not chosen:** the redundancy is load-bearing evidence, and this bug is the proof.

### Alternative 2: scan `projects_dir` and `staged_root` as two equal roots
- **Description:** Concatenate both scans and let the existing dedup sort it out.
- **Pros:** trivial.
- **Cons:** double-counts every session staged while still live. 308 staged dirs exist on `desk.lan`
  against 214 archived-with-staged rows, so ~94 sessions have bytes in both roots right now. The
  per-file `seen_usage_msg_ids` dedup cannot catch a cross-file duplicate.
- **Why not chosen:** it converts a 50% undercount into a large overcount.

### Alternative 3: store an `unrecoverable` column at reconcile time
- **Description:** Have `reconcile_archived` write a flag when a row has neither a live nor a staged
  transcript, and let `report` read the column.
- **Pros:** report does zero stats.
- **Cons:** a second stored signal for a filesystem fact, able to go stale between reconcile and
  read.
- **Why not chosen:** the resolver already answers the question, and only for NULL-efficiency rows,
  which after Phase 2 is a handful.

### Alternative 4: fail closed on unrecoverable rows too
- **Description:** Treat "no bytes anywhere" like "not yet indexed": no artifact, non-zero exit.
- **Pros:** maximally loud, matches the house fail-closed instinct.
- **Cons:** permanently bricks `report collect` for any window containing one. The 64 May rows are
  gone forever, so no remedy exists and the error would name none.
- **Why not chosen:** a fail-closed guard whose remedy is impossible is not a guard, it is a
  denial. Loud disclosure with a correct partial total is the honest form. This is the one place the
  doc deliberately departs from fail-closed, and it does so because the unhappy path has no fix.

### Alternative 5: bounded lazy staging batch
- **Description:** Instead of Phase 5's explicit-command hook, have `lazy_reindex` stage at most N
  newly-dormant sessions per invocation so the 1,471-file backlog drains across many commands and no
  single command pays for all of it.
- **Pros:** automatic, no operator scheduling required, no single slow command.
- **Cons:** a new tunable, a cap that must log what it deferred (silent truncation reads as "done"),
  and it puts file copying on the path of every `clyde session ls`. It also duplicates a policy
  `reindex_efficiency` already settled the other way for the same cost profile.
- **Why not chosen:** the explicit hook plus a documented timer gets the same outcome with no new
  mechanism. Recorded here so it is not re-proposed as if unconsidered; the revisit condition is
  evidence that people are not running `clyde session reindex` and are still losing sessions.

## Technical Considerations

### Dependencies

- New internal edge: `session -> common` (for `default_staged_dir`). Verified acyclic: `common`'s
  manifest has no `session` dependency.
- `efficiency` already depends on `sessions`, so `collect_layouts` can take `EfficiencyCandidate`
  and call `transcript_layout_parts`.
- `cost` already depends on `common`; the staged root reaches it through `common::scan`, so `cost`
  does not gain a `session` dependency.
- No new external crates.

### Performance

- Phase 2 makes the backfill **cheaper**: `collect_ids` walked the entire projects tree and filtered
  by id, while `collect_layouts` stats only the candidate rows' own paths.
- Phase 4 adds one directory walk of `staged_root` per `clyde cost` invocation, gated behind the
  live-id set so no extra file is parsed for a session already found live.
- Phase 5 adds a staging sweep to every `session reindex`. Steady state is one stat per dormant
  candidate (`copy_if_newer` compares mtimes before reading); the first run after this ships copies
  the current dormant backlog once.
- `cost`'s cache invalidates once on first run after Phase 4 (staged mtimes are new to the hash).

### Security

- No new off-machine calls, no new secrets, no LLM involvement. Staging already only reads and
  writes local files.
- The `enrich` work/personal routing gate is untouched: this doc never sends transcript content
  anywhere, it only prices bytes already on disk.
- The staged root is clyde-owned under `$XDG_DATA_HOME`, and `default_staged_dir()` resolves through
  the same XDG helpers as every other clyde path.

### Testing Strategy

- Unit tests per phase as listed, using `tempfile::TempDir` fixtures with real files in both roots.
- The regression that matters most: **an archived row with a staged copy must be priced**. That
  assertion is the inverse of the currently-passing
  `v6_sessions_missing_efficiency_excludes_annotated_and_archived`, so the old test is rewritten,
  not deleted, and the rewrite is what proves the class cannot recur.
- Break-the-code check before each phase lands: re-add `AND archived = 0` and confirm the new tests
  fail.
- Live verification on `desk.lan` after Phase 4: the A1-A6 commands, re-run and recorded.
- Independent confirmation on a second machine before the Slack reply: at least one of Keegan,
  Patrick, or Stephen runs `clyde session reindex` then the A2 and A3 commands against their own
  June window and compares to their Analytics number above. The bug is proven on one host so far;
  the code path is shared, so it should reproduce for anyone whose history has aged.

### Rollout Plan

- **Gate: PR #78 (https://github.com/tatari-tv/clyde/pull/78) merges first.** Then re-resolve this
  doc's `file.rs:line` references against the new tree before Phase 1 starts.
- One branch, six commits, one per phase, each `otto ci` green.
- Gated repo: `bump --no-tag` inside the PR, tag after merge per `rules/git.md`.
- `cargo install --path clyde` after merge, then re-run A1-A6 on `desk.lan` and record the numbers.
- Reply in the Slack thread with the before/after on a settled month and the one-line root cause,
  plus the two commands anyone can run to check their own machine.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Double-counting a session with bytes in both roots | Med | High | Live-then-staged precedence in both discovery shapes; a both-roots test in Phases 1 and 4 |
| Report totals jump and read as a new bug | High | Med | Expected and quantified here; the `notes` line and the Slack reply state the before/after |
| A window containing an unrecoverable row starts failing | Med | High | Alternative 4 rejected for exactly this; Phase 3 test asserts exit 0 plus the note |
| Staged copy has a torn final line | Med | Low | Parser already warn-and-skips; costs at most the last record of one session |
| Phase 5's in-reindex staging surprises with disk writes | Low | Med | Same clyde-owned dir `session stage` already writes; counts reported in reindex output |
| Recovered spend still short of ground truth | Med | Med | Multi-host is a known separate factor (`report merge`); A3 is a lower bound, not an equality, so the criterion does not pretend otherwise |
| Enrichment-coverage warning starts firing on old windows | High | Low | Quantified, not a defect: June coverage goes 188/359 (52.4%, above the 0.5 floor) to 238/558 (42.7%, below it), so `enrichment_warning` (`report/src/lib.rs:338`) fires. It is stderr-only and never fails the run, and `clyde session enrich` already resolves staged copies through `transcript_layout`, so the coverage is recoverable rather than permanently degraded |

## Open Questions

- [ ] None.

## References

- Slack thread: `C039YLDJW5T`, permalink `p1785432360721679`
- Handoff: `handoff-clyde-cost-undercounting-2026-07-30.md` (this session's predecessor)
- Ground truth: Claude Enterprise Analytics API via the `anthropic-usage-report` skill,
  `--report user-cost --group-by model --group-by product`, June 2026 (settled)
- `sessions/src/db.rs:379` (backfill predicate), `:690` (`reconcile_archived`), `:677`
  (`staging_candidates`)
- `sessions/src/transcript.rs:38` (`transcript_layout_parts`, the precedence rule)
- `report/src/lib.rs:209` (window filter), `:244` (fail-closed guard), `:429` (`jsonl_paths`)
- `cost/src/lib.rs:275` (live-only scan), `common/src/scan.rs:197` (`filter_by_date_range`)
- `efficiency/src/persist.rs:98` (`reindex_efficiency`), `efficiency/src/collect.rs:86` (`collect_ids`)
- `session/src/stage.rs` (staging), `session/src/paths.rs:96` (`staged_dir`)
- `report/src/report.rs:65` (`Report::notes`, the disclosure channel)
