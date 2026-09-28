# Implementation Notes: Work Reposlug Patterns From Config

Design doc: `docs/design/2026-09-27-reposlugs-ptns-from-config.md`

## Phase 1: `reposlugs-ptns` config key

### Design decisions
- `ReposlugPtn::parse` lives on the type itself, not as a free function in `common::config`
  (`common/src/repo/ptn.rs:ReposlugPtn::parse`), so the parsing/validation rules travel with the
  type they build, and `common::config::de_reposlugs_ptns` is a two-line adapter (`Vec<String>` ->
  `map(ReposlugPtn::parse)` -> `collect`), matching how `common::repo::host` owns its own allowlist
  logic rather than `config.rs` owning it inline.
- Every rejection message is hardcoded to say `reposlugs-ptns` (`common/src/repo/ptn.rs:parse`),
  the same choice `de_work_remote_hosts` and `de_repo_roots` already make in `config.rs`, because
  `serde_yaml`'s `de::Error::custom` reports the enclosing YAML location, never the field name.
- The empty-string case (`raw.is_empty()`) is checked and messaged separately from a real `!` with
  nothing after it (`rest.is_empty()`, `common/src/repo/ptn.rs:parse` lines 80-96), because they
  have different causes: the former is (almost always) serde_yaml silently collapsing an unquoted
  `- !owner/repo` YAML tag into `""`, which needs the quote-it hint; the latter is a real, already
  quoted string `"!"` a config author typed, which does not need the YAML-tag explanation attached
  to it.
- `ReposlugPtn` and `RepoPart` derive `PartialEq, Eq` (no `Ord`/`Hash` yet); Phase 1's own tests are
  the only current consumer of equality, and Phase 2's `ScopePolicy::new` dedup is out of scope here
  per the assignment ("NOT read by the classifier yet").
- `Display` for `ReposlugPtn` produces the normalized text form (`!`-prefixed, lowercased) rather
  than echoing the raw configured spelling (`common/src/repo/ptn.rs:156`), because the design doc
  names this exact form for both the Phase 3 fingerprint and the Phase 4 `doctor` line ("ptns in
  normalized text form (`!` kept, lowercased)"), so building it once now means neither later phase
  reinvents it.

### Deviations
- The design doc's struct sketch (`pub struct ReposlugPtn { exclude: bool, owner: String, repo:
  RepoPart }`) shows all three fields written bare in a code block; the actual fields are private
  with accessor methods (`is_exclude()`, `owner()`, `repo()`) instead, matching every other
  `common` config type in this file (`EfficiencyConfig`, `RenderConfig`) and the crate's own
  `#[deny(dead_code)]` posture. Same effect, correct seam: nothing outside this module needs to
  construct a `ReposlugPtn` by hand, only via `parse`.
