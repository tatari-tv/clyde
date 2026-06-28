# CLI Shakedown Report: clyde v0.3.0 (FULL)

**Date:** 2026-06-28
**Binary:** `/home/saidler/.cargo/bin/clyde` (v0.3.0)
**Scope:** Full — every subcommand exercised, including the mutating/long-running
ones (`tag`, `reindex`, `stage`, `enrich`, `serve`) run safely, plus `bootstrap`
by discovery only. Absorbed tools (`report`, `cost`, `permit`) fully exercised.
**Goal:** surface every issue/rough edge for a follow-up design doc.

## Summary

| Metric | Count |
|--------|-------|
| Commands discovered | ~30 (sessions×9, report×3, cost×8, permit×8, bootstrap, doctor) |
| Commands tested | 27 |
| Passed | 25 |
| Failed / not-implemented | 1 (`report merge`) |
| Discovery-only (not executed) | 1 (`bootstrap` — real migration, no dry-run) |
| Edge cases | 6 |

**Verdict:** the v0.3.0 `--sort` feature is solid (re-confirmed). The full sweep
surfaced **13 issues** — none are crashes; they are inconsistencies, missing
features, doc gaps, and an unfinished migration. All are candidates for the
follow-up design doc.

---

## Issues List (for the design doc)

### Migration / consistency (medium)

1. **`bootstrap` has no `--dry-run`.** It performs a real config/data/cache
   migration but offers no preview (`--force`, `--skip-systemd`,
   `--skip-statusline`, `--install-timer` only). The CLI conventions say a
   *default-destructive* op warrants a dry-run. Can't safely show what it would
   do, so it was discovery-only here.

2. **`clyde doctor` reports the migration is incomplete.** Exits non-zero with
   `✗ legacy targets/state remain — run clyde bootstrap`. Corroborated by
   `permit check`, which validates the **legacy `claude-permit` binary + hook**
   (`~/.cargo/bin/claude-permit`, hook in `~/.claude/settings.json`), not a
   clyde-native hook. The umbrella migration hasn't been finalized on this host.

3. **Old binary names leak in output.** `report merge` errors as `cr: merge is
   not implemented`; `permit check` reports `claude-permit`. The absorbed tools
   still self-identify by their pre-clyde names (`cr`/`ccu`/`claude-permit`).

### `--since` parsing inconsistency (medium)

4. **`--since` accepts different formats across subcommands.** `sessions ls
   --since 24h` / `7d` (relative spans) works; `report collect --since 2d`
   **fails** — `could not parse datetime '2d': expected RFC 3339 or YYYY-MM-DD`.
   Same flag name, two different parsers. (Absolute `--since 2026-06-27` works
   for `report collect`.)

5. **Internal source location leaks into user-facing errors.** The failed
   `report collect --since 2d` printed `Location: report/src/config.rs:168:5` —
   an eyre/panic location surfaced to the user instead of a clean message.

### Missing features (medium / low)

6. **`report merge` is not implemented** — `merge is not implemented in this
   release`. Stub.

7. **MCP `sessions_search` has no `--sort`/recency option.** The CLI gained
   `--sort recency` in v0.3.0; the MCP tool schema (`query`/`limit`/
   `include_archived`) doesn't expose it. Documented decision, but a parity gap
   for MCP consumers.

8. **`sessions tag` cannot clear tags.** `<TAGS>...` requires ≥1 value, so there
   is no way to set a session back to zero tags. Manual tagging also flips
   `tags_source` to manual irreversibly (no provenance restore via CLI).

### Polish / docs (low)

9. **`report` options are undocumented.** Every `collect`/`render`/`merge` flag
   (`--since`, `--until`, `--output`, `--projects-dir`, `--no-rollup`,
   `--skip-title`, `--input`, `--pdf`, `--template`, `--prompt`,
   `--include-tradeoffs`) has a blank help description, and the subcommands have
   no summaries.

