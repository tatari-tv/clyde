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

## Phase 3: Dormancy off activity time, not filesystem mtime (B)

### Design decisions

- **`SessionRecord::activity_at` is `#[serde(skip)]`, like `id`.** Found by a test, not by reading:
  `mcp::tests::sessions_search_clamps_limit_to_hard_max` went from 100 hits to 99. Root cause, not a
  guess: `sessions_search` caps its whole response at `SEARCH_RESPONSE_MAX_CHARS` (60,000) and drops
  whole hits from the end when it would exceed that, so ~20 extra bytes per hit for
  `"activity_at":null` came straight out of the hit budget. `activity_at` is an internal input to the
  dormancy decision, read only through `dormancy_at()`; it was never part of the public/JSON surface
  and `id` already sets the precedent for skipping such a field. This also keeps every MCP and CLI
  JSON response byte-identical, which sits well beside the design's Non-Goal of not touching published
  contracts.
- **`skip_key_of` returns a named `SkipKey` struct, not the tuple the plan wrote.** The plan said
  "widen that helper to return the stored `(modified, parse_version)` pair and rename it".
  `Result<Option<(Option<DateTime<Utc>>, Option<i64>)>>` trips `clippy::type_complexity` under
  `-D warnings`, and two same-typed `Option`s in a tuple are swappable at a call site. A struct fixes
  both and matches the house rule about typed values at seams.
- **`COLS_LEN` is a const, and both trailing-index sites derive from it.** The plan said to "bump both
  queries' trailing indices in the same commit," which is correct but leaves the next person doing the
  same four-site edit by hand. `map_catalog_entry` now reads `COLS_LEN + n` and `search_table` reads
  `COLS_LEN` / `COLS_LEN + 1`, so growing `COLS` again is a two-site edit (the string and the const)
  and the consumers follow. `COLS`'s doc comment names all four sites and which one fails silently.
- **`snapshot_before_v10` and `snapshot_before_v11` share one `snapshot_before(conn, path, target)`
  implementation.** The plan said "add `snapshot_before_v11` mirroring `snapshot_before_v10`". A
  literal copy would be a second place to get the `-wal`/`-shm` sidecar handling subtly different, and
  the only difference between them is an integer.
- **`row_exists` is `skip_key.is_some()`, not `existing.is_some()`.** The old code derived
  "row exists" from a parsed `Option<DateTime>`; with the widened helper that would send a row whose
  stored `modified` is unparseable down the INSERT arm and trip the UNIQUE constraint. Row existence
  and timestamp parseability are now separate questions, because they are.
- **`sessions/src/db/activity.rs` is a new submodule.** `db.rs` hit **1566** lines with the v11 write
  side inline, over the 1500 limit (`otto bloat` caught it). Extracted `SkipKey`, `skip_key_of`, and
  `set_activity_many` into `db/activity.rs`, mirroring the existing `catalog`/`query`/`repo` split.
  `db.rs` is now **1492**. **This is a live constraint for Phase 4**, which adds `scope_version` writes
  at three sites in `db.rs`: it will need its own extraction, not a raise of the limit.
- **New tests live in `db/tests/activity.rs`**, following the `db/tests/efficiency.rs` precedent, for
  the same reason: `db/tests.rs` was already at 1474 lines.

### Deviations

- **`sessions/src/stage/tests.rs::stage_dormant_filters_by_cutoff_and_records_path` was rewritten,
  not just extended.** It distinguished a dormant session from a fresh one purely by file mtime, with
  both transcripts carrying a hardcoded `2026-06-21` message timestamp. Under the new definition BOTH
  are dormant, and correctly so: today is 2026-07-31, so both conversations are 40 days stale and only
  a freshly-written file made one look current. That is precisely the defect. The fixture now uses
  now-relative message timestamps (A: 30 days ago, B: 1 hour ago), which also removes a latent time
  bomb -- a hardcoded date is a "fresh" session only until the calendar passes it.
- **Three `SCHEMA_VERSION` pins in `db/tests/efficiency.rs` were raised 10 -> 11.** They carry
  `"bump me deliberately"` / `"raise me deliberately"` messages, so this is the mechanism working as
  designed rather than an unplanned edit. One stale comment (`// Reopen: migrate v7 -> current (v10).`)
  and one stale message (`"pins the v9->v10 hop"`) were corrected to say "current" instead of naming a
  version they no longer pin.
