## Phase 0: Spike the real outcome vocabulary and size the attribution recovery

### Design decisions
- Measured the rule-3 ceiling against the EXACT tool-call shape `efficiency::outcome::union` already
  extracts (`Edit`/`Write` only, `input.file_path`, confirmed by a non-error `tool_result`) rather
  than a looser scan of every edit-shaped tool -- `efficiency/src/outcome.rs:318-329` (`classify_tool`)
  is the ground truth for what Phase 3 will actually build `repos_touched` from, so the ceiling had
  to be measured against that exact filter, not an approximation.
- Reproduced the doc's own 562 / 283 / 279 session-and-dollar figures live before measuring anything
  new, to confirm the window (`--since 2026-06-26 --until 2026-07-25`) and the `$HOME`-or-temp-dir
  split are exactly reproducible against today's catalog, not drifted since the doc was authored.

### Deviations
- None. Zero code changed, per the phase's own constraint.

### Tradeoffs
- A naive scan (also counting `MultiEdit`/`NotebookEdit`, and not filtering on `tool_result`
  confirmation) gives 76 unique-argmax sessions instead of 73, and 83 touched-at-least-one instead
  of 80. Reported only the code-matching numbers in Resolved Decisions since those are what Phase 3
  will actually produce; the wider scan would overstate the ceiling Phase 3 is held to.

### Open questions
None.

## Phase 1: `common::repo` with the four-rule chain

### Design decisions
- Rule 2's port is a POINT lookup, not a prefix lookup: `PathMap::repo_for_path(&Path)` answers for
  exactly one path and `common::repo::from_known_path` walks `Path::ancestors()` longest-first.
  Putting the longest-prefix semantics in the chain (not in each implementation) means Phase 2's
  catalog-backed impl is a handful of `repo_paths` PRIMARY KEY point lookups rather than a scan, and
  there is exactly one place where "longest prefix" is defined.
- The rule-2 ancestor walk STOPS at a blocked root instead of skipping it -- `from_known_path`,
  `common/src/repo.rs`. Rule 1 can never record `$HOME` (it rejects a blocked toplevel), so this is
  belt-and-braces, but it makes the `$HOME` block a property of the whole chain rather than of one
  rule: nothing at or above `$HOME` can attribute a session even if a stray row lands there.
- `RepoSource` variants are declared best-first so the derived `Ord` and `rank()` agree, and a test
  pins that they do. Phase 2's upgrade-only upsert compares on `rank()`; any in-memory comparison
  uses `Ord`. Two orderings that could disagree would be exactly the "two signals encoding the same
  meaning" failure.
- `FromStr` lands with `as_str` in this phase even though Phase 2 is the first reader. The kebab
  spellings are a persistence contract, so the round trip is written and tested where the type is
  defined rather than re-derived at the call site, and an unknown value is a loud error naming the
  legal set (a dropped provenance would let a guess read back as an observation).
- `Resolver` keeps rule 1's memo and now owns the chain (`Resolver::resolve`), because the blocked
  roots and the git cache both already live there. Rules 2, 3, and 4 are free functions so each is
  testable with no `Resolver` at all.
- `Resolver::blocked_roots()` is exposed so a caller running the rules individually blocks exactly
  the set the chain does, rather than re-deriving `$HOME` and drifting.

### Deviations
- The doc's Data Model shows `pub enum RepoSource` / `pub struct Resolved` and nothing else; the
  shipped module adds `rank()`, `as_str()`, `Display`, `FromStr`, and the `PathMap` port. Same
  effect, correct seam: the rank ordering and the kebab spellings are both named in the doc's prose
  ("git-origin(0) < known-path(1) < ...", "git-origin | known-path | files-touched | path-guess"),
  so they are the type's contract, not new scope.
- `repo-root` validation is applied to an EXPLICITLY SET value only, not to the `<home>/repos`
  default. The doc says "validated at load (absolute path, existing directory)"; validating the
  default too would make every clyde command fail on a machine with no `~/repos`, which is a
  fail-OPEN-to-fail-BRICKED trade nobody asked for. An unset root that does not exist simply means
  rule 4 never fires, which is the fail-closed answer. The distinction is documented on
  `de_repo_root` and in the README paragraph.
- Phase 1's bullet also names Phase 3's `--min-enrichment` ("same treatment"). Not shipped here: its
  consumer (`report collect`'s enrichment warning) is Phase 3, and a config key with no reader is a
  surface that cannot be tested end to end. Phase 3 ships the key, the CLI override, and the example
  together, exactly as this phase shipped `repo-root`.

### Tradeoffs
- `PathMap` as a generic (`fn resolve<M: PathMap>`) vs `&dyn PathMap` -- house DI rule, and it keeps
  `common` free of any SQLite linkage. Cost: `Resolver::resolve` is monomorphized per caller, which
  is irrelevant at two call sites.
- Rule 3's tie handling abstains rather than tie-breaking on slug order. Measured cost from Phase 0:
  7 sessions / `$159.42` fall through to rule 4 instead of being resolved. Accepted, per the doc:
  a tie is evidence of ambiguity, and a slug-ordered winner would fire precisely in the cold-cwd
  case rule 3 exists to serve.
- Rule 4 declines a non-UTF-8 path component instead of lossily converting it. A mangled slug would
  be silently wrong forever in the catalog; declining just means that one session stays
  unattributed.
- `report` re-exports `common::repo` (`pub use common::repo;`) rather than every call site being
  rewritten to `common::repo::...`. Keeps this phase to the move plus the new rules; Phase 3 deletes
  the `report` call site entirely when collect starts reading the persisted column.

### Open questions
None.
