# Implementation Notes: Close the Open Register

Running record of decisions, deviations, tradeoffs, and open questions while executing
`docs/design/2026-07-31-close-the-open-register.md`. Append-only: a later entry supersedes an earlier
one rather than rewriting it.

## Phase 1: Widen both lints and kill the fail-open (E + F)

### Design decisions

- **The widened `_variable` lint got the em-dash lint's full comment treatment, not just its status
  shape** (`.otto.yml`, `lint` task). The plan said "copy the explicit status shape verbatim from
  `:42-61`"; the copy also carries a scope rationale and the drop-guard note the plan asked for, so a
  reader of either block finds the same three facts (why the scope is the whole tree, why the status
  is checked explicitly, what to do when the pattern is wrong). Two lints that behave identically now
  read identically.
- **The em-dash lint's user-facing strings were retitled, not left alone** (`.otto.yml`, `lint`
  task). Adding `--include='*.pmt'` made `=== Deny em dash in Rust source ===`, `❌ Found em dash in
  Rust source.`, and `✅ No em dashes in Rust source` all understate what the scan covers. They now
  say "Rust source and slot templates" / "Rust source or slot template". House rule: names tell the
  truth. AC1's recorded pre-state quotes the old success line, which is a pre-state observation, not
  a post-condition.
- **`var_status` is the new variable name**, mirroring `em_status`, so the two `case` blocks are
  symmetric.

### Deviations

- None. All five plan bullets landed as specified: `.` plus `--exclude-dir=target` on the
  `_variable` scan, the explicit status `case`, `--include='*.pmt'` on the em-dash scan, the
  `report/src/render/slots/tests.rs:531` assertion left in place, and the drop-guard comment.

### Tradeoffs

- **`--include='*.pmt'` on the existing em-dash `grep` vs. a second scan for templates.** One `grep`
  with two `--include` filters keeps one status `case` and one pass over the tree; a separate scan
  would need its own status block and could drift from the first. Cost: the failure message cannot
  name which of the two file kinds matched, so it names both. `grep -rn` prints the offending path on
  the line above, so the operator is not actually left guessing.
- **Widening the `_variable` scan to `.` vs. enumerating `*/src/ */tests/ */build.rs`.** The
  enumeration is narrower and would need editing every time a crate grows a new top-level target;
  `.` plus `--exclude-dir=target` is what the em-dash lint already does and needs no maintenance.
  Measured zero new violations either way.

### Open questions

- None.

### Success criteria, executed

- `otto lint` exits **0** unchanged, printing `✅ No _variable patterns found` and `✅ No em dashes
  in Rust source or slot templates`.
- Planting `fn _plant() { let _foo = 1; }` in `clyde/tests/collect.rs` makes `otto lint` exit **1**
  with `❌ Found _variable binding pattern.`; `git checkout` of that file restores **0**. This is the
  half the old `*/src/` scope could not see.
- Planting an em dash in `report/templates/slots/closing.pmt` makes `otto lint` exit **1** with
  `❌ Found em dash in Rust source or slot template.`; restoring the file returns **0**.
- Full `otto ci` exits **0**.

## Phase 2: Stop laundering an unpriceable model into $0 (D)

### Design decisions

- **`usage_has_tokens` is a named free function in `efficiency/src/metrics.rs`, not an inline
  comparison.** The plan said "gate the insert on the turn carrying non-zero tokens" and pointed at
  `report::has_tokens` for the definition. `has_tokens` reads `TokenTotals::total`, which does not
  exist yet at the point `add_usage` decides (it decides per turn, on a raw `TokenUsage`), so the
  predicate is restated over the pre-aggregation shape with a doc comment naming the pairing. The two
  definitions agreeing is the whole reason `unpriced-models` and report's `untracked_models` count the
  same thing as a real gap, so it is stated once and named rather than open-coded.
- **`add_usage` now `match`es the `Option` instead of `unwrap_or_else`.** `unwrap_or_else` cannot
  carry a second statement cleanly, and the design named `cost/src/oracle.rs:326` as the in-house
  precedent for matching this call rather than `.ok()`-ing it. The `warn!` is preserved verbatim.
- **`clyde efficiency session` and MCP `session_efficiency` needed NO code change.** The plan listed
  "surface it in the `clyde efficiency session` rendering" as its own bullet, which implied an edit.
  Measured: `efficiency/src/output.rs`'s `SessionJson` borrows `&EfficiencySignals` whole and
  `render` emits YAML or JSON with no hand-rolled human table anywhere, so a new `RawCounters` field
  appears on that surface the moment it is on the struct, exactly as the plan already predicted for
  MCP. Verified by `reindex_discloses_and_counts_a_session_whose_model_could_not_be_priced`, which
  asserts the field in the persisted blob those surfaces pass through.
