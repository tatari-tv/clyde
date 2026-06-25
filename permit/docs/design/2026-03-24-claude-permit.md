# Design Document: claude-permit

**Author:** Scott Idler
**Date:** 2026-03-24
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

`claude-permit` is a Rust CLI that manages Claude Code permission hygiene by logging permission events to SQLite, auditing accumulated permission rules for risk classification, and surfacing actionable recommendations to promote, keep, or remove permission entries. It integrates with Claude Code via hooks and a custom skill.

## Problem Statement

### Background

Claude Code uses a dual-config permission system (`settings.json` for global/tracked rules, `settings.local.json` for machine-specific overrides) with pattern-based allow/deny lists. Over time, users accumulate hundreds of permission rules through one-off approvals during interactive sessions. These rules are never reviewed, never pruned, and never promoted from local to global even when they've proven safe across many sessions.

The current permission setup (458 lines in `settings.local.json` alone) has no tooling to answer basic questions:
- Which permissions are stale or redundant?
- Which one-off approvals keep recurring and should be promoted to permanent rules?
- Which rules are dangerously broad?
- Are any rules violating personal safety policies (e.g., `rm -rf` instead of `rkvr rmrf`)?

### Problem

Permission sprawl degrades security posture and creates friction. Users either:
1. **Over-permit** - approve everything to avoid interruptions, accumulating risky rules
2. **Under-permit** - refuse to add rules, suffering repeated approval prompts for safe operations

There is no feedback loop between permission decisions and permission policy.

### Goals

- Log every tool invocation observed via PreToolUse hook to a persistent local store
- Classify existing permission rules by risk tier (Safe, Moderate, Dangerous)
- Detect permanently-deny-listed patterns and flag violations
- Recommend promotions (local to global) for frequently-approved safe patterns
- Provide session-end summaries of new one-off approvals
- Integrate seamlessly via Claude Code hooks and skills

### Non-Goals

- Modifying `settings.json` or `settings.local.json` automatically (recommendations only)
- Syncing permissions across machines
- Managing MCP server configurations
- Replacing Claude Code's built-in permission system
- Supporting non-Claude-Code permission systems

## Proposed Solution

### Overview

A single Rust binary with six subcommands (`log`, `audit`, `suggest`, `report`, `clean`, `check`) that operates on two data sources: the Claude Code settings files (read-only) and a local SQLite database (read-write). Integration is through Claude Code's hook system (PreToolUse) and a custom skill for on-demand auditing.

### Architecture

```
                          ┌──────────────────────┐
                          │   Claude Code Host    │
                          │                       │
                          │  PreToolUse Hook ─────┼──► claude-permit log (stdin JSON)
                          │  Skill (/perm-audit) ─┼──► claude-permit audit + suggest
                          │  Manual invocation ───┼──► claude-permit report
                          │                       │
                          └──────────────────────┘
                                    │
                     ┌──────────────┼──────────────┐
                     ▼              ▼              ▼
              ┌────────────┐ ┌───────────┐ ┌────────────┐
              │  SQLite DB │ │settings.  │ │settings.   │
              │ events.db  │ │json       │ │local.json  │
              └────────────┘ └───────────┘ └────────────┘
              ~/.local/share/  ~/.claude/    ~/.claude/
              claude-permit/
```

**Components:**

1. **Event Logger** (`log` subcommand) - Parses hook JSON from stdin, extracts tool name + command pattern, writes to SQLite. Must output valid JSON to stdout (`{}` for passthrough) to not break the hook pipeline.
2. **Rule Auditor** (`audit` subcommand) - Reads settings files, classifies each permission rule by risk tier, checks against deny list, outputs recommendations
3. **Pattern Suggester** (`suggest` subcommand) - Queries event DB for patterns observed N+ times across M+ sessions, recommends promotion to permanent allow rules
4. **Session Reporter** (`report` subcommand) - Summarizes permission events from the current/latest session, highlights new one-off approvals. Invoked manually or via skill - there is no "Stop" hook event in Claude Code.

### Data Model

**Hook JSON Schema (PreToolUse - stdin):**

Claude Code pipes JSON to hook commands on stdin. The structure varies by tool type:

```json
{
  "tool_name": "Bash",
  "tool_input": {
    "command": "git status",
    "description": "Show working tree status"
  },
  "session_id": "..."
}
```

