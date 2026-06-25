# Design Document: Subagent JSONL Scanner

**Author:** Scott A. Idler
**Date:** 2026-04-27
**Status:** Implemented
**Review Passes Completed:** 3/3

## Summary

Claude Code writes subagent session JSONL files into `<project>/<uuid>/subagents/` subdirectories that the ccu scanner never reads. As a result, all cost from agent-spawned subagents is silently omitted. With 1,697 such files already on disk, the gap between reported and actual cost is significant.

## Problem Statement

### Background

The ccu scanner was written when Claude Code stored all session data as flat `.jsonl` files directly inside each project directory:

```
~/.claude/projects/
  <project-slug>/
    <uuid>.jsonl
```

Claude Code later added the Agent SDK, which spawns subagents with their own API calls. Each subagent gets its own JSONL file stored in a sibling directory:

```
~/.claude/projects/
  <project-slug>/
    <uuid>.jsonl           ← parent session  (scanned)
    <uuid>/
      subagents/
        agent-<id>.jsonl   ← subagent session (NOT scanned)
      tool-results/        ← raw tool outputs, no JSONL
```

### Problem

`scanner::find_session_files` iterates one level inside each project directory, collecting only `.jsonl` files at that level. The `<uuid>/subagents/` tree is one level deeper and is never visited.

Every agent-spawned API call - whether using haiku for cheap tool work or opus for heavy reasoning - goes untracked and unreported. The data exists on disk; ccu just never reads it.

**Observed data shape (verified on disk):**
- Subagent entries use the **same `sessionId`** as their parent session
- Subagent message IDs do **not** overlap with parent JSONL entries
- Consequence: adding subagent files increases daily **cost** but not **session count** for days that already have a parent session entry; session count rises only for sessions where the parent JSONL was deleted or never written (verified: this does occur in the wild)

### Goals

- Scan `<project>/<uuid>/subagents/*.jsonl` in addition to `<project>/<uuid>.jsonl`
- Report correct daily/weekly/monthly totals that include subagent costs
- Keep the cache invalidation mechanism working correctly
- Pass all existing tests; add tests for the new layout

### Non-Goals

- Recursive descent beyond `subagents/` (tool-results directories contain no JSONL files)
- Changes to the parser, pricing, or output layers
- Backfilling or migrating cached data (cache will naturally invalidate on hash mismatch)

## Proposed Solution

### Overview

Extend `find_session_files` to check each directory entry inside a project dir. When an entry is itself a directory (a session UUID dir), look for a `subagents/` child and collect any `.jsonl` files found there.

### Architecture

No structural changes. The fix is entirely within `scanner::find_session_files`. All downstream code (`filter_by_date_range`, `compute_mtime_hash`, `parse_jsonl_file`) works on a flat `Vec<SessionFile>` and requires no changes.

### Data Model

`SessionFile` is unchanged:

```rust
pub struct SessionFile {
    pub path: PathBuf,
    pub mtime: SystemTime,
    pub size: u64,
}
```

Subagent files get the same treatment as parent files: mtime-filtered, mtime-hashed for cache invalidation, and parsed for assistant entries.

### API Design

`find_session_files` signature is unchanged. The returned `Vec<SessionFile>` grows to include subagent entries.

### Implementation Plan

#### Phase 1: Extend scanner and add tests
**Model:** sonnet

- In `find_session_files`, when iterating a project dir's entries: if the entry is a directory (a session UUID dir), check whether `<entry>/subagents/` exists; if so, iterate its contents and collect non-empty `.jsonl` files using the same metadata logic as direct files.
- Add a test case that creates the nested `<uuid>/subagents/<agent>.jsonl` layout and verifies `find_session_files` returns all files.
- Run `otto ci` to confirm all tests pass.

## Alternatives Considered

### Alternative 1: Full recursive walk with `walkdir`
- **Description:** Add the `walkdir` crate and replace the manual iteration with `WalkDir::new(projects_dir).into_iter()`, collecting all `.jsonl` files at any depth.
- **Pros:** Future-proof against further nesting; less code.
- **Cons:** Pulls in a dependency; walks `tool-results/` needlessly; no depth bound.
- **Why not chosen:** The directory structure is well-known and fixed. Two-level specific traversal is explicit and fast; no new dependency needed.

### Alternative 2: Glob pattern match
- **Description:** Use `glob` crate to find `**/*.jsonl` under each project dir.
- **Pros:** Concise.
- **Cons:** Another dependency; same over-breadth concern as walkdir.
- **Why not chosen:** Same reasoning as Alternative 1.

## Technical Considerations

### Dependencies

None. The fix uses only `std::fs` already imported in `scanner.rs`.

### Performance

Each project directory now potentially opens one additional subdirectory per session UUID. In practice the overhead is negligible - `find_session_files` runs once at startup and takes milliseconds even at 100+ project directories.

### Security

No change. Scanner reads only from the same `~/.claude/projects` path it already reads.

### Testing Strategy

- Unit test: create `<project>/<uuid>/subagents/<agent>.jsonl` in a `TempDir` and assert it appears in the result alongside the sibling `<uuid>.jsonl`.
- Smoke test: run `ccu --no-cache daily 30` before and after the patch; confirm previously-missing days gain cost.

### Rollout Plan

Drop-in binary update. No config changes, no migration. The per-day cache files will miss on their mtime hash (new files included → different hash) and recompute automatically.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Subagent entries duplicated in parent JSONL | Very Low | Medium | Verified: subagent message IDs are distinct from parent JSONL entries. The dedup pass in `main.rs` handles the within-file streaming duplicates that do exist. |
| `tool-results/` directory grows to include JSONL someday | Low | Low | Only `subagents/` is targeted; other dirs are ignored |
| Performance regression on very large project trees | Very Low | Low | Two extra `is_dir()` + `read_dir()` calls per session UUID; cost is negligible |

## Open Questions

- [ ] Are there other subdirectory names besides `subagents/` that could contain billable JSONL files in future Claude Code versions? (Verified: `tool-results/` contains no JSONL files; no JSONL files exist deeper than `subagents/` level.)

## References

- `src/scanner.rs` - file under change
- `src/main.rs:compute_summaries` - dedup logic that handles duplicated entries
- Observed on-disk structure: 1,697 subagent JSONL files, 620 `subagents/` dirs across 102 project directories
- Verified: some `<uuid>/subagents/` exist without a sibling `<uuid>.jsonl` parent; this is handled correctly (subagents become standalone session entries)
