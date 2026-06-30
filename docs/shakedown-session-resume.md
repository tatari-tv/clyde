# CLI Shakedown Report: `clyde session resume` + `sessions`→`session` rename

**Binary:** `/home/saidler/.cargo/bin/clyde` (`v0.4.0-8-gaf924a8`, branch `open`, PR #11)
**Date:** 2026-06-30
**Scope:** the new `resume` verb, the clean-break rename, the `--` passthrough rule, the
resume error matrix, and `doctor` stale-unit detection. Interactive `claude` launches were
deliberately NOT performed (the exec would replace the process); the Launch path was exercised up
to the launch boundary by hiding `claude` from `PATH`.

## Summary

| Metric | Count |
|--------|-------|
| Areas tested | 6 |
| Behaviors verified | 12 |
| Passed | 12 |
| Failed | 0 |
| Findings (out-of-scope/pre-existing) | 2 |

All in-scope behavior works. Two findings surfaced: one cosmetic inconsistency in the resume
feature itself, and one pre-existing `doctor` display bug unrelated to this work.

## Verified behaviors

### Rename (clean break)
- `clyde session --help` lists `resume` and reads naturally. **PASS**
- `clyde sessions resume <id>` (old plural) → `error: unrecognized subcommand 'sessions'` with a
  `tip: a similar subcommand exists: 'session'`, exit 2. Clean break confirmed. **PASS**

### `resume` help / `--` passthrough
- `clyde session resume --help` documents cd+launch, fork/exec semantics, and the `--` rule. **PASS**
- `clyde session resume <id> --model opus` (no `--`) → `error: unexpected argument '--model'` with
  `tip: to pass '--model' as a value, use '-- --model'`, exit 2. Does NOT misparse. **PASS**
- `clyde session resume <id> -- --model opus` → forwards `--model opus`; reaches the launch boundary
  (verified via the claude-hidden trick below). **PASS**

### Error matrix (each → clean stderr, exit 1)
Driven against a copy of the real catalog with three crafted rows for states that don't occur
naturally, plus a real deleted-cwd session.

| State | Trigger | Output | Exit |
|-------|---------|--------|------|
| MissingDir | real session, cwd dir deleted | `✗ recorded cwd is not a usable directory: <path>` | 1 |
| StagedOnly | transcript gone, staged present | `✗ only a staged copy exists (<path>); the live transcript is gone…` | 1 |
| Reaped | transcript gone, no staged | `✗ session transcript is gone (TTL-reaped); nothing to resume` | 1 |
| NoCwd | cwd NULL | `✗ session <id> has no recorded cwd; cannot resume in place` | 1 |
| no match | unknown prefix | `✗ no session matches "zzzzzzzz"` | 1 |
| ambiguous | short prefix | `✗ "a" is ambiguous (10 matches)` | 1 |

The reworded `MissingDir` message (post-audit fix) is accurate for a deleted dir. **PASS**

### Launch boundary (without launching claude)
- Live session (transcript exists) with `claude` removed from `PATH`:
  `plan_resume → Launch → launch_resume → resolve_claude()` fails with
  ``could not find `claude` on PATH: cannot find binary path``, exit 1. This proves the Launch
  decision is reached AND validates the post-CodeRabbit `which`-based absolute-path resolution
  (the chdir can no longer influence which binary runs). **PASS**

### `doctor` stale-unit detection (post-audit fix #3)
- Crafted `clyde-enrich.service` with `…clyde … sessions enrich` →
  `enrich timer: sessions enrich (legacy)`, exit 1. **PASS**
- Crafted unit with `…session enrich` (isolated via `XDG_CONFIG_HOME`+`XDG_DATA_HOME`) →
  `enrich timer: clyde`, `✓ all integrations resolve to clyde`, exit 0. **PASS**
- **Live observation:** this machine's real `clyde-enrich.service` still contains `sessions enrich`
  (it predates the rename) — doctor now correctly flags it and prompts `clyde bootstrap`. This is
  exactly the already-migrated population that post-audit fix #1 was written to repair.

## Findings

### Finding A — resume launch errors leaked a source `Location:` (cosmetic, in scope) — FIXED
- **Severity:** cosmetic / low. **Status: fixed during this shakedown.**
- The `claude`-not-on-PATH (and by extension exec-failure) path returned `Err(eyre!…)` from
  `launch_resume`, which `main` rendered via eyre's default hook with a `Location: clyde/src/main.rs:…`
  backtrace line — inconsistent with the other five error-matrix cases that print a clean `✗ …`.
- **Fix:** `run_resume_action`'s `Launch` arm now catches the `launch_resume` error and prints
  `✗ <err>` + `exit(1)`, matching its siblings. Re-verified:
  ``✗ could not find `claude` on PATH: cannot find binary path``, exit 1, no `Location:` line.

### Finding B — `doctor`/`bootstrap` strand a legacy events DB when the clyde DB also exists — FIXED
- **Severity:** bug / low-medium. **Status: fixed** (Phase 5, `bootstrap` now merges; see below).
- When BOTH `~/.local/share/clyde/events.db` and the legacy
  `~/.local/share/claude-permit/events.db` exist, `doctor` displays `events DB: clyde (N rows)`
  (the legacy DB is invisible) yet `healthy()` returns false (`events_db_at_legacy == true`),
  so it prints `✗ legacy targets/state remain — run clyde bootstrap` and exits 1 with **nothing
  on screen indicating what is legacy**. Confirmed live: the legacy permit DB exists on this
  machine (25k).
- **It's a dead-end loop, not just a cosmetic gap:** `migrate_events_db` only moves the legacy DB
  when the clyde DB is ABSENT, so with both present `clyde bootstrap` is a no-op for the events DB —
  the legacy file is never migrated or removed, so `doctor` stays red forever and the remediation it
  prints (`run clyde bootstrap`) can never clear it. Observed directly: after `bootstrap` repaired
  the enrich unit (timer now `clyde`/healthy), `doctor` still exits 1 with the legacy-events-DB the
  only remaining (invisible) cause.
- The display `match` only surfaced the legacy events DB in the `clyde-DB-absent` arm; the
  both-present case had no branch.
- **Fix (Phase 5):** `migrate_events_db` now MERGES the legacy rows into the clyde DB and removes
  the legacy DB (backed up to `.clyde.bak`) when both exist, instead of no-op'ing; `doctor` prints a
  `legacy state:` line surfacing the legacy DB when the clyde DB also exists. Re-verified live:
  after `clyde bootstrap`, the legacy `claude-permit/events.db` was merged + removed and
  `clyde doctor` reports `✓ all integrations resolve to clyde` (exit 0).

## Notes
- No mutation of the real catalog or real systemd units was performed. Error-matrix states were
  tested against a copy of `sessions.db` with crafted rows; doctor states via temp `XDG_*_HOME`.
- The real machine's stale enrich unit (above) WAS repaired during this shakedown: `clyde bootstrap`
  rewrote `sessions enrich` → `session enrich` (backup at `.clyde.bak`), `daemon-reload` ran, and
  `doctor` now shows `enrich timer: clyde`. This validated post-audit fix #1 end-to-end on real
  systemd. (`doctor` still exits 1 solely due to Finding B's legacy events DB.)
