# Design Document: configurable `report render` output ceilings

**Author:** Scott Idler
**Date:** 2026-07-25
**Status:** Implemented (all three phases shipped; live-verified keyless on the full 2026-07 month, see AC-C5)
**Review Passes Completed:** 5/5
**Funnel position:** five passes, then a two-seat review panel over two rounds (Architect on Gemini, Staff Engineer on Codex, both completed both rounds). Sixteen findings, every one dispositioned in "Review Panel Dispositions". The panel killed Phase 1's stated proof of behavior-neutrality (impossible as written AND insufficient if it had compiled), found two unnamed compile breaks, and found both mechanical completeness gates blind to the sites they most needed to see. Open Questions empty. Ready to build, starting at Phase 1.
**Closes:** the single Open Question left by `docs/design/2026-07-24-report-render-claude-cli-transport.md`
**Sibling:** `docs/design/2026-07-25-pricing-feed-staleness-and-index.md`, separate work, different crate and branch, no shared code

## Summary

The markdown render job has a hardcoded 16,000-token output ceiling. The full 1,310-session July report produces 16,117 tokens, so `report render --format markdown` fails on the largest months. Make both ceilings config keys in `clyde.yml`, siblings of the model pins that already live there, and raise the markdown default to 32,000. That forces a rebaseline of AC3, the byte-identical api request-body assertion, and retires the "markdown stays byte-identical to the pre-HTML behavior" contract.

## Problem Statement

### Background

- The cli-transport design shipped all six phases and is live-verified. It left exactly one Open Question, surfaced by its own Phase 5 shakedown rather than by any test.
- Rendering the real `--since 2026-07-01` month (1,310 sessions) as markdown produced **16,117 output tokens** against a 16,000 ceiling. Guard 6 refused to publish and the render failed.
- The same month renders fine as html (19,574 tokens against 64,000). A normal 173-session window renders fine as markdown.
- This is not a cli-transport regression. The ceiling is a pre-existing property of the markdown job that both transports enforce, with different error text:
  - api path: `max_tokens: 16000` goes on the wire, the model is cut off at exactly the ceiling, `stop_reason: "max_tokens"` bails. The artifact is genuinely truncated and unusable.
  - cli path: no ceiling can be set on the wire, so the model finishes and Guard 6 compares `usage.output_tokens` after the fact. The artifact is **complete and valid**; it is rejected for exceeding a budget.
- The 2026-07-25 implementation audit sharpened that difference into the decisive fact: on the cli path the ceiling is a **self-imposed budget, not a capability limit**. The CLI had granted 64,000 output tokens for that call.

### Problem

`clyde report render --format markdown` is a hard failure on exactly the reports most worth reading, and the number that causes it is a compile-time constant no user can move.

### Requirements, and who asked

| # | requirement | asked by |
|---|---|---|
| R1 | the largest real month renders as markdown, keyless, with no flag | Scott, 2026-07-25, scoping this work: implement configurable ceilings with a raised default |
| R2 | the ceilings are configurable in `clyde.yml`, not hardcoded | Scott, precedent set verbatim on the model pins during the prior work: "those values should be configurable in the XDG .config .yml" |
| R3 | AC3 is rebaselined, and every assertion and comment that named the old ceiling moves with it | Scott, 2026-07-25, named as the second half of the task |
| R4 | the retired byte-identical contract is retired explicitly, not quietly dropped | standing rule: docs state ground truth |

Nothing else is in scope.

### Goals

- `render.markdown-max-output-tokens` and `render.html-max-output-tokens` in `clyde.yml`, siblings of `render.markdown-model` / `render.html-model` (R2).
- Markdown default raised to 32,000; html default unchanged at 64,000 (R1).
- The prior design's Open Questions section ends empty, and its AC1 flips to true on live evidence (R1, R3).
- Every site that names the old ceiling moves in the same commit series (R3).

### Non-Goals

- **No `--markdown-max-output-tokens` CLI flag.** The model pins are config-only and these are the same class of tunable. Siblings behave identically. Excluded, not parked.
- **No per-model capability table in clyde.** Deriving "what is the largest ceiling this model can honor" would bake fast-changing capability data into slow-changing logic, which is the exact decomposition mistake the pricing feed exists to avoid. Excluded.
- **No change to the html ceiling.** 19,574 observed against 64,000 is 3.2x headroom. Nothing measured argues for moving it.
- **No change to prompts, the report schema, transport selection, or the guard chain's membership.** Guard 6 stays (see Alternative 4).
- **No adaptive ceiling.** Excluded, see Alternative 3.

## Proposed Solution

### Overview

Two new config keys, and one refactor that the keys force.

```yaml
render:
  markdown-max-output-tokens: 32000   # raised default
  html-max-output-tokens: 64000       # unchanged default
```

The refactor is not optional and it is the interesting part of this design. The ceilings currently reach both transports through `Job::max_output_tokens()`, a method on a `Copy` enum. A user-configurable value is not a compile-time fact, so that method stops being expressible for exactly the reason `Job::model()` was already removed during the prior work.

### Architecture

Two consumers need the value, and they are on opposite transports:

| consumer | what it does with the ceiling |
|---|---|
| `report/src/summarize/api.rs`, `Job::api_limits()` | SETS it as `max_tokens` on the wire |
| `report/src/summarize/cli.rs`, Guard 6 | CHECKS `usage.output_tokens` against it after the fact |

`Transport::complete` already takes five arguments. Adding the ceiling as a sixth is the wrong move and would be the third pass through the same lesson. Instead, `Job` stops being the discriminant and becomes the resolved job:

