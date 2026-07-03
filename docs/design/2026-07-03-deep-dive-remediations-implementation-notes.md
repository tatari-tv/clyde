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

## Phase 3: hook panic containment (F2)

### Design decisions
- The `log`-path degradation moved out of `run`'s inline `match` into a new
  `contain_log_panics<F, W>(out, f)` helper (`permit/src/lib.rs`). `run` now branches on `is_log`
  BEFORE dispatching: the log path calls `contain_log_panics(&mut stdout, || run_inner(...))`; every
  other command calls `run_inner` directly and propagates its `Err`/panic unchanged. Because
  `run_inner` runs `setup_logging` -> `Config::load` -> `EventStore::open` -> the `Command::Log` arm
  all inside the wrapped closure, `catch_unwind` covers the ENTIRE log path, not just the dispatch
  arm - the exact panel finding the doc calls out (`lib.rs:62-64` promises `{}` for ANY failure).
- `catch_unwind` uses `std::panic::AssertUnwindSafe(f)` around the closure. The closure captures
  `args`/`globals` by move and consumes them exactly once; nothing is observed again after an
  unwind, so there is no broken-invariant hazard - `AssertUnwindSafe` is the correct, minimal way
  to satisfy the `UnwindSafe` bound without restructuring unrelated code (per the phase requirement).
- The `{}` marker is written through an injected `W: std::io::Write` sink rather than `println!`.
  Production passes `std::io::stdout()`; tests pass a `Vec<u8>` so the exact bytes (`{}\n`, matching
  the old `println!` newline) and the Ok/Err/panic branch choice are asserted deterministically
  in-process. This is a DI shape consistent with the repo's "inject deps, test with fakes" rust
  convention; it is the design of the new helper, not a restructuring of existing code.
- `panic_message(&(dyn Any + Send)) -> String` downcasts the caught payload to `&str` or `String`
  (the two shapes `panic!` produces) for the failure log, generic fallback otherwise. Logged via
  `log::error!("log command panicked: ...")` before `{}` is emitted, mirroring the existing
  `log::error!("log command failed: ...")` on the `Err` branch.
- NO global panic-hook mutation (`panic::set_hook`) - it races other threads on swap/restore. The
  default hook's stderr backtrace is tolerated because Claude Code parses only stdout (per doc).
- Pinned `panic = "unwind"` in the workspace root `[profile.dev]` and `[profile.release]`
  (`Cargo.toml`). Unwind is already the default, so this is insurance-only: a future
  `panic = "abort"` would otherwise silently turn the `catch_unwind` boundary into a process-abort
  no-op. Profiles must live in the workspace root, which is where they were added.
- Test-only panic injection points are `#[cfg(test)]`-gated statements at two spots in `run_inner`:
  `InjectPoint::Setup` at the very top (before `setup_logging`) and `InjectPoint::Dispatch` at the
  top of the `Command::Log` arm (before the DB open / stdin read). They read a `thread_local!`
  `Cell<Option<InjectPoint>>` and `panic!` when armed. `catch_unwind` runs its closure on the
  calling thread, so a thread-local armed in a test is visible inside `run_inner`; the gates compile
  to nothing in production builds (zero production cost).

### Deviations
- The doc's test bullet says "an injected panic (test-only panicking store path)". Implemented as a
  hybrid rather than a single mechanism: the assertable-`{}`-output coverage of a store-path panic
  is a unit test on `contain_log_panics` with a panicking closure (`contain_degrades_panic_to_empty_json_and_zero`),
  while the "panic in setup (before dispatch)" case AND the dispatch-arm case are covered
  end-to-end through the real `run` entry point via the `#[cfg(test)]` injection points
  (`run_contains_setup_panic_before_dispatch`, `run_contains_dispatch_panic`). Reason: the log
  path's success/failure output goes to the process stdout, which cannot be asserted in-process
  without a redirect crate; the injected `Vec<u8>` writer on the helper is what makes the exact
  `{}\n` bytes assertable, and the real-`run` tests then prove the boundary genuinely wraps the
  whole path (setup included) rather than only the arm.

