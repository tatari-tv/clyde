# Implementation Notes: Month over Month deltas, computed

Design doc: `docs/design/2026-07-27-month-over-month-deltas.md`

## Phase 0: Prove the rejected tokens are comparison figures

### Design decisions

- Established the per-render `--prior` configuration from the exact shell commands in the shipit
  subagent's transcript
  (`~/.claude/projects/-home-saidler-repos-tatari-tv-clyde/6dbe3155-f40f-46d3-9b1c-52a760d99246/subagents/agent-ashipit-d399c59e432f168f.jsonl`),
  not from this doc's own summary table, because the log never records `--prior` (confirmed by
  reading `render.rs:41` and `lib.rs:150`: the field isn't in the INFO line, and the two `debug!`
  lines that would carry it never fired because all nine renders ran at the default `Info` filter).
  Ten literal `clyde report render -i ...` commands exist in the window; the first is the
  already-discarded warm-up, and the remaining nine map to the nine `render::run` log lines one for
  one by count and chronological order.
- Classified each of the five prose-guard rejections from its token and its configuration, not its
  excerpt, per the phase's own instruction, and confirmed why: `excerpt()` (`render.rs:474-490`)
  scans the WHOLE document for the first occurrence of the literal substring, not the flagged
  sentence, and the `prose` it's called on (`render.rs:315,359-360`) is the entire markdown body or
  HTML visible text.

### Deviations

- The design doc's own Problem Statement table (5 no-`--prior` / 4 with-`--prior`, 40% / 75% /
  100% rejected) does not match the per-render `--prior` configuration recovered from the exact
  shell commands (4 no-`--prior` / 5 with-`--prior`, and the with-`--prior` rejection rate is 80%,
  not 75%). Corrected in a new Resolved Decision rather than silently used; see the doc for the full
  per-render table.
- Render #1 in this doc's nine is not a rejection of the prose/claim guard this design is about at
  all. It is a separate, correctly-firing reconcile-identity guard, rejecting a deliberately
  corrupted `--reconcile` export before the render reached the LLM. Recorded as its own class rather
  than folded into "no-`--prior` rejections."

### Tradeoffs

- None. Zero code this phase; the only tradeoff was which evidence source to trust for the
  `--prior` configuration (chosen: the exact commands actually run, over inferring from this doc's
  own summary table, per the phase's explicit instruction not to treat the doc's input as
  independent confirmation of itself).

### Open questions

- None from this phase. The open question the STOP creates is for Scott and the team lead: whether
  to revise this design's root-cause section against the corrected evidence (round-number/threshold
  invention in the current-period narrative, not a missing subtraction) before Phase 1 proceeds, or
  to treat the `--prior`-specific fix as still worth shipping on its own independent merits (the
  template contradiction in Resolved Decisions and the `excerpt`/persistence fixes in Phases 1-2 are
  unaffected by this finding) while opening a new doc for the round-number/threshold defect.

## STOP: Phase 0 result