```rust
/// WHICH artifact a job produces. Stays a `Copy` enum because it IS a compile-time fact: the two
/// arms are the two call sites in `render.rs`, and nothing about the choice is user-configurable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Markdown,
    Html,
}

/// A render job with its user-configurable pins RESOLVED from `clyde.yml`.
///
/// Every per-job tunable lands here. Both fields were once compile-time facts reachable as methods
/// on the old `Job` enum (`Job::model()`, then `Job::max_output_tokens()`), and both stopped being
/// expressible the moment a user could set them: a `Copy` enum arm cannot return a `String` it does
/// not own, nor a number it has not read. Bundling them means the NEXT configurable per-job value
/// is a field, not a sixth argument on `Transport::complete`.
#[derive(Clone, Copy, Debug)]
pub struct Job<'a> {
    pub kind: Kind,
    /// `render.markdown-model` / `render.html-model`.
    pub model: &'a str,
    /// `render.markdown-max-output-tokens` / `render.html-max-output-tokens`.
    pub max_output_tokens: u32,
}

pub trait Transport {
    fn complete(&self, job: Job<'_>, system: &str, prompt: &str, json_body: &str) -> Result<String>;
}
```

Consequences, each deliberate:

- The port goes 5 arguments -> 4. The thing that was growing now shrinks.
- `Job<'a>` stays `Copy` (a `&'a str`, not a `String`), so no call site churns on borrows.
- `Job::max_output_tokens()` and `Job::api_limits()` are deleted. `Kind::streams()` stays api-private in `api.rs`, unchanged: streaming is still a delivery choice the cli transport does not make, so it still must not appear on the port.
- `api_limits()` existed only to pair the ceiling with the streaming flag. That pairing was always two signals in one return value; splitting it is the same rule the prior design invoked when it killed the threshold-derived `stream`.
- Guard 6's error text formats `job.kind`, not `job`. A `{job:?}` on the struct would start printing a model pin into a user-facing error message.
- **Guard 6's bail names the config key.** Today it reads "over the {ceiling}-token ceiling for the {job:?} job; refusing to publish" (`cli.rs:220-223`), which tells the user a number and no remedy. This design's own risk table leans on "the user can raise it in one line" without telling them which line. The file already holds the doctrine that a remedy which cannot remedy is worse than none (`cli.rs:14-18`), and there is already a test asserting ceiling errors offer a working remedy (`cli/tests.rs:610-621`, which checks Guard 4 points at `--since`) whose Guard 6 half only asserts the ABSENCE of `--llm api` and never the presence of a fix. Naming `render.markdown-max-output-tokens` closes that, and it is what makes Alternative 4's burnt-cost objection survivable.
- `check_envelope(envelope, job, model, observations)` loses its separate `model` argument: it is `job.model`.

#### Call-site churn, and the two signatures it hides

`Job::Markdown` / `Job::Html` appear 33 times across `report/src`:

| file | occurrences | how they change |
|---|---|---|
| `summarize/cli/tests.rs` | 22 | **18** go through one helper, `fn check(json: &str, job: Job)` at `:434`: change the helper to take `Kind` and build the `Job` inside, and those become a one-word rename. **4 do not** (`:58`, `:89`, `:100`, `:214`); they sit BEFORE the helper and call `build_spawn(Job::Markdown, MODEL, ...)`, passing the model separately. Each is a two-part edit |
| `summarize.rs` | 4 | the two free functions constructing the job |
| `summarize/api/tests.rs` | 4 | the two byte-identical tests plus the mapping test being replaced anyway |
| `summarize/tests.rs` | 2 | `FakeTransport`. **This is a struct redesign, not a rename.** See below |
| `summarize/api.rs` | 1 | `Kind::streams()` |

So: one helper change, four two-part edits, and eleven single edits. Change `check`'s signature FIRST; doing it last means hand-editing 18 sites and then deleting the work.

**`build_spawn` is a second signature the reshape collapses.** `CliTransport::build_spawn(&self, job: Job, model: &str, system: &str, prompt: &str)` at `cli.rs:80` has exactly the `(job, model, ...)` shape that `Transport::complete` has. It takes `job` for `--model` and the debug log and `model` for the argv. Post-reshape it takes `job` alone and reads `job.model`. It goes 4 args -> 3 alongside `complete`'s 5 -> 4 and `check_envelope` dropping its `model`.

**`FakeTransport` cannot store a borrowing `Job`, and this is a hard compile error rather than churn.** `summarize/tests.rs:198-212` holds `struct Recorded { job: Job, ... }` inside `seen: RefCell<Vec<Recorded>>`. Making `Job` borrow forces `Recorded<'a>` and `FakeTransport<'a>`, but `RefCell<Vec<T>>` is **invariant** in `T`, so pushing a `Job<'_>` from the trait method (a fresh, shorter lifetime) is rejected:

```
error: lifetime may not live long enough
   self.seen.borrow_mut().push(Recorded { job, ... });
   = note: requirement occurs because of the type `RefCell<Vec<Recorded<'_>>>`,
           which makes the generic argument invariant
```

The fix is to destructure on the way in: `Recorded { kind: Kind, model: String, max_output_tokens: u32, ... }`, owned. Assertions at `:258` and `:272` move from `call.job` to `call.kind`. Recording owned data is the right shape for a recording fake anyway; the borrow was only ever incidental.

**Five debug logs format the whole job** (`api.rs:89`; `cli.rs:82`, `:132`, `:158`, `:170`) and every one already prints `model={model}` separately. Post-reshape `{job:?}` carries the model, so those five drop their separate `model=`. Tidy while the lines are open, not correctness.

`summarize::markdown` / `summarize::html` take the resolved ceiling and build the `Job` internally, so `render.rs` passes config values and never constructs a `Job`:

```rust
pub fn markdown<T: Transport>(
    transport: &T,
    model: &str,
    max_output_tokens: u32,
    prompt: &str,
    json_body: &str,
) -> Result<String> {
    let job = Job { kind: Kind::Markdown, model, max_output_tokens };
    transport.complete(job, MARKDOWN_SYSTEM_PROMPT, prompt, json_body)
}
```

### Data Model

No persisted structure changes. Two config keys, kebab-case, `deny_unknown_fields`, on the existing `RenderConfig` in `common/src/config.rs`, which is the single home for the `render:` defaults (the `report` crate reads them from there and deliberately keeps no second copy).

```rust
/// Default output ceiling for the markdown job (`render.markdown-max-output-tokens`).
///
/// 32,000: twice the largest markdown output ever measured (16,117 tokens on the 1,310-session
/// 2026-07 month, the render that surfaced this problem), and half of what the `claude` CLI grants
/// `claude-opus-4-8` (64,000), so the cli path can deliver a document at this ceiling untruncated.
/// Raised from the pre-config ceiling, which that same month exceeded by 117 tokens.
pub const DEFAULT_MARKDOWN_MAX_OUTPUT_TOKENS: u32 = 32_000;