Tool-specific `tool_input` fields:
- `Bash` - `command`, `description`
- `Edit` - `file_path`, `old_string`, `new_string`
- `Write` - `file_path`, `content`
- `Read` - `file_path`
- `WebFetch` - `url`
- `Glob/Grep` - `pattern`, `path`
- MCP tools (`mcp__server__tool`) - varies per tool

**Hook JSON Output (stdout):**

The `log` subcommand must output valid JSON. To passthrough without affecting permission decisions:
```json
{}
```

To also enforce deny rules (optional, Phase 2+):
```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"rm -rf is permanently denied; use rkvr rmrf"}}
```

**Permission Rule Syntax:**

Rules in settings files follow the pattern `Tool(pattern)`:
- `Bash(command:*)` - allow any args to command (e.g., `Bash(git status:*)`)
- `Edit(glob)` / `Write(glob)` / `Read(glob)` - file pattern (e.g., `Edit(src/**/*.rs)`)
- `WebFetch(domain:host)` - domain allowlist (e.g., `WebFetch(domain:github.com)`)
- `WebSearch` - no pattern, allows all web searches
- `Skill(name)` - skill name (e.g., `Skill(rust-cli-coder)`)
- `mcp__server__tool` - MCP tool name, no pattern

**SQLite Schema: `events` table**

```sql
CREATE TABLE events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp   TEXT NOT NULL,          -- ISO 8601
    session_id  TEXT NOT NULL,          -- from hook JSON or env var
    tool_name   TEXT NOT NULL,          -- e.g., "Bash", "Edit", "WebFetch", "mcp__*"
    tool_input  TEXT NOT NULL,          -- normalized: command for Bash, file_path for Edit, etc.
    raw_input   TEXT,                   -- full tool_input JSON for complex tools
    risk_tier   TEXT,                   -- "safe" | "moderate" | "dangerous" | NULL
    raw_json    TEXT                    -- full hook payload for debugging
);

CREATE INDEX idx_events_session ON events(session_id);
CREATE INDEX idx_events_tool ON events(tool_name, tool_input);
CREATE INDEX idx_events_timestamp ON events(timestamp);
```

Note: The `decision` column from the draft is removed. PreToolUse hooks fire before the decision is made - the hook itself can only observe the request or enforce a deny. Whether the user approved or denied is not available in the hook payload.

**Tool Input Normalization:**

The `tool_input` column stores a normalized string extracted from the hook JSON's `tool_input` object. This is the primary key for pattern analysis:

| Tool Type | Normalization | Example Input | Normalized Value |
|-----------|--------------|---------------|-----------------|
| `Bash` | `tool_input.command` | `{"command": "git status --short"}` | `git status --short` |
| `Edit` | `tool_input.file_path` | `{"file_path": "/home/user/foo.rs", ...}` | `/home/user/foo.rs` |
| `Write` | `tool_input.file_path` | `{"file_path": "/home/user/bar.rs", ...}` | `/home/user/bar.rs` |
| `Read` | `tool_input.file_path` | `{"file_path": "/home/user/baz.rs"}` | `/home/user/baz.rs` |
| `WebFetch` | `tool_input.url` | `{"url": "https://docs.rs/clap"}` | `https://docs.rs/clap` |
| `Glob` | `tool_input.pattern` | `{"pattern": "**/*.rs"}` | `**/*.rs` |
| `Grep` | `tool_input.pattern` | `{"pattern": "fn main"}` | `fn main` |
| MCP tools | JSON string of full `tool_input` | `{"account": "home", ...}` | `{"account":"home",...}` |

**Risk Tier Classification:**

```rust
enum RiskTier {
    Safe,       // Read-only, local-only, no side effects
    Moderate,   // Local writes, repo-scoped, reversible
    Dangerous,  // Destructive, irreversible, shared state
}
```

Classification rules (applied in order, first match wins):

| Rule | Tier | Examples |
|------|------|---------|
| Matches permanent deny list | Dangerous | `rm -rf *`, `git tag -d *`, `cd && *` |
| `Bash` with `sudo` prefix | Dangerous | `sudo apt install`, `sudo rm`, `sudo systemctl` |
| `Bash` with `git push --force` | Dangerous | `git push --force origin main` |
| MCP write tools (Slack, Jira, PagerDuty) | Dangerous | `mcp__slack__conversations_add_message` |
| `Bash` with `git push`, `git commit` | Moderate | `git push origin main` |
| `Bash` with `cargo build`, `cargo test` | Moderate | `cargo build --release` |
| `Edit`, `Write` tools | Moderate | `Edit(src/**/*.rs)` |
| `Bash` with write commands (`mkdir`, `chmod`) | Moderate | `mkdir -p src/foo` |
| MCP read tools | Moderate | `mcp__atlassian__getJiraIssue` |
| `Read`, `Glob`, `Grep` tools | Safe | `Read(**/*.rs)` |
| `Bash` with read-only commands | Safe | `ls`, `tree`, `git log`, `git diff`, `cat`, `head` |
| `WebFetch`, `WebSearch` | Safe | `WebFetch(domain:docs.rs)` |
| `Skill` | Safe | `Skill(rust-cli-coder)` |