- **`efficiency/src/narrate.rs` was left alone.** Correction 1 names `--narrate` as a consumer of the
  blob's `cost-usd`, but no Phase 2 bullet asks for a narrate change, and the narrator's foreign-number
  check (`narrate_rejects_prose_that_invents_a_number`) makes feeding it a new set a design question
  of its own. Out of scope for this phase, and flagged here rather than done quietly.

### Deviations

- **The three committed report fixtures were regenerated**, which no plan bullet mentions.
  `fixtures/report/{small,medium,pathological}/report.json` and `medium/prior.json` embed each
  session's efficiency blob verbatim at `sessions.<id>.efficiency.aggregate.raw`, so
  `report/src/eval/tests.rs`'s `the_committed_fixtures_match_the_generator` went red the moment the
  struct grew a field. Regenerated with the generator the test itself names
  (`cargo run -p report --bin fixtures -- fixtures/report`). Verified additive-only: every changed
  line is either the new `"unpriced-models": []` or a trailing comma on what used to be the last
  field. No number moved, and no golden needed re-rendering, because `report` does not read the field.
  This is mechanical fallout of a serialized-struct change, not a design change, and the design's
  Non-Goal about the export contract (`EXPORT_SCHEMA_VERSION`, the `dormant` flag) is untouched.

### Tradeoffs

- **A `BTreeSet<String>` on `RawCounters` vs. a count.** A set names the models, so an operator can
  act (add the model to the feed); a count would only say "some dollars are missing." Cost: the blob
  grows by a key on every row, and `merge` gains a union. The design already specified the set; the
  measured cost is one empty array per session in the fixtures.
- **`unpriced` counts SESSIONS, not models.** `PersistStats::unpriced` is the number of catalog rows
  whose dollars cannot be trusted, which is the actionable number for a reindex summary line; the model
  ids live in each row's blob. A cross-session union of model names was considered and dropped: it
  would be a second definition of the same thing, derivable from the blobs, and `PersistStats` is a
  per-run ledger, not a catalog query.
- **No `SCHEMA_VERSION` bump, per the design's Resolved Decision.** Cost: an existing row's blob keeps
  an absent key until something else recomputes it, so `unpriced-models` is empty there by default
  rather than by measurement. Accepted because the set would be empty on every existing row anyway
  (all 9 measured catalog models either price or carry zero tokens), and the alternative forces a full
  recompute of ~1,800 rows. Pinned by `blob_without_the_field_deserializes_to_an_empty_set`.

### Open questions

- None.

### Success criteria, executed

- `RawCounters` fed `claude-not-in-any-feed-9` with 1,500 tokens: the model is in `unpriced-models`,
  the turn returns `0.0`, and `cost_usd` is unchanged from the priced control turn
  (`unpriced_model_with_tokens_is_disclosed_and_contributes_zero`).
- The same fed `<synthetic>` with all-zero tokens leaves `unpriced-models` **empty** while still
  counting the turn and the model mix (`zero_token_unpriced_model_is_not_disclosed`).
- `rg -n 'unpriced_models' --type rust -g '!target' -g '!*tests*' .` returns **5** production lines
  (AC2 requires at least one), up from **0** on `main`.
- Every test verified to BITE, each on its own behavior:
  - restoring the fail-open (`if false && usage_has_tokens(..)`) fails exactly
    `unpriced_model_with_tokens_is_disclosed_and_contributes_zero` and
    `reindex_discloses_and_counts_a_session_whose_model_could_not_be_priced` (117 pass, 2 fail);
  - dropping the zero-token gate (`if true || ..`) fails exactly
    `zero_token_unpriced_model_is_not_disclosed` (118 pass, 1 fail);
  - removing `#[serde(default)]` fails exactly
    `blob_without_the_field_deserializes_to_an_empty_set`.
- Full `otto ci` exits **0** (119 efficiency tests, whole workspace green).
- AC2's second half, `clyde session reindex` printing an `unpriced` count on every run, is asserted at
  the library seam (`reindex_populates_null_sessions_without_bumping_updated_at` pins `unpriced == 0`;
  the new persist test pins `== 1`) and is verified at the live CLI surface during the Rollout run,
  not mid-phase: exercising it against the real catalog writes staging copies and efficiency blobs.
