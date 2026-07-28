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
