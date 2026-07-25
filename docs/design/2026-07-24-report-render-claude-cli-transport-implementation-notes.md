# Implementation Notes: `report render` over the local `claude` CLI

Companion to `2026-07-24-report-render-claude-cli-transport.md`. Append-only: a later entry
supersedes an earlier one rather than rewriting it.

## Phase 0: Spike both jobs against a real report (GATE)

### Design decisions
- Used the real `--since 2026-07-01` report as the single spike input (1,310 sessions, 42 repos,
  5.4MB collected JSON -> a 513,530-byte context block) instead of also building the synthetic
  high-output case the plan called for. The plan wanted the synthetic case to de-risk the html
  output ceiling; the real month answered that question with 3.2x headroom (19,574 output tokens
  against a granted 64,000), so a synthetic case would have re-answered a settled question at
  ~$3/run. Recorded as a deliberate reduction, not an oversight.
- Extracted the real context block via a temporary `#[ignore]`d scratch test appended to
  `report/src/render/tests.rs` (`build_context_block` is `pub(crate)`, so no production code was
  touched). The scratch test was reverted immediately after the dump; Phase 0 shipped zero code.
- Ran the cheap probes (env minimization, concurrency, lock-file accounting) on
  `claude-haiku-4-5` with a trivial payload rather than repeating the ~$3 opus jobs. Those probes
  test process/filesystem behavior, which is model-independent.

### Deviations
- **Both jobs pinned `claude-opus-4-8`; the markdown job re-pinned off `claude-opus-4-7`.** Scott's
  directive mid-execution, verbatim: "just use claude opus 4-8", given as the spike was about to
  verify the 4-7 pin. Consequence: AC3's "byte-identical request body" baseline is rebaselined onto
  the new pin (every other byte unchanged), and the doc's "markdown pin rejected by `--model`" risk
  row is moot. Design doc Non-Goals amended.
- **Model pins became config, not consts.** Scott's directive, verbatim: "those values should be
  configurable in the XDG .config .yml". Consequence: `Job::model()` returning `&'static str` is no
  longer expressible, so `model` is a `Transport::complete` parameter threaded from `RenderConfig`;
  and render's `clyde.yml` load becomes unconditional (widening the config-load blast radius the
  Staff Engineer flagged, since no flag opts out of needing a model pin). Design doc Data Model,
  API Design, and Phases 2/4 amended.

### Tradeoffs
- `model` as a port parameter vs. a `Job` field carrying the resolved pin. Chose the parameter: a
  `Job` that is `Clone + Copy` and purely identifies which job is running stays cheap and keeps
  every transport knob private to its transport. Stuffing a `String` pin into `Job` would make it
  allocate and would blur "which job" with "which model".
- Allowlisting `PATH` + `HOME` even though F4 proved neither is required. Chose belt-and-suspenders:
  depending on the runtime's `getpwuid` home-directory fallback would make a future runtime change
  present as "logged out", which is the misdiagnosis the whole error-message design fights.
- `CLAUDE_TIMEOUT = 900s` vs. something tighter like 300s. Chose wide: an overrun discards a call
  that has already been billed (~$3), so the expensive direction to be wrong in is too tight.

### Findings that changed the design (all folded into the doc)
- **F1** wall clock 145s (markdown) / 204s (html) both exceed the 120s `SUBPROCESS_TIMEOUT` ->
  `CLAUDE_TIMEOUT = 900s` is load-bearing, not tidy.
- **F2** `modelUsage` is a multi-entry map: the CLI makes an internal `claude-haiku-4-5` sub-call
  (~187K input, ~$0.19) despite `--tools ""` and `--safe-mode`, with no flag to suppress it. The
  model guard must be a KEYED lookup, never a scan-and-compare-all, or it bails on every good
  render. New AC12 makes this checkable.
- **F3** the doc's withdrawn cost argument is reinstated, corrected. Probes 7/8 measured a trivial
  payload and so only proved the harness preamble is gone (true). On the real payload the CLI bills
  ~242K tokens as a 1-hour cache WRITE at $10/Mtok where the api path bills plain input at
  $5/Mtok, plus the haiku sub-call: **$2.93 cli vs ~$1.53 api** for a markdown render (derived from
  the envelope and matching `total_cost_usd` to the cent). cli-default costs a key holder ~+$1.40
  per render, not "essentially nothing". Does not reopen the default (Scott's explicit call on
  keyless-access grounds); it corrects the claim and sharpens the bulk-render advice.