The classification engine uses a configurable rules list. The above is the default. Users can override via config to reclassify specific patterns (e.g., promote `cargo build` to Safe if desired).

**Permanent Deny Patterns (hardcoded + configurable):**

These are patterns that `claude-permit` should actively block via hook deny responses and/or flag during audit. Some already exist in `settings.json` deny list; others are aspirational rules that claude-permit enforces as an additional safety layer.

```rust
const PERMANENT_DENY: &[&str] = &[
    // Already in settings.json deny list:
    "git tag -d",                // per CLAUDE.md
    "git push * :refs/tags/*",   // per CLAUDE.md
    "git push * --delete * tag*",// per CLAUDE.md

    // Enforced by claude-permit (not yet in settings):
    "rm -rf",                    // must use rkvr rmrf
    "rm -r",                     // must use rkvr rmrf
    "cd &&",                     // compound cd commands, bare repo attack vector
];
```

### CLI Design

```
claude-permit <COMMAND>

Commands:
  log       Log a permission event from hook JSON (reads stdin)
  audit     Audit current permission rules and classify by risk
  suggest   Suggest promotions based on usage patterns
  report    Session-end summary of permission activity
  clean     Prune old events from the database
  check     Verify hook installation and DB connectivity

Options:
  -h, --help     Print help
  -V, --version  Print version

--- log ---
claude-permit log
  Reads JSON from stdin (Claude Code hook payload)
  Extracts: tool_name, tool_input (normalized), session_id
  Writes event to SQLite
  Outputs JSON to stdout: "{}" for passthrough, or deny decision for blocked patterns
  Must complete in <50ms to avoid hook latency

--- audit ---
claude-permit audit [OPTIONS]
  --settings <PATH>        Override settings.json path
  --settings-local <PATH>  Override settings.local.json path
  --format <FORMAT>        Output format: table (default), json, markdown
  --risk <TIER>            Filter by risk tier: safe, moderate, dangerous

Output: table of rules with columns: Rule | Source | Risk | Recommendation

Recommendations include:
  - "promote" - move from settings.local.json to settings.json
  - "keep" - rule is appropriate where it is
  - "narrow" - rule is overly broad (e.g., Bash(git:*) covers both safe and dangerous operations)
  - "remove" - rule is redundant (covered by a broader rule) or stale
  - "deny" - rule matches a permanently-denied pattern

--- suggest ---
claude-permit suggest [OPTIONS]
  --threshold <N>          Min observations to trigger suggestion (default: 3)
  --sessions <M>           Min distinct sessions (default: 2)
  --format <FORMAT>        Output format: table (default), json, markdown

Output: patterns that should be promoted to permanent allow rules

Pattern grouping for suggestions:
  Events are grouped by (tool_name, normalized_command_prefix).
  For Bash, the prefix is the first word(s) before arguments:
    "git status --short" -> "git status"
    "cargo build --release" -> "cargo build"
    "ls -la /tmp" -> "ls"
  The suggested rule uses Claude Code's permission syntax:
    "Bash(git status:*)" for Bash commands
    "Edit(src/**/*.rs)" for file operations (derived from common path prefixes)

--- report ---
claude-permit report [OPTIONS]
  --session <ID>           Session ID (default: current/latest)
  --format <FORMAT>        Output format: table (default), json, markdown

Output: summary of session permission activity

--- clean ---
claude-permit clean [OPTIONS]
  --older-than <DAYS>      Delete events older than N days (default: 90)
  --dry-run                Show what would be deleted without deleting

--- check ---
claude-permit check
  Verifies:
    1. SQLite DB exists and is writable
    2. Hook is registered in settings.json or settings.local.json
    3. claude-permit binary is in PATH
  Outputs: pass/fail for each check with fix instructions
```

### Implementation Plan

