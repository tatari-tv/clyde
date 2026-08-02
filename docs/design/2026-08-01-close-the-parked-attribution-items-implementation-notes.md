# Implementation Notes: Close the Parked Attribution Items

Running record of decisions, deviations, tradeoffs and open questions from executing
`docs/design/2026-08-01-close-the-parked-attribution-items.md`. Append-only.

## Phase 0: Prove rule 5 has a population

### Design decisions
- None. Zero-code phase.

### Deviations
- **Phase 0 did not run, and Phase 4 is therefore NOT built.** The doc states it up front
  ("it needs three teammate catalogs and cannot run on this host") and gates Phase 4 on it. The
  query needs `sessions.db` from Keegan's, Stephen's and Luke's machines; none is reachable from
  desk.lan. This is the documented blocked-phase path, not a silent skip.
- The doc's decision rule has two branches, and NEITHER fired: "any catalog recovers more than
  zero -> Phase 4 builds rule 5" and "every catalog recovers zero -> Phase 4 is struck". An
  unmeasured population is a third state the rule does not cover, and it is the one we are in.
  Phase 4 is left UNBUILT and UNSTRUCK, pending the measurement.

### Tradeoffs
- Build Phase 4 anyway on the desk.lan null result vs. leave it pending. Left pending: the doc's
  own Resolved Decision says rule 5 ships "despite recovering zero sessions here" specifically
  because desk.lan is not the population, so shipping it with zero measurements anywhere would
  ship a rule with a proven false-positive mode and no proven population. That is the exact thing
  the Phase 0 spike exists to prevent.

### Open questions
- Run the two Phase 0 queries on Keegan's, Stephen's and Luke's catalogs, then either build Phase 4
  or strike it. Both queries are recorded verbatim in the design doc (P2 section).

## Phase 1: Stop the code lying about rule 3

### Design decisions
- `collect_layouts`'s `debug!` lost its only interesting value when `repo_root` was dropped, so it
  now logs `work_remote_hosts.len()` rather than nothing -- `efficiency/src/collect.rs:129`. Same
  for `reindex_efficiency` (`efficiency/src/persist.rs:115`). A count, not the hosts themselves:
  the allowlist is operator config, but a log line is not where it needs to be enumerated.
- The four stale comments were replaced with what is TRUE now, not deleted. Each one names why the
  old claim went stale (v0.23.0, Problem 5) so a reader who finds the old wording in git history
  can tell it was retired deliberately.
- `common/src/repo.rs:833` gained an explicit negative: "Nothing in `efficiency` calls this, so
  nothing there constrains its signature." The doc calls this comment the worst of the four
  because it invents a coupling an implementer would work to preserve, and Phase 2 is about to
  change that exact signature. Stating the absence is what stops the next reader re-deriving the
  phantom constraint.

### Deviations
- The doc names four comments plus the dead parameter. AC1 (`rg -c 'repo_root'
  efficiency/src/collect.rs` returns no match) forced two MORE sites in the same file that the
  bullet list does not enumerate: the `CollectedSession` doc at `:31` ("which passes a
  `repo_root`") and the `build_session` body comment at `:223` (`outcomes_repo_root` is `Some`,
  naming a parameter that has not existed since v0.23.0). Both are the same defect class the phase
  is for, and AC1 is unsatisfiable without them.

### Tradeoffs
- Drop the `repo_root` parameter vs. leave it and fix only the comment. Dropped, per the phase
  bullet. It was read exclusively by its own `debug!`, so it was a parameter that existed to be
  logged, and threading a value through two crates to print it is how the stale docstring stayed
  plausible for a release.

### Open questions
- None.

## Phase 2: `repo-roots`, a list

### Design decisions
- **The expanded root list holds BOTH spellings, deduped, in configured order** --
  `common/src/config.rs:de_repo_roots`. The doc says roots are canonicalized at load and that a cwd
  matches either spelling; it does not say where the second spelling lives. Putting both in the one
  `Vec<PathBuf>` the API design already specifies (`repo_roots(&self) -> &[PathBuf]`) means every
  matcher stays a plain "is this path under any of these" loop with no symlink logic of its own. For
  a root not reached through a symlink the two coincide and dedupe to one entry, so the common case
  returns exactly what was configured.
