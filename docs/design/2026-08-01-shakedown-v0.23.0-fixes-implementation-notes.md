# Implementation Notes: Shakedown v0.23.0 Fixes

Running record for `docs/design/2026-08-01-shakedown-v0.23.0-fixes.md`. Append-only: a later
decision that overrides an earlier one gets a NEW entry that supersedes it, never an edit in place.

## Phase 1: One error renderer for the whole binary

### Design decisions

- **`main` splits into `parse_cli` + `dispatch`** -- `clyde/src/main.rs:52-114`. The doc said "`main`
  becomes `fn main() -> ExitCode`, catching what it used to return", and separately that "errors
  raised before the log level is known (a clap `from_arg_matches` failure) render with
  `debug = false`". A single monolithic fallible body cannot express both: the `debug` flag is only
  knowable after `Cli` exists. Splitting at exactly the point argv is parsed makes the rule
  structural rather than a comment -- `parse_cli`'s `Err` arm has no `Cli` in scope, so it CANNOT
  render at anything but `debug = false`.
- **`format_error(&Report, bool) -> String` extracted alongside `render_error`** --
  `clyde/src/main.rs:213-218`. The doc specified `render_error(e, debug)` and two tests asserting
  what it "emits". `render_error` writes to stderr, which is not assertable without capturing a
  process's fd. `format_error` is the rendering; `render_error` is the thin `eprintln!` shim over it.
  Behavior is byte-identical to the inlined `dispatch_tool` branch it replaces.
- **`debug_rendering(&Cli) -> bool` added** -- `clyde/src/main.rs:236-238`. `main` and `run` both
  need the flag, and both resolved it as
  `is_debug_level(cli.log_level.as_deref().unwrap_or(DEFAULT_LOG_LEVEL))`. Two copies of that
  expression is two signals encoding one meaning, which is the defect class this doc exists to fix.
  One function, called from both.
- **`setup_logging` takes `&Path` instead of `&PathBuf`** -- `clyde/src/main.rs:1381`. Mechanical
  consequence of `dispatch` receiving the log path by reference; the old signature only worked
  because `main` owned the `PathBuf`.

### Deviations

None. The three rendering paths named in the doc (`dispatch_tool`, the `update` arm, `main`) all
route through `render_error`, the `Error: ` prefix is dropped as the doc directed, and the `update`
arm keeps renew's documented exit 2.

### Tradeoffs

- **`eyre::Report::new(e)` in the `update` arm** vs. changing `render_error` to accept
  `&dyn std::error::Error`. `renew::Error` is a `thiserror` enum, not a `Report`. Wrapping it costs
  one allocation on a path that is about to exit the process, and keeps `render_error`'s signature
  the `&eyre::Report` the doc specified -- which matters because the alternate-Display `{e:#}` cause
  chain is an eyre feature, not a `std::error::Error` one. Widening the parameter would have
  silently degraded that arm's rendering to plain Display, which is the third form being deleted.

### Open questions

None.

## Phase 2: An override re-offers its row

### Design decisions

- **`Db::enriched_at_of` added** -- `sessions/src/db/routing.rs:118`. The doc required the
  `--set personal` already-enriched warning but named no accessor for the state it warns about.
  Modelled exactly on the adjacent `Db::probe_of`, which is the precedent the same CLI block already
  uses for the conclusive-negative warning: presence is the signal, the timestamp is for the
  operator reading the message. An absent session returns `Ok(None)`, never an error.
- **Five tests became six.** The doc named five directions; `enriched_at_of` is new production code
  and gets its own test rather than being covered only incidentally through the CLI.

### Deviations

- **The doc's break-it recipe was imprecise, and the recorded evidence corrects it.** It said "drop
  the `scope_version = NULL` clause and watch 1 and 2 fail". Measured: dropping it from
  `set_scope_override` fails ONLY test 1. Test 2 (`--clear` with an override present) never reads
  `set`'s clause -- its `record_enrich_skip` call re-stamps `scope_version` afterward regardless, so
  the row's re-offer depends entirely on `clear_scope_override`'s `CASE`. Each of the three clauses
  was broken independently and each bites exactly one test:
  - `set`'s `scope_version = NULL` removed -> test 1 only
  - `clear`'s `CASE` clause removed -> test 2 only
  - `clear`'s write made unconditional -> test 3 only

  This is a tighter property than the doc claimed (one clause, one failing test, no overlap), not a
  weaker one. No code changed as a result.

