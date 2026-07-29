# Implementation Notes: Render Inversion

Running record of decisions, deviations, tradeoffs, and open questions while executing
`2026-07-29-render-inversion.md`. Append-only: a later entry supersedes an earlier one rather than
rewriting it.

## Phase 0: Prove marquee sibling-SVG URL resolution and the slot call shape

### Design decisions

- **Slot-call verification used the exact `CliTransport` argv**, not a bare `claude -p`
  (`report/src/summarize/cli.rs:80-119`): `--model claude-opus-4-8 --output-format json
  --system-prompt <s> --tools "" --safe-mode --strict-mcp-config --no-session-persistence
  --max-turns 1`, with the brief on stdin as the same fenced JSON block the api transport puts in
  its user message. Verifying a different call shape than the one Phase 2 builds would prove
  nothing about Phase 2.
- **Validated the reply mechanically rather than by eye** -- placeholder-span strip, then a Unicode
  `\p{N}` scan, brace-residue check, and block-structure check. That is the Phase 2 validator's
  algorithm run by hand, so the criterion measured is the criterion that ships.

### Deviations

- **Criterion (1), marquee sibling-SVG URL resolution, is NOT verified.** The Okta device grant
  timed out twice unapproved, so no live publish ran. Per the 2-strike rule I stopped rather than
  re-firing a third code. What IS established, statically and dispositively:
  marquee's routes are `/p/{space}/{slug}` (post) and `/p/{space}/{slug}/{file}` (asset)
  (`marquee/server/src/routes.rs:165-180`), and `rg` finds NO `<base href>` anywhere in the marquee
  tree. By RFC 3986 relative resolution, `![](chart-0.svg)` on a post at `/p/{space}/{slug}`
  resolves to `/p/{space}/chart-0.svg`, which matches the two-segment POST route and cannot serve
  an asset. That is the doc's own High-likelihood risk, confirmed by construction.
  This does NOT gate Phase 1: the doc requires the chart-table form in Phase 1 regardless (PDF and
  stdout can never carry sibling files), so both forms are implemented and tested. It gates only
  the CHOICE of default for the marquee/file path, which is a one-line change in
  `render::chart_mode`. Carried as an open question below.

### Tradeoffs

- Statically proving the URL-resolution failure vs. blocking Phase 1 on an Okta approval: proceeding
  is safe because the fallback is already built and the switch is one line. Blocking would have
  stalled every remaining phase on an approval that is not mine to give.

### Open questions

- **Does a marquee markdown post resolve a relative sibling-asset reference?** Static analysis says
  no. Needs one live publish to confirm, then either (1) a small marquee `<base href>` PR shipping
  BEFORE this doc's PR, or (2) flipping `render::chart_mode` to return `ChartMode::Table`
  everywhere. Both branches are pre-recorded in the design doc; I need Scott's marquee login (or
  his call to skip straight to the table form).

## Phase 1: Deterministic document renderer

### Design decisions

- **`build_views` lives in `render/document.rs`, not `render.rs`.** `render.rs` sat at EXACTLY the
  1500-line bloat cap (`.otto.yml:7`), so the phase had to be net-negative on that file. Moving the
  `ContextBlock` assembly into the document layer both created headroom and put the assembly next to
  its two consumers. `render::build_context_block` now calls it, so the eval's serialized block and
  the document layer's tables are built by ONE function -- the design's "one source, two consumers"
  claim is now structural rather than asserted.
- **`ViewOpts` struct** (`render.rs`) carries the four same-shaped optional inputs
  (`include_tradeoffs`, `prior`, `reconcile`, `reconcile_user`) so callers cannot transpose them.
- **The fact registry is wired in Phase 1, not Phase 2.** `#![deny(dead_code)]` forbids landing it
  unreferenced, so `document::render` builds the registry and `section()` interpolates every slot's
  prose through it at placement time. Phase 2 adds generation and validation IN FRONT of a path that
  already exists and is already tested.
- **An unresolvable placeholder drops the whole slot, with a WARN** (`document::section`). A
  sentence built around a missing figure is a complete-looking claim with a hole in it; silence is
  better. This is stricter than the doc's letter (which makes an unknown key a validation failure in
  Phase 2) and is the same outcome one layer later, so a slot that somehow reaches the document
  layer with a bad key still cannot print it.