- **Duplicate roots get their own error message, distinct from the nesting one** --
  `de_repo_roots`. `starts_with` is true in both directions for equal paths, so one message would
  have read "X is inside X", which reads as a bug in the checker rather than as a config error.
- **`slug_under_roots` and `Anchors::org_slot` both break ties by ROOT COMPONENT COUNT**, not string
  length -- `common/src/repo.rs:841`, `session/src/scope.rs`. A root with more components is the
  deeper one regardless of how its names are spelled.
- **`canonicalize()` failure falls back to the configured spelling** rather than erroring. The
  `is_dir` check immediately above already succeeded, so a failure here is a race (the directory
  vanished between the two calls) and the configured path is the honest answer: it is what was asked
  for.
- **Doctor prints one line per root**, not a folded verdict -- `clyde/src/doctor.rs:print_attribution`.
  With two roots a single "exists" line cannot say WHICH one exists, so a teammate whose second root
  is typo'd would read a healthy line. The `<org>/<repo>`-presence note is per root too, because one
  root can be inert while another is not.

### Deviations
- The doc's API sketch shows `pub fn repo_roots(&self) -> &[PathBuf]` and nothing about how the
  symlink twin is carried. Returning the expanded list means `doctor` prints BOTH spellings for a
  symlinked root. That is honest (both really are roots for matching purposes) but it is more output
  than the doc's "prints each root" implies.

### Tradeoffs
- Reject a duplicate/nested pair vs. dedupe silently. Rejected loudly, per the doc's decision on
  nesting, and extended to exact duplicates by the same reasoning: a config that says something
  twice is a config the operator should look at.
- Keep a `slug_under_root` singular alongside the plural. Dropped: two entry points onto one shape is
  how the two drift, which is the exact lesson the four stale comments in Phase 1 taught.

### Open questions
- None.

## Phase 3: The anchor reads the roots

### Design decisions
- **`bare_work_org_is_an_org_dir` matches `ProbeOutcome` with NO wildcard** --
  `session/src/scope.rs`. Six arms, one per variant, each with its own comment, so a seventh variant
  is a compile error. The doc asks for the rule to be "asserted as an exhaustive table"; a wildcard
  arm would let a new variant silently join the fail-closed group, which is the safe direction but
  hides the decision.
- **`RoutingFacts.repo_probe` is `Option<&'a ProbeOutcome>`, borrowed, not owned.** `RoutingFacts`
  derives `Copy` and `ProbeOutcome::Resolved` carries two `String`s. The caller
  (`sessions::routing::classify_row`) owns the parsed value for the row.
- **`ProbeOutcome::from_stamp` accepts ONLY the two conclusive-negative tokens** --
  `common/src/repo.rs`. `Db::record_probe` enforces at the write that nothing else is ever stored, so
  a `resolved`/`blocked`/`indeterminate` stamp means a hand-edited catalog. `Resolved` cannot even be
  reconstructed (the stamp carries no slug or host), and accepting `blocked` would let a transient
  environment failure anchor a routing decision. `sessions::routing::parse_repo_probe` wraps it with
  the same loud warning `parse_repo_source` has.
- **The two v3 anchor branches collapsed into ONE call to `Anchors::scope_of`.** Splitting the work
  and personal verdicts across two `if` blocks is what let `<root>/<repo>` fall into the personal arm
  on a path shape that says nothing at all.
- **`anchor_disagrees_with_remote` passes NO probe**, so the bare-work-org shape never reports a
  disagreement -- `session/src/scope.rs`. It is the one anchor verdict that is not a path fact, so
  calling it a disagreement would report a conflict between the remote and a verdict the remote
  itself helped decide.
- **`EnrichOptions::default()` and `Anchors::default()` carry an EMPTY root list.** A caller that
  forgets to set the roots loses cwd-anchored coverage; it does not gain scope. Unlike
  `work_remote_hosts`, whose default of `["github.com"]` keeps existing callers correct, there is no
  root value that is right for an arbitrary caller, and guessing `<home>/repos` would silently anchor
  against the maintainer's own layout.
- **`ExportEnvelope::scope_version` is `#[serde(default)]`.** Additive means additive in both
  directions: an envelope stored before the field existed must still deserialize, or "no schema bump"
  is a claim the code contradicts. The default `0` is not a real classifier version, which reads
  correctly as "this envelope predates the field".