- **Twelve test fixtures across seven crates gained `activity_at: None`.** Mechanical fallout of a new
  required field on `ParsedSession` / `SessionRecord`. `None` is the right default for every one of
  them: it exercises the `dormancy_at()` fallback, i.e. the behavior those tests were already
  asserting.

### Tradeoffs

- **`Upsert::Backfilled` + a caller-side collect vs. writing inside `upsert_session`.** Writing in
  place would be less plumbing (no `pending_activity` vec, no new variant, no `ReindexStats` field) but
  would put a trigger DROP/CREATE pair inside a per-session function that runs bare `conn.execute` with
  no transaction. The review panel's finding stands: a crash between the two would leave
  `sessions_updated_at_update` permanently dropped and silently freeze `export_meta.revision` forever.
  Cost of the chosen shape: one `Vec<(String, Option<DateTime<Utc>>)>` held across the reindex loop
  (2,111 entries worst case on desk.lan, a few hundred KB).
- **`parse_version` as an integer const vs. `activity_at IS NOT NULL` as the gate.** The const costs a
  column and a bump ritual on every future parse-derived field. `IS NOT NULL` costs nothing but never
  terminates for a transcript with no parseable timestamp, and each non-termination is a content-arm
  UPDATE that NULLs that row's efficiency blob. Pinned by
  `a_transcript_with_no_timestamps_is_backfilled_once_then_skipped`.
- **`activity_at` stays `None` on an unparseable stored value** rather than falling back to
  `MIN_UTC` the way `modified` does. `modified` sinks a corrupt row under `ORDER BY modified DESC`,
  where `MIN_UTC` is the fail-closed answer; here `MIN_UTC` would mean "dormant since the dawn of
  time" and sweep the row on the strength of a corrupt column. `None` degrades to exactly today's
  behavior instead.

### Open questions

- None.

### Success criteria, executed

All in `sessions/src/db/tests/activity.rs` unless noted; 8 new tests there plus 2 in
`session/src/parse/tests.rs`.

- A fixture with messages 30 days old and every mtime set to `now` is returned by BOTH
  `staging_candidates(now - 7d)` and `enrich_candidates(now - 7d, ..)`
  (`a_wholesale_mtime_reset_does_not_hide_a_dormant_session`). Reverting `enrich_candidates` to
  `r.modified` fails exactly that test; reverting `staging_candidates` fails it plus
  `a_recently_active_session_is_fresh_even_with_an_ancient_mtime` (7 pass / 1 fail and 6 pass / 2 fail
  respectively).
- A row whose `activity_at` is NULL is filtered exactly as today
  (`a_null_activity_at_falls_back_to_mtime_exactly_as_today`), asserting both directions: fresh mtime
  is not swept, old mtime is.
- `PRAGMA user_version` is **>= 11** after open (stated as `>=` on purpose, per the criterion) and a
  v10 DB gets a `.pre-v11.bak` exactly once, verified by mtime across a reopen
  (`opening_an_on_disk_db_migrates_to_v11_and_snapshots_once`). A brand-new catalog gets no snapshot.
- A backfill leaves `efficiency_json` and `updated_at` UNCHANGED, and a second pass reports
  `SkippedUnchanged` (`the_backfill_leaves_efficiency_and_the_export_cursor_untouched`). Dropping the
  trigger sandwich in `set_activity_many` fails exactly that test.
- A transcript with no parseable `timestamp` is backfilled once then skipped with `activity_at` still
  NULL (`a_transcript_with_no_timestamps_is_backfilled_once_then_skipped`). Swapping the gate to an
  `activity_at IS NOT NULL` probe fails 3 tests.
- The `COLS` growth is pinned from both ends: an off-by-one in `map_catalog_entry` fails
  `the_catalog_round_trips_both_blobs_after_cols_grew` plus the two existing catalog round-trip tests
  (the new one uses a row with BOTH blobs populated, as the panel required), and shifting `COLS_LEN`
  fails **10** tests including 7 ranking tests -- confirming the panel's severity split: `search_table`
  fails loudly, the catalog site is the silent one.
- `activity_at` is a MAX fold independent of record order, and `None` when no record carries a
  timestamp (`session/src/parse/tests.rs`).
- Full `otto ci` exits **0**.