- **Registry size and sentinel guards.** `MAX_FACT_BYTES = 96` keeps sentences (`basis.note`, the
  reconciliation scope note) out of the registry -- those are prose the document layer prints
  verbatim, and registering them would hand a slot a paragraph to paraphrase. `insert_measured`
  refuses the efficiency view's literal `"n/a"`, so a slot can never print "cache reuse stayed high
  at n/a".
- **`slug()` generalizes the doc's `/` -> `-` rule** to every non-alphanumeric character, because
  the same key segment also carries model names and agent types like `(main-session)`. Collisions
  are a `debug_assert` failure, not a silent overwrite.
- **`quantity()` for irregular plurals**, separate from `plural()`. An English-pluralization engine
  inside a report renderer is the kind of cleverness that fails silently.
- **Section mapping.** Rust owns: frontmatter, header, Quantified Output, Cost Summary,
  Reconciliation, Agent-Type Cost Attribution, The Efficiency Story, per-repo stat lines under What
  This Funded, the by-day charts and Outlier table under Usage Profile, Month over Month. Slots own:
  Executive Summary, What This Funded narrative, Usage Profile narrative, Tradeoffs, Conclusion.

### Deviations

- **`render_via_opus_markdown` deleted** (a 12-line `cfg` -> `Pins` adapter). Once
  `generate_markdown` routes to the document layer, nothing calls it, and `deny(dead_code)` will not
  allow it to sit unreferenced. The PATH it adapted is untouched: `markdown_from_context` and every
  guard behind it stay alive and are still exercised by the eval, exactly as the phase requires.
  This is the narrowest possible deletion consistent with "DELETE NOTHING in this phase".
- **`route_markdown_artifact` replaced by `route_document_artifact`**, which carries sibling assets;
  the markdown side no longer goes through `generate_then_route`. That wrapper exists to enforce "a
  rejected render writes nothing to the output path", and there is no longer anything on this side
  that can reject a whole artifact. It stays alive for the html path until Phase 3.
- **The `**Sessions:**` header line is now grammatical** ("44 across 7 repositories", "17 sessions
  across 1 repository"). `report.pmt:245` hardcoded the plural noun, and the committed goldens
  faithfully reproduce "9 across 1 repositories". Rust owns this prose now; Phase 3 rebaselines the
  goldens regardless.
- **The Forward-Looking section is not rendered.** `report.pmt` defined it, and the doc's Non-Goals
  say "no new report sections" (not "no removed"), but its content contract is "in-flight signals
  from late-period sessions (handoff docs, mid-execution design phases)" -- i.e. session summaries
  and titles, which the design explicitly forbids putting in a slot brief ("user-derived free text
  ... that slots must never receive"). There is no fact-registry input that could produce it, and
  the doc's five-slot enum has no slot for it. Flagged for Scott rather than silently dropped;
  see Open questions.

### Tradeoffs

- **Interpolation at placement time vs. inside the slot module.** Placement-time means Phase 1 can
  land the registry wired and tested with stub prose; Phase 2 then only adds generation and
  validation. The cost is that `document.rs` knows about placeholders at all.
- **Test fixtures reused from `render/tests.rs`** (`ts`, `pricing`, `sample_report`,
  `report_with_outcomes`, `report_with_efficiency` widened to `pub(super)`) rather than duplicated.
  `render/tests.rs` is already 1445 lines, so adding the new tests there would have risked the bloat
  cap; reusing the fixtures avoids both duplication and that risk.
- **The licensing test compares numeric tokens against every string leaf of the serialized block.**
  Substring matching is loose (a single digit passes if any licensed string contains it), which is
  the same looseness `quotable` had. It is paired with a `#[should_panic]` break-it proof on a
  fabricated `$123,456.78`, so the test is demonstrated to bite rather than assumed to.
- **Chart y/x label placement uses fixed offsets** inside the binary-owned 1000x300 viewBox rather
  than computed positions. `chart.rs` owns every coordinate that encodes data; these constants only
  place labels, so no arithmetic on data happens in the SVG assembler.

### Open questions

- The Phase 0 marquee question above (chart default: sibling SVG vs. table form).
- **Forward-Looking:** drop it permanently (my read: its inputs are structurally unavailable to a
  slot), fold it into the `closing` slot as one combined section, or give slots a curated
  late-period signal the registry does not currently carry? Scott's call; nothing else blocks on it.

## Phase 2: Slot generation and degradation

### Design decisions

- **`comrak` added with `default-features = false`.** The default feature set is `cli` +
  `syntect-onig` + `bon`, which drags a CLI binary's dependency tree (syntect, onig, clap,
  shell-words, xdg, yaml-rust) into a library that only needs a parser. Grammar parity with marquee
  is preserved because parity lives in the PARSER and the extension flags, not in syntax
  highlighting: `structure_ok` enables exactly the four extensions marquee's markdown lane enables
  (`table`, `strikethrough`, `tasklist`, `autolink`, per
  `marquee/server/src/render/markdown.rs:45-49`).
- **The structural check is a strict node ALLOWLIST**, not a rejection list. Anything not named
  (`Document`, `Paragraph`, `Text`, `SoftBreak`, `Emph`, `Strong`, `Strikethrough`, `Code`, `Link`,
  `Escaped`) is a violation, so a comrak upgrade that adds a node type fails closed instead of
  waving it through. This also covers inline raw HTML, which a block-only check would miss.
- **Slot model pin is `render.markdown-model`**, per the design's explicit config list (which retires
  `html-model` and adds only `slot-max-output-tokens`). Flagged as a naming wart below.
