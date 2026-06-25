# Design Document: Startup Performance

**Author:** Scott Idler
**Date:** 2026-04-17
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

`ccu` takes 7.4ms on average to execute (measured via `hyperfine`). For a tool used in shell
statuslines and prompt hooks, this latency is perceptible. Three targeted changes eliminate
the redundant work at startup: merging the double-parse of the user config, switching the
embedded pricing data from YAML to JSON, and lazily opening the log file only when output will
actually be produced.

## Problem Statement

### Background

`ccu` is invoked on every shell prompt via a statusline hook, making cold-start latency
visible to the user. The `statusline` subcommand was purpose-built for this path and already
returns before `compute_summaries` is called. But even the lightweight startup path - config
loading, logging setup, pricing initialization - adds up.

### Problem

Profiling the startup sequence reveals three redundant operations that fire before any real
work begins:

1. **Double YAML parse of the user config** - `resolve_log_filter` calls
   `Config::load_log_level()`, which reads and parses `~/.config/ccu/ccu.yml` just to extract
   the `log_level` field. `Config::load()` then reads and parses the same file again, moments
   later. Two `serde_yaml::from_str` calls on the same bytes.

2. **Slow YAML parser for embedded pricing** - `pricing::default_pricing()` calls
   `serde_yaml::from_str` on the 91-line `data/pricing.yml` embedded via `include_str!`.
   `serde_yaml` uses a streaming libyaml parser and is measurably slower than `serde_json`
   (typically 5-10x) for equivalent data volumes.

3. **Unconditional log file open** - `setup_logging` creates the log directory and opens the
   log file for append on every invocation, even when the effective log level (default: `warn`)
   guarantees nothing will be written.

### Goals

- Reduce mean cold-start latency from 7.4ms toward 3-4ms
- No behavioral change: log output, cache behavior, and all subcommands work identically
- No new dependencies (all required crates already in `Cargo.toml`)

### Non-Goals

- Persistent daemon / background process model
- Changing the cache invalidation strategy
- Optimizing the `compute_summaries` hot path (file parsing, deduplication)
- Reducing process startup overhead intrinsic to the OS and dynamic linker

## Proposed Solution

### Overview

Three independent changes, each addressing one root cause:

1. **Merge config load** - Load the config file once, return both `Config` and the resolved
   log filter string. Eliminate `load_log_level`.
2. **Pricing as JSON** - Convert `data/pricing.yml` to `data/pricing.json`. Update
   `pricing::default_pricing()` to use `serde_json::from_str`. The `serde_json` crate is
   already a dependency.
3. **Lazy log file open** - Only open the log file if the resolved log filter will produce
   output. At the default `warn` level, a normal `ccu` run logs nothing; skip the open.

### Architecture

The changes touch `src/config.rs`, `src/pricing.rs`, `src/main.rs`, `data/pricing.yml` (→ `data/pricing.json`), `bin/update`, and `build.rs`.
All callers of the embedded pricing use `pricing::default_pricing()`, which fully encapsulates the format.

#### Change 1: Single config parse

Current `main.rs` startup sequence:
```
resolve_log_filter(cli.log_level)   // reads + serde_yaml::from_str on user config
  └─ Config::load_log_level()
setup_logging(filter)
Config::load(cli.config)            // reads + serde_yaml::from_str on user config again
```

New sequence:
```
// Early exits (no config, no logging needed)
if Statusline  → return immediately
if Pricing --check → run check, exit

Config::load(cli.config)            // reads + parses once; returns Config
resolve_log_filter(cli.log_level, config.log_level.as_deref())  // no file I/O
setup_logging(filter, has_explicit_level)
```

The `Statusline` and `Pricing --check` early exits must stay **before** `Config::load`. Without
this guard, `ccu statusline` would start parsing the config (including the full pricing table)
before returning - regressing the very path this change is meant to help.

`resolve_log_filter` already accepts `Option<&str>` from the CLI. Extend its signature to also
accept `Option<&str>` from the config object. `Config::load_log_level` is deleted.

#### Change 2: Pricing as JSON

`data/pricing.yml` is machine-maintained (updated by the `update` subcommand and PRs).
There is no readability requirement for the runtime-embedded copy. Converting to JSON:

- Rename `data/pricing.yml` → `data/pricing.json`
- Rewrite content as equivalent JSON
- `pricing::default_pricing()`: replace `serde_yaml::from_str` with `serde_json::from_str`,
  update `include_str!` path
