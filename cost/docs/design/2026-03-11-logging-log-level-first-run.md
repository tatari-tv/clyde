# Design Document: Logging Improvements, Log-Level Configuration, and First-Run Experience

**Author:** Scott Idler
**Date:** 2026-03-11
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

Three improvements to `ccu`: (1) show log file location in `--help` output and instrument functions with debug/info logging, (2) add log-level configuration with standard CLI > env > config > default override precedence, and (3) detect missing config on first run and offer to run `ccu pricing --update` interactively.

## Problem Statement

### Background

`ccu` has basic logging infrastructure (`env_logger` writing to `~/.local/share/ccu/logs/ccu.log`) but the log file path is not discoverable from `--help`, there is no way to control log verbosity without setting the raw `RUST_LOG` env var, and most functions lack debug-level instrumentation. Additionally, first-time users who install `ccu` and run it without config get an unhelpful error message.

### Problem

1. **Discoverability:** Users cannot find the log file path from `ccu --help`. The sibling tool `gx` shows its log path in help output.
2. **Observability:** Most functions have no debug logging, making it hard to diagnose issues. Only `scanner`, `cache`, and `config` have any `info!` statements.
3. **Log-level control:** The only way to change log verbosity is `RUST_LOG=debug ccu ...`. There is no CLI flag or config file option, which is non-standard for CLI tools.
4. **First-run UX:** Running `ccu` without config produces `Error: Failed to load configuration` with a suggestion to run `ccu pricing --update` - but the user must copy-paste and re-run manually.

### Goals

- Show log file path in `ccu --help` after-help text (matching `gx` pattern)
- Add `debug!` statements at function entry points for key functions, `info!` for significant operations
- Add `--log-level` CLI flag, `CCU_LOG_LEVEL` env var, and `log_level` config field with standard override precedence
- Detect missing config and interactively offer to run pricing update

### Non-Goals

- Structured/JSON log output (not needed for a local CLI tool)
- Log rotation (append-only log file is fine; users can manage with logrotate or manual deletion)
- Colored log output to stderr (logs go to file only)
- Auto-running pricing update without user confirmation

## Proposed Solution

### Feature 1: Log Path in `--help` and Debug Instrumentation

#### Log Path Display

Add the log file path to the `--help` after-help text, matching the `gx` pattern:

```
Parses Claude Code JSONL session logs to compute cost summaries.

Logs are written to: ~/.local/share/ccu/logs/ccu.log
```

**Implementation:** The current `after_help` is a static string in the clap derive macro. Since the log path is computed at runtime via `dirs::data_local_dir()`, we need dynamic after-help. Approach:

1. Remove static `after_help` from `#[command(...)]` in `cli.rs`
2. Extract `log_file_path() -> PathBuf` helper in `main.rs`
3. In `main()`, use `Cli::command().after_help(dynamic_string).get_matches()` with `Cli::from_arg_matches()` to parse

This requires importing `clap::{CommandFactory, FromArgMatches}` in `main.rs`.

#### Debug Instrumentation

Add `log::debug!` at function entry points with parameter values. Be judicious - skip per-entry hot paths and pure formatting functions.

**Functions to instrument:**

| Module | Function | Level | What to log |
|--------|----------|-------|-------------|
| `main.rs` | `compute_summaries` | debug | start, end, verbose, model filter |
| `main.rs` | `run` | debug | command variant |
| `scanner.rs` | `find_session_files` | debug | projects_dir |
| `scanner.rs` | `filter_by_date_range` | debug | start, end, input count |
| `parser.rs` | `parse_jsonl_file` | debug | path |
| `cache.rs` | `compute_mtime_hash` | debug | file count |
| `cache.rs` | `prune_cache` | debug | keep_days |
| `update.rs` | `run` | debug | from path |
| `update.rs` | `show` | debug | model count |
| `update.rs` | `fetch_markdown` | debug | URL |
| `update.rs` | `extract_pricing` | debug | markdown length |
| `config.rs` | `load` | debug | config_path |

**Excluded (too noisy or not useful):**
- `pricing.rs` - `normalize_model_id` and `calculate_cost` are called per JSONL entry (thousands of times)
- `output.rs`, `graph.rs`, `table.rs`, `average.rs` - pure formatting, no side effects

### Feature 2: Log-Level Configuration