- **F4** `HOME` is not load-bearing: `env -i` with nothing at all still authenticates. Allowlist is
  `PATH` + `HOME` + `NO_UPDATE_NOTIFIER=1`, chosen fail-closed.
- **F5** a headless render leaves nothing behind: session JSONL 3,073 -> 3,073, locks 599 -> 599,
  no file newer than the run. Two concurrent renders both clean. Re-verified zero SQLite under
  `~/.claude` by content-typing every file, independently confirming the doc's struck row.

### Gate result
**PASSED for both jobs.** Both exit 0 keyless, `is_error: false`, `subtype: "success"`,
`stop_reason: "end_turn"`, `canonicalModel == claude-opus-4-8`; html starts `<!doctype html>` and
ends `</html>`. Contingency table resolves to row 1 (granted ceiling 64,000 >= observed html output
19,574), so both jobs go cli-default as designed. `--max-turns 1` confirmed accepted though still
undocumented in `--help`, so the minimum supported `claude` version pins at **2.1.219**.

### Open questions
- Quota/plan contention under concurrent renders is unproven either way. It cannot be demonstrated
  cheaply, and the fail-loud design surfaces it if it fires. Left as accepted risk, per the doc.
- The ~$0.19 internal haiku sub-call per render is unsuppressible by any flag in 2.1.219. Worth
  re-checking on a future CLI version; not worth blocking on.

## Phase 1: Extract subprocess helpers into `proc.rs`

### Design decisions
- `run_bounded` and `SUBPROCESS_TIMEOUT` are `pub(crate)` items in a `pub mod proc`. The module is
  `pub` to match its 12 siblings in `report/src/lib.rs` -- 10 of which are `pub` despite zero
  external use, so uniform `pub mod` is the crate's actual convention. Encapsulation is carried by
  the ITEMS (`pub(crate)`), which is tighter than the siblings' `pub fn`s.
- Added an exit-status debug line at the end of `run_bounded` (status code + stdout/stderr byte
  counts). The original logged only on entry and on timeout, so a bounded run that exited non-zero
  left no trace of its outcome -- the repo's logging rule wants the exit recorded too.

### Deviations
- None. Pure move; no behavior change.

### Tradeoffs
- `pub(crate) mod proc` (tightest that compiles) vs. `pub mod proc` (sibling symmetry). Initially
  wrote the former, then switched: it bought no real encapsulation because the items are already
  `pub(crate)`, and it made `proc` the only asymmetric module declaration in the crate.

### Open questions
- None.

## Phase 2: Introduce the `Transport` port and `ApiTransport`

### Design decisions
- `MARKDOWN_MODEL` / `HTML_MODEL` stay as `pub const`s in `summarize.rs` for this phase and are
  passed by the two `render.rs` call sites. Phase 4 relocates the default into
  `common::config` and threads the resolved pin from `RenderConfig`. Keeping the consts here for one
  phase means Phase 2 is independently green rather than dragging config plumbing forward.
- `ApiTransport`'s `Debug` is HAND-WRITTEN and redacts the key to a byte count. `unwrap_err()` in a
  test forced `Debug` onto the type; a derived one would print the api key into any log line or
  panic message. A transport whose purpose is to minimize credential handling must not be the thing
  that leaks one.
- `Job -> (max_tokens, stream)` is a single `api_limits()` mapping rather than two lookups, so the
  two facts are stated together per job and cannot drift apart.
- `FakeTransport` records into a named `Recorded` struct, not a 5-tuple. The tuple tripped
  `clippy::type_complexity`; a named struct fixes the lint honestly and makes each assertion read as
  the field it checks (an `#[allow]` was the wrong fix).

### Deviations
- None from the (already-amended) doc.

### Tradeoffs
- Byte-identical assertion as an exact serialized-string compare vs. a structural `json!` compare.
  Chose the exact string: it pins field ORDER and the `stream`-omitted-when-false behavior, which a
  structural compare would silently accept if either changed.

### Process note (my error, recorded because it cost work)
- To prove the byte-identical tests bite, I sabotaged the code and then reverted with
  `git checkout <path>` -- on files whose Phase 2 changes were NOT yet committed. That reverted all
  of `summarize.rs` to its pre-phase state and left the sabotage in the untracked `api.rs`. Both
  tests DID fail as intended (bite proven: flipping markdown's `stream` to `true` failed
  `markdown_body_is_byte_identical_to_baseline` and `job_api_limits_map_to_todays_behavior`;
  re-pinning to `claude-opus-4-7` failed those plus `both_jobs_default_to_opus_4_8`), but the
  restore cost a re-apply. Correct order is commit first, then break, then `git checkout` is safe.
  Applied for the rest of the phases.

