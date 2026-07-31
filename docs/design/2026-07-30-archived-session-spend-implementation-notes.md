# Implementation notes: Archived Session Spend

**Design doc:** `docs/design/2026-07-30-archived-session-spend.md`
**Branch:** `archived-session-spend`
**Implemented:** 2026-07-30 / 2026-07-31

Append-only record of decisions, deviations, tradeoffs, and open questions per phase. A later entry
supersedes an earlier one rather than rewriting it.

## Pre-phase: ready-to-build gate

### Design decisions

- **Branched from `open-defects-handoff`, not bare `main`.** That branch is `main + 1` (0 behind), the
  one commit being `docs: register the open defects after v0.19.0` -- unpushed and local. The
  addendum this work also owes is an addendum TO that register, so the register has to be in the
  tree. Branching from it is identical to `main` + cherry-picking `d09da3c`, without the duplicate
  commit.
- **The design doc rides the Phase 1 commit, not the final one.** The skill's default is to commit the
  doc with the last phase, but every phase commit message cites
  `Design doc: docs/design/2026-07-30-archived-session-spend.md`, and those references would not
  resolve at the commits that make them. The doc's own Rollout pins the structure at "one branch, six
  commits, one per phase", so a seventh doc-only commit was not an option. Status flips to
  `Implemented` in Phase 6, which is the docs phase anyway.

### Deviations

- None.

### Tradeoffs

- **Re-resolved every `file.rs:line` anchor against post-#78 `main` before starting, as the doc's Ship
  Order requires.** The doc warned `sessions/src/db.rs` alone would move 54 lines. Measured: the
  load-bearing anchors did NOT move. `sessions/src/db.rs:379` (the `archived = 0` predicate),
  `report/src/lib.rs:209` (`include_archived: false`), `report/src/merge.rs:179` (the hardcoded
  `notes`), `cost/src/lib.rs:275` (the live-only scan), `report/src/lib.rs:244`/`:429`/`:338`,
  `clyde/src/main.rs:1009` (`print_reindex`), `clyde/src/main.rs:855` + all six `lazy_reindex` call
  sites (264/272/344/486/785/799), and `clyde/src/bootstrap.rs:130`/`:134`/`:145`/`:928` (the post-#78
  `Systemd` trait shape the doc was authored against) all resolved verbatim. A handful drifted 1-10
  lines (`session/src/paths.rs:96`->`:95`, `cost/src/config.rs:33-34`->`:34-35`,
  `efficiency/src/extract.rs:359`->`:233`/`:368`/`:379`, `efficiency/src/fold.rs:85`->`:76`,
  `session/src/stage.rs:26-30`->`:31`). No doc amendment was needed: the named symbols are the
  anchors, exactly as the doc says.

### Open questions

- None.

## Phase 1: Layout-explicit and staged-union discovery in `common::scan`

**Commit:** `b0a2ec8` -- `feat(scan): resolve session bytes from live or staged roots`

### Design decisions