/// Default output ceiling for the html job (`render.html-max-output-tokens`). Unchanged at 64,000:
/// exactly what the CLI grants `claude-opus-4-8`, against a largest observed html output of 19,574
/// tokens (3.2x headroom). Nothing measured argues for moving it.
pub const DEFAULT_HTML_MAX_OUTPUT_TOKENS: u32 = 64_000;
```

`RenderConfig::default()` is hand-written on purpose and both fields must be added to it. A derived `Default` substitutes `0`, and a ceiling of 0 fails every render.

**A configured ceiling of 0 is a loud config error naming the key.** It is the one value that is never a legitimate budget, and the hand-written `Default` only protects the absent case, not an explicit `markdown-max-output-tokens: 0`. No upper bound is enforced: the api path returns a loud 400 for a ceiling the model cannot honor, and the cli path gets cut at the granted ceiling and bails via `check_stop_reason` on `stop_reason: "max_tokens"` before Guard 6 is ever reached. Both already fail loudly, and enforcing an upper bound would require the per-model capability table this design refuses to carry.

### API Design

Config surface (both `render:` blocks in the repo gain two rows):

```yaml
render:
  format: markdown
  llm: auto
  markdown-model: claude-opus-4-8
  html-model: claude-opus-4-8
  markdown-max-output-tokens: 32000   # output ceiling for the Markdown narrative
  html-max-output-tokens: 64000       # output ceiling for the HTML dashboard
```

Accessors mirror the model pins exactly: `Config::render_markdown_max_output_tokens()` / `Config::render_html_max_output_tokens()`, carried onto `report::config::RenderConfig` by `resolve_command`, read at the two `render.rs` call sites.

**There are TWO structs named `RenderConfig` and both need the fields.** This trips people:

| struct | role | what to add |
|---|---|---|
| `common::config::RenderConfig` (`common/src/config.rs:95`) | the serde view of the `render:` YAML section | two `#[serde(default = "...")]` fields, two entries in the hand-written `Default`, two accessors on `Config` |
| `report::config::RenderConfig` (`report/src/config.rs:46`) | the resolved per-invocation command config | two plain `u32` fields, populated in `resolve_command` (`report/src/config.rs:198-210`, right next to `markdown_model` / `html_model`) |

The model pins already run this exact path, so copy their two hops rather than inventing a third.

### The AC3 rebaseline: nine sites

Raising the markdown default invalidates the byte-identical baseline for the second time (the first was the `claude-opus-4-7` -> `claude-opus-4-8` re-pin). All of these move together:

| # | site | what changes |
|---|---|---|
| 1 | `common/src/config.rs` | new consts, fields, hand-written `Default`, accessors, doc comments carrying the rationale |
| 2 | `report/src/summarize.rs:36-42` | `MARKDOWN_MAX_OUTPUT_TOKENS` / `HTML_MAX_OUTPUT_TOKENS` consts are deleted; their doc comments defend numbers that no longer live there |
| 3 | `report/src/summarize/api/tests.rs:35` | the expected body literal, `16000` -> `32000`, in `markdown_body_is_byte_identical_to_baseline` |
| 4 | `report/src/summarize/api/tests.rs:89-90` | `job_api_limits_map_to_todays_behavior` asserts against a method that no longer exists |
| 5 | `report/src/summarize/api/tests.rs:57,72,167` | three `build_body(..., 16_000, ...)` helper call sites |
| 6 | `report/src/summarize/cli/tests.rs`, five sites | see the breakdown below |
| 7 | `README.md:99-107` | the `render:` block |
| 8 | this doc + the prior doc | AC3 text, AC1 flip, Open Questions -> Resolved Decisions |
| 9 | `report/README.md:50-56` | a SECOND `render:` YAML block documenting the same config surface, which the top-level README points readers at (`README.md:113`). Two new keys landing in one block and not the other leaves a sibling doc stale on day one |

The handoff brief named five. Sites 5, 6, and 7 came from grepping rather than from the brief; Site 9 came from the review panel.

**Site 2 is the one with a track record.** Both consts carry doc comments justifying their specific values with measured observations, and one of them asserts the contract this change retires: "The markdown path stays byte-identical, so this value and the system prompt must not move." Two comments in this codebase already went stale in exactly this way and became audit finding M3. Deleting the consts removes the comments with them, which is the safe direction, but the rationale has to be reproduced on the new config consts rather than lost.

**Site 6 needs judgment, not a find-and-replace.** Five Guard 6 tests encode the old ceiling, and two of them lose their *premise* at 32,000 rather than just their literal:

| test | line | what the raise does to it |
|---|---|---|
| `guard_output_ceiling_bails_when_the_job_budget_is_exceeded` | 513 | 20,000 tokens no longer exceeds a 32,000 budget. Both the fixture value and the ceiling assertion move |
| `guard_output_ceiling_allows_the_same_output_for_the_larger_job` | 523 | **premise dies.** It asserts 20,000 is over budget for markdown and fine for html. At 32,000 it is fine for both, so the test passes vacuously while proving nothing. Needs a value between the two ceilings, e.g. 40,000 |
| `guard_output_ceiling_accepts_exactly_the_ceiling` | 531 | the boundary value must become `32_000` or it stops testing the boundary |
| the truncation fixture | 612, 618, 621 | the fixture value and a second ceiling assertion, PLUS the same test's Guard 6 half at `:618`, which feeds `20_000` to `Job::Markdown` and calls `.unwrap_err()` at `:619`. At a 32,000 ceiling that returns `Ok` and the test panics. It fails loudly rather than silently, but this is the fifth Guard 6 site, not the fourth |

A mechanical sweep would leave test 523 green and worthless. That is the exact shape of a test that has stopped biting.

**The retired contract, stated plainly.** `report render --format markdown` on the api path no longer produces the request body it produced before the html-render design. The `max_tokens` value moves to 32,000 by default and is user-settable. AC3 still exists and still keeps the api path from rotting, but it now asserts "the body matches the current declared baseline" rather than "the body matches pre-HTML behavior". The doc stops claiming the latter.

### Implementation Plan

Three phases. The behavior-neutral refactor first, the value change second and small enough to audit at a glance, live proof last.

#### Phase 1: reshape `Job`, plumb the ceilings as config, defaults hold today's values
**Model:** opus

- `Kind` enum + `Job<'a>` struct; `Transport::complete` drops to four arguments; delete `Job::max_output_tokens()` and `Job::api_limits()`; `Kind::streams()` stays api-private.
- Guard 6 reads `job.max_output_tokens` and formats `job.kind`; `check_envelope` drops its separate `model` argument.
- `build_spawn` (`cli.rs:80`) drops its separate `model`, alongside `complete` and `check_envelope`.
- Redesign `Recorded` in `summarize/tests.rs` to own its fields (`kind`, `model: String`, `max_output_tokens`), and move the `:258` / `:272` assertions to `call.kind`. A borrowing `Job` inside `RefCell<Vec<Recorded>>` does not compile.
- Two config keys in `common/src/config.rs`, **defaulting to 16,000 / 64,000 in this phase only**, with hand-written `Default`, `deny_unknown_fields`, and a loud rejection of 0. Accessors, `report::config::RenderConfig` fields, `resolve_command` wiring, both `render.rs` call sites.

**What proves this phase, and what does not.** An earlier draft claimed the two byte-identical tests "pass unchanged, and that is the proof the reshape moved no behavior." That was wrong twice and both reviewers caught it independently:

- **It cannot even compile.** Both tests open by calling the method this phase deletes: `Job::Markdown.api_limits()` at `api/tests.rs:23` and `Job::Html.api_limits()` at `:44`. "Unchanged" is not available. What survives unchanged is the **expected body literal**; the two harness lines that source `max_tokens` and `stream` necessarily change to read them off the constructed `Job` and `Kind::streams()`.
- **It would not prove neutrality if it did compile.** Both tests call `build_body` directly (`api/tests.rs:24`, `:45`). They never touch `ApiTransport::complete` (`api.rs:85`), `summarize::markdown`/`html` (`summarize.rs:71`, `:82`), the `render.rs` call sites (`:264-265`, `:286-287`), or anything on the cli path (`cli.rs:168`, Guard 6 at `:216`). A dropped ceiling in the cli plumbing or a mis-constructed `Job` in `summarize::markdown` leaves both green.

- **Success criteria:**
  - `otto ci` exit 0. Every phase is ci-green with exactly one commit; the earlier draft gated only Phase 2, which given the point above would let a broken `cli/tests.rs` satisfy Phase 1.
  - **the two-sided plumbing probe is the real proof:** a `clyde.yml` setting `markdown-max-output-tokens: 12345` produces `"max_tokens":12345` in the built api body AND a Guard 6 ceiling of 12345 on the cli path. That crosses both transports, which the byte tests never do.
  - both byte-identical tests pass with their **expected literal** untouched (`"max_tokens":16000` / `"max_tokens":64000` still, since defaults do not move until Phase 2). Anti-rot, not the neutrality proof.
  - `rg 'fn max_output_tokens|fn api_limits' report/src/` returns nothing.
  - `markdown-max-output-tokens: 0` fails config load with an error naming the key (see AC-C2 for why the obvious implementation does not achieve this).

#### Phase 2: raise the markdown default and rebaseline every site that named the old ceiling
**Model:** sonnet

- `DEFAULT_MARKDOWN_MAX_OUTPUT_TOKENS: 16_000 -> 32_000`, with the doc-comment rationale above.
- Sites 3 through 7 and Site 9 from the rebaseline table, including both `render:` YAML blocks. Replace `job_api_limits_map_to_todays_behavior` with two separate assertions: `Kind::Markdown.streams() == false` / `Kind::Html.streams() == true`, and the two default ceilings, mirroring how `both_jobs_default_to_opus_4_8` pins the model defaults. Splitting them is the point: the old tuple conflated two signals.
- Guard 6's bail names `render.markdown-max-output-tokens` (see the Architecture note on the error text), and the existing test at `cli/tests.rs:610-621` gains an assertion that the key appears.
- **Four of the sites are cosmetic, and they get an arbitrary value rather than the new default.** `api/tests.rs:57`, `:72`, `:167` and `cli/tests.rs:612` carry the old ceiling where the number is inert: the first three assert stream omission, prompt joining, and model presence, and the fourth is a truncation fixture where Guard 4 fires on `stop_reason: "max_tokens"` before Guard 6 ever sees the count. They must change anyway to satisfy AC-C4, so the only question is what they change TO. Use an obviously arbitrary value (`1_024`), not `32_000`. Today they track the real default by coincidence, and setting them to it again re-couples them, so the *next* ceiling change drags four unrelated tests for no reason. This is not extra scope: the lines are already being edited, this only picks the right value for them.
- Doc work: this doc's AC3 text; the prior doc's Open Questions section emptied with a pointer here plus a Resolved Decisions row; the retired byte-identical contract struck where it is claimed.
- **Success criteria:**
  - `otto ci` exit 0.
  - `rg -n '16[_,]?000|16K' report/src common/src/config.rs README.md report/README.md` returns zero hits.
  - `guard_output_ceiling_allows_the_same_output_for_the_larger_job` still discriminates: its output value sits strictly between 32,000 and 64,000, proven by flipping its markdown assertion and watching it fail.
  - the prior doc's Open Questions section is empty.