**The phase's own success criteria required at least two of the three (turned out to be four)
`--prior` rejections to be Month over Month comparison figures. Zero of the four are.** One is
confirmed by content ("also running above 100 sessions," a threshold about the current period,
explicitly named in this doc's own Problem Statement). Two share its exact token. One shares its
exact token and shape with a no-`--prior` rejection where a comparison fabrication is mechanically
impossible. Per the phase's STOP condition: this plan's root cause does not hold against the
evidence as it stands. Phases 1 through 6 were not started.

## Phase 1: Quote the span the guard rejected

### Design decisions

- `QuotableFacts::foreign_figures` now returns `Vec<ForeignFigure { token, start, end }>`, one entry
  per rejected OCCURRENCE in match order, not deduped by token through a `BTreeSet<String>` the way
  the old code was. The span is the same `m.start()`/`m.end()` the regex already produced; nothing
  new is computed, it just survives past the point the token used to be extracted alone.
- `claim::Violation` gained `start`/`end` from the same capture group it already matched
  (`claim.start()`/`claim.end()`), for the same reason: the claim guard's rejection is quoted the
  same way the value guard's is, and it already had the span sitting in the `Match` it discarded.
- `render::excerpt(prose, needle)` is gone. `render::excerpt_at(prose, start, end)` takes the span
  directly, so there is no longer a second, independent search of the document for the excerpt to
  land on wrong. This is the actual fix, not a signature change with the same body: the whole class
  of "excerpt quotes a lookalike" bugs is removed by construction, because the only span `excerpt_at`
  can produce IS the one the guard's own regex matched.
- `excerpt_at`'s byte-to-char conversion walks `prose.char_indices()` once, recording the char index
  at which `byte_at == start` and at which `byte_at == end`, then applies `EXCERPT_RADIUS` in that
  char space before collecting into a `Vec<char>` window (never a `&str` slice, per the crate's
  `clippy::string_slice` lint). This is the exact shape the crate got wrong once already
  (`eval/mechanical.rs`'s `em_dash`, whose own comment documents `char_indices` handing back byte
  offsets that a char-counting excerpt then walked as if they were char offsets), which is why the
  multibyte test is not optional.
- Every site that compared `foreign_figures`'s old `Vec<String>` output against a literal `vec![...]`
  now goes through a small test-local `tokens(&[ForeignFigure]) -> Vec<String>` helper in
  `quotable/tests.rs` (or an inline `.iter().map(|f| f.token.clone())` in the two render test files
  that only needed it once). This is test-side adaptation, not a second production accessor: the
  production API returns exactly one thing, `Vec<ForeignFigure>`, and nothing in `report/src`
  outside the test tree ever asks for tokens alone.
- One assertion's expected order changed for real, not cosmetically:
  `small_integers_are_not_blanket_exempt` expected `["14", "7", "99"]` (a `BTreeSet<String>`'s
  lexicographic order, an artifact of the old dedup-and-sort step) and now expects `["7", "14",
  "99"]` (the order the tokens were actually found in the prose). The new order is the more honest
  one: a caller reading the vec in order now reads the document in order.

### Deviations

- None from the phase's spec. The three required tests exist under the exact names the spec gave,
  `claim/tests.rs` needed no edit despite being named in the churn list (its one `foreign_figures`
  call only checks `.is_empty()`, which is type-agnostic), and no parallel `Vec<String>` accessor was
  added to `quotable.rs`.

### Tradeoffs

- Per-occurrence rather than per-token dedup in `foreign_figures`'s output. The old `BTreeSet<String>`
  collapsed three rejections of the same fabricated number down to one line in the error; the new
  `Vec<ForeignFigure>` reports each occurrence separately, so the same fabricated figure repeated
  three times in one artifact now produces three cited lines instead of one. Chosen because dedup and
  "quote the exact span" are in direct tension (the whole point of this phase is that a token can no
  longer speak for a span it wasn't found at), and because a slightly longer error on a rare
  repeated-fabrication case costs the operator nothing next to a rejection message that used to
  misquote the artifact entirely.
- `EXCERPT_RADIUS` (60 chars) is unchanged from the prior `excerpt`. The new
  `excerpt_quotes_the_rejected_span_not_an_earlier_lookalike` test needed roughly 200 chars of real
  separation between its two occurrences to prove the two spans do not bleed into each other's
  windows; that is a property of the test's own prose, not a reason to touch the constant, which
  every existing rejection message is already tuned around.

### Open questions

- None.

### Amendment, 2026-07-27: cap the citation list

Supersedes the "Per-occurrence rather than per-token dedup" tradeoff above, which shipped a real
regression: `foreign_figures` moving from a deduped `BTreeSet<String>` to one `ForeignFigure` per
OCCURRENCE meant `render::reject_foreign_numbers` and `claim::reject_fabricated_claims`, which map
every entry straight into the WARN and the `bail!` with no cap, could turn one repeated fabrication
into a multi-thousand-character wall of near-identical citation lines. Measured directly: 50 repeats
of one fabricated token produced a 6,667-character message before this amendment; 266 characters
after. A real rejection on the `pathological` fixture (`~/.local/share/clyde/logs/report.log`,
2026-07-27T09:21:16Z) named four distinct tokens in one message under the old deduped code, which is
the shape a wide fabrication actually takes and the number `MAX_CITED` is set against.

- Per-occurrence spans stay internal to `QuotableFacts::foreign_figures` and `claim::Violation` --
  that part of the phase was correct and is what lets `excerpt_at` quote the actual rejected span.
  The regression was only in how the citation MESSAGE was built from them.
- Added `render::group_by_label` (`render.rs`), a small generic that collapses `items` into one
  [`Occurrence`] per distinct label in first-seen order, counting repeats, generic over an `extra`
  field so the claim guard's `rule` string rides along the same machinery the value guard uses over
  `ForeignFigure`. Linear search per item against the groups seen so far, not a `HashMap`: the crate
  has never carried a violation list long enough for that to matter, and a hash map would sacrifice
  the deterministic, scan-order output every rejection message relies on.
- Added `render::cite`, which renders grouped occurrences into the semicolon-joined citation string
  both guards emit, capped at `MAX_CITED = 8` distinct labels. Chosen against the one real multi-token
  rejection logged (four distinct tokens), leaving headroom above it without inviting unbounded
  growth. A label that recurred gets an "(and N more occurrences)" tail; when the cap elides whole
  labels, a trailing "and N more citations not shown" names it. Nothing is silently dropped, per the
  same principle `excerpt_at` exists to serve for the span itself.
- Both `render::reject_foreign_numbers` and `claim::reject_fabricated_claims` now go through
  `group_by_label` + `cite` instead of a bare `.iter().map(...).collect().join("; ")`.
- New test: `a_token_repeated_past_the_cap_is_cited_once_with_the_elided_count_named`
  (`render/tests/quotable.rs`), 50 repeats of one fabricated token. Confirmed it bites: temporarily
  reverted `reject_foreign_numbers` to the un-grouped per-occurrence form and re-ran it, which failed
  with a 6,667-character message (50 cited entries) before the assertion on entry count even ran;
  restored the fix and it passes at 266 characters.
- Two production doc comments over-cited a fixture claim ("`05` recurs 41 times" in
  `pathological/golden.html`) that did not survive checking: the actual regex tokenizer resolves most
  of those substrings into whole `2026-05-DD` date tokens, and the file carries exactly one BARE `05`
  token, not 41. Corrected both comments to cite only the directly-verified real log line instead.

No new open questions from this amendment.

## Phase 2: Persist a rejected render

### Design decisions

- One call site, not three. `reject_foreign_numbers`, `claim::reject_fabricated_claims`, and
  `geometry::reject_foreign_geometry` still each own their own guard logic and their own error
  message; persistence wraps the OUTCOME of running all of them, not each one individually. A new
  `guarded(kind, ext, artifact, guards)` in `render/rejected.rs` takes a closure that runs the
  guards for one format and, on `Err`, hands the artifact and the error to the persist path. Both
  `markdown_from_context` and `html_from_context` now read as "run the guards, wrapped" instead of
  three sequential `?`s with no shared failure handling, and there is exactly one place a future
  fourth guard has to be added to for its rejections to persist too.
- The artifact persisted is the FULL render, not what the guards scanned. HTML's guards run over
  `visible_text(&html)` (style/script stripped, markup stripped), because that is what a fabricated
  DATA figure must surface in; but the diagnostic worth keeping on disk is the html a human can open
  in a browser, so `guarded("html", "html", &html, ...)` is called with `html`, and `visible` stays
  local to the closure that runs the guards over it.
- All three html guards ride the same closure: prose (`reject_foreign_numbers`), claim
  (`claim::reject_fabricated_claims`), and geometry (`geometry::reject_foreign_geometry`), in that
  order, short-circuiting on the first `?`. A rejection from any of the three persists the same
  html artifact; the caller does not need to know which guard fired to know what to do about it.
- `persist_rejected` never rescues and never masks: on a successful write it calls
  `eyre::Report::wrap_err` on the guard's own error to prepend the path, so the operator's first
  read names both the violation and where to go look at it; on any failure to persist (no
  resolvable `xdg_data_dir()`, `fs::create_dir_all` failing, the write itself failing, or the
  uniquify loop running out of suffixes) it logs a WARN naming the persist failure and returns the
  ORIGINAL error completely unchanged. There is no path through this function that turns a
  rejection into a success, and no path that silently drops why the render was rejected in the
  first place.
- Filename uniquification is a bounded counter loop (`MAX_REJECTED_SUFFIXES_TRIED = 1000`), not an
  unbounded `loop`, per the crate's no-magic-numbers convention: a directory that somehow never
  frees up a fresh name is a genuine failure (persist errors out, the guard error still
  propagates), not an infinite loop.
- Split into its own file, `report/src/render/rejected.rs`, rather than added inline to
  `render.rs`. `render.rs` was already at 1488 of the crate's 1500-line cap before this phase (the
  cap this same file's own doc comment on `reconciliation.rs` cites Phase 11 for splitting
  `chart`/`geometry` out over); the new persistence logic pushed it over on first pass, so it moved
  out along the same seam `reconciliation.rs`/`template.rs`/`workload.rs` already established.
  `guarded` is `pub(super)`, re-exported into `render`'s namespace with `use rejected::guarded;`, so
  both call sites and the crate's test convention (tests live in `render/tests/rejected.rs`, not a
  submodule of `rejected.rs` itself) needed no further change.