#### Override Precedence

Standard industry pattern (most specific wins):

1. CLI flag: `--log-level <LEVEL>`
2. Environment variable: `CCU_LOG_LEVEL`
3. Config file: `log_level` field in `ccu.yml`
4. Fallback env var: `RUST_LOG` (for Rust ecosystem compatibility)
5. Default: `warn`

Valid levels: `trace`, `debug`, `info`, `warn`, `error`

#### CLI Argument

Add to `Cli` struct:

```rust
/// Set log level (trace, debug, info, warn, error)
#[arg(long, env = "CCU_LOG_LEVEL")]
pub log_level: Option<String>,
```

The `env = "CCU_LOG_LEVEL"` attribute makes clap check the env var automatically when the CLI flag is not provided. This collapses levels 1 and 2 into a single `Option<String>`.

**Dependency:** Requires `env` feature for clap in `Cargo.toml`:
```toml
clap = { version = "4.5.60", features = ["derive", "env"] }
```

#### Config File Field

Add to `Config` struct:

```rust
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub projects_dir: Option<PathBuf>,
    pub log_level: Option<String>,
    pub pricing: HashMap<String, ModelPricing>,
}
```

Example `ccu.yml`:
```yaml
log_level: info
pricing:
  claude-opus-4-6:
    ...
```

#### Chicken-and-Egg: Log Level Before Config

Problem: `setup_logging()` must run early, but full config loading may fail (no config file, no pricing). Solution: add a lightweight `Config::load_log_level()` method that reads only the `log_level` field from the config file, returning `None` if the file doesn't exist or can't be parsed.

```rust
impl Config {
    pub fn load_log_level() -> Option<String> {
        let config_dir = dirs::config_dir()?;
        let path = config_dir.join("ccu").join("ccu.yml");
        let content = fs::read_to_string(&path).ok()?;
        let config: Config = serde_yaml::from_str(&content).ok()?;
        config.log_level
    }
}
```

#### Resolution Flow in `main()`

The resolution function returns an `env_logger` filter string. For app-specific sources (CLI, `CCU_LOG_LEVEL`, config), the level is scoped to the `ccu` crate (e.g., `ccu=debug`) so dependency crates like `rayon` and `serde` don't flood the log. For `RUST_LOG`, the value is passed through as-is since users expect full control over module-level filtering.

```rust
fn resolve_log_filter(cli_level: Option<&str>) -> String {
    // CLI / CCU_LOG_LEVEL (merged by clap)
    if let Some(level) = cli_level {
        return format!("ccu={}", level);
    }
    // Config file
    if let Some(level) = Config::load_log_level() {
        return format!("ccu={}", level);
    }
    // RUST_LOG - pass through as-is (advanced users expect full filter syntax)
    if let Ok(filter) = std::env::var("RUST_LOG") {
        return filter;
    }
    // Default
    "ccu=warn".to_string()
}
```

#### Updated `setup_logging`

Takes the resolved filter string (e.g., `ccu=debug` or a raw `RUST_LOG` value):

```rust
fn setup_logging(filter: &str) -> Result<()> {
    let log_file = log_file_path();
    let log_dir = log_file.parent().expect("log file has parent");
    fs::create_dir_all(log_dir).context("Failed to create log directory")?;

    let target = Box::new(
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
            .context("Failed to open log file")?,
    );

    env_logger::Builder::new()
        .parse_filters(filter)
        .target(env_logger::Target::Pipe(target))
        .init();

    info!("Logging initialized, filter={}, file={}", filter, log_file.display());
    Ok(())
}
```

### Feature 3: First-Run Config Detection

#### Current Behavior

```
$ ccu
Error: Failed to load configuration

Caused by:
    No config file found. Run `ccu pricing --update` to generate one.
    Config location: ~/.config/ccu/ccu.yml
```

#### New Behavior

```
$ ccu
No config file found at ~/.config/ccu/ccu.yml
Would you like to fetch pricing data now? [Y/n]
```

- Enter or `y`/`yes`: runs `update::run(None)?`, then reloads config and continues
- `n`/`no`: exits with `"Run 'ccu pricing --update' when ready."`
- Non-interactive (piped stdin): skips prompt, bails with error message (same as current behavior but with dynamic path)

#### Implementation

