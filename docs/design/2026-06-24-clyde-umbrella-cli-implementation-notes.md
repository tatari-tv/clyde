# Implementation Notes: clyde umbrella CLI

Running record of how the implementation diverges from or interprets
`docs/design/2026-06-24-clyde-umbrella-cli.md`. Append-only.

## Phase 0: Rename klod to clyde

### Design decisions
- Renamed the XDG namespace constant `KLOD_DIR` -> `CLYDE_DIR` (`session/src/paths.rs`)
  rather than keeping the symbol name and only changing its value, so the symbol matches
  the value and greps stay honest.
- Left the literal on-disk path `/home/saidler/repos/tatari-tv/klod/main` in
  `session/src/scope/tests.rs` unchanged. That test only exercises org-based work/personal
  classification (any `tatari-tv/*` path is Work); the string reflects the real local
  checkout dir, which is still named `klod`. Renaming the GitHub repo (and thus the local
  dir) is a separate ops step out of scope for the code rename.
- Renamed the `argv[0]` literals in `clyde/src/cli/tests.rs` from `"klod"` to `"clyde"` for
  accuracy even though clap ignores the program name during `try_parse_from`.

### Deviations
- None. The rename is exactly the Phase 0 scope: member dir, bin name, crate name,
  `default-members`, XDG path constants, doc comments, crate descriptions, README, and the
  `CARGO_BIN_EXE_*` reference in the serve integration test.

### Tradeoffs
- Used a scoped `perl -i` bulk replace for the doc-comment/description references in the
  `session`/`sessions` library crates and the README (all unambiguous `\bklod\b` -> `clyde`),
  vs. per-line Edits. The whole-word boundary plus an explicit exclude of `scope/tests.rs`
  kept the real on-disk path intact. The load-bearing source edits (paths.rs constants, cli.rs
  name, main.rs log filename) were done as explicit Edits, not the bulk pass.

### Open questions
- None.

## Phase 1: Subtree-merge the four repos

### Design decisions
- Subtree-added from the local clones (`/home/saidler/repos/tatari-tv/<repo>` `main`) rather
  than the GitHub remotes; the local checkouts were clean and on `main`, and a local fetch is
  faster and offline. Full history (no `--squash`), so each merge commit carries the original
  lineage as its second parent (verified: `git log <add-commit>^2` reaches the pre-merge HEAD;
  total workspace history grew to 222 commits).
- Added all four (`report`, `cost`, `permit`, `pricing`) to `[workspace] members` in this phase
  per the design. This leaves the workspace intentionally non-building (git-pinned pricing dep,
  unreconciled dep versions, two `[[bin]]` packages with no lib) until Phase 2 — consistent with
  the design's "import-only; no clean build expected yet" and the PR-B grouping (Phases 1-3 land
  as one green unit).

### Deviations
- None.

### Tradeoffs
- Left the imported nested `Cargo.lock`, `.otto.yml`, `install.sh`, `clippy.toml`, and
  `rustfmt.toml` files in place for now. They are redundant under a single workspace but removing
  them is Phase 2 (deps/lints reconciliation) and Phase 6 (CI/docs) work; deleting them in the
  import commit would muddy the "import-only" boundary.

### Open questions
- None.

## Phase 2: Convert absorbed crates to libraries (two-type clap shape)

### Design decisions
- Added a `common` crate (`common/`, lib `common`) as the home for [`common::Globals`]. The
  design diagram's member list omitted it, but the design text explicitly names "the clyde-common
  surface" where `Globals` is defined. A shared single struct needs a crate all of
  report/cost/permit/clyde can depend on without a cycle; `session`/`pricing` were wrong homes
  (permit depends on neither). `Globals` is intentionally one field, `log_level: Option<String>`
  — `None` means "no explicit level," which preserves each tool's prior default (permit's
  `RUST_LOG`, ccu's config/`RUST_LOG`/`ccu=warn` chain) when a shim is invoked without a level.
- `run()` owns logging setup (using `globals.log_level`) for all three tools, so `globals` is a
  used parameter (the otto lint forbids `_globals`, and an unused field would trip workspace
  `dead_code`). clyde will set up logging per-arm in Phase 3 so only one logger init happens per
  process.