- **`DEFAULT_SLOT_MAX_OUTPUT_TOKENS = 1_500`.** Phase 0's live `executive-summary` call returned 217
  output tokens, so this is ~7x the measured shape -- generous for a slot, and small enough that a
  model which starts writing a whole document hits the ceiling instead of billing for one.
- **A transport error degrades immediately, with NO retry** (`slots::one`). The retry exemption the
  design negotiated is scoped to CONTRACT VIOLATIONS; re-firing a failed subprocess is exactly what
  the no-retry doctrine forbids. Only a validation failure earns the second attempt.
- **The retry names the specific violation** (`Violation: Display`), e.g. "the prose contained the
  numeric character '1'". That naming is what distinguishes this from a blind re-fire, and it is
  asserted by a test rather than left to inspection.
- **An allowlisted key the registry does not carry is OMITTED from the brief**, so the model is never
  shown a fact it cannot truthfully cite -- and the validator, reading the same allowlist, would
  reject it if the model cited it anyway.
- **`generate` is infallible by signature** (returns `SlotProse`, not `Result<SlotProse>`). The type
  makes "no slot failure can cost the artifact" unrepresentable-otherwise rather than merely
  intended.
- **`slot_prose`/`no_transport` in `render.rs` degrade loudly**, to both the log and stderr. An
  operator must be able to tell a thin report from a broken one without re-running.

### Deviations

- None from the design. The five slots, the per-slot allowlists, the one-retry ladder, the
  empty-plus-WARN degradation, `Kind::Slot`, `render.slot-max-output-tokens`, and the
  post-interpolation re-check are all as specified.

### Tradeoffs

- **Allowlist sizes run four to seven keys**, against the design's "~5 each". `executive-summary`
  needs seven to state the temporal shape (`period.days`, `period.active-days`), the scale
  (`totals.sessions`, `totals.repo-count`), the cost (`totals.spend`), the concentration
  (`repos.top`), and the efficiency (`efficiency.cache-read-share`) -- dropping any one of those
  makes the section's stated intent unwritable. Still a handful, still per-slot, blast radius still
  bounded.
- **The brief cap is a `debug_assert`, not a runtime bail.** A brief over 4,096 bytes means a
  collection leaked into an allowlist, which is a programming error caught in tests; failing a
  production render over it would trade a real artifact for a hypothetical one.
- **`Violation` carries a `String` for the off-allowlist key and the node name** rather than borrowing.
  Slots run a handful of times per render; the allocation is irrelevant and the ownership is simpler.
- **Sequential slot calls**, not parallel. The design calls parallelism unrequested scope. Measured:
  four slots at ~6s each on the live run.

### Open questions

