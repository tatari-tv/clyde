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
