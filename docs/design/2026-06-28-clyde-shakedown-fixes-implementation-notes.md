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