- pricing keeps an explicit `version = "2.0.0"` instead of inheriting the workspace version line.
  Its crate major is contractually locked to the feed `schema_version` via
  `LIBRARY_VERSION = env!("CARGO_PKG_VERSION")` (feed.rs), compared against the feed's
  `min_library_version`; a 0.x version would make every fetched feed be rejected as
  "library too old." This is the one intentional exception to the single-version-line goal.
- pricing passes `app_name = "clyde"` (was per-tool "cr"/"ccu"), unifying the override namespace
  to `~/.config/clyde/pricing.json` per the Data Model.
- Relocated the two disposable caches per the Data Model: pricing `~/.cache/claude-pricing` ->
  `~/.cache/clyde/pricing` (`pricing/src/fetch.rs`), cost `~/.cache/ccu` -> `~/.cache/clyde/cost`
  (`cost/src/cache.rs`). Both rebuild at the new path on first run.

### Deviations
- The permit **library** does NOT carry `#![deny(clippy::unwrap_used)]`. The pre-merge permit lib
  never had it (only its `main.rs` bin did), and adding it surfaced ~30 pre-existing `unwrap()`s in
  permit's lib modules. Phase 2 is structural/behavior-exact, not a lint cleanup of permit's
  internals, so the deny stays only on the new `claude-permit` shim bin (matching the old bin).
  The design's lint reconciliation explicitly scoped to workspace `dead_code`/`unused_variables`,
  not `unwrap_used`.
- Shim error-path stderr: the `cr`/`ccu` shims print `{e:?}` on a non-handled error (the design's
  canonical shim pattern); the pre-merge tools printed `Error: {e:?}` (cr) / bare (ccu). The
  `claude-permit` shim keeps `Error: {e:?}` to match its old bin. This is a cosmetic prefix
  difference on non-behavior-critical error paths; all exit codes and the permit `{}`-hook
  contract are byte-exact.

### Tradeoffs
- Hoisted the shared dep union into `[workspace.dependencies]` and reconciled versions to the
  highest seen: rusqlite 0.39.0 -> 0.40.1 (permit), clap 4.5.60 -> 4.6.1 (ccu), serde_json
  1.0.149 -> 1.0.150, log 0.4.29 -> 0.4.33, env_logger 0.11.9 -> 0.11.10, chrono 0.4.44 ->
  0.4.45, rayon 1.11 -> 1.12, tempfile 3.13 -> 3.27. Feature flags stay additive per-crate
  (ccu keeps clap `env`, permit keeps rusqlite `bundled`, report keeps ureq `json`). All tests
  pass after the bumps. Crate-unique deps (comfy-table, rasciichart, sparklines, include_dir,
  gethostname, regex, terminal_size, wait-timeout, mockito) stay in their own manifests.
- report: the former `pub fn run(&Config)` was renamed to `run_with_config` (its tests and the
  new `run(args, globals)` both call it); the `TryFrom<Cli>` impl became a free
  `resolve_command(command)` so `run` can build the `Config` from the nested `ReportArgs` plus
  globals. cost: `main`'s dispatch `run(cli, config, pricing)` was renamed to
  `dispatch(args, config, pricing)`; the dynamic log-path `after_help` moved to the `ccu` shim
  (parsing layer) since `run` starts post-parse. permit: the lib was renamed `claude_permit` ->
  `permit`, and the `{}`-on-failure hook contract moved from the old `main` into `permit::run` via
  an `is_log`-guarded wrapper.

### Open questions
- None blocking. (Process note: of the two delegated forks, both happened to convert `cost`;
  `permit` was converted directly. No correctness impact — the merged workspace is green.)

## Phase 3: Wire the clyde umbrella

### Design decisions
- clyde's top-level `--log-level` changed from `String` (default "info") to `Option<String>`
  (mirrors sb). Unset = `None` flows to `Globals`, so the absorbed tools keep their own prior
  defaults (parity-preserving: `clyde cost` with no level behaves like `ccu` with no level).
  The clyde-native `sessions` subtree defaults to `info` at the logging-setup site
  (`DEFAULT_LOG_LEVEL`), preserving its prior behavior.