### Tradeoffs
- Injected `W: Write` sink vs. `println!` + a stdout-capture crate: the injected writer keeps the
  test in-process and dependency-free and lets the failure branches assert exact bytes; the cost is
  `run` now threads `&mut std::io::stdout()` into the helper (one extra line). Chosen over adding a
  `gag`/`libc dup2` stdout-capture dependency purely for a test assertion.
- `run_contains_dispatch_panic` drives real `setup_logging` (which calls `env_logger::Builder::init`,
  a process-global one-shot). It is the ONLY test that reaches `setup_logging` (the setup-panic test
  fires before it), so exactly one init happens per test process. It isolates `XDG_DATA_HOME` /
  `XDG_CONFIG_HOME` to a `TempDir` (under an `ENV_LOCK` mutex, per the repo's env-test convention) so
  nothing touches the real home. The assertion is robust regardless: if `init` ever double-fired, that
  panic is itself inside the boundary and still degrades to exit 0.
- The `ENV_LOCK` guard is bound as a bare `guard` + explicit `drop(guard)` at the end, not
  `let _guard = ...`. The repo's `lint-unused` CI task denies the `_varname` pattern even for RAII
  guards (it flagged the initial `_guard`), and every existing env-lock test in the workspace
  (`common/src/config/tests.rs`, `report/src/tests.rs`, `session/src/paths/tests.rs`) uses the bare
  `guard` + `drop` form; matched local convention over the language-level drop-guard carve-out.

### Open questions
- None.

## Phase 4: lint hardening pass (F2)

### Design decisions
- Crate-root deny placement matches `session`/`sessions`/`clyde` exactly: `#![deny(...)]` lines as
  the first lines of the crate root, in the order `clippy::unwrap_used`, `clippy::string_slice`,
  `dead_code`, `unused_variables` wherever more than one applies.
  - `permit/src/lib.rs`: had none of the four; added only `#![deny(clippy::unwrap_used)]` and
    `#![deny(clippy::string_slice)]` (the two this phase specifies for `permit`) - left
    `dead_code`/`unused_variables` untouched since the phase's bullet list does not ask for them
    there and adding lints outside the stated scope risks unrelated churn.
  - `common/src/lib.rs`: already had `dead_code`/`unused_variables`; added
    `clippy::unwrap_used` + `clippy::string_slice` ahead of them, matching the order above.
  - `cost/src/lib.rs`, `report/src/lib.rs`, `pricing/src/lib.rs`: each already had
    `clippy::unwrap_used`; added `clippy::string_slice` immediately after it.
- Re-swept with `cargo clippy --workspace --all-targets --all-features` after the denies landed
  (not just `-p permit`) - the doc's known-sites list undercounted by a wide margin once
  `permit`'s `unwrap_used` deny went from absent to present. The **full set of sites actually
  fixed**, in the order clippy surfaced them:
  - **`permit/src/risk/tier.rs`**: `split_paren` (byte-index into `broad`/`narrow` for the tool
    name and inner-pattern substrings) -> `rule.strip_suffix(')')?.get(i + 1..)?` +
    `rule.get(..i)?`; `extract_bash_pattern` (`&rule[5..rule.len()-1]` after manual
    `starts_with("Bash(")`/`ends_with(')')` checks) -> `rule.strip_prefix("Bash(")?.strip_suffix(')')?`,
    which also subsumes the manual checks; `glob_match`'s two `text[pos..]` sites (loop-local,
    `pos` accumulated from prior `.find()`/`.len()` results) -> a single `let Some(remaining) =
    text.get(pos..) else { return false };` computed once per iteration, used by both the `i == 0`
    and `else` branches.
  - **`permit/src/cmd/suggest.rs`**: NOT a slice site - `unwrap_used` (not `string_slice`) flagged
    five `writeln!(out, ...).unwrap()` calls inside `run_suggest`'s plain-text formatter (`out` is
    a `String`, so the write is infallible in practice but clippy can't prove it). Changed each to
    `.expect("write to String cannot fail")`, per the rust-conventions carve-out that `.expect()`
    with a clear reason is acceptable in production and is not itself flagged by
    `clippy::unwrap_used`.
  - **14 inline `#[cfg(test)] mod tests { ... }` blocks across `permit`** (`filter.rs`,
    `cmd/report.rs`, `settings/parser.rs`, `cmd/log.rs`, `risk/tier.rs`, `hook/payload.rs`,
    `config.rs`, `cmd/clean.rs`, `cmd/audit.rs`, `db/store.rs`, `cmd/check.rs`, `cmd/install.rs`,
    `cmd/apply.rs`, `cmd/suggest.rs`): none of `permit`'s test modules live in separate
    `tests.rs` files (pre-existing structure, unchanged by this phase - restructuring test file
    placement is a separate mechanical pass, not this phase's scope), so the workspace's
    `#![allow(clippy::unwrap_used)]`-at-top-of-`tests.rs` convention doesn't have a literal landing
    spot; added `#[allow(clippy::unwrap_used)]` directly above each `mod tests {` instead, which is
    the equivalent scoping for an inline module. Applied uniformly to all 14 (not just the ones
    clippy happened to flag first), matching the observed workspace-wide convention that every
    crate with the deny has the allow on every one of its test modules, not a subset.
  - **`pricing/src/pricing.rs`**: `strip_date_suffix`'s two slices
    (`&model_id[pos + 1..]`, `&model_id[..pos]`, `pos` from `.rfind('-')`) ->
    `model_id.get(pos + 1..).unwrap_or("")` / `model_id.get(..pos).unwrap_or(model_id)`.
  - **`cost/src/output.rs:192`** (`format_verbose_sessions`) and **`cost/src/lib.rs` (three
    sites: the `Command::Session { id: "current" }` arm, the single-match arm, and the
    multiple-matches loop)**: all the same pattern, `&s.session_id[..8.min(s.session_id.len())]`
    -> `s.session_id.get(..8).unwrap_or(&s.session_id)` (identical truncate-or-keep-whole
    semantics: `Some` when the id is >= 8 bytes, `None` -> full id otherwise, matching what
    `8.min(len)` produced).
  - **`report/src/title.rs:170`** (`truncate`, called from the title pipeline): the function
    already floors `end` to a char boundary via a `while !s.is_char_boundary(end)` loop before
    slicing, so the site is boundary-safe today - clippy still flags the terminal `s[..end]`
    because it can't see that invariant. Changed to `s.get(..end).unwrap_or_default().to_string()`.
  - **`report/src/report/tests.rs`** (`title_appears_first_in_session_entry`): three
    `body[session_idx..]` re-slices of the same suffix. Rather than adding a new
    `#[allow(clippy::string_slice)]` (no such allow exists anywhere in the workspace today to
    match), computed the suffix once via `body.get(session_idx..).unwrap()` (test code, `.unwrap()`
    already allowed there) into a `tail` binding and reused it for both `.find()` calls and the
    assertion message - avoids the lint by construction and reads cleaner than three repeated
    slices of the same substring.
- No `#[allow(clippy::string_slice)]` was added anywhere in this phase - every flagged
  `string_slice` site had a safe `strip_prefix`/`strip_suffix`/`.get()` equivalent, so none needed
  a blanket exemption (this also means there was no existing in-tree pattern to copy for that
  specific allow, unlike `unwrap_used`).

### Deviations
- The design doc's known-sites list (`permit/src/risk/tier.rs` x4, `pricing/src/pricing.rs:116`,
  `cost/src/output.rs:192`, `cost/src/lib.rs:699`, `report/src/title.rs:170`) covered every
  `string_slice` site but named none of the `unwrap_used` fallout in `permit` (the five
  `suggest.rs` `writeln!().unwrap()` calls, plus the need to annotate all 14 inline test modules).
  This is exactly the "re-sweep at implementation time" the phase called for, not a scope
  deviation - `permit` had zero crate-root denies before this phase, so its `unwrap_used` surface
  was necessarily larger than `string_slice` alone. Line numbers in the doc's list had also
  drifted from Phases 1-3 (e.g. `cost/src/lib.rs:699` is `:700`/`:720`/`:730` post-drift, three
  sites not one); the re-swept set above is authoritative.

