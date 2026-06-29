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

## Phase 6: Output-format autodetect + report output abstraction

### Design decisions

- `wants_json(explicit_json: bool) -> bool` - `cost/src/lib.rs:wants_json` - free helper mirroring the `sessions` model (`clyde/src/main.rs` `print_hits`/`print_records`): returns JSON when stdout is not a terminal (`!std::io::stdout().is_terminal()`) OR when `-j/--json` was passed. Centralises the format-selection so each `cost` subcommand calls it identically, and is unit-testable.
- Applied `wants_json(...)` to every `cost` subcommand that has a JSON representation - `cost/src/lib.rs:dispatch` (today/yesterday/daily/weekly/monthly) - replacing the bare `if json` / `if *json` checks. `session` (no JSON formatter) and `statusline`/`pricing` (inherently human) stay as-is. The `--total` scalar path is untouched: it already emits a bare pipe-friendly number.
- `enum Output { File(PathBuf), Stdout }` - `report/src/config.rs:Output` - models the collect destination as a first-class enum instead of a sentinel `PathBuf`. `-o <path>` -> `File`, omitted -> `Stdout`. Threaded through `CollectConfig.output`.
- `Output::title_cache_dir() -> Result<PathBuf>` - `report/src/config.rs` - resolves the directory used to seed the cross-run Haiku title cache: the output file's parent for `File`, the default report dir under XDG data for `Stdout`. This is the financial-hazard fix (see Deviations/Tradeoffs).
- `report::build_json(...)` split out of `report::write_json(...)` - `report/src/report.rs:build_json` - returns `(String, usize)` (the pretty JSON + session count) with no I/O, so the file path and the stdout path emit byte-identical JSON.
- `enum OutputDest { File(PathBuf), Stdout }` + `Display` - `report/src/lib.rs:OutputDest` - replaces `RunResult.output_path: PathBuf` so the post-run "wrote N sessions to <dest>" message can say `stdout` for a streamed collect; render maps the `-` sigil to `Stdout`.
- `resolve_titles_source(&Output)` and `latest_prior_report_in(dir, &Output)` - `report/src/lib.rs` - the title-cache resolver now works off a directory (resolved for both File and Stdout) instead of the old path-only `latest_prior_report`.

### Deviations

- None. Both review-flagged hazards were handled as specified.

### Tradeoffs

- HAZARD 1 (stdout corruption) - moved the `wrote N sessions` message from `println!` (stdout) to `eprintln!` (stderr) in `report::run` rather than suppressing it. Stderr never corrupts the stdout JSON, and the operator still sees the confirmation in both file and stdout modes. The `cr` integration test (`report/tests/collect.rs`) proves stdout is pure JSON and the note lands on stderr.
- HAZARD 2 (financial / re-billing) - `Output::Stdout` resolves its title-cache source to the default report dir under XDG data (`<xdg-data>/claude-report`), the same dir `collect` would have written to with a file target. So streaming runs still carry titles forward from the newest prior `claude-report-*.json` and do NOT re-bill the paid Anthropic Haiku API. Chosen over the alternative of silently disabling the cache in stdout mode (which the review explicitly forbade) or writing a side-file (which would defeat the point of stdout-only output).
- `cost` autodetect helper vs inline `is_terminal()` at each call site - chose one `wants_json` helper so the convention can't drift between subcommands and is unit-testable; the cost of one extra function is trivial.
- Subprocess integration test (`report/tests/collect.rs`) vs in-process stdout capture - the `wrote N`-to-stderr guarantee can only be proven with genuinely separable stdout/stderr streams, which requires spawning the real `cr` binary (`CARGO_BIN_EXE_cr`, available only to integration tests, not lib unit tests).

### Open questions

- None.

## Phase 7: Implement `report merge`

### Design decisions