10. **Output-format model is inconsistent.** `cost` uses an explicit `-j/--json`
    flag; `sessions` auto-detects TTY (JSON when piped, no flag); `report`
    writes to `-o <file>`. Three conventions in one binary.

11. **`tags_source` not exposed in JSON.** `search`/`ls` records include `tags`
    but not `tags_source`, so provenance (manual vs ai) isn't observable.

12. **`sessions serve` doesn't exit on stdin EOF.** After answering an
    `initialize` it kept running until the timeout (124); a clean EOF shutdown
    would be tidier (MCP hosts kill it, so low impact).

13. **`cost session current` may not resolve the actual current session.** It
    returned `6e427ce3` while the live clyde session is `049209b7` — "current"
    likely resolves to the most-recently-modified JSONL, which can differ.
    Needs confirmation; possibly intended.

---

## Command Results (passed)

### sessions (the release surface)
- `search` — relevance (default, with the `modified DESC` tiebreak lifting today's
  tied session above the June-01 one), recency (`modified DESC`, tiering
  dissolved), `--sort RECENCY`/`Relevance` (case-insensitive), `--sort bogus`
  (exit 2), multi-term AND, no-match → `[]`, `--limit`. TTY shows the two-line
  stacked listing with `●`/`○` match markers.
- `ls` — `--repo`, `--since 24h`, `--limit`; `modified DESC` ordering verified;
  `marquee:main` vs `second-brain` confirms the dangling-`:` drop.
- `open` — full id + 8-char prefix → resume line; bogus id rejected.
- `reindex` — `{scanned:460, upserted:24, skipped-unchanged:436, archived:64}`.
- `tag` — set test tag (replaced all 6), verified, restored the 6 originals.
- `stage` — `{considered:308, staged:308, files-copied:571}` (idempotent durability copies).
- `enrich --dry-run` — gate preview `{considered:17, would-enrich:6,
  skipped-personal:11, redactions:2, tokens:0}`, no off-machine send.
- `serve` — valid MCP `initialize` response (rmcp 1.8.0; tools `sessions_search`,
  `sessions_ls`, `session_open`).
- `doctor` — JSON enrichment health (513 total / 94 enriched / 2 failed).

### cost
- `today`/`weekly`/`monthly`/`daily` (+ `-d`, `-g` graph w/ bars + braille trend +
  line chart), `session current`, `pricing --show`, `--json`, `--offline`. Tables
  well-aligned.

### report
- `collect --since <date> -o f.json` → 71 sessions, structured JSON
  (`generated/host/schema-version/sessions/since/totals/until`).
- `render -i f.json -o out.md` → 8.7k markdown with frontmatter (exit 0; an
  earlier "no output" reading was a pipe/timeout test artifact, not a tool bug).

### permit
- `check` (PASS ×3), `audit` (risk table), `report` (event summary + dangerous
  activity), `suggest` (promotion table). All read-only, all clean.

### global
- `-l/--log-level` passthrough works; `--db` honored across subcommands.

---

## Edge Cases

| Input | Behavior |
|-------|----------|
| `search --sort bogus` | clap error, exit 2, lists valid values |
| `search` (no query) | required-arg error, exit 2 |
| `open zzzznotanid` | `✗ no session matches`, non-zero |
| `search zzqqxxnomatch` | `[]`, exit 0 |
| `report collect --since 2d` | parse error + leaked source location (issue #4/#5) |
| `report merge <one>` | "not implemented" (issue #6) |

## Release Validation
(unchanged from the prior pass) Tag `v0.3.0` annotated, points at `origin/main`
HEAD; 4 targets (linux amd64/arm64, macos arm64/x86_64) + checksums; downloaded
linux-amd64 checksum OK, `--version` matches local.

## What works great (no action)
`--sort` feature end-to-end; the whole `cost` tool (rich tables + charts); the
TTY two-line listing; MCP serve; the enrich gate's scope/redaction classification;
JSON validity + kebab-case keys with internal `id` omitted.
