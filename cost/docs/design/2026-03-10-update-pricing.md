# Design Document: LLM-Powered Pricing Management (`ccu pricing`)

**Author:** Scott Idler
**Date:** 2026-03-10
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

Add a `ccu pricing` subcommand with `--update`, `--show`, and `--from` flags for managing model pricing configuration. The `--update` flag fetches Anthropic's pricing page as markdown (via jina.ai reader), passes it to `claude -p` with a structured extraction prompt, validates the output, and writes/updates `~/.config/ccu/ccu.yml`. This eliminates embedded pricing defaults from the binary - the config file becomes the sole source of truth, generated on first use and updatable at any time.

## Problem Statement

### Background

`ccu` needs accurate per-model pricing to compute costs from JSONL session logs. Currently, pricing is hardcoded in `pricing.rs` via `default_pricing_table()`. This creates two problems: new users have no config file and silently get defaults that may be stale, and existing users have no way to update pricing without editing YAML by hand or waiting for a new binary release.

### Problem

1. **Initial setup:** A new user installs `ccu` and runs it. Today it silently falls back to embedded defaults. If the binary is old, those defaults are wrong. The user has no way to bootstrap a correct config without manually researching pricing and writing YAML.
2. **Ongoing maintenance:** When Anthropic changes pricing or adds models, the user must manually discover the changes and update their config. This is error-prone human toil.

### Goals

- Provide a single command (`ccu pricing --update`) that generates or updates `~/.config/ccu/ccu.yml` with current pricing
- Remove all embedded pricing defaults from the binary - config file is the sole source of truth
- Make first-run onboarding clear: if no config exists, error with instructions to run `ccu pricing --update`
- Support offline/manual use via `--from <file.md>` for users without network or claude CLI
- Allow users to inspect current pricing with `ccu pricing --show`

### Non-Goals

- Automatic/silent pricing updates (user must explicitly run the command)
- Anthropic API key management or direct API pricing queries
- Long context pricing tiers (standard pricing is sufficient for now)
- Price change notifications or alerts

## Proposed Solution

### Overview

```
ccu pricing [FLAGS]

Flags:
  --update            Fetch current pricing from Anthropic and update config
  --show              Display current pricing table from config
  --from <file.md>    Read pricing from a local markdown file instead of fetching

Examples:
  ccu pricing --update                    # fetch + extract + write config
  ccu pricing --update --from pricing.md  # extract from local file
  ccu pricing --show                      # display current config pricing
  ccu pricing                             # same as --show (default)
```

**Update flow:**

```
1. Fetch markdown:
   - Default: curl https://r.jina.ai/https://docs.anthropic.com/en/docs/about-claude/models
   - --from: read local file
2. Spawn: claude -p "<extraction prompt>" < markdown
3. Parse LLM output as YAML, validate against Config struct
4. Show diff of what changed
5. Write to ~/.config/ccu/ccu.yml
```

The LLM is the parser. It handles format changes on Anthropic's pricing page gracefully - no brittle HTML/regex scraping. `r.jina.ai` converts the page to clean markdown with zero dependencies (just an HTTP GET). `claude -p` uses whatever auth the user already has configured.

### Architecture

New module: `src/update.rs`

```
src/update.rs    Fetch markdown, spawn claude, validate, write config
```

Integrates with existing:
- `cli.rs` - replace `UpdatePricing` variant with `Pricing` subcommand with `--update`, `--show`, `--from` flags
- `config.rs` - needs changes to error on missing config (remove Default impl)
- `pricing.rs` - remove `default_pricing_table()` (already in progress per git diff)

### CLI Design

```rust
#[derive(Subcommand)]
pub enum Command {
    // ... existing commands ...

    /// Manage model pricing configuration
    Pricing {
        /// Fetch current pricing from Anthropic and update config
        #[arg(long)]
        update: bool,

        /// Display current pricing table
        #[arg(long)]
        show: bool,

        /// Read pricing from a local markdown file instead of fetching
        #[arg(long)]
        from: Option<PathBuf>,
    },
}
```

Behavior:
- `ccu pricing` (no flags) - defaults to `--show`
- `ccu pricing --show` - display current pricing from config (requires config)
- `ccu pricing --update` - fetch, extract, validate, write
- `ccu pricing --update --from file.md` - extract from local file, validate, write
- `ccu pricing --from file.md` - implies `--update`

### The Extraction Prompt

The prompt sent to `claude -p` is critical. It must:

1. Define the exact YAML schema expected (matching `ModelPricing` struct fields)
2. List what model families to extract (opus, sonnet, haiku)
3. Specify the cache pricing derivation rules (if not explicit on the page)
4. Request only YAML output with no commentary