#### Phase 3: live verification and the AC1 flip
**Model:** sonnet

```bash
clyde session reindex
clyde report collect --since 2026-07-01 -o <tmp>/real.json
env -u ANTHROPIC_API_KEY clyde --log-level info report render \
  -i <tmp>/real.json --format markdown -o <tmp>/ac1.md
```

- Flip the prior doc's AC1 from `[ ]` PARTIAL to `[x]`; append implementation notes (append-only).
- **Success criteria:** exit 0; `ac1.md` exceeds 5,000 bytes, contains at least three `^## ` headers, and does not contain `Generated offline via`; the log line reads `selected=Cli (requested=Auto)`.

## Acceptance Criteria

- [x] AC-C1: the keys are plumbed end to end. `render.markdown-max-output-tokens: 12345` in `clyde.yml` yields `"max_tokens":12345` in the built api request body AND a Guard 6 ceiling of 12345 on the cli path. An absent `clyde.yml` resolves markdown to 32,000 and html to 64,000. An unknown key under `render:` still fails loudly via `deny_unknown_fields`.
- [x] AC-C2: `render.markdown-max-output-tokens: 0` fails config load with an error that **names the key**, and no render runs. **The house `deserialize_with` pattern does not achieve this on its own.** `de_fraction` (`common/src/config.rs:175-186`, wired at `:137` and `:141`) is the precedent, and serde_yaml renders its `Error::custom` as `render: must be greater than 0, got 0 at line 2 column 3`: the section and the location, never the field. So the key must be hardcoded into each `Error::custom` string, which means two functions (or a macro), not one shared `de_nonzero`. The existing fraction tests (`common/src/config/tests.rs:248-281`) only assert `is_err()`, so they are no precedent for the stronger claim.
- [x] AC-C3: the ceiling is no longer a compile-time fact. `rg 'fn max_output_tokens|fn api_limits' report/src/` returns nothing, and `Transport::complete` takes four arguments.
- [x] AC-C4: the rebaseline is complete and mechanically checkable:

  ```
  rg -n '16[_,]?000|16K' report/src common/src/config.rs README.md report/README.md
  ```

  returns **zero** hits. Three defects in the earlier form of this AC, all of which would have let the rebaseline pass while incomplete:
  - the path was `report/src/summarize`, which resolves to the **directory** and therefore never searches `report/src/summarize.rs`, the file holding the const at `:38`. Run verbatim it returns 9 hits, none of them the const. The gate skipped the single most important site.
  - the pattern `16_?000` matches neither `16K` nor `16,000`, so four live comment sites survive it (`api/tests.rs:33`, `:87`; `cli/tests.rs:515`, `:524`). Stale comments are the M3 failure class this gate exists to prevent, and it could not see the prose form of the number.
  - `report/README.md` was not in the path list at all. See Site 9.

  Plus: `markdown_body_is_byte_identical_to_baseline` asserts `"max_tokens":32000`; `html_body_is_byte_identical_to_baseline` asserts `"max_tokens":64000`.

  **Zero is reachable, verified twice.** The command returns 14 hits on today's tree, every one the markdown ceiling, classified as 3 deleted / 7 must-change / 4 cosmetic / **0 permanent**. Two things keep zero reachable and are deliberate, not incidental: the new const's doc comment says "the pre-config ceiling" rather than spelling the old number, and it cites the measurement as `16,117`, which the pattern does not match. Anyone rewriting that comment to name the old value re-breaks this AC. The html references (`64K`, `64000`) are untouched because that ceiling does not move. A sweep for other 16-family constants in `report/src` (`16K`, `16KB`, `16384`, `0x4000`) found nothing, so widening the path to the whole crate introduces no false positive today.

  **This is a point-in-time gate, not an invariant.** It is checked once at the end of Phase 2. `16K` is generic enough that some future unrelated use (a buffer comment, a byte limit) would trip it. Nothing to do about that now; just do not read AC-C4 as something that enforces itself forever.
- [x] AC-C6: no Guard 6 test passes vacuously. `guard_output_ceiling_allows_the_same_output_for_the_larger_job` uses an output value strictly between the two ceilings, proven by flipping its `Job::Markdown` assertion to `is_ok()` and watching it fail.
- [x] AC-C5: the prior design's AC1 holds live. `env -u ANTHROPIC_API_KEY clyde report render -i <largest>.json --format markdown -o <f>` exits 0; `<f>` exceeds 5,000 bytes, has at least three `^## ` headers, and contains no `Generated offline via`; the log names `selected=Cli (requested=Auto)`.

  **Verified live, 2026-07-25.** The full month (1,328 sessions, 519,124-byte payload) rendered keyless at exit 0 into 14,846 bytes, 9 `^## ` headers, no offline marker, `claude` 2.1.220, and the log carried both `selected=Cli (requested=Auto)` and `job=Job { kind: Markdown, model: "claude-opus-4-8", max_output_tokens: 32000 }` -- the reshaped `Job` carrying the config-resolved ceiling across a real transport. **One honest caveat:** that run's output came in under even the OLD ceiling, so it proves the path works end to end but does not independently re-prove the 16,117-token failure this design fixes. The necessity rests on that earlier measurement, recorded in the prior doc's Open Questions and this doc's Problem Statement.

## Resolved Decisions

