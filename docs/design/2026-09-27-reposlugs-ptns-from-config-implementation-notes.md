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