### Deviations
- **Export's SELECT gains a FOURTH column, `outcome_json`, beyond the three the phase names.** The
  phase bullet lists `repo_source`, `repo_probe`, `repo_host`. Without `outcome_json`,
  `classify_row`'s touch-set branch reads an empty evidence set inside export while the gate reads
  the real one, so a row with a unanimous work touch set and no stored scope would export `personal`
  and gate `work`. That is precisely the two-answers-to-one-question defect this phase exists to
  remove, so the column is threaded. The cost objection the doc raised is answered by paying it
  LAZILY: the column is selected (free) and parsed only inside the fallback arm, which on a gated
  catalog is no rows.
- **`session::classify` (the cwd-only classifier) is DELETED, not left beside the new one.** The doc
  says export stops running a second classifier; leaving the function in place would have left a
  `pub` entry point still implementing the retired literal-`repos` rule for the next caller to find.
- **Four Matrix fixture rows were added in Phase 3, not Phase 5.** `flat_under_root`,
  `flat_under_root_bad_host`, `work_org_named_repo` and `empty_repo_named_work_org`
  (`common/src/checkout.rs`). Phase 5 is the phase that adds matrix rows, but the Testing Strategy
  says every shape in the doc goes in `checkout.rs` as a real-`git` fixture, and Phase 3's own
  success criteria name four shapes that had no fixture. Building them inline in `scope/tests.rs`
  would have been a second fixture mechanism.
- **`work_org_named_repo` and `empty_repo_named_work_org` each get their OWN alternate root**
  (`<home>/altroot`, `<home>/altroot2`). `<repo-root>/tatari-tv` is already the org DIRECTORY in the
  fixture, and one path cannot model three occupants at once.

### Tradeoffs
- Store the full `ProbeOutcome` in `sessions.repo_probe` vs. parse the existing stamp. Parsed the
  existing stamp: widening what `record_probe` writes would turn a transient environment failure into
  a permanent record, which is the severest finding the v3 review panel raised. The stored data
  already distinguishes `NotARepo` from `NoOrigin`, which is exactly what the anchor needs.
- Thread `Anchors` through `classify_with_evidence` as a parameter vs. make it a field on
  `RoutingFacts`. Parameter, per the doc's API sketch: `RoutingFacts` is per-ROW state and `Anchors`
  is per-RUN config, and merging them invites constructing it per row -- the exact cost the doc says
  to avoid.

### Open questions
- **A SUBDIRECTORY of a flat clone still anchors Personal.** `<root>/clyde/src` is shape-identical to
  `<root>/<org>/<repo>`, so the org slot reads `clyde` and the repo slot reads `src`. No rule reading
  the path alone can separate them, so P3's fix covers a session that ran AT a flat clone's root and
  not one that ran below it. Asserted explicitly in
  `a_flat_repo_under_a_root_is_unanchored_so_the_remote_can_answer` so it is a stated limit rather
  than a silent gap. Closing it would need the anchor to consult the probe for the non-work-org case
  too, which is a design change, not an implementation choice.

## Phase 4: Rule 5, the learned name map

### Design decisions
- None. NOT BUILT.

### Deviations
- **Phase 4 is not implemented.** It is gated on Phase 0, and Phase 0 could not run: it needs
  `sessions.db` from Keegan's, Stephen's and Luke's machines, none of which is reachable from
  desk.lan. The doc states the gate up front. Building rule 5 anyway would ship a rule with a proven
  false-positive mode (`/tmp/clyde` -> `tatari-tv/clyde`) and zero measured recoveries anywhere,
  which is precisely what the Phase 0 spike exists to prevent.
- **AC4 therefore FAILS**, and it is a sound criterion, not a doc defect: `clyde doctor` prints no
  `name-guess` line because nothing writes that source. It is not amended. It stays failing until
  Phase 0 runs and Phase 4 is either built or struck.

### Tradeoffs
- None taken; the phase was not entered.

### Open questions
- Same as Phase 0's: run the two recorded queries on the three teammate catalogs, then build Phase 4
  or strike it. Until then `RepoSource` has four variants, not five, and rule 4 remains the last rule
  in the chain.

## Phase 5: The two missing matrix rows