**Phase 1: Foundation**
- Scaffold Rust project with clap for CLI parsing
- Set up SQLite with rusqlite (bundled feature)
- Implement `log` subcommand - parse hook JSON, write to DB, output `{}`
- Implement `check` subcommand - verify DB and hook setup
- Create PreToolUse hook registration
- Basic integration test: pipe sample JSON to `claude-permit log`, verify DB row

**Phase 2: Auditing**
- Implement risk tier classification engine (rule table + first-match logic)
- Implement permanent deny list checker
- Implement `audit` subcommand - read settings files, classify, output table
- Add output formatters (table, JSON, markdown)

**Phase 3: Intelligence**
- Implement `suggest` subcommand - query patterns, apply thresholds, output permission rule syntax
- Implement `report` subcommand - session summaries
- Implement `clean` subcommand - prune old events
- Create `/permissions-audit` skill for Claude Code

**Phase 4: Enforcement and Config**
- Add configurable thresholds and risk overrides via `~/.config/claude-permit/claude-permit.yml`
- Hook-based deny enforcement for permanent deny patterns (output deny JSON instead of `{}`)
- Comprehensive test suite
- Documentation and README

### Example Output

**`claude-permit audit --risk dangerous`:**
```
Rule                                    Source          Risk       Recommendation
--------------------------------------  --------------- ---------- ---------------
Bash(sudo rm:*)                         local           dangerous  remove
Bash(git tag:*)                         global          dangerous  narrow
Bash(yes:*)                             local           dangerous  remove
mcp__slack__conversations_add_message   local           dangerous  keep
```

**`claude-permit suggest`:**
```
Pattern                  Count  Sessions  Suggested Rule              Risk
-----------------------  -----  --------  -------------------------   --------
git fetch                12     5         Bash(git fetch:*)           safe
cargo test               8      4         Bash(cargo test:*)          moderate
docker compose           6      3         Bash(docker compose:*)      moderate
```

**`claude-permit report`:**
```
Session: abc123 (2026-03-24)
Events: 47 total (32 safe, 12 moderate, 3 dangerous)

New patterns not in any allow list:
  Bash(pipx list:*)              - 3 occurrences - suggest: add to global (safe)
  Bash(rkvr rmrf:*)              - 2 occurrences - suggest: add to global (moderate)

Dangerous activity:
  sudo apt install libssl-dev    - 1 occurrence
```

## Alternatives Considered

### Alternative 1: Shell Script
- **Description:** Implement as a collection of bash scripts using jq and sqlite3
- **Pros:** Zero compile step, immediate iteration, matches hook ecosystem
- **Cons:** Fragile parsing, no type safety, hard to test, poor error handling, slow for DB queries
- **Why not chosen:** The risk classification logic and pattern matching are complex enough to benefit from Rust's type system. Shell scripts would become unmaintainable quickly.

### Alternative 2: Python CLI
- **Description:** Python CLI using Click and SQLite
- **Pros:** Rapid prototyping, good library ecosystem
- **Cons:** Requires Python runtime, slower startup (matters for hook latency), user's CLAUDE.md mandates pipx for Python tools (adds complexity)
- **Why not chosen:** Hook performance matters (PreToolUse is in the critical path). Rust's near-zero startup time and single binary deployment fit better. Also aligns with user's Rust-first preference.

### Alternative 3: Embed in Claude Code Hooks Directly
- **Description:** Put all logic in hook scripts without a separate binary
- **Pros:** No separate install, immediate availability
- **Cons:** Hooks have limited output capabilities, can't easily query historical data, no persistent state beyond flat files
- **Why not chosen:** The historical analysis (suggest, report) requires structured queries against accumulated data. Hooks alone can't provide this.

## Technical Considerations

### Dependencies

**Rust crates:**
- `clap` - CLI argument parsing (with derive feature)
- `rusqlite` - SQLite bindings (with bundled feature for zero system deps)
- `serde` / `serde_json` - JSON parsing for hook payloads and settings files
- `chrono` - Timestamp handling
- `tabled` or `comfy-table` - Table output formatting
- `dirs` - XDG directory resolution (~/.local/share)
- `glob` or `regex` - Pattern matching for permission rules

**External:**
- SQLite (bundled via rusqlite, no system dependency)
- Claude Code hook system (PreToolUse events)

### Performance