- **`layout_files` sorts its output by path.** `session::parse`'s `discover_layout_files`, the function
  it mirrors, does not. `read_dir` order is filesystem-dependent, and this output feeds `pricing_files`
  (whose result becomes the artifact's `jsonl_paths`) and the staged half of the union scan, so the
  order is observable. Sorting here makes the union's own sort redundant but harmless.
- **`pricing_files` warns when it resolves nothing.** The callers count the unrecoverable set
  themselves, but a resolver that silently returns empty on the one path that loses money is the
  `.ok()`-swallows-the-error shape the house rules forbid.

### Deviations

- **`common::scan` gained a private `xdg_data_dir()` rather than calling `session::paths`'s.** The doc
  makes `common::scan::default_staged_dir` THE definition of the staged path with
  `session::paths::staged_dir` delegating to it, which fixes the direction of the dependency:
  `session -> common` (verified acyclic -- neither `common` nor its `claude-pricing` dep references
  `session`). But `common` had no XDG data helper, so resolving the path there needed one. That is a
  fifth in-tree copy of a ~10-line helper: `session/src/paths.rs`, `permit/src/config.rs`,
  `report/src/config.rs`, and `cost/src/config.rs` already each carry their own. Accepted rather than
  fixed here: the divergence the doc targets is the staged PATH (now single-source, two callers), the
  helper duplication is pre-existing and tree-wide, and consolidating five copies is its own change
  nobody asked for. `dirs::data_local_dir()` is still not used anywhere, so the macOS trap stays
  closed.

### Tradeoffs

- **Deliberate asymmetry kept, as specified:** a non-UUID name in the projects tree `bail!`s, a
  non-UUID staged directory WARNs and is skipped. Reason in the code: the staged filename is derived
  from the directory name, so a wrong name finds nothing rather than misclassifying something, and
  bailing would let one stray directory in a clyde-owned cache brick every `clyde cost` run.

### Open questions

- None.

### Break-the-code check

Disabled the live-id precedence check in `find_session_files_with_staged` (the doc's rejected
Alternative 2). `staged_union_counts_a_both_roots_session_exactly_once` FAILED; the other four
staged-union tests still passed, confirming the assertion is specific to the double count. Reverted.

## Phase 2: Price every un-annotated row from wherever its bytes are

**Commit:** `b5d0c03` -- `feat(efficiency): price archived rows from their staged copies`

### Design decisions

- **Resolution inside `collect_layouts` runs sequentially, only the compute is parallel.** Resolving is
  a few stats per candidate with no tree walk, and doing it in order keeps `unrecoverable` in the
  catalog's own stable row order instead of parallel-completion order.
- **`print_reindex` prints `unrecoverable` unconditionally, not only when non-zero.** It is the
  accounting half of `candidates` (`candidates == computed + unrecoverable`); a count that appears
  only sometimes reads as an error rather than a standing ledger line. The JSON shape gets it for
  free from `PersistStats`'s existing `Serialize`.

### Deviations

- **`reindex_efficiency` lost its `projects_dir` parameter.** Not mentioned in the doc, but once
  `collect_layouts` resolves each row from its own path fields there is no tree to walk, so the
  parameter was dead. `#![deny(unused_variables)]` would have required silencing it with an
  underscore, which the house rules forbid. One caller updated (`clyde/src/main.rs`).
- **The old test was RENAMED, not just rewritten.**
  `v6_sessions_missing_efficiency_excludes_annotated_and_archived` became
  `..._includes_archived_and_excludes_annotated`, per the doc's instruction that the name stop
  asserting the old behavior. A second test
  (`..._carries_the_resolver_path_fields`) covers the new return type's payload, which the doc
  specified in the API but named no criterion for.

### Tradeoffs

- The archived-row test asserts `aggregate.raw.cost-usd` is numeric in `efficiency_json` rather than
  reading the `cost_usd` COLUMN, because `sessions::Db` exposes no `cost_usd` getter to the
  `efficiency` crate. Sound because `from_session_scalars_match_the_serialized_json` already pins the
  column to that exact JSON value, so asserting the JSON asserts the column.

### Open questions

- None.

### Break-the-code check

Re-added `AND archived = 0` to `Db::sessions_missing_efficiency`. Three tests failed:
`v6_sessions_missing_efficiency_includes_archived_and_excludes_annotated`,
`reindex_prices_an_archived_row_from_its_staged_copy`, and
`reindex_counts_an_archived_row_with_no_staged_copy_as_unrecoverable`. Reverted.

## Phase 3: The report window counts archived spend and discloses the residue

**Commit:** `91abf37` -- `feat(report): count archived spend and disclose the residue`

### Design decisions

- **The `pricing_files` resolution runs ONCE for every windowed row, into a `BTreeMap`, and serves both
  the guard split and `jsonl_paths`.** Resolving twice would be two stat passes and, worse, two places
  that could disagree about a row's recoverability.
- **`extra_notes: &[String]` threaded through `build_report`/`build_json`/`write_json`** rather than a
  new `Report` field. `notes` already exists as the disclosure channel; the builders just needed the
  caller's lines. All three already carry `#[allow(clippy::too_many_arguments)]`.
- **`jsonl_paths` is EMPTY, not the stale `transcript_path`, when nothing resolves.** This happens for
  an annotated row whose bytes were reaped after it was priced. The doc's stated goal is that the
  artifact's paths always point at readable bytes; naming an unopenable file would defeat that. Empty
  is already a supported shape (three existing fixtures use it) and nothing on the collect path
  asserts otherwise.

### Deviations

- **One EXISTING test needed a real transcript on disk.**
  `collect_fails_closed_on_null_efficiency_and_writes_no_artifact` seeded its NULL-efficiency row with
  the fixture's fake `/tmp/<sid>.jsonl` path. Under the new split that row resolves to no bytes, so it
  would be classified unrecoverable and DISCLOSED (exit 0) rather than failing closed -- the test
  would have failed for the right reason. Its premise is "not yet reindexed", which implies the
  transcript is there, so it now writes a real one via a new `insert_unindexed_live` helper. This is
  the honest fix: the two states the phase separates were previously indistinguishable in the fixture.

### Tradeoffs

- `report collect` still advertises "no JSONL is scanned", which stays true: the new resolution
  `stat`s candidate paths and parses nothing. Noted here because the claim now needs that distinction
  to be exact.

### Open questions

- None.

### Break-the-code check

Two separate breaks. (1) `include_archived: false` restored ->
`collect_counts_an_archived_but_priced_session` and
`collect_excludes_and_discloses_an_unrecoverable_row` both FAILED. (2) `merge.rs`'s
`notes: vec![WINDOW_NOTE]` restored -> `merge_preserves_per_host_unrecoverable_disclosure` FAILED.
Both reverted.

## Phase 4: `clyde cost` and `clyde efficiency` see the staged root

**Commit:** `f205e8b` -- `feat(cost): scan the staged root on the cost and efficiency surfaces`

### Design decisions

- **An explicit `--path` suppresses the default staged root.** The doc specifies only "Default:
  `common::scan::default_staged_dir()`" and does not address the `--path` interaction. Resolution
  order is now: configured `staged-dir` wins; else if `--path` was given, NO staged pass; else the
  default. Two reasons, and the second is a correctness bug avoided rather than a preference:
  1. `--path` means "price this tree". Silently unioning a clyde-owned cache into an
     explicitly-named tree makes the flag unpredictable, and would have made the doc's own Phase 0
     spike (`clyde cost --path <tree>`, measuring exactly 199 staged dirs) impossible to express.
  2. Every existing `cost` test passes `path: Some(fixture)` with `Config::default()`. Without this
     rule they would all have scanned the developer's REAL `~/.local/share/clyde/staged` (308 dirs on
     desk.lan), silently breaking every hand-computed cost assertion in the suite.