### Design decisions
- **The warning is produced by a named function, `unusable_marker_warning`
  (`common/src/repo.rs`), not by an inline `warn!`.** Both diagnoses return `Indeterminate`, so the
  OUTCOME is blind to which one was produced and an assertion on it cannot bite. The message is the
  only observable difference and it is the part that was wrong, so it has to be the thing the test
  reads. Returning a `String` makes that a plain equality assertion rather than a log-capture
  harness.
- **`orphaned_worktree_target` returns `Some` only when the pointer EXISTS and its target does
  NOT.** An ordinary linked worktree whose main checkout is intact declines, so the generic
  `safe.directory` diagnosis stays available for the case it was actually written for. Asserted by
  `an_intact_linked_worktree_is_not_reported_as_an_orphan`, which is the guard against the orphan
  branch swallowing the generic one.
- **Row 34's fixture builds its own repo and deletes it** (`common/src/checkout.rs`), rather than
  orphaning an existing row's worktree. Deleting a shared checkout would silently change what every
  other row asserting against it observes.
- Row 33 is asserted at BOTH altitudes: the probe directly (`common/src/repo/tests.rs`) and end to
  end through the real gate (`clyde/tests/matrix.rs`). The unit test names the deletion that breaks
  it; the integration test proves the composition reaches a `work` decision, which is the thing
  Keegan actually reported.

### Deviations
- None. The two rows are the two the phase names.

### Tradeoffs
- Assert the warning text vs. assert only the outcome. Text, per the phase's own success criterion.
  It couples the test to a message string, which is normally a smell; here the message IS the fix, so
  a test that ignored it would pass against the bug.

### Verification: every new test was proven to bite
Each was run with its production branch deleted, and each failed:

- Row 33: early-return `Err(NotARepo)` from `no_work_tree_root` ->
  `matrix_row_33_a_container_root_under_an_off_layout_root_resolves` fails,
  `left: None, right: Some("tatari-tv/airflow-dags")`.
- Row 34: replace `orphaned_worktree_target(cwd)` with `None` ->
  `matrix_row_34_an_orphaned_worktree_is_diagnosed_as_itself` fails on
  `the warning must name the orphan: ... check safe.directory and .git permissions`.
- AC6: restore the v3 cwd-only classifier as export's fallback ->
  `export_scope_fallback_agrees_with_the_enrich_gate_for_every_cwd_shape` fails with
  `export and the gate disagree for cwd /home/saidler/repos/scottidler/repos/tatari-tv/x`,
  `left: "work", right: "personal"`.

### Open questions
- None.

## Acceptance criteria: results

Run against the branch at Phase 5, with a debug binary built from it.

- **AC1 PASS.** `rg -c 'repo_root' efficiency/src/collect.rs` -> no match (exit 1). Was `8`.
- **AC2 PASS.** `rg -n 'pub const SCOPE_VERSION' session/src/scope.rs` -> `71:pub const SCOPE_VERSION: i64 = 4;`
- **AC3 PASS.** `rg -c 'repo_roots' common/src/config.rs` -> `9`. And live, with a `clyde.yml`
  carrying the old key: exit 1, ``failed to load clyde config: ... `repo-root` is now `repo-roots`, a
  list: replace `repo-root: /path` with `repo-roots: [/path]` ``. The two sibling load errors were
  exercised the same way: `repo-roots: []` -> "must name at least one root"; a nested pair ->
  "entries must not nest, but /home/saidler/repos/tatari-tv is inside /home/saidler/repos", naming
  both.
- **AC4 FAIL.** `clyde doctor` prints no `name-guess` line under `resolved by:`. Phase 4 is unbuilt
  (see above). The criterion is SOUND and is left failing rather than amended.
- **AC5 PASS.** `Matrix::offlayout_container_root` and `Matrix::orphaned_worktree` are named fields,
  and both tests were proven to fail with their production branch deleted (evidence above).
- **AC6 PASS.** `export_scope_fallback_agrees_with_the_enrich_gate_for_every_cwd_shape` drives both
  export and `routing::classify_row` over three cwd shapes and was proven to fail against a restored
  cwd-only fallback (evidence above).

Live `clyde doctor` on the maintainer's catalog, with two roots configured, prints one line per root:

```
  repo-roots:    /home/saidler/repos
                 /tmp
```

## Implementation audit, 2026-08-01 (review-panel: Gemini Architect + Codex Staff Engineer)