### Tradeoffs
- `.expect("write to String cannot fail")` vs. threading a `Result` up through `run_suggest`
  for the `writeln!` sites: `run_suggest` already returns `Result<()>`, so propagating with `?`
  was possible, but `fmt::Write` for `String` genuinely cannot fail (the underlying allocator
  failure path aborts, it doesn't return `Err`), and the existing workspace precedent for
  `writeln!`-to-buffer (`cost/src/scanner.rs`, test-only) also uses `.expect("write")` rather than
  `?`. `.expect()` with a specific reason keeps the call sites flat and matches that precedent
  while staying outside `clippy::unwrap_used`'s scope.
- Inline `#[allow(clippy::unwrap_used)]` per test module vs. migrating `permit`'s inline
  `mod tests` blocks to the `foo/tests.rs` submodule convention first: the latter is the
  documented house style and would have let the allow live at the top of a dedicated file like
  every other crate, but doing that migration as a drive-by inside a lint-only phase would have
  produced a much larger, harder-to-review diff mixing file-structure churn with the lint-hardening
  change this phase is scoped to. Left as a candidate for a future dedicated test-file-placement
  pass.

### Open questions
- `permit`'s test modules are still inline (`#[cfg(test)] mod tests { ... }` at the bottom of
  their source files) rather than the workspace's `foo/tests.rs` submodule convention used
  everywhere else (`session`, `sessions`, `cost`, `report`, `pricing`, `common`). Worth a dedicated
  mechanical pass to align `permit` with the rest of the workspace; out of scope here.