```
Extract Claude model pricing from the following markdown and output YAML.

Output format - pricing map keyed by model ID, values in dollars per million tokens:

pricing:
  claude-opus-4-6:
    input_per_mtok: 5.0
    output_per_mtok: 25.0
    cache_5m_write_per_mtok: 6.25
    cache_1h_write_per_mtok: 10.0
    cache_read_per_mtok: 0.50

Rules:
- Include ALL Claude models listed on the page
- Use the base model ID without date suffixes (e.g., "claude-opus-4-6" not "claude-opus-4-6-20260101")
- If multiple versions share the same pricing, include each as a separate entry
- Cache write pricing: if a "5-minute" and "1-hour" cache write price are listed, use those.
  If only a single "cache write" price is listed, use it for cache_5m_write_per_mtok and
  set cache_1h_write_per_mtok to cache_5m_write_per_mtok * 1.6 (the standard ratio)
- Cache read pricing: use the listed cache read price
- Output ONLY the YAML block, no explanations or markdown fences
```

### Config Changes

**Before (current):** `Config::load()` falls back to `Default::default()` with embedded pricing.

**After:** `Config::load()` returns an error if no config file exists, with a message directing the user to run `ccu pricing --update`.

```rust
// config.rs - no more Default for Config
impl Config {
    pub fn load(config_path: Option<&PathBuf>) -> Result<Self> {
        // ... existing file lookup logic ...

        // No config found - error instead of silent defaults
        eyre::bail!(
            "No config file found. Run `ccu pricing --update` to generate one.\n\
             Config location: ~/.config/ccu/ccu.yml"
        );
    }
}
```