### Open questions
- None.

## Phase 3: `CliTransport`

### Design decisions
- The child process spec is built as DATA (a `Spawn` struct: program, args, complete env) and only
  then turned into a `Command`. Reason: `std::process::Command` exposes no getter for "was
  `env_clear()` called", so a test against the built `Command` could not prove AC4's central claim
  that the child inherits NOTHING. Asserting on the struct can.
- The guard chain is a pure `check_envelope(envelope, job, model, observations)`, separate from
  `complete()`. Every failure mode is then driven by a recorded envelope fixture; no test shells out
  to the real `claude` binary, per the doc's testing strategy.
- `Job::max_output_tokens()` MOVED from api-private (where Phase 2 put it) onto `Job` itself. The doc
  required the cli transport to compare `usage.output_tokens` against "the job's ceiling", which is
  impossible if the ceiling is api-private. It is genuinely shared -- api SETS it on the wire, cli
  CHECKS it because it cannot set it -- so it is a fact about the job. Only `stream` stayed
  api-private, since the cli transport has no delivery choice and would have to ignore it.
- `MIN_CLAUDE_VERSION` is reported, NOT enforced as a pre-flight gate. A version-string parse is a
  foreign format that could change, and failing closed on a CLI that actually works is the wrong
  trade. The version is logged every render and named in every failure instead.
- `probe_version` failure is non-fatal (reports `unknown`). The version exists for the operator; not
  being able to read it must not fail a render. Confirmed live: the fake `claude` used to test AC7
  had no `--version` and correctly reported `version: unknown`.
- Observations are passed INTO `check_model` rather than wrapped around its error. `wrap_err` made
  the observations the OUTERMOST message, so a plain `{}` format -- what the CLI's top-level error
  printer uses -- showed only "binary: ... version: ..." and HID the real cause. Caught by a test
  that asserted on the message; the other six guards format observations inline and this now matches.

### Deviations
- None from the amended doc.

### Tradeoffs
- Tolerating leading stdout noise (seek the first `{`) vs. requiring stdout to begin with the JSON
  root. Chose tolerance plus `NO_UPDATE_NOTIFIER=1`: the failure mode it prevents is a FALSE NEGATIVE
  on a call already billed ~$3, which is the expensive direction.
- `String::from_utf8` (hard error) vs. `from_utf8_lossy` on the envelope. Chose the hard error: the
  envelope carries the artifact, and lossy decoding would silently substitute replacement characters
  inside a document about to be published. Lossy is used only for the stderr preview, which is display.

### Bite proofs (all six confirmed)
Sabotaged each guard and watched its test fail, then restored from the commit: stop_reason (accept
anything), empty result, output ceiling, is_error, the model check turned into a scan instead of a
keyed lookup, and the leading-noise seek. All six bit.

### Open questions
- None.

## Phase 4: Wire transport selection

### Design decisions
- Split resolution into a pure `resolve_transport(selection, claude_present, key_present, format)`
  plus a two-line impure probe in `render.rs`. The whole precedence matrix is then unit-testable
  without touching PATH or the process env.
- `Llm` (Auto|Api|Cli) and `TransportKind` (Api|Cli) are SEPARATE types. `auto` is a request, not an
  answer; separate types mean a resolved value can never still be `Auto`, so no call site handles a
  case that cannot happen.
- The model-pin defaults live in `common::config`, and `summarize`'s Phase-2 `MARKDOWN_MODEL` /
  `HTML_MODEL` consts were DELETED rather than kept alongside them. Two copies of the same default
  string is exactly the drift the "a field derived from another never diverges" rule warns about.
- `common::config::RenderConfig`'s `Default` is hand-written. A derived one would give
  `String::new()` for the two pins, and an empty `--model` argument is not a valid model -- it would
  fail at the transport instead of resolving to the documented pin.

### Deviations
- **AC10's second clause was inverted, and the doc was amended to match.** It originally asserted that
  `--llm` present means `clyde.yml` is not loaded at all. The model-pin directive makes the load
  unconditional (render always needs a pin, and no flag opts out), so the assertion became the
  opposite: a malformed config must fail loudly even when BOTH `--format` and `--llm` are given.
  Verified live.

### Tradeoffs
- Accepting the wider config-load blast radius vs. keeping the lazy load and sourcing pins elsewhere.
  Accepted the wider radius: a config key that is not read is not config. The cost is named in the
  doc and covered by tests.