- Pre-existing, unrelated `clippy::question_mark` failure in `common::repo::parse_slug`'s `ssh://`
  branch (`common/src/repo.rs`, around line 1170) blocked `otto ci` on this branch before any of
  this phase's code was touched (confirmed via `git stash` plus a standalone `cargo clippy -p
  common` on the pre-Phase-1 tree). Fixed with clippy's own suggested `?`-operator rewrite, a pure
  mechanical refactor with no behavior change, so `otto ci` could run at all. Not part of the
  design doc's Phase 1 scope; flagged here rather than silently folded in.

### Tradeoffs
- `de_reposlugs_ptns` re-parses every entry from scratch on every `Config::load()` rather than
  caching a compiled form: chosen because Phase 1 has no hot path yet (no classifier reads it), and
  a `Vec<ReposlugPtn>` of a handful of entries costs nothing to reparse; premature to add caching
  before Phase 2 shows whether `ScopePolicy` needs one.
- `DEFAULT_REPOSLUGS_PTNS` entries are parsed via `ReposlugPtn::parse(..).expect(..)` in
  `default_reposlugs_ptns()` rather than being hand-constructed literals: chosen so the one literal
  spelling (`"tatari-tv/*"`) stays the single source of truth for both the const and the parsed
  default, at the cost of an `.expect()` that can only ever fire on a programmer typo in the const
  itself (caught by `cargo test`, not at runtime).

### Open questions
None.

## Phase 2: `ScopePolicy` replaces `Anchors` and `WORK_ORGS`

### Design decisions
- Shape guards moved into one private `slug_parts` (`session/src/scope.rs:slug_parts`) that `ScopePolicy::matches` calls, so `matches` is fail-closed on its own (it is `pub`) and `is_work_slug` is a one-line delegate kept only for its doc comment on the two matching forms. The BITES note at `scope/tests.rs` `a_malformed_slug_with_a_slash_fails_closed` now names `slug_parts`.
- The git-origin arm was lifted into `git_origin_decision(slug, is_work, facts)` (`session/src/scope.rs`), shared by the unanchored path and `Anchor::Deferred` via `deferred_decision`, so a named repo reaches Work only through the same host and probe refusals as any other git-origin slug; there is one copy of those guards, not two.
- The Wide-owner exclude check reads the stored `repo` regardless of `repo_source` (`ScopePolicy::wide_anchor`). An exclude can only take Work away, so checking every stored slug is the fail-closed reading of the doc's "the stored `repo`".
- `Excluded` and the no-remote `Deferred` case both report `Basis::CwdAnchor` with `settled: false`: the path decided, and provisional so a later policy change reaches the row.
- A bare `<root>/<owner>` under a `Named` owner also returns `Deferred` (not today's unanchored), so a named owner never reaches the touch set at any depth.
- `ScopePolicy::new` sorts and dedups roots as well as patterns; patterns sort by their normalized text. `fingerprint()` is `serde_json::json!({"ptns", "roots"})`, roots as sorted lossy strings. `serde_json` was already a `session` dependency.
- Field and parameter renamed `anchors` -> `scope_policy` everywhere (`EnrichOptions`, `ExportContext`, `classify_row`, `routing_summary(_with)`, `build_export_record`), not `policy`, so it cannot be misread as the `HostPolicy` next to it.
- Test fixtures in the `sessions`/`efficiency` crates each get a local `tatari_wide_ptns()` helper that parses `"tatari-tv/*"`, so every construction names its pattern.
- Two test files crossed the 1500-line `otto bloat` limit and were split: the v5 policy tests went to `session/src/scope/tests/policy.rs`, the default-options test to `sessions/src/enrich/tests/defaults.rs`.

### Deviations
- `ScopePolicy::scope_of(cwd, repo, facts)` takes the stored slug as its own parameter, where the doc has `scope_of(cwd, facts)`. `RoutingFacts` does not carry `repo` (that is a positional argument of `classify_with_evidence`), and adding it would have given the slug two sources. Same effect, correct seam.
- `Anchor::Deferred { owner: &str }` carries the cwd's owner segment, where the doc has a unit `Deferred`. Both the deferred decision and the disclosure need "slug owner == cwd owner", and carrying it avoids walking the roots a second time.
- `repo_host_slug` is not checked in the Wide-exclude rule: the column and its `RoutingFacts` plumbing do not exist until Phase 3. The `scope_of` doc comment says the pairing check lands with that column.
- `ExportContext` got no `Default` (it never had one; it needs a clock and a host). Every caller names `scope_policy` explicitly, which is stricter than an empty default.
- "Unresolved ssh alias -> refused" at the deferred shape is tested by feeding `HostPolicy::with_resolver(["github.com"], <resolver that answers nothing>).confers_work("github-work")` into `RoutingFacts::host_confers_work`. That is the production host path minus `ssh -G`, not a matrix checkout.

### Tradeoffs
- Criterion tests use hand-built stored rows (`decide` in `scope/tests/policy.rs`) vs a real git checkout for every path shape: the policy reads only the stored slug and the path, and the resolver path is already covered by the matrix tests. The two fixture criteria (fork in a work dir, `work_remote_in_personal_dir`) use the real `Matrix` checkouts.
- A legacy test helper `anchored()` maps `Anchor` back to `Option<Scope>` and panics on `Excluded`/`Deferred`, vs rewriting ~40 legacy assertions to `Anchor`. The legacy tests run under `tatari-tv/*`, where neither variant can occur, so a panic means the test is in the wrong file.
- Break-it checks run (then reverted, restored file byte-compared to the committed one): putting a compiled-in `["tatari-tv"]` lookup back into `matches`/`owner_rule` failed 10 policy tests; dropping the exclude checks in `matches` and `wide_anchor` failed 6.

### Open questions
None.

## Phase 3: Schema v14 and re-offer

### Design decisions
- `migrate_v14_scope_policy` is its own ladder step with `snapshot_before_v14` registered in `Db::init` after v13 (`sessions/src/db/migrate.rs`, `sessions/src/db.rs:Db::init`); both columns also go into `SCHEMA_SQL`, the v13 precedent. Column-add only, no backfill: NULL `scope_policy` is what re-offers every enrich-eligible `skipped-personal` row once.
- The fingerprint is rendered ONCE per sweep (`sessions/src/enrich.rs:enrich`, `opts.scope_policy.fingerprint()`) and passed to `enrich_candidates` and all three writers, so the predicate and the writes cannot compare against two different strings.
- `record_enrich_skip` writes the fingerprint on provisional decisions too, not only settled ones. It records the policy the decision was made under either way; provisional rows stay eligible through NULL `scope_version`, and the `IS NOT` guard keeps a repeat provisional pass at 0 writes.
- The pairing refusal lives in a private `host_confers_work` beside `classify_row` (`sessions/src/routing.rs`), so the enrich gate, `doctor`, and export's fallback all get it through the one seam. The slug check runs before the host lookup and fires even when `repo_host` is NULL: it only removes authority.
- The slug comparison is ASCII case-insensitive, matching the policy's per-segment case rule. Both sides come from the same rule-1 parse today, so case never differs in practice; a case-sensitive compare would only ever refuse the same GitHub repo.
- `RoutingFacts` gained `repo_host_slug: Option<&str>`, and `ScopePolicy::wide_anchor` checks excludes against `[repo, repo_host_slug]`. This closes the Wide-exclude check Phase 2 deferred.
- `ScopePolicy::fingerprint()` was checked against the doc: `serde_json::json!` renders `{"ptns":[...],"roots":[...]}` compactly with keys in sorted order, and Phase 2's `the_fingerprint_is_the_normalized_policy` pins the exact string. No change needed.
- Test call sites pass `crate::db::no_policy()` (a `#[cfg(test)]` helper in `sessions/src/db.rs`, the empty policy's fingerprint) to both the writers and `enrich_candidates`. The pre-v14 tests then behave exactly as before, with `scope_policy` inert.
- New tests go in their own submodules (`sessions/src/db/tests/policy.rs`, `sessions/src/enrich/tests/policy.rs`), because `db/tests.rs` and `enrich/tests.rs` sit at the 1500-line `otto bloat` limit (1500 and 1498 after this phase).

### Deviations
- `record_repo_host` is written only when the probe resolved both a host and a slug (`if let (Some(host), Some(slug))`, `sessions/src/index.rs:apply_chain`). The doc names `outcome.resolved_slug()` as the source; for `ProbeOutcome::Resolved` both are always `Some`, so the effect is the same.
- The fingerprint reaches `set_enrichment` as a new `EnrichSuccess::scope_policy` field, not a new parameter. `EnrichSuccess` is already the payload struct for that writer. Same effect, correct seam.
- `enrich_candidates` takes the fingerprint as a new trailing `scope_policy: &str` parameter. The doc gives only the SQL (`?4`), not the signature.
- The "export projection" criterion is met by appending `s.repo_host_slug` to `EXPORT_COLS` (index 31) and carrying it into `evidence_from_row` in the export fallback. Stored scopes are emitted as before; only an undecided row's fallback classification reads the column.

### Tradeoffs
- End-to-end re-point test uses a real `git init` checkout plus `index::reindex` (`enrich/tests/policy.rs:a_re_pointed_checkout_loses_the_old_slugs_authority`), not hand-written columns. It is the only test that proves `apply_chain` writes the slug a second pass observes, and that the rank-0 `repo` survives the re-point.
- Rows written by an "old binary" are simulated with raw SQL on a second connection (`UPDATE ... scope_version = 4`, `scope_policy = NULL`), not by building a v13 binary. A v13 writer's only effects on these columns are exactly those two statements, since it does not know `scope_policy` exists.
- The interrupted sweep is driven by the typed transport failure that aborts `enrich` mid-sweep (`Flaky` fatal), not a simulated crash. It is the production abort path, and it leaves a known set of rows unvisited.
- Break-it checks were run and reverted, each restored file byte-compared to its backup:
  - dropping `OR s.scope_policy IS NOT ?4`: 8 tests failed
  - dropping the pairing refusal in `routing::host_confers_work`: 2 failed
  - dropping `repo_host_slug` from `wide_anchor`: 1 failed
  - reverting the `record_repo_host` guard to host-only: 2 failed
  - restoring the pre-v14 two-`AND` predicate: 3 failed
- `/tmp` (tmpfs) filled to 100% mid-phase, from `/tmp/marquee-bite` (13G, not this work). Test runs during development used a `TMPDIR` on disk. The final `otto ci` ran with the default `TMPDIR` once space freed. Two `common` tests need a temp root outside any git repo, and `~` has a stray `~/.git`, so those two fail under any `TMPDIR` inside `$HOME`.

### Open questions
None.

## Phase 4: Doctor line and docs

### Design decisions
- The `reposlugs-ptns:` line is computed independently of `attribution()` (`clyde/src/doctor.rs:
  reposlugs_ptns_line`), loading `common::config::load()` a second time in `run()` rather than
  threading the already-loaded `Config` out of `attribution()`. `attribution()` returns `Ok(None)`
  before ever loading config when `db_path` does not exist (`doctor.rs:126-129`), and the doc
  requires the line to print in exactly that case (the "no clyde.yml and no catalog" acceptance
  criterion), so the line cannot depend on a value `attribution()` may never produce.
- A second, small, best-effort `config::load()` mirrors the file's existing posture (attribution and
  catalog reads are already each individually best-effort, per the module doc comment on `run`): a
  malformed `clyde.yml` prints `reposlugs-ptns: (could not load config: <e>)` rather than aborting
  the whole report, matching how `attribution`'s own load failure is reported inline instead of
  propagated.
- `format_reposlugs_ptns` is a free function taking `&[common::repo::ptn::ReposlugPtn]` rather than
  going through `session::ScopePolicy` (which already normalizes/sorts/dedups internally): `Config`'s
  own accessor already returns entries in the normalized (lowercased) `Display` form `ReposlugPtn::
  parse` produces, so sorting and deduping the `Display` strings directly reproduces
  `ScopePolicy::new`'s ordering without constructing a policy (which additionally wants `repo-roots`)
  just to print one line.
- The `reposlugs-ptns:` label is exactly 15 characters, the same fixed field width every other label
  in `print_report`'s paths block uses (`binary:` + 8 spaces, `hook (global): `, etc., all 15 chars
  before the value). Kept the literal one-space separator after the colon instead of matching that
  width exactly (which would print zero spaces, `reposlugs-ptns:tatari-tv/*`), because the design
  doc's own acceptance criteria quote the line WITH a space (`reposlugs-ptns: tatari-tv/*`,
  `reposlugs-ptns: none`); readability and the literal expected text both win over one column of
  visual alignment.
- `clyde/tests/doctor.rs` is a new integration test file, spawning the real `clyde` binary against a
  hermetic `$HOME`/`$XDG_*_HOME` with `--db` pointed at a path that does not exist, mirroring
  `serve.rs`/`matrix.rs`'s pattern rather than unit-testing `print_report` directly: the acceptance
  criteria are about the LINE THE BINARY PRINTS in the no-clyde.yml/no-catalog case, and
  `common::config::load()` reads real XDG env vars process-wide, which a unit test in the same
  process as other config tests cannot mutate safely without `common::ENV_LOCK` (not visible outside
  the `common` crate).
- `common/src/config/tests.rs` gained the two doc-accuracy tests (`clyde_yml_example_loads`,
  `every_supported_key_is_documented_in_the_readme_and_the_example`) rather than a new test file,
  because `load_from` is private to the `config` module and these tests need it directly (the
  `clyde.yml.example` is deliberately all-commented, so proving it loads means proving it equals
  `Config::default()`, not just that some subcommand exit code is zero).
- `SUPPORTED_CONFIG_KEYS` lists nested keys (`format`, `model`, `cache-read-share-floor`, ...) bare,
  without their parent section prefix, and checks each with a plain substring search rather than a
  YAML-aware parse: the goal is "documented somewhere a reader would see it," not "documented at one
  exact line," and README.md already documents `render:`/`efficiency:`/`projects-dir`/
  `reindex-on-start` in sections outside the one this phase edited (lines ~191-230, ~272-278) written
  by earlier work, not this phase.

### Deviations
- The design doc's per-key wording ("`clyde doctor` prints `reposlugs-ptns: otto-rs/otto,
  scottidler/claude, ...` ... beside the always-printed `config:` line") is followed exactly for
  placement, but the design's `ScopePolicy`-flavored phrasing ("normalized, sorted") is implemented
  by sorting/deduping the `Display` strings directly in `doctor.rs` rather than by constructing a
  `session::ScopePolicy` and asking it. Same effect (same normalization rules,
  `ReposlugPtn::parse` already lowercases), correct seam: `doctor`'s paths block has no `repo-roots`
  context at that call site and doesn't need one just to render this one line.

### Tradeoffs
- `reposlugs_ptns_line()` calls `common::config::load()` a second time (once here, once inside
  `attribution()` when a catalog exists) rather than restructuring `run()`/`attribution()` to load
  config once and share it: the doc scopes this phase to the doctor LINE and the docs, not to
  `doctor.rs`'s existing best-effort-per-section architecture, and a second `load()` call is cheap
  (a small YAML file, no catalog I/O) next to a rewrite that would touch `attribution()`'s signature
  and every one of its call sites.
- `clyde/tests/doctor.rs` spawns the real compiled binary (`env!("CARGO_BIN_EXE_clyde")`) per test
  rather than calling `doctor::run` or `print_report` in-process: `print_report` is a private,
  stdout-printing function with no return value, so the only way to observe its output without
  changing its signature (out of scope for this phase) is to capture a child process's stdout, same
  as `serve.rs` already does for the same reason.

### Open questions
None.