| date | decision | rationale |
|---|---|---|
| 2026-07-25 | option 3 (configurable ceilings, raised default) over raise-only or accept-it | Scott scoped this work to it directly. Two supports already on the record: his own directive that moved the model pins to `clyde.yml`, and the audit finding that the cli-path ceiling is a self-imposed budget rather than a capability limit, so raising it carries no truncation risk up to 64,000 |
| 2026-07-25 | markdown default 32,000; html unchanged at 64,000 | 2x the largest measured markdown output (16,117), half the CLI's granted 64,000 for `claude-opus-4-8`, and well under opus-4-8's 128K api max output so the api path honors it too. Html has 3.2x headroom already and no measurement argues for moving it |
| 2026-07-25 | config only, no CLI flag | the model pins are config-only and these are the same class of tunable. Siblings behave identically |
| 2026-07-25 | `Job` becomes the resolved job struct; `Kind` carries WHICH | collapses the second repeat of the `Job::model()` lesson instead of repeating it. Takes the port from five arguments to four, and makes the next per-job tunable a field rather than a sixth argument |
| 2026-07-25 | `Job::api_limits()` is deleted, not extended | it packed the shared ceiling and the api-private streaming flag into one return value. Two signals, one value, which is the rule the prior design already applied to threshold-derived streaming |
| 2026-07-25 | the "markdown stays byte-identical to pre-HTML behavior" contract is RETIRED explicitly | it is the real price of this option. AC3 survives as an anti-rot assertion against the current declared baseline; the doc stops claiming the stronger thing |
| 2026-07-25 | a ceiling of 0 is rejected loudly at config load; no upper bound is enforced | 0 is never a legitimate budget and the hand-written `Default` only covers the absent case. An upper bound would need a per-model capability table, which is fast-changing data that must not live in slow-changing logic. Both transports already fail loudly on an over-large ceiling |
| 2026-07-25 | Guard 6 stays, and its error names the config key | with the ceiling configurable it stops being a hardcoded mirror of an api limit and becomes enforcement of a budget the user set. See Alternative 4 |
| 2026-07-25 | new dated doc, not a reopening of the Implemented one | the prior doc is `Status: Implemented` and reopening it to add phases would muddy that. Its Open Questions section closes with a pointer here, so nothing dangles |
| 2026-07-25 | rides the existing `report-render-claude-cli-transport` branch and its (unopened) PR | this closes that design's last open question. Splitting it into a second PR would land a doc that admits it is incomplete |

## Review Panel Dispositions (2026-07-25)

Design Review, both seats. Architect on gemini-3.1-pro-preview (rc=0), Staff Engineer on codex gpt-5.5 at high reasoning (rc=0). Every finding dispositioned. Nothing dropped or deferred silently.

| # | finding | disposition |
|---|---|---|
| F1 | Phase 1's "the byte tests pass unchanged" is impossible: both tests open by calling `Job::api_limits()`, which Phase 1 deletes (`api/tests.rs:23`, `:44`) | **ACCEPTED.** Flat self-contradiction, and my own churn table counted those tests. Phase 1's criteria rewritten: the expected body LITERAL stays fixed, the harness lines necessarily change |
| F2 | those tests could not prove neutrality anyway: they call `build_body` directly and never touch `ApiTransport::complete`, `summarize::*`, the `render.rs` call sites, or any cli path | **ACCEPTED.** The neutrality proof is now the two-sided plumbing probe that crosses both transports. Byte tests demoted to anti-rot |
| F3 | `Recorded { job: Job }` inside `RefCell<Vec<Recorded>>` cannot hold a borrowing `Job`; `RefCell` is invariant, so the push is rejected | **ACCEPTED**, compiler-confirmed by the panel and verified against `summarize/tests.rs:198-212`. `Recorded` destructures to owned fields. Named as a struct redesign, not the "rename" the doc called it |
| F4 | the churn table's "all 22 through ONE helper" is wrong: four sit before the helper at `:58`, `:89`, `:100`, `:214` and call `build_spawn`, whose `(job, model, ...)` signature the doc never named as changing | **ACCEPTED.** Table corrected to 18 + 4; `build_spawn` (`cli.rs:80`) added to the signature-collapse list |
| F5 | AC-C4's `rg` path `report/src/summarize` is the DIRECTORY, so it never searches `report/src/summarize.rs`, the file holding the const | **ACCEPTED.** Ran it verbatim: 9 hits, zero from `summarize.rs`. The completeness gate skipped the most important site. Path is now `report/src` |
| F6 | the regex `16_?000` cannot match `16K` or `16,000`, so four comment sites survive it | **ACCEPTED.** Verified all four. This is the M3 stale-comment class defeating the gate written to prevent M3. Pattern widened |
| F7 | AC-C2's "an error naming the key" does not hold under the house `deserialize_with` pattern; serde_yaml emits the section and location, never the field | **ACCEPTED.** The panel proved it empirically against serde_yaml 0.9.34. AC-C2 now states the mechanism: hardcode the key into each `Error::custom`, two functions not one |
| F8 | `report/README.md:50-56` is a second `render:` block, a ninth rebaseline site not in the table or the `rg` | **ACCEPTED.** Added as Site 9 and to AC-C4's path list |
| F9 | Site 6's row misses `cli/tests.rs:618`, a fifth Guard 6 site that returns `Ok` at a 32,000 ceiling and panics | **ACCEPTED.** Added to the row |
| F10 | Guard 6's bail names no remedy, against the file's own doctrine and an existing test that enforces remedies on ceiling errors | **ACCEPTED.** The bail now names `render.markdown-max-output-tokens`; Phase 2 extends `cli/tests.rs:610-621` to assert it |
| F11 | Phase 1 has no `otto ci` gate while Phase 2 does, against the house one-commit-ci-green rule | **ACCEPTED.** Added |
| F12 | five debug logs format the whole job and already print `model=` separately, so they duplicate post-reshape | **ACCEPTED** as tidy-while-open. Noted in the churn section |
| F13 | (Architect) drop Guard 6 on the cli path: the tokens are already billed, so rejecting a complete artifact burns ~$3 for nothing | **RECOMMENDATION REJECTED, FACT FOLDED IN.** It relitigates a Resolved Decision and the transport-symmetry counter holds. But the burnt-cost point is genuinely new and is now in Alternative 4's cons. F10 is the right response to it: make the error actionable rather than delete the guard |