The two seats SPLIT on the probe-parsing focus area. Gemini returned PASS; Codex returned two High
findings there. **Codex is right and Gemini's PASS was a syntax read** -- the Gemini seat is
plan-mode and cannot run a shell, so its "verified" claims are reads of the source, not executed
checks. Both High findings were confirmed against the code by hand before being accepted.

### Folded: the two High findings, one root cause

Both are the same mistake seen twice: **this branch changed a security-relevant signal's TYPE and
silently dropped half of it.** `RoutingFacts.repo_probe` went from `Option<&str>` (PRESENCE of a
recorded negative) to `Option<&ProbeOutcome>` (its CONTENT). Presence and content answer two
different questions, and the refactor collapsed the first into the second.

- **An unreadable stamp stopped refusing a work slug.** v3 keyed the git-origin guard on the column
  being non-NULL, so ANY stored value refused. v4's first form keyed it on the parsed outcome, and
  `parse_repo_probe` yields `None` for anything it cannot read -- so a hand-edited or forward-dated
  stamp read as "nothing recorded" and the slug was granted Work. A leak introduced by the change
  meant to make the signal MORE precise. The doc comment on `parse_repo_probe` even claimed this was
  "the fail-safe direction", which was false.
- **`ProbeOutcome::from_stamp` accepted values `Db::record_probe` can never write.** It used
  `split_once('@').map_or(stamp, ...)`, so a bare `not-a-repo` with no timestamp parsed. The writer
  emits `<token>@<rfc3339>` and nothing else, so a bare token is a value this binary never produced,
  and accepting it let an edited catalog hand the bare `<root>/<work-org>` anchor a `NotARepo` it
  never observed. The `@` half is now REQUIRED: the parser matches the persisted contract rather than
  the token prefix.

**Fix:** `session::RecordedProbe`, a two-variant type carrying both signals in ONE field so they
cannot diverge -- `Negative(&ProbeOutcome)` when the stamp parsed, `Unreadable` when it did not.
Both variants REFUSE a git-origin work slug (presence is that signal). They differ only at the bare
`<root>/<work-org>` anchor, where a readable `NotARepo` is the one thing that grants Work and
`Unreadable` defers like every other outcome. Two fields would have been settable inconsistently at
a call site; the enum makes the inconsistent state unrepresentable.

### Folded: two coverage gaps

- **The `outcome_json` deviation had no biting test** (Codex). Its whole justification was "otherwise
  a unanimous-work touch set with no stored scope exports personal and gates work", and AC6's test
  drove three cwd shapes without ever storing a touch set -- so dropping the fourth column would not
  have falsified the rationale. The deviation was argued, not asserted.
  `export_reads_the_touch_set_so_its_fallback_cannot_diverge_from_the_gate` now asserts it, and was
  proven to fail with the column withheld (`left: "personal", right: "work"`).
- **The doc's `~/code/repos/tatari-tv/x` row was unasserted** (BOTH seats, independently). A literal
  `repos` component INSIDE a configured off-layout root is an ordinary org slot named `repos`, not an
  anchor. Now pinned by `a_repos_component_inside_an_off_layout_root_is_an_ordinary_org_slot`.

### Folded: the two doc findings

- The Resolved Decision reading "rule 5 ships despite recovering zero sessions here" was stale
  relative to what shipped. Corrected to record that the decision was always conditional on Phase 0,
  that Phase 0 never ran, and that rule 5 is therefore neither built nor struck.
- "Each entry must be absolute and exist" overstated the invariant: the code deliberately does not
  validate or canonicalize the DEFAULT `[<home>/repos]`, exactly as the singular default was not
  validated. Reworded to "each CONFIGURED entry", in both the P1 section and Security.

### PUSHED BACK: the bare-work-org shape reaching Work via the touch set

Codex, Medium: when the anchor abstains for `<root>/<work-org>` (`NoOrigin`, `Indeterminate`,
`OutsideRoot`, `Blocked`, or nothing recorded), that same cwd can still reach Work through the
touch-set branch, which the doc's table describes as "fail closed, Personal".

Not changing the code, for three reasons:

1. **The table's column is the ANCHOR's verdict, not the classifier's final one.** Its `Resolved` row
   reads "defer to git-origin, guards apply", which is explicitly a deferral rather than a verdict,
   so the column is already understood as "what the anchor does". "Fail closed, Personal" is what the
   fail-safe default yields absent OTHER evidence.
