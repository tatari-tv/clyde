# Design Document: `report render` over the local `claude` CLI (no API key)

**Author:** Scott Idler
**Date:** 2026-07-24
**Status:** Implemented (all six phases shipped and live-verified; the one open question this design left open -- the markdown job's output ceiling -- is CLOSED by `docs/design/2026-07-25-render-output-ceilings-config.md`, see Resolved Decisions)
**Review Passes Completed:** 5/5
**Funnel position:** five passes, then a two-round review panel (Architect on Gemini, Staff Engineer on Codex, four invocations). Every finding dispositioned in Resolved Decisions; both reviewers declined a third round. Open Questions empty. Ready to build, starting at Phase 0.

## Summary

`clyde report render` needs `ANTHROPIC_API_KEY` for every model-authored artifact. Without a key you get a 3-row template table (markdown) or a hard bail (html). Add a second transport for the same two LLM calls: shell out to the locally installed `claude` CLI in headless print mode, which uses the Claude Code login the user already has. Same prompts, same model pins, same math-free guard, no key, no new credential.

## Problem Statement

### Background

- Stephen Price ran `clyde report render` and published `~stephen/claude-code-usage-report-ltm-3914`. The whole artifact: a 3-row stats table (sessions, tokens, spend) plus the footer `Generated offline via clyde report render --template (no ANTHROPIC_API_KEY set - no per-session narrative)`.
- Scott: "hmmm thats terrible Stephen". Stephen: "yeah you need to figure out how to piggy-back on the existing oauth login".
- Every clyde user has a logged-in Claude Code. That login is WHY they have sessions to report on. An API key is a second, separately-provisioned credential most of them do not have and should not need.

### Problem

`report render`'s LLM surface is key-only. Two call sites, one transport:

| site | call | model pin | today, keyless |
|---|---|---|---|
| `render.rs:241` `render_via_opus_markdown` | `summarize::markdown` | `claude-opus-4-7` | bail; `--template` degrades to 6-token string replacement |
| `render.rs:263` `render_via_opus_html` | `summarize::html` | `claude-opus-4-8` | bail; no offline path exists |

Both funnel through one `summarize::request()`, the single render-summary POST of `x-api-key` to `api.anthropic.com/v1/messages` (`summarize.rs:104`). The key is read by `title::api_key_from_env()` (`title.rs:232`). Precision matters here (Staff Engineer): `summarize.rs:104` is not the repo's ONLY `x-api-key` POST, because `title.rs:96` posts too. That one is out of scope, it is the uncalled Haiku helper.

Consequence: clyde's flagship artifact is unreachable for a keyless teammate, and the fallback is bad enough that Scott called it terrible in public.

### Requirements, and who asked

| # | requirement | asked by |
|---|---|---|
| R1 | keyless host renders the FULL markdown artifact | Stephen (the failed render), Scott ("thats terrible") |
| R2 | keyless host renders the FULL html artifact | same; html has no offline path at all |
| R3 | piggy-back the EXISTING login, do not provision a credential | Stephen, verbatim |
| R4 | `claude -p` is the DEFAULT; an api key remains available to whoever wants it | Scott, 2026-07-24, explicit: "allow the user to use an api key if they want but default to claude -p" |
| R5 | failures stay loud (truncation, transport, model drift) | Scott, standing rule: fail loudly, fail closed |

Nothing else is in scope. `--template` quality is a separate ask nobody has made yet.

### Goals

- Keyless render of both artifacts, full fidelity (R1, R2).
- Zero credential custody in clyde: no token read, no refresh, no storage (R3).
- One default path for everyone: `claude -p`. The api transport stays fully supported and, at the time this shipped, byte-identical when selected, but it is opt-in (R4). (The byte-identical-to-pre-HTML contract was retired later by the ceilings design; the api path is still asserted byte-for-byte against the current declared baseline. See AC3.)
- Every unhappy path is an error, never a degraded artifact (R5).

### Non-Goals

- Reading `~/.claude/.credentials.json` and minting Bearer calls ourselves. Rejected, see Alternatives.
- A Tatari-hosted LLM proxy. Parked; revisit condition in the Addendum.
- Improving the offline `--template` output. Separate problem, separate doc.
- Changing prompts or the report JSON schema.
- ~~Changing model pins~~ **SUPERSEDED by Scott, 2026-07-24, two directives given during execution:**
  - **Both jobs pin `claude-opus-4-8`.** Verbatim: "just use claude opus 4-8", given when Phase 0 was about to spike the unverified markdown pin `claude-opus-4-7`. So the markdown job re-pins 4-7 -> 4-8. This moots the "markdown pin rejected by `--model`" risk row (4-8 is verified accepted by both transports) and it is the value Phase 0 actually measured.
  - **The pins are CONFIGURABLE in `clyde.yml`, not hardcoded consts.** Verbatim: "those values should be configurable in the XDG .config .yml". This follows the house rule that tunables ride the standard delivery path; a model pin is a WHAT (the shape of the behavior), which is legitimate config, not a WHETHER gate.
- Streaming progress to the terminal mid-render.
- `title::haiku` and friends. Already uncalled by collect after the 2026-07-24 collect-once redesign; left exactly as-is.

## Probe Evidence

Measured on desk.lan, 2026-07-24, `ANTHROPIC_API_KEY` unset via `env -u`. This is the proof that R3 is achievable; it is not a claim about the final artifact (that is Phase 0's job).

| # | model | flags | cache-create tokens | cost | result |
|---|---|---|---|---|---|
| 1 | `haiku` | none | 32,369 | $0.065 | `pong` |
| 2 | `haiku` | lean | 15,930 | $0.032 | one-sentence summary of stdin JSON |
| 3 | `claude-opus-4-8` | none, fresh untrusted dir | 39,194 | $0.392 | `pong` |
| 4 | `claude-opus-4-8` | system-prompt + exclude-dynamic | 37,588 | $0.376 | summary |
| 5 | `claude-opus-4-8` | lean | 17,251 | $0.173 | summary |
| 6 | `haiku` | lean but `--allowed-tools ''` instead of the disallow list | 26,403 | $0.053 | summary |
| **7** | **`haiku`** | **FINAL argv** (`--tools ''` `--safe-mode` `--strict-mcp-config` `--no-session-persistence` `--max-turns 1` `--system-prompt`) | **0** | **$0.0015** | summary |
| **8** | **`claude-opus-4-8`** | **FINAL argv** | **0** (243 input) | **$0.0024** | summary |

"lean" = `--system-prompt <ours>` + `--exclude-dynamic-system-prompt-sections` + `--disallowed-tools <list>`.

**Correction (Staff Engineer, verified against local `claude --help`):** `--exclude-dynamic-system-prompt-sections` is ignored whenever `--system-prompt` is set, which every "lean" probe did. So that flag contributed NOTHING to the measured reductions; the savings came from `--disallowed-tools` alone. Probe 4 is the proof sitting in my own table and I misread it: system-prompt override plus the exclude flag moved 39,194 -> 37,588, essentially noise, and the real drop to 17,251 in probe 5 arrived only when the tool list was added.

**Consequence for this evidence:** the numbers remain valid as measurements of what they actually measured (tool-schema removal), but they do NOT describe the final argv, which now uses `--tools ""` plus `--safe-mode` instead. Probe 6's finding (an empty `--allowed-tools` does not strip schemas, 26,403) is also superseded: `--allowed-tools` governs auto-approval, not availability, which is why it never stripped anything. `--tools ""` is the availability lever.

**Re-measured against the final argv (probes 7 and 8), and the result kills the cost argument entirely.** `--safe-mode` plus `--tools ""` does not merely trim the preamble, it removes it: cache-creation drops from 17,251 tokens to **ZERO**, and per-call cost from $0.173 to **$0.0024** on opus. The harness "preamble tax" that this doc spent three sections quantifying, framing, and mitigating does not exist under the flags the doc now specifies. Every cost-based objection to cli-as-default is void, including the one I raised against Scott's flip.

**Probe 8 also settles the html output boundary**, which was the Architect's CRITICAL finding. The envelope reports the granted ceiling directly:

| model | granted `maxOutputTokens` | the job's ceiling |
|---|---|---|
| `claude-haiku-4-5` | 32,000 | n/a |
| `claude-opus-4-8` | **64,000** | html = 64,000 |

The CLI grants opus-4-8 exactly the 64K the html job asks for. The truncation contingency stays in the doc as a gate, but it is now expected to pass rather than feared to fail.

What this establishes:

- Headless `claude -p` authenticates with no API key present. R3 is reachable.
- The pinned id `claude-opus-4-8` is accepted verbatim by `--model`. (`claude-opus-4-7`, the markdown pin, is NOT yet verified. Phase 0 covers it.)
- A large payload piped on stdin is read and used (probes 2, 4, 5).
- No directory-trust prompt in a brand-new `mktemp -d` (probe 3). Headless mode does not gate on trust, so it cannot hang there.
- The envelope carries `is_error`, `subtype`, `stop_reason`, `result`, `usage`, `total_cost_usd`, and `modelUsage.<id>.canonicalModel`.
- The harness preamble is a fixed per-call tax: ~17K tokens on opus with lean flags, ~39K without. The lean flags cut it 55%. This is a real cost delta versus the api path, quantified in Technical Considerations.

## Phase 0 Results: measured against a real report (2026-07-24, desk.lan)

Phase 0 ran and the GATE **PASSED** for both jobs. Payload: the real `--since 2026-07-01` report (1,310 sessions, 42 repos, 5.4MB collected JSON reducing to a **513,530-byte context block** + a 17,122-byte markdown / 14,950-byte html prompt). `claude 2.1.219`, `ANTHROPIC_API_KEY` unset via `env -u`, FINAL argv.

| job | model | stdin bytes | wall | output tokens | job ceiling | granted `maxOutputTokens` | cache-create | cost |
|---|---|---|---|---|---|---|---|---|
| markdown | `claude-opus-4-8` | 513,543 | **145s** | 12,706 | 16,000 | 64,000 | 242,534 | **$2.93** |
| html | `claude-opus-4-8` | 513,543 | **204s** | 19,574 | 64,000 | 64,000 | 241,952 | **$3.10** |

Success criteria, all met: both exit 0 with no key; both `is_error: false` / `subtype: "success"` / `stop_reason: "end_turn"`; the html result starts `<!doctype html>` and ends `</html>`; `canonicalModel == claude-opus-4-8` for both. Contingency table resolves to the first row (granted ceiling 64,000 >= real observed html output 19,574), so **both jobs go cli-default as designed**. `--max-turns 1` confirmed accepted (still undocumented in `--help`), so the minimum supported version pins at **2.1.219**.

Three findings that change the design. Each is folded into the sections below.

**F1: wall clock exceeds `SUBPROCESS_TIMEOUT`.** 145s and 204s both blow past the 120s `SUBPROCESS_TIMEOUT` that bounds pandoc and marquee. The doc already called for a separate `CLAUDE_TIMEOUT`; this measurement makes it load-bearing rather than tidy. Set to **900s**: 204s observed on a 23-day month, and a full month with more sessions is slower, so the margin is deliberately wide. A timeout here wastes a call we already paid for, which is the expensive direction to be wrong in.

**F2: `modelUsage` is a multi-entry map, because the CLI makes an internal haiku sub-call.** Both envelopes carry TWO entries: the pinned `claude-opus-4-8` AND `claude-haiku-4-5-20251001` (187,459 input tokens, 13-15 output, ~$0.19), which happens despite `--tools ""` and `--safe-mode`. No flag suppresses it. The consequence is mechanical: the guard must **look up `modelUsage[job.model()]` by key and compare that entry's `canonicalModel`** — it must NOT iterate the map comparing every entry, which would see haiku and false-positive a model-mismatch bail on a perfectly good render. The doc's earlier phrasing ("`modelUsage`'s `canonicalModel` equals `job.model()`") was ambiguous and the naive reading is a bug.

**F3: the withdrawn cost argument is REINSTATED, corrected. The cli path costs roughly 1.9x the api path per render, not "essentially nothing".** Probes 7 and 8 measured a trivial payload, so they measured only the absence of a harness preamble. That part holds: there is no preamble tax. But the *payload* is billed, and the CLI bills it as a **1-hour cache write at $10/Mtok** rather than as plain input at $5/Mtok. Derived from the envelope and confirmed to the cent:

| path | markdown render, 242,534 payload tokens + 12,706 output |
|---|---|
| cli | 242,534 @ $10/Mtok (1h cache write) = $2.425 + 12,706 @ $25/Mtok = $0.318, **plus** the haiku sub-call $0.188 = **$2.93** |
| api | 242,534 @ $5/Mtok (plain input) = $1.213 + 12,706 @ $25/Mtok = $0.318 = **~$1.53** |

The cache write is pure waste for this workload: one turn, no session persistence, nothing ever reads the cache back, and we pay 2x the input rate to populate it. So the honest statement is that cli-default costs a key holder about **+$1.40 per markdown render** (~+$1.6 on html), not nothing.

This does NOT reopen the default. Scott decided cli-default explicitly (R4) with the keyless-teammate rationale, and the escape hatch (`render.llm: api`) is already designed and documented. What changes is that the doc must stop claiming the flip is free, and the Rollout Plan's advice to bulk renderers gets sharper: a key holder rendering in volume should set `render.llm: api` for cost, not just for rate limits.

**F4: `HOME` is NOT load-bearing; the child authenticates with a completely empty env.** The doc predicted an `env_clear()` child without `HOME` would fail auth "in a way that reads as logged out". Measured, that is wrong: `env -i` with nothing at all -- no `HOME`, no `PATH` -- still returns `is_error: false` / `end_turn` / a correct result, because node resolves the home directory via `getpwuid` when `HOME` is unset. The one observable difference without `PATH` is a **stderr** warning that the child could not find `bwrap`/`socat` and disabled its own internal sandbox, which is irrelevant to us (`--tools ""` leaves it nothing to sandbox) but is noise in a failure report.

The allowlist is therefore set on fail-closed grounds rather than necessity:

```
PATH, HOME, NO_UPDATE_NOTIFIER=1
```

`HOME` is passed EXPLICITLY rather than leaning on the `getpwuid` fallback -- that fallback is an implementation detail of the runtime, and if it ever changes the failure would present as "logged out", which is the exact misdiagnosis this design keeps trying to avoid. `PATH` is passed to keep the child's own dependency resolution quiet; it is not a secret. `NO_UPDATE_NOTIFIER=1` is the stdout-contamination guard already decided. The binary is still exec'd by the absolute path `which::which("claude")` resolved, so `PATH` is never load-bearing for finding `claude` itself. Everything else -- all 13 `CLAUDE*` variables including the three secrets, and `ANTHROPIC_API_KEY` -- stays excluded by construction.

**F5: a headless render leaves nothing behind, and the SQLite refutation re-verified.** Before/after a render: session JSONL count unchanged (3,073 -> 3,073), lock-file count unchanged (599 -> 599), no file under `~/.claude` newer than the run. So `--no-session-persistence` suppresses per-session state AND lock files, not just the catalog entry -- the "each render adds a session clyde then catalogs" risk is eliminated in fact, not merely in intent. Independently re-confirmed the struck SQLite row by content-typing every file under `~/.claude`: zero files begin with `SQLite format 3` at any depth (the 599 lock files are 528 stale `security_warnings_state_*.lock` from 2026-07-08 interactive sessions, 67 `.lock`, 4 `bun.lock`). Two concurrent renders both succeeded with `end_turn` and no contention of any kind. Quota contention remains unproven-either-way: it cannot be demonstrated cheaply, and the fail-loud design reports it if it ever fires.

## Proposed Solution

### Overview

One port, two transports. The two call sites keep their shape and their prompts.

```
render_via_opus_markdown ─┐                                        ┌─ ApiTransport  (x-api-key -> api.anthropic.com)
                          ├─ summarize::{markdown,html}<T: Transport> ─┤
render_via_opus_html    ──┘                                        └─ CliTransport  (claude -p, existing login)
```

Selection precedence, house convention (flag > config > default):

1. `--llm api|cli` on the command line
2. `render.llm` in `clyde.yml`: `auto` | `api` | `cli`, default `auto`
3. `auto` resolves: `claude` on PATH -> cli | else key present -> api | neither -> loud error naming both remedies

`auto` prefers cli, per Scott 2026-07-24. A key is honored when the user asks for it (`--llm api`, `render.llm: api`) and is also the automatic fallback on a host with no `claude` binary.

**Correction (Staff Engineer round 2): the CI/server claim was overstated.** The api fallback only fires when `claude` is ABSENT. A CI image that HAS a `claude` binary but no usable login now fails even with a valid key, unless it sets `--llm api` or `render.llm: api`. That follows directly from the fail-loud decision below, and it is a rollout instruction, not a bug: any automated caller that renders must pin its transport explicitly rather than rely on `auto`. Phase 5 documents that.

**Selection is a presence check, never a success check. There is no fallback after a transport is chosen** (Scott, 2026-07-24: "fail loud"):

- `auto` asks exactly one question: does `which::which("claude")` resolve? Yes -> cli, committed. No -> is a key present? -> api. Neither -> error.
- Once cli is selected, EVERY failure is terminal: logged out, non-zero exit, malformed envelope, non-`end_turn` stop, model mismatch, timeout. None of them retry, and none of them silently switch to the api transport.
- The consequence is deliberate and worth stating: a host with a stale or logged-out `claude` on PATH AND a valid `ANTHROPIC_API_KEY` will FAIL, where the pre-flip default would have quietly rendered via api. That is the intended behavior. A silent fallback would make one command nondeterministic (two transports, two billing paths, two artifacts) and would mask a broken login indefinitely instead of surfacing it once.
- So the cli failure message must carry the escape hatch, every time:

The error must NOT hardcode a cause (Staff Engineer round 2). A non-zero exit means "the CLI failed", and `which::which("claude")` only proved an executable of that name exists; it distinguishes nothing about a stale version, a wrapper or shim, a broken install, bad global config, an expired login, a plan cap, or rate-limit exhaustion. So the error reports observations and lets the reader diagnose, and it always carries the escape hatch:

```
claude -p failed (exit 1)
  binary:  /home/user/.local/bin/claude
  version: 2.1.219
  stderr:  <first 500 bytes, trimmed>
try `claude` interactively to check the install and login, or pass --llm api to use ANTHROPIC_API_KEY
```

**Minimum supported `claude` version** is therefore part of this design, because the transport depends on flags whose availability varies: `--tools`, `--safe-mode`, `--strict-mcp-config`, `--no-session-persistence`, and `--max-turns` (which the installed 2.1.219 accepts but does not advertise in `--help`). Phase 0 pins the minimum to the version it verifies against, the transport reports the resolved version in every failure, and an unsupported-flag exit is surfaced verbatim rather than translated into a guess.

Why cli-default is the better default, not merely the requested one:

- Siblings behave identically. Every teammate's render comes out of the same path, so an artifact never silently differs because one person happens to have a key provisioned.
- The credential everyone already has drives the feature. The api key becomes the exception it should be, not the entry fee.
- It inverts the failure mode that started this: the default now works for the person with nothing configured, and the person with special configuration opts into it.

The cost consequence is real and stated plainly in Technical Considerations: a key-holding host now pays the ~17K preamble tax per render by default where it previously paid none. That is cents per render, against an artifact that was previously unreachable for most of the team.

The neither-credential error replaces today's `ANTHROPIC_API_KEY is required...` text and must name both doors, because there are now two:

```
no LLM transport available for --format html: set ANTHROPIC_API_KEY (--llm api),
or install the `claude` CLI and log in once (--llm cli)
```

Statelessness across replicas: every render is one process, one subprocess, no lock, no cache, no shared file. The design is identical at N=1 and N>1.

**Correction (both reviewers, round 2):** that claim is true of process state and false of two shared resources.

- **Message-based limits, not just token limits** (Architect). A logged-in seat is gated by message quota, not the api's token ceiling. Concrete failure: someone who just spent three hours heavily using Claude Code finds their `report render` fails on human quota. An api key would have absorbed it.
- ~~Local SQLite contention~~ (Architect). **REFUTED and struck.** The panel challenged its own reviewer's mechanism, and I verified independently: typing every file under `~/.claude` by CONTENT (not extension) yields zero SQLite databases at any depth. Session state there is JSONL. The only SQLite in play is clyde's own index (`clyde/src/main.rs:94`, `~/.local/share/clyde/sessions.db`), which `claude` never touches. A `database is locked` failure has no mechanism. Recorded as closed rather than deleted, so it is not re-raised.

 With cli as the default, concurrent renders contend on ONE Claude Code plan (rate limits, plan caps) rather than on a service key, so N>1 does share something after all. Consequences, stated rather than hidden: several parallel renders can hit a per-user rate limit that an api key would have absorbed; a plan cap surfaces as a render failure, which the fail-loud design reports rather than masks. This is accepted, not solved, and it is the strongest practical argument for a key holder setting `render.llm: api` on a machine that renders in bulk.

### Architecture

| module | change |
|---|---|
| `report/src/proc.rs` | NEW. `run_bounded` and `SUBPROCESS_TIMEOUT` moved out of `render.rs`; later `run_with_payload`. (`Output` is `std::process::Output`, already imported at `render.rs:21`. Nothing to define.) |
| `report/src/summarize.rs` | keeps the `Transport` port, the job consts, `postprocess_html`, the SSE parser |
| `report/src/summarize/api.rs` | NEW. `ApiTransport`: today's `request()` verbatim, plus `api_key_from_env` moved here from `title.rs` |
| `report/src/summarize/cli.rs` | NEW. `CliTransport` |
| `report/src/render.rs` | two call sites construct a transport instead of fetching a key |
| `report/src/cli.rs`, `config.rs` | `--llm` flag, `RenderConfig.llm`, and the two resolved model pins on `RenderConfig` |
| `common/src/config.rs` | `render.llm`, `render.markdown-model`, `render.html-model` serde fields, siblings of the existing `render.format` |

**Config-load blast radius (Staff Engineer finding 4, verified).** This is a real behavior change the first draft did not name. Today `report render` loads `clyde.yml` ONLY when `--format` is absent (`config.rs:91`), and `report::run` deliberately defers the load so a malformed config cannot break `render`/`merge` (`lib.rs:69`). Adding `render.llm` means render must load config whenever `--llm` is absent, even when `--format` IS present. So a malformed `clyde.yml` can now break a `--format html` invocation that previously worked.

Disposition: accept the change, because a config key that is not read is not config. But it is called out here, and Phase 4 owes explicit tests for it: malformed config with `--llm` present (must NOT load, must succeed) and malformed config with `--llm` absent (must fail loudly naming the config file, never silently default).

`api_key_from_env` moves to `summarize/api.rs` because that is the only thing that consumes a key after this change. `title.rs`'s `haiku()` takes its key as a parameter, so it is unaffected.

Invariants that survive untouched, and are the reason this is safe:

- `reject_foreign_numbers` runs in `render.rs` AFTER the transport returns (`render.rs:250`, `:271`). The no-invented-numbers contract is transport-agnostic.
- `postprocess_html` (fence strip, doctype/closing-tag/self-containment validation) runs inside `summarize::html` after the transport returns. Also transport-agnostic.
- Swapping transports therefore cannot weaken either guard. That is the whole safety argument.

### Data Model

None. No persisted structure changes: no schema version bump, no new table, no new report field, no cache. The only new durable artifacts are **three** config keys, all siblings of the existing `render.format`:

```yaml
render:
  llm: auto                        # auto | api | cli    (default auto, which prefers cli)
  markdown-model: claude-opus-4-8  # the markdown job's pin
  html-model: claude-opus-4-8      # the html job's pin
```

Both model keys default to `claude-opus-4-8`, so an absent `clyde.yml` resolves to exactly what Phase 0 measured. Kebab-case per the house convention, `deny_unknown_fields` like every other owned config struct. The `Job`/envelope types below are in-process only.

**Consequence for the config-load blast radius, which widens beyond what the Staff Engineer flagged.** That finding was about `render.llm` making render load `clyde.yml` even when `--format` is present. The model keys make it unconditional: render ALWAYS needs a model, so it ALWAYS loads config, and there is no flag that opts out (unlike `--llm`). A malformed `clyde.yml` therefore breaks every `report render` invocation. Accepted for the same reason as before -- a config key that is not read is not config -- but the Phase 4 test matrix grows a case: malformed config with BOTH `--format` and `--llm` present must still fail loudly, naming the config file, because the model pin is still needed.

### Fit with the rest of clyde

- Third consumer of an established in-house pattern: `report` already shells out to `pandoc` (`render.rs:1046`) and to `marquee` (`render.rs:1111`), where `marquee` owns its own Okta tokens and clyde never touches them. `claude` is the same shape: a CLI that owns its auth.
- The port mirrors `sessions::llm`'s `Completer`/`Narrator` seam, so `report` stops being the one crate that hardcodes its LLM transport.
- Deliberate scope boundary: `session enrich` and `efficiency --narrate` construct `AnthropicClient::from_env()` and have the identical keyless problem (`sessions/src/llm.rs:89`, `efficiency/src/lib.rs:122`). They are NOT changed here, because Scott asked for render. The `Spec`-shaped port is intentionally compatible with `Narrator`, so extending it later is a wiring change rather than a redesign. Recorded in the Addendum so it is a known choice, not an oversight.

### API Design

```rust
/// One prose completion: the job's system prompt plus its instruction and facts -> the model's
/// text reply. Implementations own their own transport knobs.
///
/// `prompt` and `json_body` stay SEPARATE arguments deliberately. The api transport joins them into
/// one user message; the cli transport must deliver them over two different channels (instruction
/// on argv, facts on stdin), and a pre-joined string would force it to either re-split a 200KB
/// blob or push the whole thing through argv into `ARG_MAX`.
pub trait Transport {
    fn complete(&self, job: Job, model: &str, system: &str, prompt: &str, json_body: &str) -> Result<String>;
}

/// The two real render jobs. Identifies WHICH job is running; every transport knob is private to
/// the transport that has one (api owns `max_tokens` + the streaming choice; cli owns its argv).
#[derive(Clone, Copy)]
pub enum Job { Markdown, Html }
```

**`model` is a parameter, not a `Job` method** (changed from the pre-execution draft, on Scott's "configurable in the XDG .config .yml"). The pin is no longer a compile-time fact, so `Job::model()` returning `&'static str` is no longer expressible. The resolved pin threads down from `RenderConfig` (which reads `render.markdown-model` / `render.html-model`) as an explicit argument. `Job` keeps its api-private knobs and gains no field it cannot honor, so the no-lying-field rule still holds: the model is the ONE fact both transports need, and it is now passed rather than derived.

`ApiTransport` maps `Job` to its own `max_tokens` and its own streaming choice. `CliTransport` maps `Job` to `--model` and nothing else. Neither transport carries a knob it does not use, and nothing is derived from a value that means something different.

This replaces the earlier `Spec { model, max_output_tokens }` plus a `max_output_tokens > STREAM_ABOVE_TOKENS` threshold. The Architect flagged that derivation as too clever and as coupling payload size to delivery mechanism, which is the "two signals never encode the same meaning" rule; he registered it without blocking. He is right, and the `Job` enum is strictly better than both his suggested fix (an explicit `stream: bool` the cli transport ignores, which is the lying field I rejected in Pass 5) and my original. Converged, not conceded: no ignored field AND no derived coupling.

- `summarize::markdown` / `summarize::html` become generic over `T: Transport` and drop `api_key: &str`.
- `MARKDOWN_MAX_OUTPUT_TOKENS` / `HTML_MAX_OUTPUT_TOKENS` stay as api-private consts on `ApiTransport`. `MARKDOWN_MODEL` / `HTML_MODEL` become the serde DEFAULTS for the two new config keys (both `claude-opus-4-8`), not the values the code reads directly.
- The `format!("{prompt}\n\n```json\n{json_body}\n```\n")` join that `request()` does today moves INTO `ApiTransport`, unchanged. That is what keeps AC3 (byte-identical api request body) mechanically true.
- The cli transport sends the identical fenced block as its stdin payload, so the model sees the same content in the same order on both transports. The transports differ only in delivery channel, never in what the model reads.
- Streaming is an api-transport concern and never appears on the port, because a field the cli transport ignores would be a lying field. `ApiTransport` maps `Job` directly to its `(max_tokens, stream)` pair: `Markdown -> (16_000, false)`, `Html -> (64_000, true)`. That is today's behavior exactly, and a unit test asserts both mappings. This is the `Job`-enum form of the decision; it replaces the earlier `max_output_tokens > STREAM_ABOVE_TOKENS` threshold the Architect flagged as two signals encoding one meaning.

Dispatch at the two call sites is a `match` on the resolved selection, monomorphized per transport. No `Box<dyn Transport>`: the house rule is generics for DI, never trait objects.

```rust
// render.rs, render_via_opus_markdown — `cfg.markdown_model` is the resolved config pin
let prose = match cfg.llm {
    Llm::Api => summarize::markdown(&ApiTransport::from_env()?, &cfg.markdown_model, prompt, json_body)?,
    Llm::Cli => summarize::markdown(&CliTransport::resolve()?, &cfg.markdown_model, prompt, json_body)?,
};
```

### The CLI transport

Verified against `claude 2.1.219` local `--help`, on the Staff Engineer's finding that my first argv was both weaker and partly no-op:

```
claude -p <instruction>
  --model <job.model()>
  --output-format json
  --system-prompt <the same system const the api path sends>
  --tools ""                  # disables ALL built-in tools, structurally
  --safe-mode                 # no CLAUDE.md, skills, plugins, hooks, MCP, agents; auth preserved
  --strict-mcp-config         # no MCP servers from any config file
  --no-session-persistence    # nothing written to disk, nothing to catalog
  --max-turns 1
< report JSON on stdin
```

What changed from the first draft, and why each is not cosmetic:

- `--tools ""` REPLACES the enumerated `--disallowed-tools` list. Help text: "Use \"\" to disable all tools". This is the structural fix I asked the reviewers for and did not find myself: tool-list drift ceases to exist as a concept, because nothing is enumerated. The whole drift risk row is deleted rather than mitigated.
- `--safe-mode` REPLACES the temp-cwd trick as the isolation mechanism. Temp cwd only defeated project `CLAUDE.md` discovery; user and global customizations still loaded. `--safe-mode` disables customizations wholesale while preserving auth, which is exactly the shape this needs. Temp cwd is retained as belt-and-suspenders, demoted from mechanism to hygiene.
- `--no-session-persistence` means a render no longer writes a session to disk at all. That deletes the "each render adds a Claude Code session that clyde then catalogs" risk instead of accepting it.
- `--exclude-dynamic-system-prompt-sections` is DROPPED. Help text: "Only applies with the default system prompt (ignored with `--system-prompt`)". Since we always set `--system-prompt`, it was a no-op in every probe that included it. See the evidence-table correction.

- Binary resolved via `which::which("claude")`, mirroring `clyde::resolve_claude` (`main.rs:565`), which already canonicalizes a relative PATH hit before any chdir.
- The instruction is small and fixed so it rides argv; the report JSON is large and rides stdin, which has no `ARG_MAX` ceiling.
- Child cwd is still a fresh `tempfile::tempdir()`, but as hygiene, not as the isolation mechanism: `--safe-mode` is what actually disables customizations, and it covers the user and global scopes that a temp cwd never did.
- **The child env is BUILT, not inherited** (`Command::env_clear()` then an explicit minimal allowlist). This started as "remove `ANTHROPIC_API_KEY`" and the Staff Engineer's round-2 blocker on scrubbing `CLAUDE*` sent me to measure what actually leaks. Measured in a live agent session on this host: 13 `CLAUDE*` variables, three of which are SECRETS -- `CLAUDE_COST_ANTHROPIC_API_ADMIN_KEY`, `CLAUDE_COST_SLACK_APP_TOKEN`, `CLAUDE_COST_SLACK_BOT_TOKEN`. An inherit-by-default child would receive an Anthropic ADMIN key and two Slack tokens on every render. That is a secret-exposure bug, not a tidiness issue, and a denylist is the wrong shape for it: the next secret-bearing variable someone adds would leak silently. So the child gets `env_clear()` plus only what it provably needs (`PATH`, `HOME`, and whatever auth resolution requires -- Phase 0 determines the exact minimum set empirically).
  - Also excluded by construction: `CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_CHILD_SESSION`, `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_CODE_EXECPATH`, `CLAUDE_TMPDIR`, `CLAUDE_EFFORT`. An agent-invoked render must not present itself to the child as a nested session of the caller.
  - `ANTHROPIC_API_KEY` is excluded for the original reason too: `--llm cli` must mean what it says, and cost attribution must never silently flip to the key.
- Tool-list drift is GONE as a risk class, not mitigated. `--tools ""` disables the built-in set wholesale, so there is no list to drift and no per-tool enumeration to maintain. `--strict-mcp-config` and `--safe-mode` close the MCP and customization surfaces the same way. The `stop_reason == "end_turn"` guard stays as defense in depth, but it is no longer the primary control. (This supersedes the Pass-4 disposition entirely; credit to the Staff Engineer for finding the flag I missed.)
- Output ceiling is CHECKED, not assumed (Staff Engineer finding 3). `end_turn` proves the model stopped naturally; it does NOT prove the output stayed under the job's ceiling, because the cli transport cannot set `max_tokens` on the wire. So the transport also compares the envelope's `usage.output_tokens` against the job's ceiling and bails when it exceeds it. Note the `Job` refactor above already removed the lying-field half of this finding: `max_output_tokens` is now api-private, so the cli transport no longer advertises a ceiling it cannot set. The usage check is the remaining, real half.
- No `--fallback-model` is passed, so the CLI cannot silently swap models.
- **Stdout contamination is guarded two ways** (raised by the Architect, 2026-07-24). An npm-installed Claude Code can print an update-available notice, and anything ahead of the JSON makes `serde_json::from_str` fail, which would misreport a successful generation as a malformed envelope. So: set `NO_UPDATE_NOTIFIER=1` in the child env, AND parse defensively by seeking the first `{` rather than assuming stdout begins with the JSON root. Belt and suspenders, because the failure mode is a false negative on a call we already paid for.
- Not logged in is its OWN path: `claude` exits non-zero and prints to stderr WITHOUT emitting a JSON envelope. The transport must detect non-zero exit before attempting to parse, and surface the trimmed stderr plus the remediation ("run `claude` once to log in, or pass `--llm api` with a key"). Parsing first would report "malformed envelope" for what is really "you are logged out".
- When the envelope carries `is_error: true`, forward its own `error.message` verbatim (Architect round 2). An expired token produces a perfectly well-formed envelope saying exactly what is wrong; reporting that as "malformed envelope" or a generic failure throws away the one useful sentence the CLI gave us.
- Envelope guards, all bail loudly: `is_error == false`, `subtype == "success"`, `stop_reason == "end_turn"`, `result` non-empty after trim, and the model check below.
- **The model check is a KEYED LOOKUP, never a scan** (Phase 0 finding F2). Look up `modelUsage[<the requested model>]`, and compare THAT entry's `canonicalModel` to the requested pin through `claude_pricing::normalize_model_id` (already a public export and already a `report` dependency, so dated-suffix normalization is not reinvented here). A missing key is itself the mismatch bail. Do NOT iterate the map asserting every entry matches: Phase 0 measured a second, internal `claude-haiku-4-5` entry in both envelopes, so a scan would bail on every successful render.
- The envelope struct is NOT `deny_unknown_fields`: it is a wire frame owned by another tool that will grow fields. It parses the five fields above and ignores the rest, which is the documented forward-compatible-envelope carve-out to the strict-serde house rule.

### Subprocess shape: files, not pipes

`run_bounded` sets `stdin(Stdio::null())` and drains stdout only after the child exits. Its own doc comment restricts it to "commands whose combined output stays well under the OS pipe buffer". Both constraints are violated here: the payload is large, and an html artifact is hundreds of KB. Writing a big payload into a pipe while not draining stdout deadlocks, and a post-exit drain deadlocks the moment the child fills the 64KB stdout pipe.

So the cli transport does NOT reuse `run_bounded`. New sibling in `proc.rs`:

```rust
/// Run `cmd` with `payload` on stdin and both output streams captured, all three wired to temp
/// files rather than pipes, under a wall-clock ceiling. No pipe exists, so no pipe can fill and
/// no drain can deadlock. For large payloads and large output (the `claude -p` LLM call).
pub fn run_with_payload(label: &str, cmd: &mut Command, payload: &str, spawn_err: ...) -> Result<Output>
```

This extends the pattern render.rs already uses for pandoc, whose own comment says "large output ... goes to a file". Timeout is its own named const (`CLAUDE_TIMEOUT`), not the 120s `SUBPROCESS_TIMEOUT` that bounds pandoc and marquee.

`CLAUDE_TIMEOUT = 900s`, set from Phase 0's measured wall clock plus wide margin (finding F1). Both real jobs exceeded `SUBPROCESS_TIMEOUT` outright -- 145s markdown, 204s html -- so reusing it would have killed every render on a real month. The margin is deliberately generous because a timeout discards a call that has already been billed.

### Implementation Plan

Six phases. Deterministic and mechanical first, the new LLM path last.

#### Phase 0: Spike both jobs against a real report AND a worst case, zero code
**Model:** sonnet
**Status: DONE, 2026-07-24. GATE PASSED for both jobs.** Measurements, the three findings (F1 timeout, F2 keyed model lookup, F3 corrected cost), and the resolved contingency are in "Phase 0 Results" above. Minimum supported `claude` version pins at **2.1.219**. The synthetic worst case was not run separately: the real report is a 1,310-session / 42-repo / 513KB-context month whose html output (19,574 tokens) sits at 31% of the granted 64,000 ceiling, so the ceiling question the synthetic case existed to answer is already settled with 3.2x headroom on real data.
- Run the FINAL argv (`--tools ""`, `--safe-mode`, `--strict-mcp-config`, `--no-session-persistence`, `--max-turns 1`) by hand for BOTH jobs with the key unset. The probe table above does NOT describe this argv, so every cost and token number is re-measured here.
- Two inputs, not one (Staff Engineer finding 5: a single real report does not de-risk the worst case, and the prior html-render design already used synthetic larger-month evidence): the largest real report available, PLUS a synthetic high-output case.
- Verify the markdown pin `claude-opus-4-7` is accepted by `--model` (probe evidence only covers `claude-opus-4-8`).
- Record per run: stdin bytes, stdout bytes, wall clock, `stop_reason`, `usage.output_tokens`, granted `modelUsage.<id>.maxOutputTokens`, `canonicalModel`, and cost.
- Confirm `--output-format json` returns the FULL large result rather than eliding it, and note the documented 10MB piped-stdin cap against the largest real report's stdin size.
- Run TWO renders concurrently. Retargeted away from the refuted SQLite theory to what provably exists: whether message/plan quota contention shows up, and whether `--no-session-persistence` also suppresses Claude Code's per-session state and lock files or whether each render still leaves some behind.
- Determine the minimal child env that still authenticates, starting from `env_clear()` and adding back only what is required. That set becomes the allowlist in code. Two known load-bearing entries to start from, so the experiment does not fail for a boring reason and get misread as "OAuth needs more env than it does":
  - ~~`HOME`, because the login lives under it.~~ **Measured false (F4): an `env -i` child with no `HOME` at all still authenticates.** `HOME` is allowlisted anyway, on fail-closed grounds, so the transport never depends on the runtime's `getpwuid` fallback.
  - `PATH`, allowlisted to silence the child's missing-`bwrap`/`socat` stderr warning. It is NOT needed to find `claude`: the transport execs the absolute path `which::which("claude")` resolved.
- Record `modelUsage.<id>.maxOutputTokens` from each envelope. This is the output ceiling the CLI actually grants, and it is the number the html job lives or dies on.
- **Success criteria:** both jobs exit 0 with no key; both report `stop_reason == end_turn`; the html body starts `<!doctype html>` and ends `</html>`; `canonicalModel` equals the requested pin for both; measured wall-clock, total input tokens, AND granted `maxOutputTokens` recorded in the implementation notes.

**Phase 0 is a GATE, and it has a defined contingency** (raised by the Architect, 2026-07-24: the doc had no fallback if the spike failed, which is building with an open question).

The concern is real but the magnitude is measurable, not speculative. The Architect's guess was that the CLI hardcodes a low ceiling like 4096; probe 1's envelope contradicts that, reporting `maxOutputTokens: 32000` for haiku. So the risk is not "4K truncation", it is precisely this: the html job's ceiling is 64K, and 32K is the CLI's granted ceiling for at least one model. If opus-4-8's granted ceiling is also below 64K, a large month can truncate.

Contingency, decided now so Phase 1 never starts on an open question:

| Phase 0 finding | what ships |
|---|---|
| granted ceiling >= the html job's real observed output (Phase 0 measures actual output tokens too) | both jobs go cli-default as designed |
| granted ceiling below real observed html output | **markdown ships cli-default; html stays api-only**, and its "requires ANTHROPIC_API_KEY" error text stands. Documented as a known limit, not a silent degrade |

Note what that contingency protects: the markdown path is the one Stephen actually needed, and it is the one with a real observed output well under any of these ceilings. A truncating html path is never shipped, because `stop_reason != end_turn` bails before an artifact is written. Worst case is reduced scope, never a clipped document.

#### Phase 1: Extract subprocess helpers into `proc.rs`
**Model:** sonnet
- Move `Output`, `run_bounded`, `SUBPROCESS_TIMEOUT` from `render.rs` to `proc.rs`; update call sites (pandoc, marquee publish/whoami/login).
- **Success criteria:** `otto ci` green; no diff in behavior; `rg 'fn run_bounded' report/src/render.rs` returns nothing.

#### Phase 2: Introduce the `Transport` port and `ApiTransport`
**Model:** sonnet
- Add the `Transport` port and the `Job` enum; move `request()` into `summarize/api.rs` behind `ApiTransport`; move `api_key_from_env`; make `markdown`/`html` generic over `T: Transport` and take `model: &str`; add a `FakeTransport` in `summarize/tests.rs` (recording its `Job` + model and returning a canned reply), mirroring how `sessions` and `efficiency` already fake `Completer`/`Narrator`.
- Re-pin the markdown job to `claude-opus-4-8` (Scott's directive). Call sites still pass the module consts here; Phase 4 swaps them for config-resolved values.
- **Success criteria:** a unit test asserts the serialized request body for each job is byte-identical to the pre-change body **modulo the deliberately re-pinned markdown model** (the baseline fixture carries `claude-opus-4-8`); a unit test asserts the `Job -> (max_tokens, stream)` mapping (Markdown -> 16K/false, Html -> 64K/true). **Both were met as written and both have since been superseded** by the ceilings design: the baseline was rebaselined onto the raised markdown default (see AC3), and the `(max_tokens, stream)` tuple was split into two separate assertions when `Job::api_limits()` was deleted, because it packed the shared ceiling and the api-private streaming flag into one value.

#### Phase 3: `CliTransport`
**Model:** opus
- `summarize/cli.rs`, `proc::run_with_payload`, argv construction, envelope parse, all five guards.
- **Success criteria:** fixture tests over recorded envelopes cover six cases: success, `is_error: true`, `stop_reason: "max_tokens"`, `canonicalModel` mismatch, empty `result`, and non-zero exit with no envelope at all (logged out). Each failure case is proven to bite by breaking the guard and watching the test fail before it is committed.

#### Phase 4: Wire transport selection
**Model:** sonnet
- `--llm` flag (`value_enum`, `ignore_case`), `render.llm` config field, precedence, rewritten missing-credential errors naming both remedies, `--help` text.
- The two model config keys (`render.markdown-model`, `render.html-model`, both defaulting to `claude-opus-4-8`) and the plumbing that carries the resolved pins on `RenderConfig` to the two call sites, replacing the Phase 2 consts.
- **Success criteria:** unit tests for all three precedence levels and all three `auto` outcomes (`claude` present -> cli; absent + key -> api; neither -> error); a test asserting a configured model pin actually reaches the built request/argv; and the malformed-config matrix -- `--llm` present + `--format` present must STILL fail loudly (the model pin is always needed), absent config resolves to the `claude-opus-4-8` defaults.

#### Phase 5: Docs and shakedown
**Model:** sonnet
- README, CLAUDE.md, the `--format` help text that currently says html "requires ANTHROPIC_API_KEY".
- **Success criteria:** `/cli-shakedown` on `report render` clean; no doc still claims a key is required.

## Acceptance Criteria

- [x] AC1: `env -u ANTHROPIC_API_KEY clyde report render --format markdown -i <real>.json -o -` exits 0 and emits the model-authored report, not the template. Mechanically: output exceeds 5,000 bytes AND contains at least three `^## ` headers AND does not contain the string `Generated offline via`. (The template path produces six string substitutions and cannot clear that bar.) **Flipped from PARTIAL to met on 2026-07-25**, once the ceilings work removed the blocker: the full 2026-07 month (1,328 sessions, 519,124-byte payload) rendered keyless at exit 0 into 14,846 bytes with 9 `^## ` headers and no offline marker, with the log naming `selected=Cli (requested=Auto)`. Evidence and the caveat about that run's output size are in `docs/design/2026-07-25-render-output-ceilings-config-implementation-notes.md`, Phase 3.
- [x] AC2: same invocation with `--format html` exits 0; output starts `<!doctype html>` and ends `</html>`.
- [x] AC1b/AC2b: both of the above are ALSO run with an explicit `--llm cli`, and the emitted log names the cli transport. Passing keyless is not by itself proof the cli path ran (Staff Engineer round 2); the transport must be asserted, not inferred.
- [x] AC1c/AC2c: and both are run with NO `--llm` flag, no key, and `claude` confirmed on PATH, which is what proves `auto`'s DEFAULT routing works rather than just its explicit form (Architect round 2). AC1b proves the transport; AC1c proves the default.
- [x] AC3 (**REBASELINED, and its stronger contract RETIRED**): `--llm api` produces a serialized request body for both jobs that matches the **current declared baseline**, asserted byte-for-byte. It no longer asserts "byte-identical to pre-change behavior", and that is a deliberate retirement rather than a quiet drift, recorded in `docs/design/2026-07-25-render-output-ceilings-config.md` ("The retired contract, stated plainly"). The baseline moved twice: once for the re-pinned markdown model (`claude-opus-4-7` -> `claude-opus-4-8`, Scott's directive), and once when the markdown `max_tokens` default rose and became the user-settable `render.markdown-max-output-tokens`. A default a user can move is not a fact about "pre-change behavior", so the assertion is now anti-rot against the declared baseline: field order, the `stream` omission, the system prompt, the prompt/json join, and the current defaults. The api path is opt-in now, but it must not rot.
- [x] AC11: the model pins are configurable and actually plumbed. `render.markdown-model` / `render.html-model` in `clyde.yml` reach the built api request body AND the built cli argv (`--model <value>`); an absent `clyde.yml` resolves both to `claude-opus-4-8`; an unknown key under `render:` still fails loudly via `deny_unknown_fields`.
- [x] AC12: the cli model guard survives a real multi-entry envelope. A fixture envelope carrying BOTH the pinned model and an unrelated `claude-haiku-4-5` entry in `modelUsage` must PASS the guard (keyed lookup), while an envelope whose pinned-model key is absent or whose `canonicalModel` differs must bail. This is Phase 0 finding F2 made mechanically checkable.
- [x] AC6: with NEITHER a key nor `claude` on PATH, render exits non-zero with an error naming both remedies, and never emits a partial artifact.
- [x] AC7: with a logged-out `claude` on PATH AND a valid `ANTHROPIC_API_KEY` set, render FAILS (it does not silently fall back to api), and the error text names `--llm api` as the remedy. This is the fail-loud decision made mechanically checkable.
- [x] AC4: the built child `Command` inherits NOTHING by default. Unit test over the built command asserts `env_clear` plus an explicit allowlist, and specifically that none of `ANTHROPIC_API_KEY`, `CLAUDE_COST_ANTHROPIC_API_ADMIN_KEY`, `CLAUDE_COST_SLACK_APP_TOKEN`, `CLAUDE_COST_SLACK_BOT_TOKEN`, `CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_CHILD_SESSION`, `CLAUDE_TMPDIR`, or `CLAUDE_EFFORT` reaches the child. The test enumerates by name so a future secret-bearing variable fails loudly rather than leaking.
- [x] AC5: every cli failure mode bails with a named, cause-honest error, each proven by a break-the-code test. The enumerated set (the earlier doc said "five", Phase 3 listed six, and both were short): non-zero exit with no envelope, `is_error: true`, `subtype != "success"`, `stop_reason != "end_turn"`, `usage.output_tokens` over the job ceiling, `canonicalModel` mismatch, empty `result`, malformed or non-JSON stdout, stdout with leading noise before the JSON root, wall-clock timeout, and unsupported-flag exit from a stale CLI.
- [x] AC8: the built argv contains `--tools ""`, `--safe-mode`, `--strict-mcp-config`, and `--no-session-persistence`. A unit test asserts each by name, so none can be dropped silently in a later refactor.
- [x] AC9: a render logs which transport was selected, and the resolved `claude` path and version when cli. An operator reading a log can tell what paid for the artifact without rerunning it.
- [x] AC10: `render.llm: api` overrides `auto` even when `claude` IS present on PATH. ~~and `--llm` present means `clyde.yml` is not loaded at all~~ **SUPERSEDED by the model-pin config directive:** render now loads `clyde.yml` UNCONDITIONALLY, because the model pin lives there and no flag opts out of needing one. So the assertion inverts -- a malformed `clyde.yml` must fail loudly and name the file even when BOTH `--format` and `--llm` are given, and an absent `clyde.yml` must resolve both pins to `claude-opus-4-8`. Proven with a deliberately malformed config file either way.

## Resolved Decisions

| date | decision | rationale |
|---|---|---|
| 2026-07-24 | `auto` prefers CLI; api key is opt-in and the no-`claude` fallback | Scott, explicit override: "allow the user to use an api key if they want but default to claude -p". Supersedes the earlier Pass-2 decision to prefer api. Settled, not to be relitigated: reviewers may flag consequences, not re-decide the default |
| 2026-07-24 | no fallback to api after the cli transport fails; fail loud | Scott, asked directly and answered "fail loud to your question". Selection is a PRESENCE check on the `claude` binary, never a success check. A silent fallback would make one command nondeterministic across two transports and two billing paths, and would hide a broken login forever. The error names `--llm api` as the escape |
| 2026-07-24 | `--llm cli` removes the key from the child env | otherwise the flag lies about which credential pays |
| 2026-07-24 | child runs in a temp cwd | removes repo `CLAUDE.md`/hooks/settings from the render; makes output independent of the invoking directory |
| 2026-07-24 | ~~enumerate `--disallowed-tools`~~ SUPERSEDED: use `--tools ""` | Staff Engineer found the flag; verified in `claude 2.1.219` help ("Use \"\" to disable all tools"). Eliminates the drift risk class instead of mitigating it. My probe-6 reasoning was sound but asked the wrong flag: `--allowed-tools` governs auto-approval, not availability |
| 2026-07-24 | `--safe-mode` + `--strict-mcp-config` are the isolation mechanism; temp cwd demoted to hygiene | Staff Engineer: temp cwd only defeated PROJECT `CLAUDE.md`; user and global customizations still loaded. Verified flag disables customizations while preserving auth |
| 2026-07-24 | `--no-session-persistence` | deletes the self-cataloging side effect rather than accepting it |
| 2026-07-24 | drop `--exclude-dynamic-system-prompt-sections` | verified no-op: "ignored with `--system-prompt`", which we always set. My probe attribution was wrong and is corrected in the evidence table |
| 2026-07-24 | check `usage.output_tokens` against the job ceiling | Staff Engineer: `end_turn` proves a natural stop, not that output stayed under a ceiling the cli cannot set |
| 2026-07-24 | replace `Spec` with a `Job` enum | Architect flagged threshold-derived streaming as too clever (two signals, one meaning); his own fix was a `stream: bool` the cli ignores, which is the lying field. `Job` beats both: no ignored field, no derived coupling |
| 2026-07-24 | Phase 0 is a GATE with a written contingency | Architect: the doc had no fallback if the spike failed, i.e. building with an open question. Contingency table now in Phase 0 |
| 2026-07-24 | accept the config-load blast radius, with tests | Staff Engineer: render will now load `clyde.yml` even when `--format` is present. A config key that is not read is not config, so accept and test both precedence paths |
| 2026-07-24 | child env is BUILT (`env_clear` + allowlist), not scrubbed by denylist | Staff Engineer round 2 flagged `CLAUDE*` inheritance; measuring it found three SECRETS in the live env (an Anthropic admin key, two Slack tokens). A denylist leaks the next secret someone adds; fail-closed means allowlist |
| 2026-07-24 | error messages report observations, never a guessed cause | Staff Engineer round 2: `which` proves only that a file named `claude` exists. "Not logged in" was my guess dressed as a diagnosis |
| 2026-07-24 | pin a minimum supported `claude` version | the transport depends on `--tools`/`--safe-mode`/`--strict-mcp-config`/`--no-session-persistence`/`--max-turns`, and `--max-turns` is accepted-but-undocumented in 2.1.219. Phase 0 pins the floor it verified |
| 2026-07-24 | log the selected transport; document the one-line rollback | Staff Engineer round 2: nobody could tell which credential paid for an artifact. Observability is an AC now (AC9) |
| 2026-07-24 | correct the CI/server fallback claim rather than defend it | the api fallback fires only when `claude` is ABSENT; a CI image with a login-less `claude` fails. Automated callers pin `--llm api` |
| 2026-07-24 | the preamble-tax cost argument is WITHDRAWN, measured | probes 7/8 on the final argv: zero cache-creation tokens, $0.0024/call on opus vs $0.173 under the superseded flags. `--safe-mode` + `--tools ""` remove the overhead entirely. My objection to Scott's cli-default flip was priced on flags the doc no longer uses |
| 2026-07-24 | html 64K boundary is measured, not feared | probe 8: the CLI grants `claude-opus-4-8` exactly 64,000 output tokens, matching `HTML_MAX_OUTPUT_TOKENS`. The Architect's CRITICAL finding is closed with evidence; the contingency stays as a gate |
| 2026-07-24 | SQLite-lock concern REFUTED and struck | panel challenged its own reviewer; I verified by typing every file under `~/.claude` by content: zero SQLite at any depth, state is JSONL. The only SQLite is clyde's own index (`main.rs:94`). No mechanism exists |
| 2026-07-24 | ~~`HOME` is load-bearing for the env allowlist~~ **CORRECTED:** allowlist is `PATH` + `HOME` + `NO_UPDATE_NOTIFIER=1`, chosen fail-closed rather than from necessity | Phase 0 F4 refuted the premise: `env -i` with NOTHING still authenticates (node falls back to `getpwuid`). `HOME` is still passed explicitly so we never depend on that fallback -- if it changed, the failure would read as "logged out", the exact misdiagnosis this design avoids. `PATH` silences a child stderr warning about missing `bwrap`/`socat`; the binary is still exec'd by the absolute `which::which` path |
| 2026-07-24 | `prompt` and `json_body` stay separate port arguments | the cli transport delivers them on two channels; a pre-joined string forces an `ARG_MAX` risk or a fragile re-split |
| 2026-07-24 | check exit status BEFORE parsing the envelope | logged-out `claude` exits non-zero with no JSON; parsing first misreports it as a malformed envelope |
| 2026-07-24 | scope stays `report render`; `session enrich` and `efficiency --narrate` are untouched | Scott asked for render. The port shape is deliberately compatible with `sessions::Narrator` so unifying later is cheap, but unrequested scope does not ride along |
| 2026-07-24 | no `stream` field on `Spec`; derive from the ceiling via a named const | a struct field one transport ignores is a lying field |
| 2026-07-24 | files, not pipes, for the cli subprocess | large payload + large output would deadlock `run_bounded`'s pipe-and-post-exit-drain shape |
| 2026-07-24 | both jobs pin `claude-opus-4-8`; markdown re-pins off `claude-opus-4-7` | Scott, during execution, verbatim: "just use claude opus 4-8". Given as Phase 0 was about to spike the unverified 4-7 pin. Moots the pin-rejected risk row; 4-8 is the value Phase 0 measured on both jobs. AC3's byte-identical baseline is rebaselined onto the new pin |
| 2026-07-24 | model pins are CONFIG (`render.markdown-model` / `render.html-model`), not consts | Scott, during execution, verbatim: "those values should be configurable in the XDG .config .yml". House rule: tunables ride the standard delivery path. Forces `model` to be a port PARAMETER rather than `Job::model()`, and makes render's `clyde.yml` load unconditional |
| 2026-07-24 | `CLAUDE_TIMEOUT = 900s` | Phase 0 F1: real jobs ran 145s and 204s, both over the 120s `SUBPROCESS_TIMEOUT`. Reusing it would kill every real render. Wide margin because a timeout throws away an already-billed call |
| 2026-07-24 | the model guard is a KEYED lookup into `modelUsage`, never a scan | Phase 0 F2: the CLI makes an internal `claude-haiku-4-5` sub-call, so both real envelopes carry two `modelUsage` entries. A scan-and-compare-all would bail on every successful render |
| 2026-07-24 | the withdrawn cost argument is REINSTATED, corrected to ~1.9x | Phase 0 F3: probes 7/8 measured a trivial payload, so they only proved the preamble is gone. On the real 513KB payload the CLI bills ~242K tokens as a 1h cache WRITE ($10/Mtok vs the api's $5/Mtok input) plus a ~$0.19 haiku sub-call: $2.93 cli vs ~$1.53 api. Does NOT reopen the default (Scott decided cli-default on keyless-access grounds); it corrects the doc's claim that the flip is free and sharpens the bulk-render advice |
| 2026-07-25 | the markdown output ceiling becomes a `clyde.yml` key and its default rises; this design's last Open Question closes | Scott priced the three options in Open Questions and chose configurable-with-a-raised-default, consistent with his own directive that moved the model pins into `clyde.yml`. Executed under `docs/design/2026-07-25-render-output-ceilings-config.md`, which also retires this doc's "markdown stays byte-identical" contract and rebaselines AC3. Nothing dangles from this doc anymore |

## Alternatives Considered

### Alternative 1: Bearer token from `~/.claude/.credentials.json`
- **Description:** read the stored OAuth access token, call the Messages API with `Authorization: Bearer` plus the oauth beta header.
- **Pros:** no subprocess; keeps one HTTP code path.
- **Cons:** undocumented internal surface; we inherit refresh and expiry; storage differs by platform (keychain on mac, file on linux); the creds are issued to Claude Code, not to third-party clients; breaks silently on any Claude Code release.
- **Why not chosen:** `claude -p` yields the identical auth with none of that ownership. Rejected on sight, and it stays rejected.

### Alternative 2: Okta-gated LLM proxy holding one org service key
- **Description:** a marquee-style service behind the existing Okta edge, proxying Messages calls with one org key.
- **Pros:** works where no Claude Code login exists (servers, CI); central cost attribution.
- **Cons:** still a credential, plus a service to run and guard; solves a problem nobody currently has.
- **Why not chosen:** parked, not rejected. Revisit condition in the Addendum.

### Alternative 3: Narrate from a Claude Code skill instead of clyde
- **Description:** `plugin/skills/report/` hands the collected JSON to the agent already in session; it writes the prose and calls `--template`.
- **Pros:** zero auth, zero LLM code in clyde.
- **Cons:** only works inside a Claude Code session; useless in cron or a pipeline; the artifact stops being one command.
- **Why not chosen:** complementary, not a substitute for R1/R2. Recorded in the Addendum as a possible follow-on.

### Alternative 4: A rich deterministic renderer, no LLM at all
- **Description:** stop needing a model for the markdown/html artifact. Have Rust compute AND lay out a full report: tables, per-repo and per-day breakdowns, outlier lists, efficiency signals. The key problem disappears for everyone.
- **Pros:** no credential, no transport, no cost, no nondeterminism, no truncation class. Fits the standing "Rust does the math" principle perfectly.
- **Cons:** it does not produce the thing the artifact is FOR. The value of the rendered report is the prose narrative over the facts, which is a model-authored judgment. A deterministic renderer is a better `--template`, not a substitute for the model path. It also relitigates a decision already made and shipped in the html-render design.
- **Why not chosen:** solves a different problem (template poverty, see the Addendum) and would leave R1/R2 unmet. Worth naming because a reviewer will ask, and because the honest answer is that both can exist.

### Alternative 5: `apiKeyHelper` in Claude Code settings
- **Description:** point Claude Code at a command that vends a key.
- **Cons:** presupposes a key exists somewhere to vend. Does not address R3 at all.
- **Why not chosen:** does not solve the problem.

## Technical Considerations

### Dependencies

Zero new crates. `report/Cargo.toml` already carries `which`, `wait-timeout`, `tempfile`, `serde_json`, `eyre`. External runtime dependency: the `claude` binary on `PATH`, which is the same class of dependency as the existing `pandoc` and `marquee` requirements, with the same not-found error treatment.

### Performance and cost

**There is no harness preamble tax under the final argv, but the payload is billed at 2x the api rate.** Corrected against Phase 0's real-payload measurement (finding F3); the probe-7/8 figures below were measured on a trivial payload and describe only the preamble's absence.

- **Preamble: gone, confirmed.** Probes 7/8 on a tiny payload: zero cache-creation, 243 input tokens, $0.0024 per call. `--safe-mode` and `--tools ""` remove the system prompt and tool schemas the earlier ~17K/$0.17 figures were made of. This holds and is a real win over the superseded flags.
- **Payload: billed as a 1-hour cache WRITE, not plain input.** On the real 513KB context the CLI reports ~242K `cache_creation_input_tokens` at $10/Mtok, where the api path (which sends no `cache_control`) bills the same tokens as input at $5/Mtok. Nothing ever reads that cache back -- one turn, `--no-session-persistence` -- so it is 2x the input rate for zero reuse.
- **Plus an internal haiku sub-call**, ~187K input tokens for ~$0.19 per render, not suppressible by any flag (finding F2).

| path | markdown render (242,534 payload tokens, 12,706 output) | html render |
|---|---|---|
| cli | **$2.93** | **$3.10** |
| api | ~$1.53 | ~$1.70 |

So the corrected statement, replacing the three the doc previously carried:

- "The cli path costs an order of magnitude more." Still false. It is ~1.9x, not 10x.
- ~~"The flip costs a key holder essentially nothing."~~ **False.** It costs a key holder about +$1.40 per markdown render and +$1.6 per html render.
- "The tax is the price of reaching keyless users." True again, and now correctly sized: ~$1.40 per render buys a full-fidelity artifact for a teammate who previously got a 3-row table or a hard bail.

Cost attribution still follows the transport: cli bills the user's Claude Code seat, api bills the key. With cli as the default, report renders land in clyde's own usage numbers by default, which is a mild self-reference worth knowing about when reading a report about report costs.

### Security

- No credential is read, stored, logged, or transmitted by clyde. The `claude` binary owns auth end to end. This is strictly less credential handling than today.
- The child runs with tools disallowed, one turn, in a temp cwd, so a render cannot execute repo hooks or touch the working tree.
- The payload is clyde's own collected report JSON (already scope-gated and redacted upstream), not third-party content.
- No secret ever reaches argv. The report JSON goes through stdin, and the api key is not passed to the child at all.

### Testing Strategy

- Unit: the fake `Transport` covers `markdown`/`html` end to end with no network, exactly as `sessions`/`efficiency` already do with their `Completer`/`Narrator` fakes.
- Unit: recorded `claude -p --output-format json` envelope fixtures drive every cli guard, positive and negative.
- Unit: precedence matrix for `--llm` / `render.llm` / `auto`.
- Regression: the byte-identical api request-body assertion is what keeps R4 from rotting.
- Every negative test is proven to bite by breaking the guard and watching it fail before it is committed.
- No test shells out to the real `claude` binary. The live path is exercised by Phase 0 and the Phase 5 shakedown, not by CI.

### Rollout Plan

Blast radius is one repo (clyde). No schema change, no pricing change, no marquee change, no cross-repo coordination.

Ship order is forced by in-flight work: `report/src/{lib,render,report,tests}.rs` and `render/tests.rs` are modified in the working tree right now by the collect-once-render-from-data follow-up. Phase 1 moves code out of `render.rs`, so it collides head-on. **That work lands first, then this rebases onto it.** Starting Phase 1 before then is how you get a merge conflict in the exact file two changes are rewriting.

After merge: `bump`, `cargo install --path .`, then verify live by rendering keyless with `env -u ANTHROPIC_API_KEY` on a real month. Green CI is not done.

**Rollback is one line, and it is documented for users, not just for us** (Staff Engineer round 2, on operator visibility):

```
render.llm: api        # in clyde.yml, or --llm api per invocation
```

Phase 5 documents three things that follow from cli-as-default: that rollback line; that any automated caller (CI, cron) must pin `--llm api` explicitly rather than trust `auto`, because a `claude` binary with no login now fails loudly; and that renders bill the user's Claude Code plan, so bulk rendering on one machine can hit plan rate limits an api key would have absorbed.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| ~~Harness preamble cost tax per call~~ | - | - | **ELIMINATED** (the preamble itself). Probes 7/8: zero cache-creation on a trivial payload under the final argv |
| Payload billed as a 1h cache write, so cli costs ~1.9x api per render | **High** (measured, every render) | Low-Med (dollars, ~+$1.40/render) | accepted, not solved (Phase 0 F3). The default is Scott's explicit call on keyless-access grounds; `render.llm: api` is the documented one-line escape for a cost-sensitive key holder, and Phase 5 tells bulk renderers to set it |
| `claude` CLI flag or envelope drift breaks the transport | Med | **High** (raised: cli is now the default path, so drift breaks renders for everyone, not just keyless users) | parse only the five fields we need and bail loudly on each; `--llm api` is the documented one-flag escape hatch for a key holder; Phase 0's spike is re-runnable as the standing diagnostic |
| Long html generation exceeds the wall clock | Med | Med | `CLAUDE_TIMEOUT` set from Phase 0's measured duration plus margin, killed and reaped on overrun |
| Interactive prompt hangs a headless render | Low | High | measured: no trust prompt in a fresh dir; temp cwd, tools disallowed, one turn; wall-clock kill is the backstop |
| ~~Markdown pin `claude-opus-4-7` rejected by `--model`~~ | - | - | **MOOT.** Scott re-pinned both jobs to `claude-opus-4-8`, which Phase 0 verified accepted on both jobs (`canonicalModel` matched). The pin is now config-driven, so a future rejection is a config edit, not a code change |
| ~~New tool not in our disallow list gets offered~~ | - | - | **ELIMINATED, not mitigated.** `--tools ""` disables the built-in set wholesale; there is no list to drift |
| ~~Each render adds a Claude Code session that clyde then catalogs~~ | - | - | **ELIMINATED.** `--no-session-persistence` writes nothing to disk |
| Output exceeds the job ceiling without a `max_tokens` to stop it | Low | Med | measured: the CLI grants `claude-opus-4-8` exactly 64,000 output tokens, matching the html job's ceiling (probe 8). The transport still compares `usage.output_tokens` to the ceiling and bails, and Phase 0 keeps the contingency as a gate |
| Malformed `clyde.yml` now breaks a `--format html` render that previously worked | Low | Med | named explicitly under Architecture; Phase 4 owes both precedence tests |
| Preamble text ("Here is your report:") leaks into the markdown artifact | Low | Low | parity with today: the same system prompt forbids preamble on both transports, and this is not a new exposure. `postprocess_html` structurally catches it on the html side |
| ~~The ~17K preamble eats context headroom on a very large month~~ | - | - | **MOOT.** Phase 0 measured the real month: ~242K payload tokens against opus-4-8's reported 1,000,000-token context window, and there is no preamble to add to it |
| ~~Each render adds a Claude Code session that clyde then catalogs~~ | - | - | **DUPLICATE** of the `--no-session-persistence` row above, which eliminates it. This stale row is struck rather than deleted so the disposition is traceable |
| Long generation exceeds even a generous wall clock on a much larger month | Low | Med | `CLAUDE_TIMEOUT` is 900s against a measured 204s worst observed (4.4x headroom); an overrun is killed, reaped, and reported loudly, never a partial artifact |
| Phase 1 conflicts with the in-flight render.rs work | High | Med | ship order above: that work lands first, this rebases |

## Open Questions

- None.

  The one item this section held -- the markdown job's output ceiling being too tight for the largest
  months, surfaced by the Phase 5 live shakedown rather than by any test -- is **CLOSED**. Scott priced
  the three options and chose the third: make both ceilings config keys in `clyde.yml` and raise the
  markdown default. That work has its own doc, `docs/design/2026-07-25-render-output-ceilings-config.md`,
  which carries the full option pricing, the audit's sharpening (on the cli path the ceiling was a
  self-imposed budget, not a capability limit), and the AC3 rebaseline it forced. See the Resolved
  Decisions row dated 2026-07-25.

## Addendum: parked, with revisit conditions

- **Okta-gated LLM proxy** (Alternative 2). Revisit when a caller needs a render where no Claude Code login exists: a CI job, a cron on a server, or a hosted clyde. Until then it is a service to run for no observed problem.
- **A `report` skill that narrates in-session** (Alternative 3). Revisit as a follow-on if the one-command path proves awkward inside agent sessions. It does not replace R1/R2.
- **Offline `--template` quality**, and its richer sibling, a fully deterministic Rust-authored report (Alternative 4). Scott's "thats terrible" is partly about how poor the template artifact is. Out of scope here by construction: this doc removes the REASON anyone falls back to the template. If the template still matters afterward, that is its own doc.
- **The same transport for `session enrich` and `efficiency --narrate`.** Both hardcode `AnthropicClient::from_env()` and both are dead without a key, so a keyless teammate cannot enrich a catalog or narrate a session either. Not touched here because it was not asked for. Revisit condition: the first time someone hits the keyless wall on one of those two commands. The port shape chosen here is already compatible with `sessions::Narrator`, so that follow-on is wiring, not redesign.

## References

- The failed render: `~stephen/claude-code-usage-report-ltm-3914`
- `docs/design/2026-07-05-report-html-render.md` (the html path and its key requirement)
- `docs/design/2026-07-24-report-collect-once-render-from-data.md` (in-flight; forces ship order)
- `sessions/src/llm.rs` (the `Completer`/`Narrator` port precedent this copies)
- `report/src/render.rs:1046` (pandoc), `:1111` (marquee) - the in-house shell-out-to-a-CLI-that-owns-its-auth pattern
