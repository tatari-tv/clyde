## Phase 1: Docs + stale-name fixes

### Design decisions

- `check_hook_registered` in `permit/src/cmd/check.rs` - accept both `claude-permit` and `clyde permit` in the content check, mirror `install.rs:has_permit_hook`, PASS message always says `clyde permit log` (the canonical form) - this accurately reflects what `install` now writes and what the hook should say post-migration.
- `check_binary_in_path` in `permit/src/cmd/check.rs` - prefers `clyde` but falls back to `claude-permit` via `or_else` so pre-bootstrap hosts still pass the binary check rather than producing a false FAIL.
- `install.rs` messages - kept the doc-comment on `run_install` unchanged (it describes behavior, not the hook command); updated only the three user-facing `println!` messages that named `claude-permit` where `clyde permit log` is now the canonical form.
- `report/src/lib.rs` context string changed from `"cr failed"` to `"report failed"` - `run_with_config` is shared between the `cr` shim and `clyde report`; the shim is being retired (Phase 10) so the neutral form is correct.
- `report/src/cli.rs` help table - changed to `report render` / `report collect` (the subcommand action names, not a binary prefix), which is invocation-neutral and matches how users see them under `clyde report`.
- All three `Command` enum variants got doc-comments describing what the subcommand does plus its key behavior (window, output, merge semantics) - matching the style of `clyde/src/cli.rs`.
- Every `CollectArgs`, `RenderArgs`, `MergeArgs` field got a `///` doc-comment describing the flag's effect, defaults, and any caveats - modeled on `clyde/src/cli.rs` field docs.

### Deviations

- The design doc says "leave `report/src/cli.rs:23 name='cr'`" - this was already untouched. Confirmed the `#[command(name = "cr")]` on `ReportCli` is still present.
- The design doc references `report/src/lib.rs:105` `bail!("'cr merge' is not implemented")` - fixed this as part of #3(a) even though the line number wasn't explicitly listed, since it is a user-facing string with the stale `cr` prefix. This is a net positive, not a deviation from intent.

### Tradeoffs

- Accept-both vs accept-only-clyde in `check_hook_registered` - chose accept-both (matching `has_permit_hook`) so users who haven't run `bootstrap` yet still pass the hook check; pure clyde-only would be stricter but would produce confusing FAILs on otherwise-functional hosts during the migration window.
- PASS message always says `clyde permit log` (not "found claude-permit hook") - this is intentionally forward-pointing: the check should communicate the desired state, not just echo what it found. A host with a legacy hook passes AND learns what the canonical form is.

### Open questions

- None.

## Phase 2: Shared `--since` parser in `common` + clyde.yml config seam

### Design decisions

- `common::parse_since(s, tz)` — `common/src/since.rs` — moved the canonical parser out of `sessions` so `report` can share it without taking the `sessions` dep (which pulls rusqlite/rmcp/tokio). Pure function: no config/env reads, takes a `DateTz` the caller resolves.
- `DateTz { Utc, Local }` — `common/src/since.rs` — a parser-input enum with NO serde derives (deliberately decoupled from the config schema). Only the bare `YYYY-MM-DD` midnight branch consults it; spans and RFC 3339 are tz-independent (verified by a dedicated test).
- `DateTzConfig` (serde) + `From<DateTzConfig> for DateTz` — `common/src/config.rs` — kept the serde-facing enum separate from the pure-parser `DateTz` so the wire schema and the parser type can evolve independently. `Config::date_tz()` projects the config value to the parser type.
- `common::config::load()` / `load_from()` — `common/src/config.rs` — the first `clyde.yml` loader. `#[serde(rename_all = "kebab-case")]` + `#[serde(deny_unknown_fields)]`. A missing file returns `Config::default()` (NOT an error); an unreadable or malformed file errors. One field today: `date-tz: utc | local`, default `Utc`.
- `xdg_config_dir()` — `common/src/config.rs` — hand-rolled `$XDG_CONFIG_HOME` → `$HOME/.config` resolver per Scott's XDG convention; explicitly NOT `dirs::config_dir()` (wrong on macOS). Tested for both env-honoring and `$HOME` fallback behavior with a serializing `ENV_LOCK`.
- `sessions::since` re-exports `common::{DateTz, parse_since}` — `sessions/src/since.rs` — keeps `sessions::parse_since` callers working; the module is now a thin re-export. `sessions/src/lib.rs` also re-exports `DateTz`.
- clyde loads config once in `run()` — `clyde/src/main.rs:run` — resolves `tz` from `common::config::load()` and threads it into `cmd_ls`, `cmd_stage`, `cmd_enrich`. The parser stays pure; clyde is the layer that reads config.
- `report::run` loads config and passes `tz` into `resolve_command` — `report/src/lib.rs:run`, `report/src/config.rs:resolve_command/collect_config_from_args` — so `report collect --since 2d` (a relative span) now works, fixing the #4 divergence. `report` depends on `common` (already present), never `sessions`.

### Deviations

- The MCP `sessions_ls` call site (`sessions/src/mcp.rs`) passes `DateTz::Utc` rather than reading config: `sessions` is a clap-free, config-free lib with no seam to read `clyde.yml`, and UTC is both its historical bare-date convention and the configured default. The CLI surfaces (clyde) carry the configured tz; the MCP path keeps UTC. Not a behavior change.
- Threaded `tz` into `cmd_stage`/`cmd_enrich` (the `--dormant-after` parse) in addition to the `ls --since` path. The doc named only the two `--since` call sites, but `dormant_after` flows through the same `parse_since`; passing the configured tz keeps every date interpretation in clyde consistent. tz only affects bare dates, so span-valued dormancy thresholds are unchanged.

### Tradeoffs

- Separate `DateTzConfig` (serde) vs deriving serde on `DateTz` directly — chose two types + a `From` to keep `common::since` free of serde and the config schema independent of the parser API. Slightly more code; cleaner seam.
- Load config in `report::run` (not only in clyde) — chose this so both the `clyde report` path and the retiring `cr` shim get the shared parser without clyde having to inject tz across the `report::run(args, globals)` signature (which would have churned `Globals`). `common::config::load` is the single platform-aware source, so loading it in two entry points is consistent, not divergent.

### Open questions

- None.