### Tradeoffs

- **`CASE WHEN` in the UPDATE** vs. `AND scope_override IS NOT NULL` in the `WHERE`. Taken as the
  doc specified, and test 3 asserts the reason: the `WHERE` form would flip `Ok(n > 0)` from "session
  exists" to "an override existed", changing what the CLI's "no session matches" path means. The
  test calls `clear_scope_override` on an override-free row and asserts `true` -- pinning the return
  semantics, not just the `scope_version` value.

### Open questions

None.

## Phase 3: `doctor` counts decisions, not conditions

### Design decisions

- **`sessions/src/routing.rs` is new, and `enrich::enrich` was rewired through it.** The doc said
  `routing_summary` should assemble `RoutingFacts` "the same way `sessions/src/enrich.rs:150` does".
  Written as a second copy of those 35 lines, "the same way" is a comment that drifts -- which is
  precisely the finding: `doctor` answered the same question its own way and drifted. So the step is
  ONE function, `routing::classify_row`, and both the enrich gate and `routing_summary` call it. P2's
  central claim ("the count IS the classifier") is then a property of the code.

  This does touch the enrich hot path, which Phase 3 otherwise does not. It is guarded by the
  existing `sessions/src/enrich/tests.rs` suite, including
  `a_scope_override_beats_a_refusal_in_both_directions`, all of which stayed green.
- **Register item 6's loud `repo_source` parse moved with it**, into `routing::parse_repo_source`.
  Behavior is unchanged (warn, name the `--reresolve-repo` remedy, classify without the remote). The
  log line's prefix changed from `enrich::enrich:` to `routing::parse_repo_source:`; the `log` crate
  records the module path either way.
- **`classify_row` returns `RowDecision { decision, repo_source }`.** The enrich site needs the
  parsed provenance for its anchor-disagreement warning. Re-parsing at the call site would emit
  `parse_repo_source`'s warning TWICE for one corrupt row, reading as two corrupt rows.
- **`Db::routing_rows` + `evidence_from_row`** -- `sessions/src/db/enrich.rs:110`. The batch form the
  doc called for, and the per-session `scope_evidence` was refactored to share the same parse rather
  than sit beside it. That is what makes the malformed-blob degradation rule provably identical
  between the enrich gate and the `doctor` scan instead of coincidentally identical.
- **`basis_remedy` lives in `sessions`, next to `basis_index`/`basis_label`** rather than in
  `clyde/src/doctor.rs`. First draft kept the remedy strings at the print site and matched them to
  counts by label lookup; that reintroduces the exact silent-drop failure the exhaustive-match rule
  exists to prevent, one column to the right -- a seventh `Basis` variant would ship with a blank
  remedy. Three exhaustive matches over `Basis`, so a new variant is a compile error at all three.
- **`RoutingSummary::decisions_total()` and a printed `(total)` line.** AC4 asserts the decisions
  group sums to the row count. Making the total a method rather than leaving it for the operator to
  add up in their head is what makes the invariant readable at 3am, and it is what Phase 3's test 1
  asserts against a seeded catalog.
- **`routing_summary_with` added alongside `routing_summary`.** Test 5 requires injecting a
  `HostResolver` fake. `routing_summary` stays the production entry and is one line:
  `self.routing_summary_with(&mut HostPolicy::new(work_remote_hosts))`.
- **Nine tests, not five.** The doc's five, plus: `BASIS_ORDER`/`basis_index` bijection (a count
  printed under the wrong label is the same class of defect), the `override`-vs-SQL cross-check the
  doc names as available on day one, malformed `outcome_json` does not abort the scan, and an
  unreadable `repo_source` classifies without the remote. The last two are the doc's stated
  degradation rules, which were otherwise asserted nowhere.

### Deviations