### Round 2

Sent the panel the thirteen dispositions plus one question I could not close alone: after widening AC-C4's pattern, is zero actually reachable, or is any of the 14 hits load-bearing and permanent? An AC that can never pass is as useless as one that always does.

| # | finding | disposition |
|---|---|---|
| F14 | zero IS reachable: the 14 hits classify as 3 deleted / 7 must-change / 4 cosmetic / 0 permanent, and a sweep for other 16-family constants in `report/src` finds nothing, so the widened path adds no false positive | **CONFIRMED**, matching my own enumeration. Recorded in AC-C4 so the next reader does not redo it |
| F15 | AC-C4 is a point-in-time gate, not a self-enforcing invariant; a future unrelated `16K` in `report/src` would trip it | **ACCEPTED** as a caveat. Stated in AC-C4. Nothing actionable now, but it should not be mistaken for a permanent guard |
| F16 | the four cosmetic sites currently track the real default by coincidence, so the next ceiling change drags them again for nothing | **ACCEPTED.** They get `1_024` instead of `32_000`. Not extra scope: AC-C4 forces those lines to change regardless, so this only picks the right value. Breaks the coupling permanently |

**Reviewer calibration.** Cleaner than the pricing panel: the Architect's line numbers held up this time and the panel verified all of them. Two Architect problems were caught and handled rather than passed through: it asserted the codebase "does not have an established pattern for bounds-checking config deserialization," which is false (`de_fraction`, `common/src/config.rs:175-186`), so the premise was dropped and only the surviving half became F7; and it certified the rebaseline table as "impeccably accurate" while missing Site 9 and endorsing an AC whose `rg` path was broken. Line-number verification kept, completeness verdict rejected. Staff's citations were exact throughout, and it correctly reported that its own `rustc` probe was blocked by the sandbox rather than asserting through it.

**One unsourced figure, now labelled:** 145s and 204s were measured; anything above that in the Performance section was an unlabelled projection and is marked as such.

## Alternatives Considered

### Alternative 1: raise the const, no config
- **Description:** raise `MARKDOWN_MAX_OUTPUT_TOKENS` and stop.
- **Pros:** smallest possible diff; no refactor, because the value stays a compile-time fact and `Job::max_output_tokens()` survives.
- **Cons:** the next month that exceeds the new value needs a rebuild and a release. Directly contradicts the directive class that moved the model pins into `clyde.yml`.
- **Why not chosen:** same argument that moved the pins. A tunable rides the standard delivery path.

### Alternative 2: accept it, the largest months use `--format html`
- **Description:** zero code. Document that markdown has a size limit.
- **Pros:** nothing to build or maintain.
- **Cons:** `report render --format markdown` stays a hard failure on the reports most worth reading, and the prior design keeps an open question forever. The audit already priced this option up: it rejects a complete, valid artifact.
- **Why not chosen:** Scott scoped the work to the configurable option.

### Alternative 3: adaptive ceiling, derived from the envelope's granted `maxOutputTokens`
- **Description:** read `modelUsage.<id>.maxOutputTokens` and use it as the ceiling.
- **Pros:** no config keys; always exactly what the model will actually grant.
- **Cons:** the value is only knowable AFTER the call on the cli path, and not at all on the api path, so the two transports would enforce different ceilings for the same command. It is derived magic of precisely the kind the prior design removed when it killed threshold-derived streaming.
- **Why not chosen:** breaks transport symmetry and reintroduces cleverness that was already rejected once.

### Alternative 4: drop Guard 6 on the cli path
- **Description:** the CLI already stops at its own granted ceiling, and Guard 4 (`stop_reason != end_turn`) catches truncation. So delete the after-the-fact budget check and the problem disappears without any config.
- **Pros:** removes the guard that rejected a complete, valid 16,117-token artifact. Fewer moving parts.
- **Cons:** the ceiling must exist anyway, because the api path puts it on the wire, so this saves no config and no plumbing. It also breaks transport symmetry in the expensive direction: the same command would truncate at the ceiling on api and ignore it entirely on cli, so one artifact would silently differ from the other.
- **The cost argument for dropping it, raised by the Architect on review and worth recording honestly:** on the cli path the tokens are already generated and already billed by the time Guard 6 runs. Rejecting a complete 32,100-token artifact burns roughly $3 (the measured cost of a full markdown render, prior doc `:127`) and delivers nothing. That is a real fact and it was not in this list before.
- **Why not chosen:** the guard still earns its keep, and the reason changes with this design: once the ceiling is a value the user set, Guard 6 stops mirroring an api limit and starts honoring a stated budget. The burnt-cost point does not argue for deleting the guard, it argues for the error being actionable, which is why Guard 6's bail now names `render.markdown-max-output-tokens`. That turns "you burned $3, sorry" into "you burned $3, here is the one line that prevents the next one." Reopening a decision the owner settled is not on the table; making its failure mode useful is.

## Technical Considerations

### Dependencies

Zero new crates. No cross-repo blast radius: one repo, no schema change, no pricing change, no marquee change.

### Performance and cost

**Raising the ceiling does not change what a render costs.** Output is billed by tokens actually produced, not by the ceiling requested. The 32,000 default moves the api path's truncation point and the cli path's budget check; it does not move a single billed token. Worth stating because "raise the ceiling" reads like it costs money and it does not.