- **Tests pin `staged-dir` explicitly** via a `fixture_config_with_staged` helper, and there is a test
  (`an_explicit_path_does_not_union_the_default_staged_root`) asserting the suppression rule itself,
  so the isolation property is enforced rather than assumed.

### Deviations

- **`find_session_files` (live-only) stays exported from `cost::scanner` under `#[cfg(test)]`.** The
  reconciliation oracle drives fixtures through an explicit `--path`, which now scans live-only, so
  the oracle must mirror that scope or its file-level equality assertion compares two different
  discovery scopes. Gated to test builds so production code cannot reach for the live-only scan.

### Tradeoffs

- The efficiency surfaces are covered at the `collect_all`/`collect_matching` seam plus a test that
  drives `rollup::daily`, `rollup::weekly`, and `rank::worst` over the union scan's output. That
  proves each of the four surfaces the panel named without spawning the binary, since all four read
  that one seam.

### Open questions

- None.

### Live verification

`clyde cost --no-cache monthly` on desk.lan, built from this branch:
`2026-06: $8,040.64` over 695 sessions (was `$4,818.54` / 475), and `2026-05: $289.63` over 24
sessions (previously absent entirely). The doc's Phase 4 criterion was ">= $7,700"; met with margin.
Against June ground truth of $9,110.96 the gap closed from -47.1% to -11.8%; the remainder is the
known multi-host factor the doc scopes to `report merge`.

## Phase 5: Close the reap-before-stage race

**Commit:** `3e80e6b` -- `feat(session): stage dormant transcripts inside reindex, on a timer`

### Design decisions

- **The 7d dormancy default became one shared constant**, `cli::DEFAULT_STAGE_DORMANT_AFTER`, used
  both as clap's `default_value` for `clyde session stage --dormant-after` and by the in-reindex
  sweep. The doc says "the same default cutoff as `clyde session stage` (7d)"; two literal `"7d"`s
  would have satisfied it while leaving the two free to drift.
- **`cmd_reindex` takes its timezone from the config load it ALREADY performs**, rather than gaining a
  `load_date_tz()` call at the dispatch site. It needs a tz only because the dormancy cutoff goes
  through the shared `parse_since` span parser (correct seam -- no second span parser), and it already
  loads `clyde.yml` for the efficiency thresholds. The lazy-config comment at the dispatch site, which
  listed `reindex` among the commands that read no dates, was corrected.
