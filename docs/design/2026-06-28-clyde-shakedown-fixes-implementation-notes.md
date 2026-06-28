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

## Phase 3: Clean error rendering boundary

### Design decisions

- `dispatch_tool(result, debug)` — `clyde/src/main.rs:dispatch_tool` — added a `debug: bool` parameter so the shared absorbed-tool dispatch path (`clyde report/cost/permit`) can pick its error rendering. Default (info or quieter) prints `{e:#}` — the full eyre **cause chain** with NO `Location:`/backtrace; `--log-level debug`/`trace` prints `{e:?}` (Debug, with the Location capture) for diagnosis. This resolves #5: `clyde report collect --since notadate` no longer leaks `common/src/since.rs:NN` to the user at the default level.
- `{e:#}` chosen as the default, NOT plain `{e}` — per the review correction in the doc's #5 resolution. Plain Display (`{e}`) shows only the top error and hides the causal chain, which would degrade normal-failure UX; the alternate-Display `{e:#}` keeps the chain while dropping the Location capture.
- `is_debug_level(level: &str) -> bool` — `clyde/src/main.rs:is_debug_level` — small helper that parses the resolved level via `LevelFilter` and returns true only for `Debug`/`Trace`. Case-insensitive (delegates to `LevelFilter::from_str`); unparseable levels fall back to the clean (non-debug) form. `run()` computes `debug` once from `cli.log_level` (defaulting to `DEFAULT_LOG_LEVEL`, the same source `main` uses for logger setup) and threads it into the three `dispatch_tool` call sites.

### Deviations

- The doc's Phase 3 prose (line 247) reads "change `cr.rs:13` and `clyde/src/main.rs:95` to print `{e}` ... Verify both the shim and `clyde report` paths." This was superseded by the CORRECTED #5 resolution (doc lines 141-154) and the task scope: (i) the shims (`cr.rs`, `ccu.rs`, claude-permit bin) are retired in Phase 10 and got NO error-rendering changes — they still print `{e:?}` and compile as-is; (ii) the default form is `{e:#}` (chain), not plain `{e}`. Scope is the clyde `dispatch_tool` path only.

### Tradeoffs

- Recompute `debug` inside `run()` from `cli.log_level` vs. threading the `level` String already computed in `main()` — chose to recompute in `run()` so `run` stays self-contained and the change is local to the dispatch path. `is_debug_level` re-parses one short string; cost is negligible and it keeps `main`'s signature untouched.
- `debug: bool` parameter vs. passing the full resolved `LevelFilter`/level string into `dispatch_tool` — chose the bool: the dispatch path only needs the binary "clean chain vs. Debug+Location" decision, so a bool is the narrowest honest contract.

### Open questions

- None.

## Phase 4: `tags_source` exposure + tag clearing

### Design decisions

- `pub tags_source: Option<String>` added after `tags` in `SessionRecord` - `sessions/src/model.rs:SessionRecord` - placed immediately after `tags` so the struct reflects that provenance is logically bound to the tag set. Serializes as `tags-source` (kebab-case) via the existing `#[serde(rename_all = "kebab-case")]` on the struct.
- `s.tags_source` appended at the END of `COLS` - `sessions/src/db.rs:COLS` - deliberately last to preserve all existing positional indices (0-17) unchanged; new index 18 is `tags_source`. Added a comment enumerating all 19 columns with their indices so future maintainers can see the full mapping at a glance.
- Score index bumped from 18 to 19 in `search_table` - `sessions/src/db.rs:search_table` - the SELECT is `{COLS}, bm25(...) AS score`; adding one column to COLS shifts the appended score from index 18 to 19. This is the load-bearing companion fix to the COLS extension.
- `row.get::<_, Option<String>>(18)?` - `sessions/src/db.rs:map_record` - explicit turbofish forces the `Option<String>` `FromSql` impl, which maps SQL NULL to `None`. Without the turbofish, type inference was ambiguous and rusqlite returned an `Invalid column type Null` error.
- `set_tags` conditional `tags_source` - `sessions/src/db.rs:set_tags` - non-empty tag slice writes `'manual'`; empty slice (clear) writes `NULL`. SQL `UPDATE ... SET tags_source = ?3` with a `Option<&str>` param lets rusqlite bind NULL directly, keeping the logic in one query rather than two branches.
- `TagArgs.tags` changed from `required = true, num_args = 1..` to `num_args = 0..` (no `required`) - `clyde/src/cli.rs:TagArgs` - makes the positional variadic optional so `clyde sessions tag <id>` with zero tags parses cleanly. The `id` positional remains a required named argument before the variadic; clap resolves the ambiguity because the required positional consumes exactly one token.
- `cmd_tag` prints "cleared tags for" when `args.tags.is_empty()` - `clyde/src/main.rs:cmd_tag` - distinct confirmation message so the user can tell a clear from a set without inspecting the session record.

### Deviations

- None.

### Tradeoffs

- Single `UPDATE ... SET tags_source = ?3` with NULL binding vs. two separate UPDATE paths - chose the single-query form with a bound `Option<&str>` param. Fewer code paths, same atomicity, and rusqlite handles NULL binding cleanly with the typed param.
- Append `tags_source` to COLS end vs. insert near `tags` in the struct's position - chose end-of-COLS for index safety; the struct field order (near `tags`) is cosmetic and does not affect the SQL read path. Documenting the mismatch in the comment block prevents future confusion.

### Open questions

- None.

## Phase 5: MCP `sessions_search` sort param

### Design decisions

- `sort: Option<String>` added to `SessionsSearchRequest` - `sessions/src/mcp/tools.rs:SessionsSearchRequest` - plain serde/schemars field with a `#[schemars(description = ...)]` matching the style of every other field in the struct. `Option<String>` keeps the schema wire format simple and backward-compatible (omitting the field is identical to "relevance"). No clap or enum derive - sessions is clap-free; the string-to-enum mapping lives at the call site.
- `parse_sort_by(s: Option<&str>) -> SortBy` - `sessions/src/mcp.rs:parse_sort_by` - free function (not a method) that converts the optional string to `SortBy`, accepting the value case-insensitively via `str::to_ascii_lowercase`. Absent, empty, and unrecognised values all default to `SortBy::Relevance`. Keeps the conversion logic out of the long `sessions_search` method body and makes it unit-testable in isolation.
- `to_ascii_lowercase` used instead of `.to_lowercase()` - `sessions/src/mcp.rs:parse_sort_by` - the sort values are ASCII identifiers ("recency", "relevance"); `to_ascii_lowercase` is the correct and cheaper choice for ASCII-only input.
- Removed the stale "MCP is relevance-only by decision" comment - `sessions/src/mcp.rs:sessions_search` - the comment pre-dated the sort param; replaced with inline assignment of `sort_by` from `parse_sort_by` which is self-documenting.
- Debug log updated to include `sort` - `sessions/src/mcp.rs:sessions_search` - the entry log now records all four request fields including the new `sort` param, per the function-level debug logging rule.

### Deviations

- None.

### Tradeoffs

- `Option<String>` vs a `#[derive(Deserialize, JsonSchema)]` enum for the sort field - chose `Option<String>` (parsed at the call site) over a typed enum. The design doc explicitly allowed either form; the string approach avoids adding a new public type to the tools module while keeping the schema description accurate. The `parse_sort_by` helper centralises the mapping so the call site stays clean.
- Free function `parse_sort_by` vs inline match in `sessions_search` - chose a free function so the case-insensitive parsing logic has its own unit tests (two tests exercise it directly) without needing to route through the async MCP dispatch path.

### Open questions

- None.