## Phase 5: cost cache stable hash (F9)

### Design decisions
- `cost/src/cache.rs::compute_mtime_hash` no longer builds a `std::collections::hash_map::DefaultHasher`
  (SipHash); it folds `FNV_OFFSET_BASIS` through a small local `fnv1a_update(hash, bytes) -> u64`
  helper, called once per field (`path` as UTF-8-lossy bytes, `mtime_secs.to_le_bytes()`,
  `size.to_le_bytes()`) per file, in the same field order the old `Hash` impl visited them - the
  observable "hash changes with path/mtime/size" behavior each existing property test asserts is
  unchanged, only the algorithm underneath is.
- `FNV_OFFSET_BASIS` (`0xcbf2_9ce4_8422_2325`) and `FNV_PRIME` (`0x0000_0100_0000_01b3`) are the
  two published FNV-1a 64-bit constants, added as crate-local `const`s in `cache.rs` (per the
  "no magic numbers" convention) rather than imported from a crate, per the design doc's Alternative 5.
- `CACHE_VERSION` bumped from `4` to `5` in the same file, so every on-disk cache entry written
  under the old SipHash-based hash misses on the `cached.version != CACHE_VERSION` branch in
  `load_cached_day` (a clean, deterministic miss) rather than only sometimes colliding on the raw
  `u64` hash value by chance.
- Added `test_compute_mtime_hash_pinned_vector` (`cost/src/cache.rs`): fixes one `SessionFile`
  (`path="/tmp/pinned.jsonl"`, `mtime=UNIX_EPOCH+1_700_000_000s`, `size=4096`) and asserts
  `compute_mtime_hash` returns the literal `0x3b63_b3cb_8480_3ced`. The expected value was computed
  independently with a standalone `rustc`-compiled program running the identical
  offset-basis/prime/byte-order algorithm, not copy-pasted from a single build's output, so the
  test is a real pin against the documented FNV-1a algorithm rather than a tautological
  self-check.