Make `update::config_path()` public so both `update.rs` and `main.rs` can use it (currently private). This avoids duplicating path computation logic.

In `main()`, wrap the config load with recovery logic. Distinguish three cases:
1. User specified `--config <path>` and it doesn't exist - error (user mistake)
2. Default config doesn't exist - offer first-run setup
3. Config exists but fails to parse - error (corrupt file)

```rust
// Phase 2: all other commands require config
let config = match Config::load(cli.config.as_ref()) {
    Ok(config) => config,
    Err(e) => {
        // If user explicitly specified a config path, always error
        if cli.config.is_some() {
            return Err(e.wrap_err("Failed to load configuration"));
        }

        // If the default config file exists but failed to parse, that's a real error
        let default_path = update::config_path()?;
        if default_path.exists() {
            return Err(e.wrap_err("Failed to load configuration"));
        }

        // No config file - offer to create one
        // Check if stdin is a terminal; if not (piped/scripted), skip the prompt
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            eyre::bail!(
                "No config file found at {}. Run `ccu pricing --update` to generate one.",
                default_path.display()
            );
        }

        eprintln!("No config file found at {}", default_path.display());
        eprint!("Would you like to fetch pricing data now? [Y/n] ");
        std::io::stderr().flush().ok();

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();

        if input.is_empty() || input == "y" || input == "yes" {
            update::run(None)?;
            Config::load(cli.config.as_ref())
                .context("Failed to load configuration after pricing update")?
        } else {
            eprintln!("Run `ccu pricing --update` when ready.");
            std::process::exit(1);
        }
    }
};
```

This requires making `update::config_path()` public (changing `fn config_path()` to `pub fn config_path()` in `update.rs`).

### Complete `main()` Flow

All three features converge in `main()`. Here is the full restructured flow with new imports noted:

```rust
// New imports needed in main.rs:
use clap::{CommandFactory, FromArgMatches};
use log::{debug, info, warn};
use std::io::Write; // for stderr().flush()

fn log_file_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ccu")
        .join("logs")
        .join("ccu.log")
}

fn resolve_log_filter(cli_level: Option<&str>) -> String { /* ... as shown above ... */ }

fn setup_logging(filter: &str) -> Result<()> { /* ... as shown above ... */ }

fn main() -> Result<()> {
    // Step 1: Build CLI with dynamic after-help showing log path
    let log_path = log_file_path();
    let after_help = format!(
        "Parses Claude Code JSONL session logs to compute cost summaries.\n\n\
         Logs are written to: {}",
        log_path.display()
    );
    let matches = Cli::command().after_help(after_help).get_matches();
    let cli = Cli::from_arg_matches(&matches)?;

    // Step 2: Resolve log level and initialize logging
    let filter = resolve_log_filter(cli.log_level.as_deref());
    setup_logging(&filter)?;

    // Step 3: Handle pricing update (doesn't need config)
    if let Some(Command::Pricing { update, from, .. }) = &cli.command
        && (*update || from.is_some())
    {
        return update::run(from.as_ref());
    }

    // Step 4: Load config with first-run recovery
    let config = match Config::load(cli.config.as_ref()) {
        Ok(config) => config,
        Err(e) => {
            // ... first-run detection and prompt logic as shown above ...
        }
    };

    info!("Config loaded, {} models in pricing table", config.pricing.len());

    // Step 5: Handle remaining commands
    if let Some(Command::Pricing { .. }) = &cli.command {
        return update::show(&config);
    }

    run(&cli, &config)
}
```

## Alternatives Considered

### Alternative 1: Static `after_help` with hardcoded path
- **Description:** Use `~/.local/share/ccu/logs/ccu.log` as static text in `after_help`
- **Pros:** No code changes to CLI parsing
- **Cons:** Not portable - `dirs::data_local_dir()` varies by OS (macOS uses `~/Library/Application Support`)
- **Why not chosen:** Dynamic computation is minimal overhead and handles all platforms

### Alternative 2: Use `RUST_LOG` as the only env var
- **Description:** Don't introduce `CCU_LOG_LEVEL`, just use `RUST_LOG`
- **Pros:** One fewer env var, Rust ecosystem standard
- **Cons:** `RUST_LOG` affects all Rust libraries (e.g., `rayon`, `serde`), not just ccu. Users get flooded with library debug output. Also doesn't support config file.
- **Why not chosen:** `CCU_LOG_LEVEL` is app-specific, cleaner UX. We keep `RUST_LOG` as a lower-priority fallback.