- `log` subcommand is on the critical path (PreToolUse hook) - must complete in <50ms
- SQLite WAL mode for concurrent read/write safety
- Bundled SQLite avoids system library version issues
- `audit`, `suggest`, `report` are interactive - no strict latency requirement
- DB size: ~1KB per event, expect <10K events/month = <10MB/year

### Security

- Read-only access to settings files (never writes to settings.json or settings.local.json)
- SQLite DB stored in user-local directory with default permissions
- No network access
- No secrets stored in DB (tool inputs may contain file paths but not credentials)
- Hook JSON is ephemeral and only the structured fields are persisted
- `raw_json` column stores full payload for debugging but could be made opt-in if privacy is a concern

### Testing Strategy

- **Unit tests:** Risk tier classification, pattern matching, deny list checking, JSON parsing
- **Integration tests:** Full CLI invocation with test SQLite DB, mock settings files
- **Hook integration test:** Simulate PreToolUse JSON payload, verify DB write
- **Snapshot tests:** Audit output format stability

### Hook Registration

Add to `~/.claude/settings.json` (or `settings.local.json`) under the existing `hooks.PreToolUse` array:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "claude-permit log"
          }
        ]
      }
    ]
  }
}
```

Note: Empty `matcher` string matches all tools. This ensures every tool invocation is logged, not just Bash commands. The existing `allow-help.sh` hook uses `"matcher": "Bash"` to scope to Bash only.

### Skill Definition

The `/permissions-audit` skill (installed as a Claude Code skill):

```yaml
---
name: permissions-audit
description: Audit Claude Code permissions - review rules, suggest promotions, report session activity
---

Run the following commands and present the results:

1. `claude-permit audit` - show current rule classifications
2. `claude-permit suggest` - show promotion recommendations
3. `claude-permit report` - show recent session activity

Present findings as actionable recommendations. For any "promote" suggestions,
show the exact line to add to settings.json.
```

### Rollout Plan

1. `cargo install --path .` from the repo
2. Add PreToolUse hook to `~/.claude/settings.json` (see Hook Registration above)
3. Run sessions normally - events accumulate in `~/.local/share/claude-permit/events.db`
4. Run `claude-permit audit` manually to review current rules
5. Install `/permissions-audit` skill for on-demand audit + suggest + report

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Hook latency blocks Claude Code | Medium | High | Benchmark `log` command; target <50ms. On failure, output `{}` and exit 0 - never block the pipeline. |
| Hook JSON format changes between Claude Code versions | Medium | Medium | Parse defensively with serde; use `#[serde(flatten)]` for unknown fields; version the schema |
| SQLite DB corruption from concurrent hook invocations | Low | Medium | Use WAL mode; `CREATE TABLE IF NOT EXISTS` on every open; DB is rebuildable (not source of truth) |
| Risk tier classification produces false sense of security | Medium | Medium | Document that tiers are advisory; dangerous tier is conservative (over-flags) |
| Permission patterns too coarse for meaningful analysis | Low | Low | Flag overly-broad patterns (e.g., `Bash(git:*)` matches both safe and dangerous git commands) |
| Settings file format changes | Low | Medium | Pin to known schema; fail gracefully on unknown keys |
| DB creation race on first run | Low | Low | `CREATE TABLE IF NOT EXISTS` handles this; SQLite handles concurrent creates safely in WAL mode |
| `log` subcommand crash/hang blocks Claude Code | Medium | High | Wrap all logic in a catch-all; on any panic/error, print `{}` to stdout and exit 0 |

## Open Questions

- [ ] Should `log` write to DB synchronously or spawn a background process to avoid blocking the hook? (Benchmark first - if sync is <50ms, keep it simple)
- [ ] Exact fields available in PreToolUse hook JSON - need to capture a live sample to confirm session_id availability and tool_input structure per tool type
- [ ] Should risk tier classification be configurable via YAML config, or is hardcoded sufficient for v1?
- [ ] Should `audit` output diffs between settings.json and settings.local.json to show what's local-only vs global?
- [ ] Should `suggest` output exact permission rule syntax ready to paste into settings files? (Likely yes - reduces friction)
- [ ] Should `log` also act as an enforcer (deny permanently-blocked patterns) in Phase 1, or defer enforcement to Phase 4?

## References

- Claude Code hooks documentation
- `~/.claude/settings.json` - current global permission rules
- `~/.claude/settings.local.json` - current local permission rules
- `~/.claude/hooks/allow-help.sh` - existing hook example
- XDG Base Directory Specification (for DB path)