### Incident: the unconditional load surfaced a pre-existing test race
Six pre-existing `resolve_command(Render(..))` tests called it bare, without holding `ENV_LOCK`. They
were safe before only because passing `--format` meant config was never read. Once the load became
unconditional they raced the tests that point `$XDG_CONFIG_HOME` at a temp dir, and the symptom was
an intermittent failure in an unrelated assertion (`resolve_command_render_threads_outliers_into_config`
failed in the suite but passed alone). All six now go through `with_clyde_yml`, and the coupling is
documented at the top of the test module so the next test added does the same. Verified with three
consecutive full-suite runs.

### Open questions
- None.

## Phase 5: Docs and shakedown

### Design decisions
- `report/README.md` gained a full "LLM transport" section rather than a one-line edit: cli-as-default
  has three consequences a reader must know before relying on it (no fallback after selection,
  automated callers must pin `--llm api`, and the ~1.9x per-render cost), and the doc owed all three.
- The root `README.md` gained the `render:` config block, which was previously only described in
  prose as "a `render:` section whose `format` sets ...". It now shows all four keys.
- Added a transport-selection `info!` log in `render.rs` covering BOTH transports. `CliTransport`
  already logged itself, but AC9 asks that a render report which transport was selected -- an
  api-path render would have logged nothing, so an operator could not tell what paid for it.

### Deviations
- The plan lists "README, CLAUDE.md". This repo has no `CLAUDE.md`, so only the two READMEs and the
  `--help` text were updated. Nothing was skipped.
- Ran the live shakedown by hand against the real binary rather than invoking `/cli-shakedown`,
  because the surface under test makes billed model calls (~$3 each) and the acceptance criteria name
  the exact invocations to run. Every AC was exercised; see below.

### Live verification (release binary, real reports)
| AC | result |
|---|---|
| AC1 (keyless markdown, no `--llm`) | PASS on a 173-session window: 11,791 bytes, 9 `## ` headers, no `Generated offline via` |
| AC1c (default routing) | PASS: log shows `selected=Cli (requested=Auto)` |
| AC2 / AC2b (keyless html, `--llm cli`) | PASS: 49,964 bytes, starts `<!doctype html>`, ends `</html>`, 303s |
| AC5 | PASS, proven live rather than only in fixtures: Guard 6 refused a 16,117-token markdown artifact |
| AC6 (neither credential) | PASS: exit 1, names both remedies, NO partial artifact written |
| AC7 (failing `claude` + valid key) | PASS: exit 1, reports observations, does NOT fall back to the key, names `--llm api` |
| AC9 | PASS: transport, binary path, and version all logged |
| AC10 (`render.llm: api` beats auto) | PASS: demanded a key instead of using the present `claude` |
| AC10b (malformed config, both flags present) | PASS: fails loudly naming the config file path |

AC1b (markdown + explicit `--llm cli`) and AC2c (html + default routing) were not run as separate
billed calls: the flag-vs-default resolution is one code path shared by both formats, it is covered
exhaustively by unit tests, and each format was live-verified on one side of it (markdown by default,
html explicit). Two billed renders instead of four; recorded so the gap is visible rather than implied.

### Open questions
- **The markdown job's 16,000-token ceiling is too tight for the largest months.** The full
  1,310-session July report produced 16,117 output tokens and Guard 6 correctly refused it. NOT a
  cli-transport regression -- the api path would bail on the same month via `stop_reason: max_tokens`,
  because it puts `max_tokens: 16000` on the wire. Three priced options are written into the design
  doc's Open Questions; the choice is Scott's because each costs something (raising the ceiling
  retires AC3's byte-identical contract, accepting it leaves a hard failure on big months, making it
  configurable adds two keys). Everything else shipped and is verified.

## Implementation audit response (2026-07-25)

Panel ran in Mode 2 against the branch. Both reviewers completed. Architect (Gemini) returned zero
findings and a clean bill on all twelve ACs; that verdict was NOT trusted, because its citations were
systematically off by hundreds of lines and it "verified" the cost arithmetic using haiku rates that
do not appear in `pricing/data/pricing.json`. Every Staff Engineer (Codex) finding landed on real code
with correct line numbers. Convergence was thin by construction, so each finding was verified
independently before acting.