**Exception:** `ccu pricing --update` must work without an existing config (it's creating one). The `main()` dispatch handles this with a two-phase approach:

```rust
fn main() -> Result<()> {
    let cli = Cli::parse();

    // Phase 1: handle commands that don't need config
    if let Some(Command::Pricing { update, from, .. }) = &cli.command {
        if *update || from.is_some() {
            return update::run(from.as_ref());
        }
    }

    // Phase 2: all other commands require config
    let config = Config::load(cli.config.as_ref())?;

    // Pricing --show needs config, handled here
    if let Some(Command::Pricing { .. }) = &cli.command {
        return update::show(&config);
    }

    run(&cli, &config)
}
```

**Merge behavior:** When updating an existing config, `pricing --update` reads the current file first and only replaces the `pricing` section. Other fields (e.g., `projects_dir`) are preserved.

### Validation

Before writing the config file:

1. Strip markdown code fences from LLM output if present (LLMs often add them despite instructions)
2. Parse LLM YAML output into `Config` struct via serde
3. Check that at least one model entry exists
4. Check that all pricing values are positive numbers (not zero, not negative)
5. Check that known model families are present (opus, sonnet, haiku) - warn if missing but don't block
6. If an existing config file exists, show a diff of pricing changes before overwriting

**Note:** The `Config` struct uses `#[serde(default)]` which means serde will happily produce a struct with an empty pricing map. Step 3 catches this - an empty pricing map is always an error.

### Implementation Plan

**Phase 1: Remove embedded defaults, require config**
- Remove `default_pricing_table()` from `pricing.rs`
- Remove `Default` impl from `Config`
- Update `Config::load()` to error on missing config
- Update `main()` to skip config loading for `pricing --update` command

**Phase 2: Implement `pricing` subcommand**
- Add `src/update.rs` module
- Replace `UpdatePricing` CLI variant with `Pricing { update, show, from }` variant
- Implement `--show` to display current pricing table in a readable format
- Implement markdown fetching via `r.jina.ai` (using `std::process::Command` with `curl`)
- Implement `claude -p` spawning with extraction prompt (60-second timeout)
- Implement YAML validation and config writing
- Wire into `main()` command dispatch

**Phase 3: Polish**
- Show diff when updating existing config
- Preserve non-pricing config fields (e.g., `projects_dir`) when updating
- Print summary of extracted models after update

## Alternatives Considered

### Alternative 1: Embedded defaults with config overrides
- **Description:** Keep hardcoded pricing in the binary, use config only for overrides
- **Pros:** Works out of the box, no extra setup step
- **Cons:** Defaults go stale between releases, two sources of truth, unclear which is active
- **Why not chosen:** Creates ambiguity about where pricing comes from; stale defaults give wrong costs silently

### Alternative 2: Direct HTML scraping with regex
- **Description:** Fetch Anthropic's pricing page and parse with regex/HTML parser
- **Pros:** No LLM dependency, deterministic
- **Cons:** Extremely brittle - any page layout change breaks it, complex to maintain
- **Why not chosen:** The LLM-as-parser pattern handles format changes gracefully

### Alternative 3: Anthropic API for pricing
- **Description:** Query an Anthropic API endpoint for current pricing
- **Pros:** Authoritative source, structured data
- **Cons:** No such public API exists for pricing data
- **Why not chosen:** The API doesn't exist

### Alternative 4: markitdown-cli instead of jina.ai
- **Description:** Use Microsoft's markitdown CLI tool to convert the page to markdown
- **Pros:** Local processing, no external service dependency
- **Cons:** Extra binary dependency to install and maintain
- **Why not chosen:** jina.ai is just a URL prefix - zero dependencies, works with a simple HTTP GET

### Alternative 5: Top-level `update-pricing` command
- **Description:** Keep pricing update as a standalone subcommand (`ccu update-pricing`)
- **Pros:** Simple, single-purpose
- **Cons:** Doesn't scale - no way to show/inspect pricing without a separate command
- **Why not chosen:** `ccu pricing` as a subcommand with `--update`/`--show` flags is more extensible and follows the pattern of grouping related operations under a noun

## Technical Considerations

### Dependencies

**New runtime dependencies:**
- `curl` (system) - for fetching from jina.ai (available on virtually all systems)
- `claude` CLI - for LLM extraction (user presumably has this since they use Claude Code)

**No new Cargo dependencies required.** All spawning done via `std::process::Command`.

### Performance

Not performance-critical. This is a manual, infrequent operation (run once at setup, occasionally for updates). Network fetch + LLM processing will take 5-30 seconds, which is acceptable for an explicit user action.

### Security

- jina.ai reader is a read-only proxy - it fetches public web pages, no auth required
- `claude -p` runs locally with the user's existing auth
- YAML output is validated through serde deserialization before being written
- No secrets or credentials are handled by this command
- Config file is written to user-owned directory with default permissions

### Testing Strategy

- **Unit tests:** YAML validation logic (valid configs, missing fields, negative prices, empty pricing maps), code fence stripping
- **Integration tests:** Mock the `curl` and `claude` commands to test the full pipeline with fixture data
- **Manual tests:** Run against live jina.ai + claude to verify end-to-end extraction accuracy

### Rollout Plan

1. Remove embedded defaults and add error message (breaking change, but config file was already the recommended path)
2. Implement `pricing` subcommand with `--update` and `--show`
3. Update README with first-run instructions
4. Release new version

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| jina.ai service unavailable | Low | Medium | `--from` flag allows manual markdown input; user can save page manually |
| claude CLI not installed | Medium | High | Clear error message with install instructions; `--from` + manual YAML editing as fallback |
| LLM extracts wrong pricing | Low | Medium | Validation step catches structural errors; diff display lets user verify before writing |
| Anthropic pricing page restructured | Medium | Low | LLM handles format changes gracefully - this is the whole point of the approach |
| jina.ai changes their API | Low | Low | It's a simple URL prefix; trivially replaceable with any markdown conversion service |
| Cache pricing ratios change | Low | Low | Prompt instructs LLM to use explicit values when available; ratio is only a fallback |
| claude -p hangs or times out | Low | Medium | Set a 60-second timeout on the subprocess; error with clear message |
| LLM returns partial model list | Low | Medium | Warn when known model families are missing; show extracted models for user verification |

## Open Questions

- [ ] Should the extraction prompt be embedded in the binary or stored as a separate file for easier iteration?
- [ ] Should we add a `--dry-run` flag that shows what would be written without actually writing?
- [ ] What's the best error message when `claude` CLI is not found? Should we suggest `npm install -g @anthropic-ai/claude-code`?

## References

- jina.ai reader: https://r.jina.ai (prepend to any URL for markdown conversion)
- Claude Code CLI: `claude -p` for non-interactive prompting
- Anthropic pricing page: https://docs.anthropic.com/en/docs/about-claude/models
- Existing design doc: `docs/design/2026-03-10-claude-cost-usage.md`

---

## Review Log

### Pass 1: Draft

Initial draft with `update-pricing` as top-level subcommand. Full coverage of problem statement, extraction prompt, validation pipeline, config changes, and alternatives.

### Pass 2: Correctness

- Verified `UpdatePricing` CLI variant already exists in `cli.rs:57-62`
- Confirmed cache pricing 1.6x ratio: opus 5m=$6.25 to 1h=$10.00, sonnet 5m=$3.75 to 1h=$6.00 - both 1.6x
- Noted `Config` has `#[serde(default)]` which would silently produce empty pricing maps - added explicit validation check
- Added note about stripping markdown code fences from LLM output (common LLM behavior)

### Pass 3: Clarity

- Made the `main()` two-phase dispatch explicit with code example showing how `update-pricing` bypasses config loading
- Clarified merge vs overwrite behavior for existing configs
- Resolved open question: merge behavior preserves non-pricing fields

### Pass 4: Edge Cases

- Added subprocess timeout risk (claude -p hanging) with 60-second timeout mitigation
- Added partial model list risk with warning mitigation
- Confirmed we're solving the right problem: LLM-as-parser is the right approach for a page format that may change

### Pass 5: Excellence

- Incorporated user feedback: restructured from `ccu update-pricing` to `ccu pricing` subcommand with `--update`, `--show`, `--from` flags
- This is more extensible (future flags like `--diff`, `--dry-run`) and follows noun-verb CLI patterns
- Updated all code examples, CLI definitions, error messages, and references to use new `ccu pricing` structure
- Added Alternative 5 documenting why the top-level command was replaced
- Removed resolved open question (merge vs overwrite - answered: merge)

**CONVERGENCE REACHED:** Document ready for implementation.
