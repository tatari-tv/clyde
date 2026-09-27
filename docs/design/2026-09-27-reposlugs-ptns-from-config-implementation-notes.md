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