### Alternative 3: Auto-run pricing update without prompting
- **Description:** When no config exists, silently run `ccu pricing --update`
- **Pros:** Zero-friction first run
- **Cons:** Spawns `curl` and `claude -p` without consent. Takes 10-30 seconds. May fail if `claude` isn't installed. Surprising behavior.
- **Why not chosen:** Interactive prompt is safer and gives user control. Default is [Y] for easy confirmation.

### Alternative 4: Use `tracing` instead of `log`/`env_logger`
- **Description:** Replace `log` + `env_logger` with `tracing` + `tracing-subscriber`
- **Pros:** Structured logging, span support, more powerful filtering
- **Cons:** Major dependency change, overkill for a CLI tool, would touch every file
- **Why not chosen:** Current `log`/`env_logger` stack is lightweight and sufficient. Not worth the churn.

## Technical Considerations

### Dependencies

- **New:** `clap` `env` feature (for `#[arg(env = "...")]`)
- **Existing (no changes):** `log`, `env_logger`, `dirs`, `serde`, `serde_yaml`

### Performance

- `log_file_path()` is called once at startup (negligible)
- `Config::load_log_level()` reads and parses YAML once before full config load (adds ~1ms)
- `debug!` statements check log level at runtime and skip string formatting when disabled (a single comparison per call, negligible overhead)

### Testing Strategy

- **Log-level resolution:** Unit test `resolve_log_filter` with various combinations of CLI/config/env
- **Config log_level:** Test that `load_log_level()` returns `None` for missing file, and correct value for valid file
- **First-run prompt:** Integration test is difficult (requires stdin mocking). Manual testing is sufficient - the prompt path is simple and self-contained.
- **Debug statements:** Verified by running `ccu --log-level debug today` and inspecting log file

### Rollout Plan

Single release. All three features are backwards-compatible:
- `--log-level` is a new optional flag
- `log_level` in config is `serde(default)`, ignored if absent
- First-run prompt only triggers when no config exists (same failure case as before, but handled gracefully)

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| `clap` `env` feature increases binary size | Low | Low | Feature is lightweight, adds minimal code |
| First-run prompt in non-interactive context | Med | Med | Detect `stdin.is_terminal()` (stable since Rust 1.70) - skip prompt and bail with helpful message when piped |
| `load_log_level()` double-reads config file | Low | Low | Extra file read is ~1ms, only happens once at startup |
| Invalid log level string from user | Low | Low | `env_logger::parse_filters()` silently ignores invalid levels, defaulting to no output. Acceptable UX. |

## Open Questions

- [x] Env var name: `CCU_LOG_LEVEL` (app-specific, clean UX) - decided
- [x] Keep `RUST_LOG` as fallback: yes, for Rust ecosystem compatibility - decided
- [x] Default log level: `warn` (standard for CLI tools) - decided
- [x] First-run: prompt with `[Y/n]` default yes - decided

## File Change Summary

| File | Changes |
|------|---------|
| `Cargo.toml` | Add `env` feature to clap |
| `src/cli.rs` | Add `--log-level` arg, remove static `after_help` |
| `src/config.rs` | Add `log_level` field, add `load_log_level()` method |
| `src/main.rs` | Add `log_file_path()`, `resolve_log_filter()`. Restructure `main()` for dynamic after-help (`CommandFactory`/`FromArgMatches`), log-level resolution, first-run prompt. Add `debug!` to `compute_summaries` and `run`. Import `std::io::Write`. |
| `src/scanner.rs` | Add `debug!` to `find_session_files`, `filter_by_date_range` |
| `src/parser.rs` | Add `debug!` to `parse_jsonl_file` |
| `src/cache.rs` | Add `debug!` to `compute_mtime_hash`, `prune_cache` |
| `src/update.rs` | Make `config_path()` public. Add `debug!` to `run`, `show`, `fetch_markdown`, `extract_pricing` |

## References

- `gx --help` output showing log path pattern
- env_logger docs: https://docs.rs/env_logger
- clap `env` feature: https://docs.rs/clap/latest/clap/_derive/index.html