### Deviations
- None. Implemented at the file/function named in the design doc's Data Model note
  (`cost/src/cache.rs::compute_mtime_hash`), no new dependency, `CACHE_VERSION` bumped exactly as
  specified.

### Tradeoffs
- Inline FNV-1a vs. the `fnv` crate: per the design doc's Alternative 5, inline is smaller than the
  `Cargo.toml`/`Cargo.lock` diff a new dependency would add and is directly testable against a
  known vector without depending on the crate's own internal layout.
- `f.path.to_string_lossy().as_bytes()` (lossy UTF-8 bytes) rather than hashing the `PathBuf`'s raw
  OS bytes (`OsStr`, platform-dependent representation): kept the exact input the old code hashed
  (`to_string_lossy()` was already the site feeding `Hash`), since changing what bytes go into the
  hash is a distinct concern from changing the hash algorithm, and the doc's Data Model note scopes
  this phase to "the same `(path, mtime-secs, size)` tuple stream."

### Open questions
- None.

## Phase 6: report help-text truth (F8)

### Design decisions
- Verified the actual six placeholders directly against `render_custom`
  (`report/src/render.rs::render_custom`) before touching any help text, per the phase
  instructions: `{{host}}`, `{{since}}`, `{{until}}`, `{{session-count}}`, `{{total-tokens}}`,
  `{{total-spend}}`, in that order, each a plain `.replace(...)` call. This matches the design
  doc's enumeration exactly - no drift found.
- `RenderArgs::template`'s doc comment (`report/src/cli.rs`) rewritten to say the rendering is
  plain `{{token}}` string replacement over exactly those six placeholders, and to state
  explicitly that no other tokens, loops, or conditionals are supported (removing any implication
  of a templating engine, not just the "Jinja2/Tera" name).
- `RenderArgs::pdf_engine`'s doc comment rewritten to: "PDF engine to use when `--pdf` is set
  (default: `wkhtmltopdf`), passed to pandoc as `--pdf-engine`; `pandoc` is the required binary
  that must be on `PATH`." This matches the phase's required wording ("passed to pandoc as
  --pdf-engine; pandoc is the required binary") while keeping the existing default-value and
  `PATH` details the old text already carried.
- Added two tests in `report/src/cli/tests.rs` that read the *actual* clap-rendered help text off
  `ReportCli::command()` (via `clap::CommandFactory`) for the `render` subcommand's `template` and
  `pdf_engine` arguments, rather than testing the doc-comment string directly: (1) the template
  help contains all six placeholder literals and does not mention "jinja" or "tera"
  (case-insensitive); (2) the pdf-engine help mentions both `pandoc` and `--pdf-engine`. The six
  placeholder literals are declared once as a `TEMPLATE_PLACEHOLDERS` const with a comment pointing
  back at `render_custom`, so a future change to the actual replacement tokens without a matching
  help-text update fails this test instead of shipping silently (there is no existing
  cross-reference mechanism between `render.rs` and `cli.rs`, so this is a manual but explicit
  tripwire, the closest fit to the repo's existing help-text test style, which was previously
  limited to `extract_version` unit tests only).

### Deviations
- None. Implemented exactly the two help-string corrections named in the phase (`--template`,
  `--pdf-engine`); the REQUIRED TOOLS log-path line (`get_tool_validation_help`,
  `report/src/cli.rs`) was left untouched as instructed - that rendering-from-the-path-function
  work is Phase 8's scope.

### Tradeoffs
- Testing the rendered clap help output vs. testing the raw doc-comment source text: clap derive
  doc comments aren't exposed as a standalone constant, only via the built `Command`, so asserting
  against `ReportCli::command()`'s `Arg::get_help()` output is the only way to pin the actual
  user-visible help string (the thing that can go stale) rather than a string literal duplicated in
  the test file that could drift from the real `#[arg]` attribute independently.

### Open questions
- None.