- **The doc's break-it recipe named test 2; the measured break also bites test 4.** Modelling the old
  SQL (`repo_probe IS NOT NULL` -> `ProbeRefused`) fails BOTH
  `a_probe_stamp_under_a_work_anchored_cwd_counts_as_cwd_anchor` and
  `a_row_carrying_both_refusals_counts_as_host_refused`. Stronger than claimed, not weaker.
- **Test 5's null-resolver break could not be done by swapping in `NullResolver`:**
  `#![deny(dead_code)]` rejects the then-unused `FakeResolver` before the test can run. Done with an
  EMPTY alias table instead, which is the same behavior. Result: `HostRefused` where the gate says
  `GitOrigin` -- exactly the divergence the panel predicted.

### Tradeoffs

- **Live-catalog numbers have MOVED since the doc was written**, because the catalog grew. Observed
  today on a fresh copy: 2186 rows (doc: 2184), `probe-recorded` 345 (doc: 326), well-formed work
  slugs 1075 (doc: 1073). Every criterion still passes because AC3 and AC4 are stated as
  EQUALITIES, not fixed numbers -- which is exactly why the doc wrote them that way. Verified today:
  - AC3: doctor's `probe-refused` decision count `0` == the independent SQL measure `0`; the old
    condition count on the same catalog is 345, so `main`'s line would read 345-vs-0
  - AC4: decisions total `2186` == `SELECT COUNT(*) FROM sessions` `2186`
  - `override` basis count `0` == `SELECT COUNT(*) ... WHERE scope_override IS NOT NULL` `0`
  - `probe-recorded` still reported, at 345
- **A full table scan plus per-row classification** replaced six `COUNT(*)` queries. Measured on the
  2186-row live catalog copy: `doctor` returns without perceptible delay, and the allowlist has one
  distinct literal-matching host so the run spawns no `ssh` at all. `doctor` already spawns up to 64
  `git` subprocesses per run; this is not the cost.

### Open questions

None.

## Phase 4: Export honors the real classification, `schema-version` -> 2

### Design decisions

- **The four Phase 4 tests live in `sessions/src/db/query/tests.rs`, not
  `sessions/tests/export.rs`.** They were drafted in the integration test, which cannot reach
  `db.conn` (it is a separate crate) and so cannot seed a stored `scope` without either a new public
  test hook on `Db` or driving a full LLM-stubbed enrich per case. The precedence is implemented in
  `build_export_record`, so pinning it at the same layer is also the right call on its own merits --
  the same argument the doc makes for putting the `routing_summary` assertions at the
  `routing_summary` layer.
- **Four tests, covering all three precedence steps plus AC5's invariant.** The doc named no Phase 4
  tests; these are what makes AC5 falsifiable in CI rather than only by hand against the live
  catalog:
  - step 2: a stored `work` on an unplaceable cwd exports `work` (this IS P4)
  - step 1: an override beats the stored scope in BOTH directions
  - step 3: a never-processed row still exports a contract token, never an empty string
  - AC5's form: emitted scope == `COALESCE(scope_override, scope, <cwd rule>)` for every row, zero
    disagreements
- **The v1 backward-parse case was KEPT and a v2 case ADDED**
  (`sessions/src/export/tests.rs`), as the doc directed: emitting 2 and refusing to parse 1 are
  different promises. The v1 case's comment now says why it is pinned at 1 so a future reader does
  not "helpfully" bump it.
- **`export_schema_version_stays_one_after_efficiency_block` renamed, not deleted.** It asserted a
  real historical fact (schema v6's efficiency block rode the envelope additively and did NOT bump
  the version). The renamed
  `export_schema_version_is_two_and_only_the_scope_derivation_bumped_it` keeps that fact in its doc
  comment and keeps the guard the original provided: bumping the version for a merely additive change
  still fails it.
- **`docs/session-export-contract.md` gained an explicit "a change to what a field MEANS is also a
  major bump" clause.** Without it the doc's own promise (which enumerates renames, removals and type
  changes) reads as PERMITTING this change without a bump -- which was exactly my original
  recommendation, and Scott overrode it. The clause is what makes the override the documented rule
  rather than a one-off exception.