- **`render.markdown-model` now governs slot generation, and the name no longer tells the truth.**
  The design's config list retires `html-model` and adds `slot-max-output-tokens` but says nothing
  about the model key, so I followed it literally rather than inventing `render.slot-model`. Once
  `Kind::Markdown` dies in Phase 3 the key's name is actively misleading. Rename to
  `render.slot-model` in Phase 4 (a one-line config change plus the README example), or leave it?
  My read: rename, because "names tell the truth" and Phase 4 is already editing that example.
  Not blocking either way.

### Live verification (not a substitute for the shakedown)

One real `--llm cli` render over the medium fixture: all four unconditional slots conformed on the
FIRST attempt, no retries, no empty sections. Every interpolated figure matched the document's own
tables (`$671.28`, `25 of 30`, `44 sessions`, `7 repositories`, `92.2%`, `openpipe-oss/quill`,
`$264.79`, `northwind-media`, `claude-opus-4-7`). One residual of the accepted class was visible:
`what-this-funded` cited `agent-types.top` as "most of it flowing through (main-session)", which is
true but awkwardly placed -- exactly the "allowlisted key in the wrong sentence" residual the design
accepted with prompt-plus-watch and built no machinery for.

## Phase 3: Delete the guard stack and HTML formats, repoint eval, rebaseline goldens

One atomic commit, as the design requires: eval is a live consumer of both the guard machinery and
the whole-document path, so no boundary inside this phase could have been `otto ci` green.

### Design decisions

- **`Guards` was repurposed, not deleted.** Its whole purpose was the whole-artifact rejection rate
  -- the metric this design exists to drive to zero. Post-inversion that rate is STRUCTURALLY zero,
  and a field that can only ever read `0.0%` is a lying field. It now counts
  `slots_attempted` / `slots_degraded` / `slot_degradation_rate`, which is the only degradation left
  and the number an operator should actually watch, plus `markdown_failures` for the infrastructure
  case (unreadable fixture, unresolvable transport) that is the only way a render can now produce
  nothing.
- **`--write-goldens` renders STUBBED and skips the judge**, making golden regeneration a FREE,
  offline operation. The design says goldens are "deterministic, slots stubbed"; paying for live slot
  prose only to throw it away would be waste. A grading run (`otto eval`) renders live.
- **The judge scores SLOT PROSE, not the whole artifact.** That follows the inversion directly: every
  figure in the document is Rust's and needs no scoring, so the only thing left to judge is the only
  thing a model wrote.
- **New byte-exact golden test** (`a_stubbed_render_reproduces_every_committed_golden_byte_for_byte`)
  plus a determinism test. This is what the inversion bought: the artifact is reproducible, so a
  golden is a real fixture rather than a sample of one stochastic render, and the comparison runs
  offline and free in `otto ci`.
- **`RenderContext` kept only `json`.** Its `facts` field carried the three quotable sets that
  licensed every figure in a model-authored artifact; nothing licenses anything now. It relocated to
  `render/facts.rs`, which had to become `pub(crate)` for the eval to name the type.
- **`stream` removed from `MessagesRequest` entirely** rather than pinned to `false`. It was already
  `skip_serializing_if` when false, so the wire bytes are byte-identical to what the non-streaming
  path always sent -- asserted by a new test. Streaming existed only to keep a long html generation
  under the 300s idle wall; a slot and a judge verdict cannot approach it.
- **`Kind::Slot` inherited the transport tests** that `Kind::Markdown` used to carry, and
  `Kind::Judge` inherited the "larger job" role `Kind::Html` played in the per-job ceiling tests. The
  envelope fixture's output-token count moved from 12,706 (a whole document) to 217 (the Phase 0
  slot measurement), and the over-ceiling probes from 40,000 to 5,000 -- which sits strictly between
  the slot ceiling (1,500) and the judge ceiling (32,000), the property those tests need to be
  non-vacuous.

### Deviations

- **`medium/eval.yml` drops its `pr-reference` required citation.** It existed to prove the
  quotable-facts whitelist did not false-positive on a prose `#118`. The whitelist is deleted, and
  the document layer states observed PRs as COUNTS ("16 PRs opened") and never writes a bare `#N`, so
  the requirement is both moot and structurally unsatisfiable. The MATCHER stays and is still tested
  directly; only the fixture requirement is gone, and the corpus test now asserts that no fixture
  requires it (a requirement nothing can satisfy is worse than no requirement).