## Phase 7: Read asymmetry comment (F7)
### Design decisions
- Added a doc comment directly above `pub fn classify_tool_input` in
  `permit/src/risk/tier.rs` (found by name; the doc's `tier.rs:~321` line reference had
  already drifted from Phases 1-6 touching the file) explaining that the Moderate-vs-Safe
  split with `classify_rule`'s bare-`Read` handling is intentional (D4): a persisted rule
  grants unbounded future filesystem access (Moderate, per the existing "carte blanche"
  comment on `classify_rule`), while a single live tool-call event is one read of one
  already-known path (Safe). The comment also names the one real interaction point:
  `suggest` builds rule proposals from event classifications, so a suggest-promoted
  bare-Read rule can be proposed at Safe; `audit` re-classifies persisted rules via
  `classify_rule` and is the backstop that catches it there.
- No new tests. This phase is documentation-only; `classify_tool_input` and
  `classify_rule` logic and every existing test pass unchanged.

### Deviations
- Line-number pointer in the phase bullet (`tier.rs:~321`) no longer matched the file
  after Phases 1-6; located the target by function name (`classify_tool_input`) instead,
  per the doc's own caveat that exact locations drift. Same seam, no behavior change.

### Tradeoffs
- Comment-only fix (chosen, per D4/Alternative 4) vs. a shared `RuleOrEvent` classifier
  parameter: a shared classifier would give typed consistency and let `suggest` label
  proposals by eventual rule tier, but it is churn in permit's most behavior-sensitive
  module for zero tier reassignment, and the two functions legitimately answer different
  questions (a persisted grant vs. a single observed event). The comment documents the
  intent at zero risk; `audit` already covers the one place the two classifiers actually
  interact.

### Open questions
- None.

## Phase 8: log unification + doctor awareness (F6)

### Design decisions
- Added `pub fn log_file_path() -> PathBuf` to `permit/src/lib.rs` and `report/src/lib.rs`,
  matching `cost/src/lib.rs::log_file_path`'s existing shape exactly (same signature, same
  `xdg_data_dir().unwrap_or_else(|| PathBuf::from("."))` fallback). All three now build
  `<xdg-data>/clyde/logs/<tool>.log` (`cost.log`, `permit.log`, `report.log`); `permit`'s and
  `report`'s private `setup_logging` were rewired to call the new function and derive their
  `create_dir_all` target from `log_file.parent()`, mirroring `cost`'s existing pattern instead
  of re-deriving the directory independently.
- clyde's own log is untouched: `clyde/src/main.rs` already resolves
  `session::paths::data_root().join("logs").join("clyde.log")` = `<xdg-data>/clyde/logs/clyde.log`
  - the exact unified directory the other three tools now share, just a different filename. No
    change needed there.
