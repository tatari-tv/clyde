# Implementation Notes: Deep-Dive Findings Remediation

Design doc: `docs/design/2026-07-03-deep-dive-remediations.md`

## Phase 1: events.db pragmas + apply/install panic removal (F5, F4-panics, F3)

### Design decisions
- `BUSY_TIMEOUT_MS: i64 = 5_000` as a crate-local `const` in `permit/src/db/store.rs`, set via
  `conn.pragma_update(None, "busy_timeout", BUSY_TIMEOUT_MS)` right after the existing
  `PRAGMA journal_mode=WAL;` batch, then `synchronous=NORMAL` the same way — mirrors
  `sessions/src/db.rs::BUSY_TIMEOUT_MS` / `apply_pragmas` exactly, per the design doc's explicit
  prior-art pointer.
- Added `debug!("EventStore::open: path=...")` on entry and a second `debug!` once the schema is
  ready, matching the `debug!("Db::open_at: path=...")` convention already established in
  `sessions/src/db.rs::open_at`. No other `permit` `cmd/*.rs` file uses `log::debug!` — that
  layer has a private submodule literally named `log` (`permit::cmd::log`), so importing the
  `log` crate's macros there would shadow/collide; `db/store.rs` has no such collision, so it got
  the logging and `cmd/apply.rs`/`cmd/install.rs` did not (matches existing surrounding style).
- `get_allow_array` (`permit/src/cmd/apply.rs`) changed from `-> &mut Vec<Value>` (terminal
  `.expect(...)`) to `-> Result<&mut Vec<Value>>`, using `ok_or_else(|| eyre::eyre!(...))` at each
  of the three failure points (root not an object, `permissions` not an object,
  `permissions.allow` not an array). This mirrors the existing pattern already used in
  `permit/src/cmd/install.rs::insert_hook` for the identical class of "hand-malformed but
  parseable JSON" error, so the fix is stylistically consistent with prior art in the same crate.
- `apply.rs:97,99`'s `.expect("valid path")` (converting `&Path` to `&str` for the `rkvr bkup`
  argv) became typed `ok_or_else(|| eyre::eyre!("... is not valid UTF-8: {}", path.display()))`
  errors, propagated with `?` since `apply_entries` already returns `Result<()>`.
- The standalone `apply` dry-run message was extracted into a named constant `DRY_RUN_MESSAGE`
  ("Pass --yes to write these changes.") rather than left as an inline string literal, purely so
  a unit test can pin the exact wording by value instead of needing to capture stdout.

### Deviations
- None from the phase's stated scope. One addition beyond the literal bullet list: a debug-log
  entry/exit pair on `EventStore::open`, per the repo's function-level logging convention (not
  explicitly requested by the design doc, but required by house rules and consistent with the
  `sessions/src/db.rs` prior art the doc itself cites).

### Tradeoffs
- `get_allow_array`'s error strings vs. a dedicated typed error enum: the design doc's API section
  says "typed error" but `permit` has no `thiserror` enum anywhere (it's `eyre::Result`
  end-to-end, CLI-shaped per the workspace's own error-handling convention). Interpreted "typed
  error" as "a real `Result::Err` instead of a panic," matching `install.rs::insert_hook`'s
  existing precedent in the same crate, rather than introducing a new error type for one function
  when the surrounding code has none.
- Testing `get_allow_array`'s non-array-`allow` fix via a direct call plus a second test that
  drives it through the real `apply_entries` write path (rather than through `run_apply`/`audit`):
  a `settings.json` with `permissions.allow` as a non-array string fails `audit()`'s typed
  `Vec<String>` deserialization (`settings/parser.rs`) before `apply_entries` is ever reached, so
  an end-to-end test via `run_apply` would exercise the pre-existing parser error path, not the
  new `get_allow_array` fix. Constructed a synthetic `AuditEntry` slice and called
  `apply_entries` directly so the new code path is actually covered.

### Open questions
- None.