- **`render/tests/{excerpt,geometry,quotable,rejected,templates}.rs` deleted wholesale**, along with
  `summarize/tests.rs`'s postprocess/SSE/fence suites: every one tested a deleted function.
  `summarize/tests.rs` shrank to the ceiling-key and stop-reason contracts, which survive.
- **Two tests removed beyond the delete list**: `cli::tests::template_help_enumerates_the_six_actual_placeholders`
  (the `--template` flag is gone) and `config::tests`'s three `--template`-validation tests. Replaced
  by new tests asserting the retired surfaces now FAIL BY NAME: `--format html` / `marquee-html` as
  unknown clap values, `format: html` in `clyde.yml` as a rejected enum value, and
  `html-model` / `html-max-output-tokens` as `deny_unknown_fields` errors. Retiring a surface silently
  is how an operator's stale config keeps "working" while doing something else.

### Tradeoffs

- **`build_context_block` still serializes the block separately from `for_eval`'s render**, rather
  than `for_eval` returning it. Both call `document::build_views` over the same inputs, so they cannot
  disagree; returning it would have made `EvalRender` carry a field with one consumer and two
  sources. The cost is one extra serialization per eval fixture, which is free relative to the
  model calls the eval makes.
- **`slots::count` duplicates `generate`'s loop shape** to give the eval a denominator. A counter
  threaded out of `generate` would have been exact rather than derived, but it would put a mutable
  output parameter on an otherwise clean infallible function; the two are pinned together by
  `tradeoffs_is_generated_only_when_requested`.

### Open questions

- None new. The two from earlier phases (the marquee chart default, and whether Forward-Looking
  should return in some form) still stand.

### Delete tally

5,624 lines removed as whole files (`claim.rs`, `quotable.rs`, `geometry.rs` and their tests,
`render/rejected.rs`, `render/template.rs`, five render test modules, `report.pmt`,
`report-html.pmt`, three `golden.html`), plus the guard flow, the HTML pipeline, and the SSE
machinery excised from `render.rs`, `summarize.rs`, and `summarize/api.rs`. `render.rs` went from
1,500 lines (exactly at the bloat cap) to well under it.

## Phase 4: Docs, config example, statuses

### Design decisions

- **The README config example gained two paragraphs it did not have before**: why the slot ceiling is
  small (Rust writes the report; the model fills a few sentences), and the degradation contract (a
  render cannot fail because of the prose). Both are the operator-facing statement of the inversion's
  two load-bearing properties, and neither was inferable from the key list alone.
- **`markdown-model` is documented as pinning "the prose slots and the eval judge"**, which is what it
  now does. See the open question below about its name.
- **Both superseded docs carry a reasoned addendum, not just a status flip.** The repair-turn note
  records the measurement that killed it (two releases of licensing expansion left the rejection rate
  flat at 5/9 then 6/10) and what carried forward (the accepted residual class, and that its review
  log's rejected alternatives stay rejected). The html-parser note records that comrak was adopted for
  a much smaller job using that doc's OWN argument -- structure injection needs no leading `#`, so a
  string scan cannot see a setext heading or a table.
- **`Guards`'s doc comment was rewritten rather than trimmed.** It described two stochastic guards and
  the rate that calibrated them; that whole paragraph is now false. It says instead why the rate
  reached zero structurally and what replaced it.

### Deviations

- **None from the design.** The doc's Phase 4 success criteria are both met: the README grep exits 0,
  and both superseded docs carry the specified status and addendum.

### Tradeoffs

- **The design doc's status flipped to `Implemented` while two SHAKEDOWN acceptance criteria remain
  open.** That is the honest reading: those two gate the TAG, not the implementation, and the doc's
  own Acceptance Criteria section separates them. The doc's header now records the Phase 0 outcome
  verbatim, including what was NOT verified, so "Implemented" cannot be read as "fully proven live".

### Open questions

- **Rename `render.markdown-model` to `render.slot-model`?** With `Kind::Markdown` deleted, the key's
  name no longer describes what it pins (prose slots and the judge). My read is that it should be
  renamed -- "names tell the truth" -- but it is a user-visible config key, so the rename is Scott's
  call, not a sweep-up. Left as-is and documented accurately in the meantime.