- **The reindex timer fires every 6h**, vs the enrich timer's daily. It is racing a TTL, so a shorter
  interval shrinks the window in which a transcript can be reaped before it is staged, and a sweep
  with nothing newly dormant is one `stat` per candidate.
- **The reindex service has no `network-online.target` dependency**, unlike the enrich unit. This pass
  reads local transcripts and writes the local catalog; nothing waits on a network it never uses.
- **`Outcome::reindex_timer_changed` is tracked separately from `systemd_changed`.** An enrich-unit
  repair alone sets `systemd_changed`, and `systemctl --user start` on a timer whose unit was never
  written is a spurious failure line in the operator's log.
- **`install_clyde_reindex_timer` is `pub(crate)` so the doctor tests seed via the REAL installer.**
  Hand-writing the unit files in the doctor test would let doctor's detection drift from what
  bootstrap actually writes -- the exact divergence class this design doc removes elsewhere.

### Deviations

- **The staging counts are a `staging` sibling in `print_reindex`'s JSON, not fields on
  `ReindexStats`.** The doc says "`ReindexStats` gains the `StageStats` counts". `sessions::reindex`
  does not stage, so a staging field on its result struct would be one its own producer never fills
  and every other caller reads as zero. The JSON shape already models this correctly -- `efficiency`
  is a sibling object, not folded into `ReindexStats` -- so staging follows that established
  precedent. The doc's actual requirement (counts visible in both TTY and JSON shapes) is met.
- **`clyde doctor` treats a missing reindex timer as a NOTE, not an unhealthy verdict.** The doc says
  doctor should "report whether that timer is installed and enabled" without specifying the verdict.
  Scheduling is a host policy choice and a diagnostic must not fail over one; but the note names the
  consequence (a session can be reaped before it is staged) and the remedy, so an open race is never
  silent. `Report::healthy()` is deliberately unchanged.

### Tradeoffs

- **`Target::Absent`, never `Target::Legacy`, for the reindex timer.** The unit is new, so there is no
  pre-rename spelling to detect.
- The three-piece completeness check (service AND timer AND enable symlink) uses
  `symlink_metadata` for the link, not `exists()`, which follows the link and reports false for a
  dangling one. A test removes each piece in turn and asserts all three are load-bearing.

### Open questions

- None.

## Phase 6: Truth-up the docs

**Commit:** this one.

### Design decisions

- **No root `CLAUDE.md` bullet**, as the doc already resolved. Verified again at implementation time:
  `ls CLAUDE.md` -> no such file. The durable guard against a money path filtering on `archived`
  again is Phase 2's renamed regression test, not prose.
- README gained a **"Reaped transcripts, staging, and what `archived` means"** section rather than
  scattering the semantics through existing sections, because the four facts (what `archived` means,
  that staged copies are priced, that no-bytes rows are stated and excluded, that merge carries the
  disclosure) only make sense together. The two operational notes the doc requires are subsections of
  it.

### Deviations

- **Amended a Phase 6 success criterion: a DOC DEFECT, with evidence.** As authored it was
  `rg -n "nothing on disk to recompute" -- '*.rs' '*.md'` returns zero hits. That can never pass:
  this design doc is itself a `.md` and must quote the phrase to explain the false premise it exists
  to kill, so the grep matches its own Phase 2 bullet and its own criterion text. Measured:
  `--include='*.md'` returns exactly 2 lines, both in the design doc; `--include='*.rs'` returns 0.
  Amended to `--type rust`, which is where the premise was load-bearing (the
  `sessions_missing_efficiency` doc comment that invited the `archived = 0` predicate straight back,
  rewritten in Phase 2). The criterion is wrong independent of any code written here: it greps a file
  class that necessarily contains its own search string.

### Tradeoffs

- None.

### Open questions

- None.

## Addendum: the open-defects register

**Commit:** separate, after Phase 6.

Not part of this design doc's six phases. Scott asked for it alongside the build, so it rides its own
commit rather than being folded into a phase, keeping the doc's "six commits, one per phase" structure
intact and the added scope legible. See
`docs/design/2026-07-30-open-defects-handoff.md`'s appended Addendum section.