- Logging is set up per-arm in `main()`: the `Report`/`Cost`/`Permit` arms install NO clyde
  logger (each tool's `run()` installs its own — env_logger can only init once per process); the
  `sessions` arm keeps the existing env_logger / serve-tracing split. Exactly one logger init per
  invocation.
- The absorbed-tool arms call `dispatch_tool(tool::run(args, globals))` which maps `Result<i32>`
  to `process::exit`, exactly mirroring each standalone shim's `main`. The `sessions` arms keep
  returning `Result<()>` to `main` as before.

### Deviations
- Dropped the `Debug` derive from clyde's `Cli` and `Command` types. The new `Cost`/`Permit`
  variant payloads (`CostArgs`/`PermitArgs`) don't derive `Debug` (their `Command` enums never
  did — ccu used `std::mem::discriminant`), so keeping `Debug` on clyde's `Command` would have
  cascaded `Debug` derives across both tool crates. Nothing in clyde relies on `Cli`/`Command`
  being `Debug`. Minimal, lower-risk than the cascade.

### Tradeoffs
- Considered keeping clyde's `--log-level` as `String` default "info" (less churn) vs.
  `Option<String>` (parity with the tools' own defaults). Chose `Option` for behavior parity and
  to mirror sb's umbrella, defaulting to "info" only for the sessions subtree at the use site.

### Open questions
- The design lists an "Implementation Audit after Phase 3 builds green" (`/architect` Mode 2)
  to confirm each tool's exit/output contract is byte-preserved. That is an external review step;
  it has not been run as part of this automated execution. Smoke checks done here: `cr`/`ccu`
  `--help`/`--version` render under the old names, `clyde report|cost|permit --help` render under
  `clyde <tool>`, the permit `{}`-on-failure hook contract holds (garbage stdin → `{}`, exit 0),
  and the `ccu --log-level debug` globals round-trip is covered by a unit test.

## Phase 4: bootstrap + doctor

### Design decisions
- All bootstrap/doctor logic operates on an injected `bootstrap::Paths { home, xdg_data,
  xdg_config, xdg_cache }` (with `Paths::from_env()` for the real run). The whole surface is
  tested against temp `$HOME`s with zero env mutation and no touching of the real machine.
- The systemd `daemon-reload` (the only step that shells out) lives in `bootstrap::run()`, OUTSIDE
  the hermetic `bootstrap()` core, gated on `!skip_systemd && outcome.systemd_changed`. Tests call
  the core (and `repoint_systemd` directly) and never shell out.
- WAL-safe events-DB move (`migrate_events_db`): open the legacy DB, `PRAGMA
  wal_checkpoint(TRUNCATE)`, drop the connection, then `fs::rename` `events.db` plus any
  `-wal`/`-shm` sidecars. A parity test asserts row count is preserved and the legacy path is gone.
- Hook rewrite walks `hooks.PreToolUse[].hooks[].command` as `serde_json::Value` and replaces only
  the exact `claude-permit log` string with `clyde permit log`, reserializing pretty — preserving
  all other fields, matchers, and order. Applied to both global and local settings.
- doctor exits non-zero when any integration is legacy, the events DB is stranded at the legacy
  path, or any config exists only at a legacy path. `Absent` integrations are healthy. doctor is
  read-only.
- `permit::EventStore::default_path()` now prefers `~/.local/share/clyde/events.db` with
  read-fallback to the legacy path (Phase-4 surgical pass), so the shim finds its DB before and
  after bootstrap.

### Deviations
- The tools' own installers were repointed in a surgical pass (not inside bootstrap): permit's
  `cmd/install.rs` writes `clyde permit log` (detection recognizes both forms for idempotency); the
  `cost` statusline templates invoke `clyde cost`. Their unit tests were updated. Doing this outside
  bootstrap keeps bootstrap focused on migrating existing state.
- doctor's systemctl `last-run` field was intentionally not wired (it would shell out and isn't
  needed for the pass/fail gate).

### Tradeoffs
- `clyde`'s top-level `--db` global (`global = true`, for sessions) also appears in
  `clyde bootstrap --help`. Cosmetic; left as-is.
- bootstrap is one ~500-line module + tests, under the 1500-line bloat limit. If it grows it should
  decompose into `bootstrap/{migrate,repoint}`.

### Open questions
- The design's "bootstrap verify pass before first live run" (`/staff-engineer`) is an external
  review step not run here, and bootstrap has NOT been run against the real machine — that is the
  operator's finalization step. All logic is covered by temp-`$HOME` unit tests (events-DB WAL move
  + row-count parity, global+local hook rewrite, statusline rewrite, systemd unit rename + env-file
  move with permission preservation, full-bootstrap idempotency, pricing-override merge, doctor
  healthy/unhealthy gates).