### Deviations

- None from the phase's spec. `xdg_data_dir()`, the `<YYYY-MM-DD>-<HHMMSS>-<kind>.<ext>` naming, the
  counter-suffix uniquification, the best-effort/fail-closed contract, and covering all three html
  guards are all built exactly as specified.

### Tradeoffs

- `Utc::now()` for the timestamp rather than `Local::now()`. Every other timestamp this crate
  writes into a filename or a report body (`report.since`/`report.until`/`report.generated`,
  `render.rs:1102-1106`) is UTC; a rejected-render filename sorting consistently with everything
  else on disk beat rendering it in the operator's local zone, and the design doc's own filename
  example does not specify which.
- The counter-suffix bound (1000) is generous relative to the one collision this path can plausibly
  see (a single operator, one render at a time, landing two rejections in the same wall-clock
  second) rather than tuned to a measured worst case, because there is no real-world data yet on how
  often that collision fires. Named as a `const` with a comment explaining the reasoning rather than
  left as a magic number, so a future reader can tell it was a judgment call and not an oversight.

### Open questions

- None.

### Amendment, 2026-07-27: pin "writes nothing on rejection" with its own test

Acceptance Criterion 3's last clause was unverified: "The render still fails and still writes
nothing to the output path." Everything else in the criterion had a test; this half only held
because `run` happens to call `generate` before `route`, with nothing pinning that order.