- Every help string naming a log path now renders from the function, never a literal:
  - `cost/src/cli.rs`: `CostCli`'s `after_help` was a hardcoded string, always overridden anyway
    by `ccu.rs`'s dynamic `~`-relative render (unaffected by this phase, already correct). Replaced
    the hardcoded static fallback with a `LazyLock<String>` built from `crate::log_file_path()`, so
    the *only* remaining hardcoded log-path literal in the crate is gone even though it was
    previously dead in the actual `ccu` invocation path.
  - `permit/src/cli.rs`: `PermitCli`'s `after_help` was a hardcoded string and, unlike `cost`, is
    NOT overridden anywhere (`claude-permit.rs`'s bin uses plain `Parser::parse()`). Replaced with
    a `LazyLock<String>` rendering `crate::log_file_path()`, matching the pattern `report/src/cli.rs`
    already used for its `HELP_TEXT` static (chosen over restructuring `claude-permit.rs` into a
    `CommandFactory`/`FromArgMatches` two-step like `ccu.rs`, since the existing `report` precedent
    achieves the same "never hardcoded, always current" property with a smaller diff).
  - `report/src/cli.rs::get_tool_validation_help` (the REQUIRED TOOLS block, the explicit Phase 6
    handoff): the trailing `"\nLogs: ~/.local/share/claude-report/logs/claude-report.log"` literal
    became `format!("\nLogs: {}", crate::log_file_path().display())`.
- `clyde/src/doctor.rs::Report` gained two new fields, both populated unconditionally (never
  affecting `healthy()`): `log_locations: Vec<(&'static str, PathBuf)>` (all four tools' current
  unified log paths, always shown so `clyde doctor` answers "where are the logs" even before a
  tool has ever run) and `legacy_log_dirs: Vec<PathBuf>` (any of `ccu/logs/`, `claude-permit/logs/`,
  `claude-report/logs/` that still exist under `xdg_data`, filtered to only present ones).
  Both are computed in a new `log_state(paths: &Paths)` helper and printed in `print_report` under
  a `logs:` section; `legacy_log_dirs` prints only when non-empty, under a yellow "informational"
  header, explicitly NOT folded into `legacy_state` or `healthy()`.
- README: added a "Log paths are the one deliberate exception to 'behavior-exact'" paragraph under
  "Compat shims" (naming D3 and this design doc), and extended the "Data layout (XDG)" code block
  with the four unified log paths (annotated with their legacy `was ...` predecessors) plus a note
  under "Install" that `doctor` now reports log locations and legacy log dirs informationally.
- Design doc: appended one line to its own "Rollout Plan" section noting Phase 8 landed and
  pointing at the README note, per the phase's "compat note in ... this doc" instruction. The
  design doc itself is intentionally NOT staged in this phase's commit (per the parent
  orchestrator owning its finalization); the note is a working-copy edit only until the
  orchestrator's own commit picks it up.

### Deviations
- None from the phase's stated scope. `permit`'s `after_help` fix uses the `LazyLock` pattern
  (matching `report`'s existing precedent) rather than the `ccu.rs`-style dynamic-override-in-`main`
  pattern the doc's `cost` reference point uses - same effect (help always renders the live path,
  never a hardcoded string), correct seam for the fact that `claude-permit.rs` doesn't already have
  the `CommandFactory`/`FromArgMatches` two-step `ccu.rs` uses.

### Tradeoffs
- `LazyLock<String>` static after-help (permit, and the `cost` static fallback) vs. restructuring
  each shim's `main()` to override `after_help` dynamically before parsing (the `ccu.rs` pattern):
  the `LazyLock` approach is a smaller, crate-internal diff that doesn't touch the compat shim
  binaries' argument-parsing flow at all, and is already the established pattern in this same
  workspace (`report/src/cli.rs::HELP_TEXT`) for exactly this "render a runtime value into a static
  clap attribute" problem. `ccu.rs`'s extra `~`-relative display polish was left as `cost`-only
  (pre-existing, unchanged), not replicated onto `permit`/`report`, since the phase's ask is
  "never hardcoded," not "identical display formatting" across all three shims.
- Two new `Vec` fields on `doctor::Report` (`log_locations`, `legacy_log_dirs`) vs. a single
  combined struct: kept them as two independent fields (one always-populated informational list,
  one presence-filtered informational list) rather than inventing a `LogState` wrapper type, since
  both are consumed identically (iterate and print) and a wrapper would add a name without adding
  behavior for a two-field, doctor-internal shape.

### Open questions
- None beyond the ones the design doc's own "Open Questions" section already tracks (legacy
  log-dir lifecycle after Phase 8 - fold into a future `clyde clean` or leave forever - is
  explicitly called out there and not re-litigated here).


## Phase 9: pricing staleness guard (F1)

### Design decisions
- Added `#[serde(default)] data_version: Option<String>` to `PricingFile` and to `EmbeddedData`,
  parsed once via the existing `embedded_data()` `OnceLock` cell, and exposed
  `pub(crate) embedded_data_version() -> Option<&'static str>` -- `pricing/src/pricing.rs` -- so the
  embedded baseline stops discarding its own timestamp and the guard has an authority to compare
  against.