The largest markdown render costs about $3 on the cli path and takes **145s measured** (204s measured for html); anything above that is projection, not measurement. $2.93 was measured at 12,706 output tokens, and the render that fails today produced 16,117, so the real figure is a little higher. That is unchanged by this design, and it is the reason Phase 3's live verification is a single run, not a loop.

### Security

No new surface. No credential is read, stored, or transmitted differently. The new config values are integers in a file the user already owns, and they reach the child process only as a number the api transport serializes or the cli transport compares against.

### Testing Strategy

- Unit: the byte-identical bodies (both jobs) are the anti-rot assertion. They are NOT the behavior-neutrality proof; see Phase 1.
- Unit: `Kind::streams()` per arm and the two default ceilings, asserted separately rather than as a tuple.
- Unit: config plumbing, both directions: a set value reaches the api body and the cli Guard 6 ceiling; an absent file resolves to the documented defaults; `0` fails loudly; an unknown `render:` key still fails.
- Unit: Guard 6 fires above the configured ceiling and passes at exactly the ceiling, and its error names the config key.
- Every new or changed assertion is proven to bite: break the code, watch that specific test fail, restore.

Landmines this suite has already produced, carried forward so they are not rediscovered:

- **Never `git checkout <path>` to undo a sabotage.** It destroyed uncommitted work twice in the prior session, once silently reverting four separate changes. Commit first, then break, or copy the file aside and restore from the copy.
- **`#![deny(unused_variables)]` blocks the obvious sabotage.** Hardcoding a value and dropping the parameter will not compile, so a bite proof that looks like it did not bite may just have failed to build. Use `{ let _ = param; <literal> }` for a compiling variant.
- **`otto lint` bans `_varname` bindings.** Bare `_` is fine; `_job` is a hard failure.
- **`ENV_LOCK` is crate-wide (`crate::ENV_LOCK` in `report/src/lib.rs`) and readers must take it too.** `common/` keeps its own separate lock deliberately, because it is a separate test process. Do not unify them.
- **Every `resolve_command(Render(..))` test goes through `with_clyde_yml`** (pass `None` for "no config file"). Render loads `clyde.yml` unconditionally, so a bare call races.
- **Green once proves little.** This suite has had two genuine parallel races. The report suite must be green on three consecutive runs.

### Rollout Plan

- Branch `report-render-claude-cli-transport` is 7 commits ahead of `main` and **2 behind** (`main` moved to `0401997`, tagged `v0.13.3`). Rebase onto current `main` before anything else.
- **The rebase is near-certainly clean.** `0401997` touches `cost/`, `pricing/`, `Cargo.toml`, and `Cargo.lock`. This branch touches `report/src/summarize*`, `common/src/config.rs`, and `README.md`. Zero source-file overlap; the only plausible conflict is the version line in `Cargo.toml`/`Cargo.lock`.
- Nothing is pushed, bumped, tagged, or installed, and there is no PR.
- `main` is PR-gated and `tatari-tv` is a gated org, so the order is: `bump --no-tag` on the branch, PR, merge, then `bump --tag-only` on `main` and push the tag by name.
- PR title is fixed by the branch slug: `feat(report): report render claude cli transport`. A PreToolUse hook hard-denies a mismatch.
- A hook also requires a release-intent line in the PR body: `Release: rides this PR (vX.Y.Z)` or `Release: none -- <why>`.
- `git push` and `gh` on this repo need the Bash sandbox disabled, or the `core.sshCommand` wrapper cannot select the work SSH key. The symptom is `Permission to tatari-tv/clyde denied to scottidler`.
- After merge: `cargo install --path .`, then re-run the Phase 3 keyless render against the installed binary. Green CI is not done.
- **Every step in this section needs Scott's explicit approval and must not be run before it.**

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Phase 1's port reshape silently changes the api request body | Low | High | the two-sided plumbing probe crosses both transports; the byte tests hold the expected literal fixed as anti-rot |
| A site naming the old ceiling is missed and its comment or assertion goes stale | Med | Med | the rebaseline table enumerates all nine; AC-C4 is a mechanical `rg` that returns zero hits. This is the M3 failure class and it has already happened twice in this codebase |
| A mechanical sweep leaves a test green but vacuous | **High** if done mechanically | High | site 6's breakdown names the two tests whose premise dies. AC-C6 asserts the surviving test still bites |
| A month eventually exceeds 32,000 markdown tokens too | Low | Low | it is now a config edit, not a rebuild. That is the whole point of the option chosen |
| A user sets a ceiling the model cannot honor | Low | Low | api returns a loud 400; cli is cut at the granted ceiling and bails on `stop_reason` before Guard 6. No silent degrade either way, and no capability table to drift |
| Guard 6 rejects a complete artifact at the new ceiling | Low | Med | accepted and reframed: above 32,000 the guard is enforcing a budget the user can raise in one line, and the error now names that line |
| Rebase onto the 2 new `main` commits conflicts | Low | Low | verified disjoint file sets (see Rollout). Rebase first anyway, before writing code |

## Open Questions

- None.

## References

- `docs/design/2026-07-24-report-render-claude-cli-transport.md` (the design this closes; Open Questions section)
- `docs/design/2026-07-24-report-render-claude-cli-transport-implementation-notes.md` (audit findings M3 and M4, the two `git checkout` process notes)
- `docs/design/2026-07-05-report-html-render.md` (where both ceilings were originally set)
- `report/src/summarize.rs:36-60` (the consts and `Job::max_output_tokens`)
- `report/src/summarize/api.rs:20-38` (`Kind::streams`, `Job::api_limits`)
- `report/src/summarize/cli.rs:80` (`build_spawn`), `:213-224` (Guard 6)
- `report/src/summarize/tests.rs:198-212` (`Recorded` / `FakeTransport`)
- `common/src/config.rs:68-119` (the model-pin precedent and the hand-written `Default`), `:175-186` (`de_fraction`, the bounds-checking precedent)