- New `report/src/merge.rs` module - `report/src/merge.rs` - merge operates over the deserialized `Report` schema (the exact struct `report::write_json` serializes), NOT `SessionSummary`. Each input is read with `serde_json::from_str::<Report>`, so it shares the schema and kebab-case behavior already defined on `Report`. Wired into the dispatch at `report::run_with_pricing` (`ResolvedCommand::Merge(cfg) => merge::run(cfg)`); the old `eprintln!("merge is not implemented")` early-return in `report::run` and the `bail!` in `run_with_pricing` were both removed.
- Keep-both via re-keying to `"<host>/<session_id>"` - `merge::merge_reports` - sessions are a `BTreeMap<String, SessionEntry>` keyed by raw session id, so two hosts with the same id collide. The host prefix comes from each input report's own `host` field (the same value `collect` records), making the key authoritative. Chosen over adding a per-entry `host` field because it requires NO change to the `SessionEntry` schema (zero risk to collect/render round-trips) while still guaranteeing same-id-different-host survival. A dedicated test seeds two reports sharing one id and asserts both `desk/<id>` and `laptop/<id>` survive.
- Totals recomputed by re-summing the merged session set - `merge::recompute_totals` - iterates the merged `SessionEntry` values, summing per-model token counts and per-model/session `spend-usd`, deduping `untracked-models` into a `BTreeSet`. It does NOT blind-sum each input's `totals` (which double-counts any overlapping session). Spend is taken from the entries' own priced `spend-usd` fields rather than re-pricing, because each input was priced at collect time; merge has no `Pricing` and should trust the recorded figures.
- Schema-version assertion - `merge::assert_uniform_schema` - returns an eyre error naming BOTH versions (`... ({first} vs {other}) ...`) on any mismatch, before any merge work. Returns the common `u32` on success.
- Window widening - `merge::merge_reports` - `since` folds to the min and `until` to the max across inputs via `Option<DateTime<Utc>>` accumulators set on the first iteration.
- Multi-host marker - `merge::multi_host_marker` - distinct hosts (a `BTreeSet`) joined with `+` (`desk+laptop`). A single distinct host (1-input identity, or several same-host reports) keeps its bare name, so identity merges preserve the original `host`.
- Output convention reuse - `merge::write_output` + `MergeConfig.output: Output` + `MergeArgs -o/--output` - merge honors the Phase 6 `Output` enum: `-o <file>` writes atomically (temp-in-target-dir + rename, mirroring `report::write_json`'s durability), omitting `-o` streams the JSON to stdout so `report merge a.json b.json | jq` works. The "wrote N sessions to <dest>" note stays on stderr via the existing `report::run` path (which now drives merge through `run_with_config`), keeping a stdout JSON stream clean. `MergeConfig` gained an `output: Output` field; `resolve_command` maps `args.output` to `File`/`Stdout` exactly as `collect` does.
- `generated` timestamp - `merge::merge_reports` - set to `Utc::now()` at merge time (the merge is itself a fresh artifact); per-input `generated` values are intentionally dropped.

### Deviations

- The doc lists "0 inputs -> error" / "1 input -> identity" as edge cases; the CLI also allows `MergeArgs.inputs` to be an empty `Vec` (clap does not enforce `1..`). The 0-input error is therefore enforced at runtime in `read_reports` (typed eyre error "no input files given; nothing to merge"), not by clap, so both the CLI and any direct `merge::run` caller get the same guard. Not a deviation from intent - it closes the path clap leaves open.

### Tradeoffs

- Re-key to `host/session_id` vs add a per-entry `host` field - chose re-keying. Both satisfy keep-both; re-keying touches zero existing schema (no `SessionEntry` change, no migration of collect/render), and the prefixed key is self-describing in `jq`. The per-entry-field alternative would have rippled into `render` and the `Report` round-trip tests for no functional gain.
- Trust recorded `spend-usd` vs re-price the merged set - chose to trust the recorded per-model/session spend. Merge has no `Pricing` instance and re-pricing would require one plus the raw token-to-usage mapping; the inputs were already priced at collect time, so re-summing their figures is both correct and avoids a pricing dependency in the merge path.
- eyre (not a `thiserror` enum) for merge errors - the whole `report` crate is an eyre-based CLI (per repo Rust conventions, CLIs use eyre); introducing a `thiserror` enum only for merge would diverge from every sibling module. The schema-version-mismatch and zero-input errors carry their distinguishing data (both versions; the "no input" reason) in the message and propagate to the clyde exit path, which is the consumer - no downstream code string-matches them.

### Open questions

- None.

## Phase 8: `bootstrap --dry-run`

### Design decisions

- `BootstrapArgs.dry_run: bool` (`--dry-run`) — `clyde/src/bootstrap.rs:BootstrapArgs` — added with a `///` doc-comment that explicitly records the carve-out: `bootstrap` is DEFAULT-destructive (no opt-in gate), so it is the documented exception to the "no `--dry-run` on opt-in destructive flags" rule.
- Gate wraps `run()`, not just `bootstrap()` — `clyde/src/bootstrap.rs:run` — the two `systemctl` shell-outs (`daemon_reload()`, `start_enrich_timer()`) live in the OUTER `run()`, outside the hermetic `bootstrap()` core. The `if !args.dry_run && ...` guard around them is the load-bearing fix the design review flagged: a gate threaded only into `bootstrap()` would let those two writes escape.
- Threaded `dry_run` through `bootstrap()` into every `migrate_*`/`repoint_*` — each function keeps its existing no-op/would-act decision logic (so the reported plan is faithful to a live run) but returns `Ok(true)` BEFORE performing the fs/DB/symlink mutation when `dry_run` is set. The `step!` macro records the same label set, so the dry-run plan and the live run's `completed` list are identical by construction.
- `migrate_events_db(paths, dry_run)` — the critical one — returns `Ok(true)` from existence checks alone and NEVER opens the DB under dry_run. The live path runs `PRAGMA wal_checkpoint(TRUNCATE)` (a WRITE to the user's events DB) before the gated rename; dry-run must not open it in any writing mode, so the early return sits above the `Connection::open` + checkpoint.
- `migrate_dir(legacy, dest, dry_run)` — both mutation branches gated: the whole-dir rename branch returns `Ok(true)` before `create_dir_all`/`rename`; the merge-into-existing branch does a read-only directory scan to compute whether any non-colliding entry WOULD move, returning that bool without creating the dest or renaming.
- `repoint_systemd(paths, install_timer, dry_run)` — short-circuits under dry_run at the top: if legacy units are present it returns `Ok(true)` (a live run would rewrite service+timer, move the env file, repoint the enable symlink, remove the legacy units); the `--install-timer` no-legacy branch returns `Ok(true)` before `install_clyde_timer`. Because of this short-circuit, `move_env_file`, `repoint_wants_symlink`, and `install_clyde_timer` are unreachable under dry_run and need no `dry_run` param — the gating stays centralized in the one function, which is easier to verify exhaustively.
- Dry-run output in `run()` prints an ordered `• would: <step>` list plus the two `• would: systemctl ...` lines (only when the systemd step would change), then a "no files were moved..." footer; a `Ok(false)` plan prints the "nothing to migrate" line. A planning failure surfaces as `✗ would fail at: <step>` and a non-zero exit, mirroring the live failure report.

### Mutation sites gated (full inventory)

- `migrate_dir` (sessions data dir + config dir) — both rename and merge branches.
- `migrate_events_db` — WAL checkpoint AND rename (DB never opened under dry_run).
- `migrate_permit_config` → `migrate_file` / `migrate_legacy_permit_config`.
- `migrate_file` (cost config) — rename + backup.
- `merge_pricing_overrides` — create_dir_all + backup + atomic write.
- `repoint_statusline` — backup + atomic write + perms restore.
- `repoint_hook` ×2 (global + local settings) — backup + atomic write.
- `repoint_systemd` incl. `move_env_file`, `repoint_wants_symlink` (symlink + create_dir_all), `install_clyde_timer` — all under the single dry_run short-circuit.
- `daemon_reload()` and `start_enrich_timer()` (the two `systemctl` shell-outs) — gated in `run()`, never invoked under dry_run.

### How the zero-write test verifies it

`dry_run_performs_zero_mutations_and_lists_planned_steps` (`clyde/src/bootstrap/tests.rs`) seeds a temp `$HOME`/XDG fixture exercising EVERY gated site (data + config dirs, events DB with a `-wal` sidecar, permit/cost config, cr pricing, statusline, global+local hooks, systemd service+timer+enable-symlink+env-file). It takes a recursive `snapshot()` (path, kind file/dir/symlink, len, mtime — via `symlink_metadata`, never following links) BEFORE and AFTER a `bootstrap(&paths, dry_run=true)`, and asserts `before == after` — proving no path was created, moved, removed, or even touched. It separately asserts: the legacy events DB row count is unchanged (the load-bearing checkpoint guard — a `wal_checkpoint(TRUNCATE)` would have rewritten the file), no clyde DB/unit/timer/enable-symlink was produced, no `.clyde.bak` backups exist, and the legacy hook/statusline still contain their pre-migration strings. Finally it asserts the returned plan enumerates all ten expected steps. The systemctl shell-outs can't run in CI; the test confirms they are skipped by never observing any systemd-side write and by relying on the `run()` gate (the hermetic core never shells out, and `run()` guards the shell-outs behind `!args.dry_run`).

### Deviations

- None. `move_env_file`/`repoint_wants_symlink`/`install_clyde_timer` did NOT get a `dry_run` parameter despite being in the inventory, because `repoint_systemd`'s dry_run short-circuit makes them unreachable under dry_run — gating them too would be dead code. The mutation they perform is still fully gated (by their unreachability), which the zero-write test confirms.

### Tradeoffs

- Reuse `Outcome.completed` as the dry-run plan vs a separate `Vec<PlannedAction>` — chose reuse. The labels already name each step in human terms and are produced by the identical control flow as a live run, so the plan is provably the live step set. A parallel plan type would risk drift between "what dry-run says" and "what a real run does," which is exactly the false-safe risk the review called out.
- Centralized short-circuit in `repoint_systemd` vs threading `dry_run` into its three helpers — chose the single short-circuit. It keeps the systemd gating verifiable in one place and avoids dead `dry_run` branches in helpers that can never be reached under dry_run.
- mtime-inclusive snapshot vs path/len only — chose to include mtime so a same-size in-place rewrite (e.g. an atomic write landing identical bytes) would still register as a mutation. The cost is that any incidental DB-open settling must happen before the baseline snapshot (handled by reading the row count first), which the test documents.

### Open questions

- None.

## Implementation Audit fixes

### Design decisions

- Lazy config load via `load_date_tz()` helper - `clyde/src/main.rs:load_date_tz` - extracted the `common::config::load()` + `.date_tz()` pair into a one-purpose free function called only from within the three `SessionsCommand` arms that parse date strings (`Ls`, `Stage`, `Enrich`). The unconditional `load()` call that formerly preceded the `match cli.command` was removed. All other arms (`search`, `open`, `tag`, `reindex`, `doctor`, `bootstrap`, `serve`, and all absorbed tools) now never touch `clyde.yml`, so a malformed config file cannot break them.
- `CollectArgs.output` doc-comment replaced - `report/src/cli.rs:CollectArgs.output` - old text said "default timestamped file under $XDG_DATA_HOME/claude-report/"; new text says "With -o, writes that file; without -o, streams JSON to stdout so `report collect | jq` works." Phase 6 changed the behavior; the doc-comment is now accurate.
- `Command::Collect` variant doc-comment updated - `report/src/cli.rs:Command::Collect` - removed the "writes a timestamped JSON file" claim; replaced with the two-path description (file with `-o`, stdout without).
- `MergeArgs.output` doc-comment added - `report/src/cli.rs:MergeArgs.output` - field had no doc-comment before; added one with the same file-or-stdout semantics as `CollectArgs.output` for consistency.
- Stale `cr` error message fixed - `report/src/render.rs:run` - replaced "cr v0.1.2+ emits and reads JSON. Re-run cr collect" with "report collect emits JSON. Re-run report collect" so the user-facing error refers to the current UX surface, not the retired shim.
- Stale `cr render` doc-comment fixed - `report/src/config.rs:DEFAULT_RENDER_INPUT` - changed "Default *input* path for `cr render`" to "Default *input* path for `report render`".
- Hook match tightened to `log` suffix - `permit/src/cmd/check.rs:check_hook_registered` - changed `content.contains("claude-permit")` to `content.contains("claude-permit log")` and `content.contains("clyde permit")` to `content.contains("clyde permit log")`. The old substring match would have accepted a bare mention of either binary name anywhere in the settings file; the new form matches only the actual hook invocation pattern.

### Deviations

- None. All four fixes were applied exactly as specified.

### Tradeoffs

- `load_date_tz()` free function vs inlining the two-liner at each call site - chose the helper so the three call sites read identically and the `.context(...)` message is in one place. No behavior difference; the helper adds zero external state.
- Match `log` suffix vs a regex or more specific anchor - chose the plain `contains("claude-permit log")` / `contains("clyde permit log")` string match. A regex would be overkill for what is a fixed token; the existing tests cover both forms and the false-positive risk (a settings comment mentioning the binary name without the subcommand) is negligible compared to the pre-fix risk (any mention anywhere passing).

### Open questions

- None.
