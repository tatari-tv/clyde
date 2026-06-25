# Design Document: claude-cost-usage (ccu)

**Author:** Scott Idler
**Date:** 2026-03-10
**Status:** Implemented
**Review Passes Completed:** 5/5 (see [Review Log](#review-log) below)

## Summary

A fast Rust CLI tool (`ccu`) that reads Claude Code's JSONL session logs, applies model-specific pricing, and returns cost summaries (daily, monthly, per-session). Designed to run within a ~100ms budget for statusline integration, replacing slower Node.js alternatives like `ccusage`.

## Problem Statement

### Background

Claude Code writes session logs as JSONL files in `~/.claude/projects/`. Each file represents one session and contains token usage data for every assistant turn. The Claude Code statusline exposes `total_cost_usd` for the current session only - there is no built-in way to see aggregate costs across sessions, days, or months.

Existing tools like `ccusage` (Node.js) can parse these logs but are too slow for statusline integration (~500ms+) and introduce a Node.js runtime dependency.

### Problem

Users have no fast, lightweight way to track Claude Code spending across sessions. Without aggregate cost visibility, it's easy to lose track of daily and monthly spend.

### Goals

- Parse Claude Code JSONL session logs and compute accurate costs using current model pricing
- Provide daily, monthly, and per-session cost summaries via CLI
- Return JSON output fast enough for statusline integration (<50ms warm cache, <200ms cold)
- Support all current Claude models with correct cache pricing tiers
- Cache computed results to avoid redundant parsing

### Non-Goals

- Real-time streaming of costs (poll-based is sufficient)
- Integration with Anthropic billing API (JSONL logs are the sole data source)
- Cost alerting or budget enforcement
- Support for non-Claude Code log formats
- GUI or web interface

## Proposed Solution

### Overview

A Rust binary (`ccu`) that scans JSONL session logs, extracts assistant-type entries with token usage, applies per-model pricing (including cache tier pricing), and outputs cost summaries as text or JSON. A disk cache avoids re-parsing unchanged files.

### Data Source

Claude Code writes session logs to:

```
~/.claude/projects/<encoded-project-path>/<session-uuid>.jsonl
```

Path encoding: `/` replaced with `-` (e.g., `/home/saidler/repos/foo` becomes `-home-saidler-repos-foo`).

Each line is a JSON object. Only lines with `type == "assistant"` contain cost-relevant data:

```json
{
  "type": "assistant",
  "parentUuid": "...",
  "sessionId": "a1b2c3d4-...",
  "timestamp": "2026-03-10T14:23:01.025Z",
  "version": "2.1.72",
  "message": {
    "model": "claude-opus-4-6",
    "id": "msg_...",
    "type": "message",
    "role": "assistant",
    "usage": {
      "input_tokens": 3,
      "cache_creation_input_tokens": 1868,
      "cache_read_input_tokens": 21827,
      "output_tokens": 2,
      "cache_creation": {
        "ephemeral_5m_input_tokens": 1868,
        "ephemeral_1h_input_tokens": 0
      },
      "service_tier": "standard"
    }
  }
}
```

**Note on model IDs:** JSONL logs may contain dated model variants (e.g., `claude-haiku-4-5-20251001`, `claude-opus-4-5-20251101`). The parser must strip date suffixes (pattern: `-YYYYMMDD`) to match against the pricing table base names.

Other line types (`system`, `user`, `progress`, `queue-operation`, `file-history-snapshot`) are skipped.

### Architecture

```
src/
  main.rs         Entry point, CLI dispatch
  cli.rs          Clap argument definitions
  config.rs       Config file loading (pricing overrides, paths)
  scanner.rs      Find and filter JSONL files by date range
  parser.rs       Stream-parse JSONL lines, extract assistant entries
  pricing.rs      Model pricing table, cost calculation
  cache.rs        Disk cache for computed daily totals
  output.rs       Text and JSON formatters
```

**Data flow:**

```
1. cli.rs       Parse args, determine command (today/daily/monthly/session)
2. config.rs    Load config from ~/.config/ccu/ccu.yml (pricing overrides, paths)
3. cache.rs     Check disk cache for requested date range
                  - Cache hit (mtime_hash matches): return cached totals
                  - Cache miss: continue to step 4
4. scanner.rs   Walk ~/.claude/projects/*/*.jsonl, filter by file mtime
5. parser.rs    Stream each JSONL file line-by-line, yield assistant entries
6. pricing.rs   Look up model in pricing table, compute cost per entry
7. cache.rs     Write daily totals + mtime_hash to ~/.cache/ccu/
8. output.rs    Format as text table or JSON, write to stdout
```

### Data Model

**Pricing table** - per million tokens:

| Model ID | Input | Output | 5m Cache Write | 1h Cache Write | Cache Read |
|----------|-------|--------|----------------|----------------|------------|
| claude-opus-4-6 | $5.00 | $25.00 | $6.25 | $10.00 | $0.50 |
| claude-opus-4-5 | $5.00 | $25.00 | $6.25 | $10.00 | $0.50 |
| claude-opus-4-1 | $15.00 | $75.00 | $18.75 | $30.00 | $1.50 |
| claude-opus-4 | $15.00 | $75.00 | $18.75 | $30.00 | $1.50 |
| claude-sonnet-4-6 | $3.00 | $15.00 | $3.75 | $6.00 | $0.30 |
| claude-sonnet-4-5 | $3.00 | $15.00 | $3.75 | $6.00 | $0.30 |
| claude-sonnet-4 | $3.00 | $15.00 | $3.75 | $6.00 | $0.30 |
| claude-haiku-4-5 | $1.00 | $5.00 | $1.25 | $2.00 | $0.10 |

**Long context pricing** (>200K input tokens):

| Model ID | Input | Output |
|----------|-------|--------|
| claude-opus-4-6 | $10.00 | $37.50 |
| claude-sonnet-4-6 / 4-5 / 4 | $6.00 | $22.50 |

**Cost calculation per assistant entry:**

Note: `input_tokens` represents non-cached input tokens only. It does NOT include `cache_creation_input_tokens` or `cache_read_input_tokens` - these are separate, additive fields.

```
cost = (input_tokens * input_price / 1_000_000)
     + (output_tokens * output_price / 1_000_000)
     + (cache_creation.ephemeral_5m_input_tokens * cache_5m_write_price / 1_000_000)
     + (cache_creation.ephemeral_1h_input_tokens * cache_1h_write_price / 1_000_000)
     + (cache_read_input_tokens * cache_read_price / 1_000_000)
```

If `cache_creation` breakdown is missing, treat all `cache_creation_input_tokens` as 5m (the common case in Claude Code).

**Cache structure:**

```
~/.cache/ccu/
  2026-03-10.json   # {"cost": 14.23, "sessions": 3, "mtime_hash": "abc123"}
  2026-03-09.json
```

- `mtime_hash` is a hash (e.g., FNV-1a or xxhash) of the concatenated (path, mtime, size) tuples of all JSONL files contributing to that day
- If any source file changes, that day is recomputed
- Today's cache is recomputed when the mtime_hash changes or a short TTL (e.g., 10s) has elapsed, since active sessions may still be writing

### API Design (CLI Interface)

Binary name: `ccu`

```
ccu [OPTIONS] [COMMAND]

Commands:
  session   Show cost for a specific session (by ID or "current")
  today     Show today's total cost (default)
  daily     Show daily costs for a date range
  monthly   Show monthly cost summary

Options:
  -c, --config <PATH>   Path to config file (default: ~/.config/ccu/ccu.yml)
  -p, --path <PATH>     Override ~/.claude/projects/ scan path
  -j, --json            Output as JSON (for statusline integration)
  -v, --verbose         Show per-session breakdown
  -d, --days <N>        Number of days to show (for daily command, default: 7)
  --model <MODEL>       Filter to a specific model
  --no-cache            Skip the cost cache, recompute from JSONL
```

**Default output (no subcommand):**
```
$ ccu
Today: $14.23 (3 sessions)
```

**JSON output (for statusline):**
```
$ ccu --json
{"today":14.23,"sessions":3,"current_session":7.40}
```

**Daily breakdown:**
```
$ ccu daily --days 7
2026-03-10  $14.23  (3 sessions)
2026-03-09  $22.17  (5 sessions)
2026-03-08   $8.91  (2 sessions)
...
```

**Monthly summary:**
```
$ ccu monthly
2026-03  $187.42  (47 sessions)
2026-02  $312.89  (83 sessions)
```

### Implementation Plan

**Phase 1: Core parsing and pricing**
- Implement `pricing.rs` with the full model pricing table
- Implement `parser.rs` to stream-parse JSONL and extract assistant entries
- Implement `scanner.rs` to discover and filter JSONL files by date
- Wire up `today` command with text output

**Phase 2: Cache and JSON output**
- Implement `cache.rs` with mtime-based invalidation
- Add `--json` output formatter in `output.rs`
- Add `daily` and `monthly` commands

**Phase 3: Polish**
- Add `session` command
- Add `--model` filter
- Add `--verbose` per-session breakdown
- Update config.rs to support pricing overrides and path configuration

**Phase 4: Statusline integration**
- Document statusline script integration
- Performance profiling and optimization
- Add `--no-cache` flag for debugging

## Alternatives Considered

### Alternative 1: ccusage (Node.js)
- **Description:** Existing open-source tool that parses Claude Code logs
- **Pros:** Already works, community maintained
- **Cons:** ~500ms+ startup (Node.js cold start), requires Node runtime, not designed for statusline polling
- **Why not chosen:** Too slow for the ~100ms statusline budget; adds a Node.js dependency

### Alternative 2: Shell script with jq
- **Description:** Bash script piping JSONL through jq for cost calculation
- **Pros:** No compilation, easy to modify
- **Cons:** Very slow for large files (seconds for multi-MB JSONL), fragile parsing, no caching
- **Why not chosen:** Performance is orders of magnitude too slow for large session logs

### Alternative 3: Python script
- **Description:** Python script with orjson for fast JSON parsing
- **Pros:** Fast development, good JSON ecosystem
- **Cons:** ~100ms Python startup overhead, dependency management (though mitigated by pipx)
- **Why not chosen:** Python startup time alone consumes most of the 100ms budget; Rust provides deterministic sub-millisecond startup

### Alternative 4: Anthropic billing API
- **Description:** Query Anthropic's API for usage data instead of parsing local logs
- **Pros:** Authoritative cost data, no local parsing needed
- **Cons:** Requires API key, network latency makes it unsuitable for statusline, may not have per-session granularity
- **Why not chosen:** Network dependency and latency are incompatible with the statusline use case

## Technical Considerations

### Dependencies

**Runtime (Cargo):**
- `clap` (4.x) - CLI argument parsing with derive macros
- `serde` / `serde_json` - JSON parsing and serialization
- `serde_yaml` - Config file parsing
- `chrono` - Date/time handling and filtering
- `eyre` - Error handling with context
- `rayon` - Parallel file processing
- `dirs` - XDG-compliant directory resolution
- `log` / `env_logger` - Structured logging

**External:**
- Claude Code JSONL logs (the sole data source)
- `jq` (optional, for statusline script consumers)

### Performance

- **Target:** <50ms warm cache, <200ms cold cache for `ccu --json`
- **File mtime filtering:** Skip files outside the requested date range without reading them
- **Streaming parsing:** Process JSONL line-by-line; never buffer entire files
- **Early termination:** Skip lines not starting with `{"type":"assistant"`
- **Parallel scanning:** Use rayon for concurrent file processing on cold cache
- **Disk cache:** Store computed daily totals; validate via mtime hash

### Security

- Reads only from `~/.claude/projects/` (user-owned directory)
- No network access required
- No secrets or credentials handled
- Config file paths are validated and canonicalized
- JSONL parsing uses serde with strict typing (no eval or dynamic execution)

### Edge Cases

- **Concurrent writes:** Claude Code may be writing to a JSONL file while `ccu` reads it. The parser must tolerate truncated/incomplete last lines (skip malformed lines with a warning in verbose mode).
- **Timezone handling:** JSONL timestamps are UTC. "Today" is determined by converting UTC timestamps to the local timezone. This means a session at 11pm local time is correctly attributed to that local day, not the next UTC day.
- **Unknown models:** If an assistant entry contains an unrecognized model ID, log a warning and skip the entry (do not error out). This avoids breaking when Anthropic releases new models before `ccu` is updated.
- **Empty/corrupt files:** Skip gracefully. A zero-byte JSONL file or one containing only non-assistant lines produces zero cost.
- **Duplicate sessions:** The scanner deduplicates by session UUID if the same session appears under multiple project paths.
- **Dated model IDs:** Model IDs in logs may include date suffixes (e.g., `claude-opus-4-5-20251101`). Strip the `-YYYYMMDD` suffix before pricing lookup. If no match after stripping, fall back to the unknown model warning path.

### Testing Strategy

- **Unit tests:** Pricing calculation accuracy per model, parser extraction from sample JSONL lines, cache invalidation logic
- **Integration tests:** End-to-end with sample JSONL fixture files, verifying text and JSON output formats
- **Performance tests:** Benchmark against representative JSONL corpus (1MB, 10MB, 100MB) to validate latency targets
- **Edge cases:** Empty files, malformed JSON lines, unknown model IDs, missing cache_creation breakdown, truncated lines from concurrent writes, timezone boundary sessions

### Rollout Plan

1. Build and test locally with real Claude Code session logs
2. Install via `cargo install` to `~/.cargo/bin/ccu`
3. Integrate into statusline script, replacing the current sidecar approach
4. Publish to GitHub for broader use

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Claude Code changes JSONL format | Medium | High | Defensive parsing with serde defaults; log warnings for unknown fields |
| Pricing changes without notice | Medium | Medium | Pricing table in config file allows user overrides; update binary for new models |
| Large log directories cause slow cold cache | Low | Medium | Parallel scanning with rayon; date-range filtering reduces scan scope |
| Claude Code deletes logs after 30 days | High | Low | Document limitation; monthly summaries for older months will be incomplete |
| Long context pricing threshold hard to detect | Medium | Low | Default to standard pricing; long context is a small percentage of typical usage |
| Unknown model IDs in future logs | Medium | Low | Warn on unknown model, skip entry or use conservative default pricing |

## Open Questions

- [ ] Should we track `session_id` from the statusline JSON to provide "current session" cost from the JSONL perspective (cross-referencing against `total_cost_usd`)?
- [ ] Long context pricing requires knowing total input tokens per request (>200K threshold) - JSONL entries have per-turn data, not per-request totals. Is standard pricing an acceptable default?
- [ ] Should the pricing table be embedded in the binary, loaded from config, or both (config overrides embedded defaults)?
- [ ] Should `ccu monthly` warn when data is incomplete due to Claude Code's 30-day log retention?

## References

- Claude pricing: https://platform.claude.com/docs/en/about-claude/pricing
- Claude Code session log format: `~/.claude/projects/` JSONL files
- ccusage (Node.js alternative): https://github.com/nicekid1/ccusage
- XDG Base Directory Specification: https://specifications.freedesktop.org/basedir-spec/latest/

---

## Review Log

### Pass 1: Draft

Initial draft created by previous agent. Good breadth - covers all template sections, has concrete examples, pricing tables, CLI interface design, and phased implementation plan.

### Pass 2: Correctness

Verified against actual JSONL session logs in `~/.claude/projects/`.

**Findings:**
- Sample JSON was missing fields present in real logs (`parentUuid`, `version`, `message.id`, `message.type`, `message.role`)
- Model IDs in real logs use dated variants (e.g., `claude-haiku-4-5-20251001`, `claude-opus-4-5-20251101`) not mentioned in the design
- The `service_tier` field location was correct (inside `usage`), confirmed against real data
- Real logs also contain an optional `inference_geo` field inside `usage`

**Changes made:**
- Updated sample JSON to match real JSONL structure
- Added note about dated model ID suffix stripping

### Pass 3: Clarity

Reviewed as someone who would implement this from the doc alone.

**Findings:**
- Cost formula doesn't clarify that `input_tokens` excludes cached tokens - could be misread as "total input including cache"
- `mtime_hash` definition was vague - "hash of all JSONL file mtimes" doesn't specify what's hashed or the algorithm

**Changes made:**
- Added explicit note that `input_tokens` is non-cached input only, separate from cache fields
- Specified `mtime_hash` as hash of concatenated (path, mtime, size) tuples

### Pass 4: Edge Cases

**Findings:**
- Missing edge case: dated model ID suffixes need stripping before pricing lookup
- Existential check: Yes, solving the right problem - local JSONL parsing is the only viable approach for statusline-speed cost aggregation

**Changes made:**
- Added "Dated model IDs" edge case to Edge Cases section

### Pass 5: Excellence

**Findings:**
- Document is comprehensive and implementable
- All sections are consistent with each other
- Pricing table matches current Anthropic pricing
- No further changes needed

**CONVERGENCE REACHED:** Document ready for implementation.