- Guard lives INSIDE `fetch_and_cache`, immediately after the incompatible-feed check and BEFORE
  `write_cache_atomic` -- `pricing/src/fetch.rs::fetch_and_cache` -- so a stale feed is rejected
  before it can overwrite a newer cache or land on disk. A stale feed returns `Err(PricingError::Fetch)`
  (same shape as the incompatible-feed guard), which routes `auto_with_config` through `record_failure`
  + the unchanged `fallback_chain` (cache -> user override -> embedded). A `warn!` naming both
  versions and the URL fires at the guard site.
- Staleness policy in `fetched_feed_is_stale` + `is_canonical_utc` -- `pricing/src/fetch.rs` --
  strictly-older-than-embedded loses; equal or newer wins; missing or non-canonical-UTC fetched
  version is treated as stale; a non-canonical or absent EMBEDDED version disables the guard
  (fail-open to pre-guard behavior). Canonical UTC is RFC-3339 parseable, zero offset, literal `Z`
  suffix (lexicographic compare is sound only across that fixed-width form; `+00:00` is rejected).
- Documented the full source-selection state machine (cache-hit / backoff / fetch-fail /
  fetch-stale / fetch-newer / fallback_chain, and the single cache-write point) in the `fetch.rs`
  module doc, since the state count outgrew per-function prose.

### Deviations
- The `data_version` plumbing (the `PricingFile` field, the `EmbeddedData` field, and
  `embedded_data_version()`) is `#[cfg(feature = "fetch")]`-gated. The design's Data Model note
  says add the field unconditionally, but only the fetch-gated guard reads it; leaving it
  ungated makes the field/accessor dead code under `#![deny(dead_code)]` when the crate is built
  without `fetch` (external consumers `ccu`/`cr`). Same effect where it matters (guard reads the
  embedded version), correct seam for the lint. `PricingFile` has no `deny_unknown_fields`, so the
  JSON key is simply ignored in a no-fetch build.
- Bumped the shared `V1_FEED` fixture in `pricing/src/fetch/tests.rs` from `2026-04-28T00:00:00Z`
  to `2099-01-01T00:00:00Z`. That fixture predates the guard and its date is now older than the
  embedded baseline (`2026-06-30T23:29:00Z`), so every existing fetch-success test would otherwise
  trip the new stale guard and fail. Bumping to a fixed far-future date keeps those tests
  exercising the "fetch newer wins" path and stable against the daily-advancing embedded baseline.
  Test bodies/assertions are unchanged; only the fixture constant moved. (The independent
  `V1_FEED` const in `pricing/src/feed/tests.rs` was left untouched -- those tests exercise
  `from_bytes` directly, never `fetch_and_cache`, so the guard does not apply.)

### Tradeoffs
- Rejecting a stale feed as `Err` (entering the failure-backoff window) vs. a softer "reject but
  keep retrying" path: chose `Err`, matching the existing incompatible-feed guard exactly. During a
  publish lag this backs off instead of hammering the stale endpoint, and reuses the already-tested
  `record_failure` + `fallback_chain` machinery rather than inventing a third resolution path.
- `is_canonical_utc` requires a literal `Z` and rejects `+00:00`: stricter than "is this UTC," but
  lexicographic ordering (what the guard uses) is only sound across a single canonical textual form.
  A `+00:00`-suffixed feed is treated as stale rather than mis-compared -- fail toward the reviewed
  embedded baseline, which is the safe direction per D2.

### Open questions
- Stale-feed observability in `clyde cost pricing` output (and/or the statusline), plus a debounce
  so a legitimately-lagging feed does not warn on every statusline tick: the design's Open Questions
  lean yes on output surfacing but say "decide debounce at implementation." DEFERRED out of this
  phase. This phase delivers the core guard and a `warn!` naming both versions + the URL; wiring the
  state into `clyde cost pricing`'s rendered output and choosing a debounce touches the cost/output
  crate and a new surfaced field, which is beyond the guard's seam. Flagging explicitly rather than
  silently expanding scope -- recommend a follow-up that reads `Pricing::source()`/`data_version()`
  at the render site and shows a one-line staleness banner, debounced on the cache's last-attempt
  timestamp.