- Extracted the shared shape both branches of `run` already had (`let x = generate(...)?;
  route(x)?;`) into `generate_then_route(generate, route)`: `generate()?` short-circuits before
  `route` ever runs. Generic over the artifact type, so the markdown and html branches of `run`
  call the SAME function rather than each holding an independent copy of the ordering.
- Added it to `render/rejected.rs` rather than `render.rs`: it is the other half of the same
  guarantee `guarded` already lives there for (a rejection writes nothing an operator did not ask
  for), and `render.rs` was again at the crate's 1500-line cap once the new function and its call
  sites landed.
- New test, `render/tests/rejected.rs`:
  `a_guard_rejection_writes_nothing_to_the_output_path`. Calls `generate_then_route` directly (the
  real function `run` calls, not a reimplementation of its shape) with a `generate` that fails the
  way a guard rejection does, and a `route` that performs a REAL filesystem write into a `TempDir`.
  Asserts the overall result is `Err` and that the output path does not exist on disk.
- **Proved it bites.** Temporarily changed the signature to `generate: impl FnOnce() ->
  Result<T>` with `T: Default`, and the body to `let artifact = generate().unwrap_or_default();
  route(&artifact)` -- the exact bug class the team lead named (a future refactor calling `route`
  regardless of whether `generate` succeeded). Ran the new test: it failed immediately on the
  `result.is_err()` assertion (`route`'s `Ok(..)` return now propagated as the function's own
  result). Reverted to the real implementation and reran: passes. The test does not merely compile
  against the happy path; it fails when the ordering it exists to pin is broken.