- `bin/update` (bash script) generates and writes `data/pricing.yml` - update it to generate
  and write `data/pricing.json` instead
- `build.rs` has `cargo:rerun-if-changed=data/pricing.yml` - update to `data/pricing.json`
- `serde_yaml` stays: the user config (`~/.config/ccu/ccu.yml`) is still YAML

#### Change 3: Lazy log open

Current `setup_logging` always opens the file. The condition for skipping the file open does not require parsing the filter string. Because
`resolve_log_filter` is the sole producer of the filter, the only way a non-default level
reaches `setup_logging` is if the CLI `--log-level` flag was set, the config `log_level` field
is set, or `RUST_LOG` is in the environment. All three are visible before `setup_logging` is
called:

```rust
fn setup_logging(filter: &str, has_explicit_level: bool) -> Result<()> {
    if !has_explicit_level {
        // Default warn level; nothing will be logged. Skip file open.
        env_logger::Builder::new().parse_filters(filter).init();
        return Ok(());
    }
    // ... existing file-open path ...
}
```

`has_explicit_level` is `cli.log_level.is_some() || config.log_level.is_some() || std::env::var("RUST_LOG").is_ok()`.

### Data Model

No data model changes. `CachedDay`, `Config`, `ModelPricing` are unchanged.

The JSON pricing file structure mirrors the YAML structure exactly - the outer key `pricing:`
becomes a top-level JSON object key, and the value is an object of model names to pricing
objects.

### API Design

Public function signatures that change:

```rust
// config.rs - load_log_level DELETED
// Config::load signature unchanged; returns Config with log_level populated

// main.rs - resolve_log_filter takes config log_level directly
fn resolve_log_filter(cli_level: Option<&str>, config_level: Option<&str>) -> String

// pricing.rs - implementation only; signature unchanged
pub fn default_pricing() -> HashMap<String, ModelPricing>
```

### Implementation Plan

#### Phase 1: Pricing JSON conversion
**Model:** sonnet

- Convert `data/pricing.yml` to `data/pricing.json` (use `python3 -c "import sys,json,yaml; print(json.dumps(yaml.safe_load(sys.stdin), indent=2))"` then verify)
- Update `pricing::default_pricing()`: change `include_str!` path and `serde_yaml::from_str` → `serde_json::from_str`
- Update `bin/update` bash script:
  - Change `PRICING_FILE` to `data/pricing.json`
  - Rewrite the awk `END` block to emit JSON: use leading-comma style (`if (i > 0) printf ","`) to avoid trailing-comma issues; wrap in `{"pricing":{...}}`
- Update `build.rs`: `rerun-if-changed=data/pricing.yml` → `data/pricing.json`
- Note: `PricingOnly` struct in `update.rs` needs no changes - `#[derive(Deserialize)]` works with any serde format
- Run `otto ci` to verify

#### Phase 2: Single config parse
**Model:** sonnet

- Move `Statusline` and `Pricing --check` early exits to before `Config::load` in `main()`
- Delete `Config::load_log_level` from `config.rs`
- Update `resolve_log_filter` signature: `(cli_level: Option<&str>, config_level: Option<&str>) -> String`, remove internal `load_log_level` call
- Reorder `main()`: `Config::load` → `resolve_log_filter` → `setup_logging` (after early exits)
- Run `otto ci` to verify

#### Phase 3: Lazy log file open
**Model:** sonnet

- Add `has_explicit_level: bool` param to `setup_logging`
- Skip file open when `!has_explicit_level`; fall back to stderr-only env_logger
- Verify log file still created when `--log-level debug` is passed
- Run `otto ci` to verify

## Alternatives Considered

### Alternative 1: Compile-time pricing table
- **Description:** Use a `const` or `proc_macro` to embed the pricing table as a Rust literal
  (`HashMap`) at compile time, eliminating the runtime parse entirely.
- **Pros:** Zero runtime cost for pricing initialization.
- **Cons:** Requires a proc macro or build.rs code generation; significantly more complex.
  The pricing table changes frequently (new models, price updates) - keeping the code-gen in
  sync adds friction to maintenance.
- **Why not chosen:** Complexity not justified; JSON parse is already much cheaper than YAML.

### Alternative 2: Separate log-level config file
- **Description:** Store log level in a tiny separate file (e.g., `~/.config/ccu/log-level`)
  so `load_log_level` is a cheap `read_to_string` without YAML parsing.