2. **It is not a widening.** Measured against v3: `has_work_org("<root>/tatari-tv")` matched the
   `repos`/`tatari-tv` adjacency and returned Work, SETTLED, without the touch set ever being
   consulted. Every one of these rows is therefore narrowing-or-equal relative to what shipped in
   v0.24.0. A leak-direction finding requires a row that gains Work, and none does.
3. **The touch-set branch is not shape-specific and must not become so.** It applies to every
   unanchored cwd, requires unanimity AND totality (`sum == files_edited`), and a session that
   provably edited only work-repo files is work by the rule's own design. Special-casing one path
   shape out of it would make the evidence branch read the path, which is the coupling this whole
   design removes.

Folded instead: the doc's P3 table now says explicitly that the column is the anchor's verdict, so
the wording cannot invite Codex's reading again.

## Mutation gate: run, red, fixed, green

`otto mutants` is this repo's zero-survivor gate on the routing path. It is deliberately NOT part of
`otto ci`, and it was **not run once across the four phases that rewrote `session/src/scope.rs`** --
`otto ci` ran twelve times and the gate that checks whether tests BITE ran zero times. That is the
process failure behind the leak-direction regression the implementation audit found.

### First run: killed, not failed

`--jobs 8` on a 32-core host produced a load average of 167. `--jobs N` bounds how many mutants are
tested at once and says nothing about what each test binary then does; `efficiency` links rayon,
whose pool defaults to `nproc`, so 8 x 32 = 256 runnable threads. That is not merely slow: `.otto.yml`
picked its 300s timeout as "roughly 15x the idle workspace suite, which absorbs the parallelism",
calibrated for 8x and not 256x, so the run was drifting back into manufacturing the timeout artifacts
that comment exists to prevent. Killed and fixed at the config (`f22c8ba`), deriving
`RAYON_NUM_THREADS` as `cores / jobs` so the two numbers cannot drift apart. Re-run: 11 minutes,
temps under threshold.

### Second run: 10 survivors, FOUR of them in code written that day

```
337 mutants tested in 11m: 10 missed, 164 caught, 163 unviable
```

| Survivor | Why nothing caught it |
|---|---|
| `from_stamp`, both match arms | No test AT ALL. The `RecordedProbe` tests construct a `ProbeOutcome` directly, so nothing drove the parser -- the very function just rewritten for a security finding was unverified |
| `RecordedProbe::token` -> `""` / `"xyzzy"` | Log-only method; nothing asserted its strings |
| `Anchors::org_slot` and `slug_under_roots`, `>` -> `<` / `==` / `>=` | The depth comparison had one test each, and neither distinguished shallowest-wins from deepest-wins |
| `Scope::from_stored`, both arms | Pre-existing. Driven end to end by `sessions`, never by its OWN crate -- the v0.23.0 `scope_override` lesson repeating verbatim |

### The `>=` mutants were unkillable, and were designed out rather than annotated

No reachable input has two DISTINCT roots of equal depth both matching one path, so `>` and `>=` are
behaviorally identical and no test can separate them. `.otto.yml` permits an annotated skip for a
genuinely unavoidable survivor, but cargo-mutants skips whole FUNCTIONS, which would have suppressed
the `<` and `==` mutants that ARE killable and that guard a real behavior.

Both sites were rewritten as `max_by_key`. Expressing "deepest wins" as an ordering key deletes the
operator, so there is nothing left to mutate and no skip to justify. Mutant count dropped 337 -> 331,
which is exactly the six operator mutants (three per site, two sites) ceasing to exist.

### Third run: clean

```
331 mutants tested in 11m: 168 caught, 163 unviable
finished successfully
```

Zero unannotated survivors, which is the threshold the design doc's Testing Strategy requires
("Mutation threshold stays zero, per the v0.23.0 decision"). Note that the background-task wrapper
reported exit 0 for the FAILING run too; the authority is the log line and otto's own exit code, not
the wrapper's.

### The lesson worth keeping

`otto ci` answers "does the code work". `otto mutants` answers "would we notice if it stopped". Four
phases of green CI hid a work-scope leak and a completely untested parser. The gate belongs in the
per-phase loop for any change to the six files it covers, not only before a release.