- Verified end to end after the change: `cargo test -p report` -> 498 passed, 0 failed, 3 ignored
  (matches the post-Phase-4 baseline); `cargo fmt -p report -- --check` clean; `cargo clippy -p
  report --all-targets -- -D warnings` clean; `render.rs` at 1498 lines, under the 1500 cap.

No new open questions from this amendment.

## Phase 4 (scoped): Delete the KPI-deltas contradiction

### Design decisions

- Deleted only `or KPI deltas` from `report/templates/report-html.pmt:444`. The clause now reads
  "two to four factual bullets comparing this period against `prior` (both figures copied, never
  subtracted)", which matches the design's own headline rule with nothing left to contradict it.
- `report/templates/report.pmt:418` was read and left untouched. It never said "deltas"; it already
  reads "spend and session figures side by side (both copied, never subtracted)", which is correct
  as written.
- Added one test, `report_html_no_longer_asks_for_kpi_deltas`
  (`report/src/render/tests/templates.rs`), asserting `DEFAULT_HTML_PROMPT` no longer contains `KPI
  deltas` and still contains `both figures copied, never subtracted`, so the deletion is pinned
  without touching the surrounding sentence's real content.
- Read the prompt-edit ledger (`report/src/render/tests/templates.rs`) before touching anything.
  It is not a checksum or a single "both templates changed" gate: it is one hand-written assertion
  per rule, each naming the specific drift it guards against, over whichever templates that rule
  actually applies to. Nothing in the file asserts "every phase touches both templates" as a
  standing invariant; each test's own assertions are the only claim it makes. Since `report.pmt`
  carries no version of this contradiction, no ledger test needed a new assertion against it, and
  none was added.

### Deviations

- Shipped scoped down per Scott's 2026-07-27 post-STOP decision recorded in this doc's Resolved
  Decisions, not the Phase 4 the original plan described. Not built, and parked with Phase 3 in the
  new doc:
  - documenting `prior.change` in either template's context-block section;
  - rewriting the Month over Month section to quote `prior.change` verbatim;
  - naming the closed set (spend and sessions only get a computed comparison);
  - the decrease-direction instruction (copy the signed string, or use a direction verb with the
    unsigned magnitude, never both);
  - rewriting the `predates-fields` branch to let the change figures follow the caveat;
  - the banned-phrasings list ("above N sessions", "roughly four times", "nearly triple", "nearly
    doubled", `Nx` multipliers).
  - `prior.change` does not exist; Phase 3 was never built. Any of the above would have documented
    or quoted a field the binary never computes, which is a lie in a template the model reads
    verbatim.
  - Dropped `both_templates_name_the_prior_change_fields_and_forbid_a_delta_on_any_other_figure`
    from the required tests. It tests the parked work above; nothing in this phase's actual change
    calls for it.

### Tradeoffs

- Single-file edit against the design's plan of "both files change in this phase, ledger test
  enforces it." Chosen because the contradiction is real in exactly one file: a KPI delta is a
  subtraction, `report-html.pmt` asked for one and forbade it in the same clause, and `report.pmt`
  never asked for a delta at all. Editing `report.pmt` to manufacture a matching diff, or weakening
  the ledger to demand one, would both be worse than leaving a correct sentence alone. Verified the
  ledger holds no assertion forcing this before deciding: each of its four existing tests names its
  own concrete drift over both files because that phase's rule genuinely applied to both; none is a
  generic "N files must change" gate that this phase's narrower, single-file fix would trip.

### Open questions

- None.