- **Pros:** Preserves the pre-logging-setup read pattern; no reordering needed.
- **Cons:** Two config files for one tool; confusing UX; doesn't eliminate the full config
  parse that follows.
- **Why not chosen:** Merging the parse is simpler and eliminates more redundancy.

### Alternative 3: Move scanner after cache feasibility check
- **Description:** Before scanning all JSONL files to compute the mtime hash, check if a
  cache entry exists for today at all. If not, skip directly to the scan. On a cache hit the
  scan cost is still paid, but a two-level check (existence check → hash check) could reduce
  syscalls on hot paths.
- **Pros:** Avoids double-scan when cache is guaranteed to miss (e.g., first run of the day).
- **Cons:** For the warm-cache path (most statusline calls), the scan still happens in full.
  Does not address the startup overhead that fires before `compute_summaries`. Limited benefit
  relative to the other three changes.
- **Why not chosen:** Deferred to a follow-up; the three targeted changes above address more
  of the measured latency with less risk.

### Alternative 4: Persistent background daemon
- **Description:** Run `ccu` as a background process that pre-computes and caches results;
  statusline reads from a socket or file.
- **Pros:** Sub-millisecond statusline reads.
- **Cons:** Process management complexity; risk of stale data; overkill given the existing
  cache already handles the hot path well.
- **Why not chosen:** Architectural overreach for a monitoring tool.

## Technical Considerations

### Dependencies

- `serde_json` is already in `Cargo.toml` - no new dependency needed for the JSON conversion.
- `serde_yaml` remains after these changes because `~/.config/ccu/ccu.yml` (the user config)
  is still YAML. Removing it would require migrating the user-facing config format, which is
  out of scope.

### Performance

Expected impact per change:
- **Merge config parse**: eliminates one `serde_yaml::from_str` + one `read_to_string`. Est.
  ~1-1.5ms saving.
- **Pricing as JSON**: replaces slow `serde_yaml::from_str` with fast `serde_json::from_str`
  on embedded data. Est. ~1-1.5ms saving.
- **Lazy log open**: eliminates `create_dir_all` + `OpenOptions::open` on the default path.
  Est. ~0.3-0.5ms saving.

Combined target: reduce mean cold-start from 7.4ms to ~4ms.

### Security

No security implications. These are purely internal startup optimizations.

### Testing Strategy

- All existing unit tests continue to pass (`otto ci`)
- Manual `hyperfine ccu` before and after to confirm measurable improvement
- Verify `--log-level debug` still produces a log file after Change 3

### Rollout Plan

Ship as a single commit to main via `/shipit` (patch bump). All three phases are small enough
to bundle; each is independently compilable if a bisect is ever needed.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Reorder slows `statusline` path | High | High | Keep `Statusline`/`Pricing --check` early exits **before** `Config::load`; document this constraint in a comment |
| Reorder silences config-load log messages | Low | Low | `Config::load`'s `log::debug!`/`log::warn!` fire before logger init and become no-ops; acceptable since they only fire at non-default levels |
| JSON pricing diverges from YAML source | Low | Medium | Delete the YAML file entirely; single source of truth is the JSON |
| Lazy log skips file when user expects it | Low | Low | File is skipped only at default warn level; `--log-level info` forces the open |
| `bin/update` generates YAML after JSON conversion | Medium | Medium | Phase 1 explicitly updates `bin/update`; CI catches parse errors at test time |

## Open Questions

- [x] Does `update.rs` write pricing back to `data/pricing.yml`? **No** - `src/update.rs`
  only reads the embedded data and compares hashes. All writes happen in `bin/update` (bash).
  Phase 1 must update `bin/update` to generate JSON.
- [ ] Should `serde_yaml` be scoped to only the user config parse path via a feature flag or
  module boundary, making the dependency isolation explicit? (Minor; not blocking.)

## References

- `hyperfine ccu` result: mean 7.4ms ± 1.1ms, 401 runs
- `src/config.rs` - `Config::load`, `Config::load_log_level`
- `src/pricing.rs` - `default_pricing`, `normalize_model_id`, `calculate_cost`
- `src/main.rs` - `resolve_log_filter`, `setup_logging`, `main`
- `data/pricing.yml` - 91-line embedded pricing table
- `src/update.rs` - pricing fetch and write logic