**Both direct questions confirmed.** The cost arithmetic is correct to the cent against
`pricing.json` read from the commit ($2.9305 cli vs $1.5303 api, ratio 1.915x). The markdown
16,000-token ceiling failure is confirmed NOT cli-specific: `api.rs` calls `check_stop_reason`
unconditionally and the byte-identical fixture proves `max_tokens: 16000` goes on the wire, so the api
path refuses the same month via a different mechanism. Identical outcome, no artifact either way.

### Fixed

- **M1 (must-fix). AC4's central security property was not test-enforced.** `Command::get_envs()`
  reports only explicit OVERRIDES, so the old assertion on its length passed with `env_clear()`
  deleted while the child inherited the parent's whole environment, including three measured secrets.
  Green test, absent property. Replaced with a test that spawns `/usr/bin/env` in place of `claude`,
  plants four secrets in the parent, and asserts the child's environment is EXACTLY the allowlist --
  by name and by value. The old comment claiming this was unprovable without the `claude` binary was
  simply wrong; `/usr/bin/env` is hermetic and the scope boundary is intact. Proven to bite.
- **M2. AC11's api half was not regression-proof.** Every `build_body` test passed the DEFAULT
  consts, which equal the literal the fixtures assert, so ignoring the `model` parameter would have
  left them all green. Added a sentinel-model case. Proven to bite in the exact shape predicted: the
  sabotage fails the sentinel test while both pre-existing byte-identical tests still pass. Note the
  crude version of this sabotage (hardcode and drop the param) cannot even compile, because the crate
  runs `#![deny(unused_variables)]` -- a stronger guarantee than a test, but one that does not cover
  the plumbing-ignored shape, which is why the sentinel case is still needed.
- **M3. Two comments asserted behavior the code no longer had.** `config.rs` still said clyde.yml was
  loaded "ONLY here" and that lazy loading meant a malformed config could not break `render` -- eight
  lines above the render branch that now loads it unconditionally. Same false claim in `lib.rs`. Both
  corrected to name `merge` as the only config-independent subcommand. Staff caught the `lib.rs` one;
  the `config.rs` one was found by the panel's own reconciliation, not by either reviewer.
- **M4. The `ESCAPE_HATCH` invariant was stated absolutely and honored partially.** Accepted the
  panel's narrowing over the Staff Engineer's literal reading: Guard 4 (truncation) and Guard 6
  (over-budget) correctly OMIT the hatch, because the api path enforces the identical per-job ceiling
  and would fail the same way -- pointing at `--llm api` there would be a remedy that does not remedy.
  The two `check_model` bails DO now carry it, since the api path puts the pin on the wire and honors
  it. One code change plus two comment corrections, not four code changes. Added tests for BOTH sides
  of the contract, which nothing previously asserted in either direction; both proven to bite.
- **M5. All twelve AC checkboxes were still unticked in a doc marked `Status: Implemented`.** Set to
  ground truth. AC1 stays deliberately UNCHECKED and marked PARTIAL: it does not hold for the largest
  report, which is the open question. A tidy checklist that overstates is worse than an honest one.

### Accepted the panel's demotions

AC9 log-string assertions (brittle, buys little), AC5's literal timeout-kill and stale-flag tests
(both collapse into covered classes), the AC1b/AC2c live-verification gap (a cross-product of two
independently proven axes; two more billed renders would prove nothing), and the absent annotated
example `clyde.yml` (repo-wide pre-existing gap, both READMEs carry annotated blocks). Also accepted
A1: the ceiling on `Job` is the right seam and not a lying field, since api sets it and cli checks it.

### Found while fixing, not in the audit

**The `ENV_LOCK` pattern was broken across modules.** Each test module declared its OWN
`static ENV_LOCK`, and two separate mutexes do not serialize against each other at all. The new M1
test reads the entire parent environment while `summarize::api`'s tests mutate `ANTHROPIC_API_KEY`
under a different lock, which produced exactly one intermittent two-test failure before it was
diagnosed. Replaced all five per-module locks with a single crate-wide `crate::ENV_LOCK` and
documented why it must stay crate-wide. Verified with three consecutive full-suite runs. This is the
same race class as the Phase 4 incident, one level up: fixing it per-module was treating the symptom.

### Process note

Twice in this work I used `git checkout <path>` to undo a deliberate sabotage on a file whose real
edits were not yet committed, and destroyed those edits both times -- the second time silently
reverting all four M4 changes, caught only by re-grepping. Bite proofs now copy the file aside and
restore from the copy. `git checkout` is for discarding changes you want gone, never for undoing a
temporary edit sitting on top of work you want to keep.
