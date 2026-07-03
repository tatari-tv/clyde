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

## Phase 2: `common::write_atomic` + route permit writes through it (F4)

### Design decisions
- `write_atomic` lives at `common/src/atomic.rs` (a new single-word module, `pub use atomic::write_atomic`
  re-exported from `common/src/lib.rs`), with its tests in the sibling `common/src/atomic/tests.rs`
  per the repo's test-file-placement convention, mirroring `common/src/config.rs` +
  `common/src/config/tests.rs`.
- Implementation: `NamedTempFile::new_in(parent)` (temp file created directly in the target's own
  parent directory, never the OS temp dir), `write_all` + `flush`, then `persist(path)` (rename).
  If `path` already existed, its `fs::Permissions` are captured before the write and re-applied
  after the rename - the same approach `clyde/src/bootstrap.rs::repoint_statusline` already uses
  (capture the whole `Permissions` object, not a raw mode bitmask), rather than a Unix-only
  `PermissionsExt` mode capture, so the code compiles (if not usefully, permissions are a no-op
  concept) on non-Unix targets too.
- `common` gained its first `log` dependency (promoted the same way as `tempfile`, via
  `cargo add log -p common`, since it's already pinned at the workspace level) so `write_atomic`
  can carry entry/exit `debug!` and a `warn!` on the one unexpected-`stat`-failure branch, per the
  repo's function-level logging convention. No other file in `common` needed `log` before this.
- `install.rs::run_install` and `apply.rs::apply_entries` route their settings writes through
  `common::write_atomic` (fully-qualified call, no `use` needed since `permit` already depends on
  the `common` crate).
- `apply.rs::apply_entries` tracks two independent booleans: `local_existed` (captured once, right
  after `settings_local_path.exists()` is checked, before any parsing) and `local_mutated`
  (accumulated via `remove_from_array`'s new return value across every call site that touches
  `local_allow`: promote, remove, deny, and the local-source arm of dupe). `settings.local.json` is
  written only when `local_existed || local_mutated`; `settings.json` is always written (it must
  already exist for `apply_entries` to have reached this far, since the read of `global_content`
  earlier already required it).
- `remove_from_array` changed from `-> ()` to `-> bool` (whether it actually removed an element),
  so `apply_entries` can distinguish "attempted a removal that found nothing" (which must NOT count
  as touching an untouched, defaulted local document) from a real content mutation. `get_allow_array`
  materializing an empty `permissions.allow` on a document that lacked one is deliberately NOT
  treated as a mutation for this purpose - only `remove_from_array`'s content-level signal is.
- Tightened `missing_local_file_handled`: it now writes a global-only `Bash(rm -rf:*)` rule (which
  the built-in deny list flags `Deny`, an actionable recommendation, independent of source) so the
  test exercises the real write path - not the pre-existing "no actionable recommendations" early
  return the old version of this test accidentally relied on - while asserting `settings.local.json`
  is still never created. Added a companion test,
  `local_file_written_when_it_already_existed`, asserting the other side of the OR: an existing,
  unmutated-by-anything-except-a-real-removal local file is still rewritten.
- `write_atomic`'s "temp file lands in the target's own directory" test does NOT mutate a
  process-global env var (e.g. `TMPDIR`). Doing so would make every *other* concurrently-running
  test's own `TempDir::new()` calls flaky, since Rust runs tests in parallel by default and
  `TempDir::new()` also consults the OS temp dir/`TMPDIR` - confirmed by an actual failure during
  implementation (`common::atomic::tests::overwrites_an_existing_file` failed transiently the one
  time this was tried). Instead, the test makes the target's own parent directory read-only and
  asserts on *which stage* fails: a correct implementation's error names "failed to create temp
  file in `<parent>`" (creation itself blocked, proving the temp file was attempted directly inside
  the target's own directory); an implementation that instead defaulted to the OS temp dir would
  get past creation and only fail later, with a different message, at the `persist`/rename step.
  This is deterministic and immune to test-parallelism races.

### Deviations
- None from the phase's stated scope.

### Tradeoffs
- Whole-`Permissions` capture-and-restore (matching `bootstrap.rs`) vs. a raw Unix mode bitmask:
  the whole-object approach is what the design doc's own prior-art pointer uses, and keeps
  `write_atomic` compiling identically on non-Unix targets (the captured `Permissions` value is
  just inert there) without `#[cfg(unix)]`-gating the whole function.
- `local_mutated` tracked via `remove_from_array`'s boolean return, not a before/after
  `serde_json::Value` equality diff on the whole local document: a whole-document diff would also
  flag `get_allow_array`'s structural `permissions.allow` insertion (which happens unconditionally,
  even when no rule is actually removed) as a "mutation", which would defeat the entire point of
  the untouched-local-file suppression added in Phase 1's design and this phase's fix.

### Open questions
- None.