### Deviations

- **No golden fixture's `scope` needed re-baselining, and the doc's instruction to inspect the diff
  row by row is what established that.** The five `sessions/tests/fixtures/export/*.json` files are
  hand-authored ROUND-TRIP pins: the test deserializes each into `ExportEnvelope` and re-serializes,
  comparing structurally. They never run `build_export_record`, so the derivation change cannot move
  their `scope` values. Only `schema-version` changed in each, 1 -> 2, so the fixtures stop claiming
  a contract version the producer no longer emits. Verified: `rg -c '"schema-version": 1'
  sessions/tests/fixtures/export/` returns no matching files (AC5's third bullet).

### Tradeoffs

- **`raw.scope_override.or(raw.scope).unwrap_or_else(cwd rule)`** vs. running the full
  `classify_with_evidence` at export time. Taken as the doc specified. The full classifier needs five
  more columns plus an `outcome_json` parse per row on the bulk paged endpoint whose whole point is
  being cheap, and it would make export a THIRD site re-implementing the routing decision. Reading
  what the gate already decided cannot drift from the gate.
- **`deny(dead_code)` turned the regression into a compile error**, which is a stronger guard than the
  tests: restoring `session::classify(cwd_path)` as the only source makes `raw.scope` and
  `raw.scope_override` unread and the crate stops building. To observe the runtime failures the
  fields had to be explicitly discarded first; with that done, three of the four tests fail.

### Verified against the live catalog

- 31 rows have stored `scope='work'` with no `repos/tatari-tv/` cwd anchor -- exactly the count the
  doc recorded. One sampled (`03640da6`, cwd `/home/saidler`) now exports `"scope":"work"`; on `main`
  it exported `"personal"`.
- Full-catalog export: 1911 records, every envelope `"schema-version": 2`, and **0** disagreements
  with `COALESCE(scope_override, scope, <cwd rule>)`. That is AC5's falsifiable form, at zero.

### Open questions

None.

## Phase 5: Correct the runbook diff

### Design decisions

- **A new section `4b` for the export contract**, placed between the `doctor` section and the config
  key. The doc asked for "a runbook section for the export contract" without naming a position; after
  Phase 4 the natural reading order is decisions -> conditions -> what leaves the machine, and the
  consumer action belongs next to what changed rather than appended after unrelated config notes.
- **The `doctor` section explains WHY the two groups exist, with the 326-vs-0 number.** The doc's
  success criterion only required the split plus "a refusal count is a count of decisions". An
  operator who screenshotted the old numbers needs to be told they were wrong, not just handed a new
  table -- the doc's own risk table lists "P2 changes numbers operators have already screenshotted"
  and names Phase 5 as the mitigation, so the explanation IS the mitigation.
- **The `--set personal` already-enriched warning is documented** in section 3, alongside the
  conclusive-negative warning it mirrors. It is new operator-visible behavior from Phase 2 and the
  runbook is where operator-visible behavior is described.
- **The `attempts`-cap limitation is stated** in section 3. The design doc records it as a known
  limitation deliberately not fixed; an operator whose override appears to do nothing on a
  retry-exhausted row would otherwise have no way to know that is expected.
- **`docs/shakedown-v0.23.0.md` gained a banner plus three per-finding resolution lines**, and its
  findings are untouched, as the doc required. The banner exists because a reader landing on the
  Findings header should not have to scroll three sections to learn the state.

### Deviations

- **I pre-named `v0.23.1` in the runbook draft on the first pass and corrected it.**
  `rules/general.md` forbids embedding an expected release version in doc content as a prediction --
  a skipped release or a different bump level makes it wrong, and readers grepping the version hit the
  wrong doc. Replaced with "the release that lands these fixes", which is the form the rule permits.
  The design doc's own Non-Goals already put the actual marquee publish outside this work.

### Tradeoffs

- **`--all` is still MENTIONED in the operator section, as a warning not a remedy.** The success
  criterion says the section "names no `--all` workaround". Silently deleting it would leave operators
  who learned the old workaround from the published runbook with no signal that it is both unnecessary
  and destructive. It is named only to say do not use it, and why.

### Verified

- The doc's sweep of every place the broken remedy is advertised was re-checked against the tree and
  all four conclusions still hold: the `attribution-and-routing` design doc and its implementation
  notes are point-in-time and were LEFT alone; `clyde/src/cli.rs:138` ("Force this session's scope,
  beating every classification rule") needed no change and is accurate now that Phase 2 has landed;
  `README.md` does not mention `scope`; there is no `CLAUDE.md` in this repo.
- No em-dashes in any of the four touched docs.

### Open questions

None.

## Post-implementation audit (review panel, 2026-08-01)

Gemini (Architect) and Codex (Staff Engineer) both ran in Mode 2 against `21c3a32..HEAD`.

**Gemini: Approved, zero findings.** It verified each disclosed deviation, and independently confirmed
the golden-fixture claim (the fixture tests round-trip static JSON and never call
`build_export_record`).

**Codex: one MAJOR, one MEDIUM, both accepted and fixed.** Where the two disagreed I did not average:
Gemini's "impossible to return NULL on the wire" is true but answers a narrower question than the
contract makes, which is about the frozen VOCABULARY. Codex was right.

### MAJOR: a non-contract stored scope reached the export wire

`build_export_record` emitted `raw.scope_override.or(raw.scope)` verbatim. Neither column has a
`CHECK` (both are plain nullable TEXT), so a hand-edited catalog -- or one written by a FUTURE clyde
that learned a third scope -- put a non-contract token on the wire.

**Reproduced on a copy of the live catalog before fixing**, which is what promoted this from
plausible to confirmed:

| stored | exported (before) |
|---|---|
| `scope_override='Work'` | `'Work'` |
| `scope='garbage'` | `'garbage'` |
| `scope=''` | `''` |

Two things make it more than pedantry. It breaks the `"work" \| "personal"` promise this doc just
bumped `schema-version` to 2 to protect. And it DIVERGES from the gate: `classify_with_evidence`'s
override step fails closed to `Personal` for an unrecognized value, so export reported `Work` for a
row the gate routes as personal. That is the same two-sites-one-question defect the whole doc exists
to remove, and my own Phase 4 comment claimed this decomposition "cannot drift".

**Fix:** `session::Scope::from_stored` (`session/src/scope.rs`) is the exact inverse of `as_str`, and
`build_export_record` validates both stored sources through it, failing LOUDLY with the session id and
the offending value. Loud rather than fail-closed-to-personal, because that is what the two OTHER
frozen-vocabulary stored values in the same function already do (`enrich_status`, `efficiency_json`),
and because silently substituting a different answer is not actionable while an error is. The
classifier keeps failing closed instead, since a routing decision must never block; `from_stored`'s
doc comment records why the two obligations differ.

Verified after the fix: all three poisoned rows now error and name the session; a healthy catalog
still exports 1908 records at `schema-version: 2` with 0 precedence divergences and exactly two
distinct scope tokens.

### MEDIUM: the strongest Phase 4 test was self-confirming

`no_exported_row_disagrees_with_the_three_step_precedence` recomputes production's own
`COALESCE(scope_override, scope, <cwd rule>)` expression, so if production emitted `garbage` the test
expected `garbage`. It does verify the precedence ORDERING, which was its purpose, but it cannot
catch a vocabulary violation. `a_never_processed_row_still_exports_a_contract_scope_token` only
covered rows with no stored value, so the gap was exactly as described.

**Fix:** two tests added. `a_non_contract_stored_scope_fails_loudly_instead_of_reaching_the_wire`
drives five poison values through the real producer; `every_exported_scope_is_a_frozen_contract_token`
asserts the INDEPENDENT property that no amount of precedence agreement implies. Both bite: reverting
to the pass-through form fails the first.

### Not accepted as findings

- Codex could not verify the `otto ci` claims, the break-it mutations, or the live-catalog numbers,
  because it runs read-only and cannot execute mutating or build commands. Those remain verified by
  the author, with the observed output recorded in the design doc's Acceptance Criteria.
