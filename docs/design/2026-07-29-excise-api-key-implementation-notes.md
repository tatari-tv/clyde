# Implementation Notes: Excise the API Key

Companion to `2026-07-29-excise-api-key.md`. Findings land here per phase.

## Phase 0: Measure keyless enrich cost (2026-07-29, desk.lan)

**Verdict: GO.** The fixed-per-invocation tax that would have killed per-session invocation does not exist. Cost is proportional to payload.

**But Phase 0 also found a hard blocker the design did not anticipate: the 512-token output ceiling rejects 100% of enrich calls.** See Finding 3. Phase 3 cannot ship as written.

### Setup

Zero code. Two hand-run `claude -p` invocations using the exact `build_spawn` argv (`report/src/summarize/cli.rs:81-100`), keyless, with `child_env()`'s allowlist mirrored via `env -i`:

```
env -i PATH="$PATH" HOME="$HOME" NO_UPDATE_NOTIFIER=1 \
  claude -p "" \
    --model claude-haiku-4-5-20251001 \
    --output-format json \
    --system-prompt "$SYS" \
    --tools "" --safe-mode --strict-mcp-config \
    --no-session-persistence --max-turns 1 \
  < payload.txt
```

`$SYS` is the real `SYSTEM_PROMPT` extracted from `sessions/src/llm.rs:41-49` (587 bytes). Payloads are real redacted enrich payloads dumped with `clyde session enrich --dry-run --show-payload <dir>` (17 written, 1,072 B to 125,267 B, p50 12,320 B). `claude` 2.1.220 at `~/.local/bin/claude`. No `ANTHROPIC_API_KEY` in the child environment.

Probes chosen to span the widest available range, a **10.17x** payload delta, which is what separates fixed from proportional:

| | A (p50) | B (largest) | ratio |
|---|---|---|---|
| payload bytes | 12,320 | 125,267 | 10.17x |

### Verbatim `usage`, probe A (12,320 B)

```json
{
  "input_tokens": 10,
  "cache_creation_input_tokens": 4336,
  "cache_read_input_tokens": 0,
  "output_tokens": 5798,
  "service_tier": "standard",
  "cache_creation": { "ephemeral_1h_input_tokens": 4336, "ephemeral_5m_input_tokens": 0 }
}
```

`modelUsage`: one entry, `claude-haiku-4-5-20251001`, `inputTokens 4571`, `outputTokens 5812`, `cacheCreationInputTokens 4336`, `costUSD 0.042303`, `canonicalModel "claude-haiku-4-5"`. `is_error false`, `subtype "success"`, `stop_reason "end_turn"`, `num_turns 1`.

### Verbatim `usage`, probe B (125,267 B)

```json
{ "input_tokens": 10, "cache_creation_input_tokens": 35803, "output_tokens": 678 }
```

`modelUsage`: one entry, same model, `inputTokens 36038`, `outputTokens 701`, `cacheCreationInputTokens 35803`, `costUSD 0.111149`. `is_error false`, `subtype "success"`, `stop_reason "end_turn"`.

### Finding 1: input cost is PROPORTIONAL. No fixed tax. This is the GO.

| | A | B | ratio |
|---|---|---|---|
| payload bytes | 12,320 | 125,267 | 10.17x |
| `cache_creation_input_tokens` | 4,336 | 35,803 | **8.26x** |
| `input_tokens` | 10 | 10 | **1.0x (flat, and trivial)** |
| bytes per cache-write token | 2.84 | 3.50 | |

Payload tokens scale with payload. The non-scaling component is `input_tokens: 10`, which is ten tokens, not a tax. The design's feared fixed branch (~$23 per 100 sessions) is ruled out.

Note the byte-per-token density (2.84 to 3.50 B/tok) is worse than the 2.118 B/tok the design projected from predecessor `:510`. That inflates token counts ~1.4x vs projection, which is folded into Finding 4.

### Finding 2: there is no separate sub-call. #60's "$0.19 haiku sub-call" does not apply here.

`modelUsage` has exactly **one** entry in both probes. The design carried forward predecessor `:511`'s "non-suppressible internal haiku sub-call, ~187K input tokens for ~$0.19 per render," and treated it as the likely fixed tax.

Root cause of why it does not appear: that render pins **opus** as the main model, so a haiku sub-call showed up as a second `modelUsage` entry and was separately attributable. Enrich pins **haiku** (`ENRICH_MODEL`, `llm.rs:17`), so any internal sub-call is the same model and folds into the single bucket. Either way there is no separate fixed charge to pay, and the measured totals are what they are.

### Finding 3 (BLOCKER): the 512-token output ceiling rejects every enrich call

`MAX_OUTPUT_TOKENS = 512` (`sessions/src/llm.rs`), which the design carries into `Job.max_output_tokens` for `Kind::Enrich`. Measured `output_tokens`:

| probe | output_tokens | vs 512 ceiling |
|---|---|---|
| A | **5,798** | 11.3x over |
| B | **678** | 1.3x over |

**Both probes exceed the ceiling, so `CliTransport` would have rejected both** as truncated artifacts, despite both returning a valid, well-formed enrichment (probe A's reply is a correct 5-tag + summary JSON object).

Root cause, and why the api path never hit it: on the api transport `max_output_tokens` is **SET** as `max_tokens` on the wire, so the model is constrained to 512 and cannot exceed it. Over the CLI transport the ceiling cannot be set at all, only **CHECKED** against the returned `usage.output_tokens` (documented at `summarize.rs:68-71`). The CLI runs haiku with `maxOutputTokens: 32000` and emits reasoning tokens, so a short JSON answer costs thousands of output tokens: probe A's actual reply is a few hundred tokens, and the other ~5,500 are not in the reply.

Worse for any naive fix: the value is **wildly variable**, 5,798 vs 678 for a *larger* payload. It does not track payload size, so no low ceiling is safe and no simple multiple of 512 is defensible.

**Design change required in Phase 3.** Options, to be resolved before Phase 3 opens:
- Raise `Kind::Enrich`/`Kind::Narrate` ceilings to a value above observed reasoning output (32,000, the model's own `maxOutputTokens`, is the only value with a principled basis). Cheapest, and makes the check a truncation detector rather than a budget.
- Check `stop_reason` only and drop the output-ceiling check for these two kinds. The ceiling's stated purpose is catching truncated artifacts, and `stop_reason: "end_turn"` already proves non-truncation; both probes returned `end_turn`.
- Suppress reasoning for these jobs if a flag exists. Unverified, needs a probe.

Recommendation: the second. `stop_reason` is the direct signal, the ceiling is a proxy for it, and 512 was only ever meaningful as a wire-level `max_tokens` the CLI cannot set. This also matches `Kind::max_output_tokens_key() -> None` for these kinds: a ceiling nobody can configure and nobody enforces should not be pretended into existence.

### Finding 4: cost projection, and it clears the GO threshold

Solving the two measurements for effective rates: output lands at **$5.05/Mtok**, matching `pricing/data/pricing.json`'s `output_per_mtok: 5` for `claude-haiku-4-5`. Cache-write lands at **~$3.01/Mtok** (see Finding 5).

Per-session cost decomposes as payload cache-write plus reasoning output:

| component | A | B |
|---|---|---|
| cache-write | $0.013 | $0.108 |
| output | $0.029 | $0.003 |
| **measured total** | **$0.042303** | **$0.111149** |

Candidate mean payload is 40 KB (286 default candidates, 284 non-empty, 11.37 MB). At ~0.30 cache-write tok/B that is ~12,300 tokens, so ~$0.037 cache-write, plus output in the measured $0.003 to $0.029 band:

| | per session | 286-row backfill | per 100 new sessions |
|---|---|---|---|
| measured range | $0.040 to $0.066 | **$11 to $19** | **$4.0 to $6.6** |
| design GO threshold | | | at or under ~$5 |
| design NO-GO threshold | | | ~$22 |

**GO.** The recurring figure lands at $4.0 to $6.6 per 100 sessions against a ~$5 GO line and a ~$22 NO-GO line. It brackets the GO threshold rather than sitting cleanly under it, so the honest statement is: this is the proportional case, roughly 2x to 3x the api path, nowhere near the 11x fixed case that would have forced batching. Scott's ruling stands as GO; the batching alternative stays parked.

The output/reasoning component is the only real variance ($0.003 to $0.029, a 10x swing that does not track payload). If Finding 3 is resolved by suppressing reasoning rather than by raising the ceiling, the upper bound drops materially.

### Finding 5: the CLI's self-reported cost exceeds pricing.json by ~1.5x on cache-write

Not a Phase 0 blocker, recorded because clyde's own cost accounting reads `pricing/data/pricing.json`.

Using feed rates (`input_per_mtok: 1`, `output_per_mtok: 5`, `cache_1h_write_per_mtok: 2`, `cache_read_per_mtok: 0.1`):

| probe | feed-derived | CLI `total_cost_usd` | gap | gap per cache-write tok |
|---|---|---|---|---|
| A | $0.037967 | $0.042303 | $0.00434 | $0.99/Mtok |
| B | $0.075346 | $0.111149 | $0.03580 | $1.00/Mtok |

The gap is almost exactly `$1.00/Mtok * cache_creation_input_tokens` in both probes, implying an effective 1h cache-write rate of **~$3/Mtok** where the feed says **$2/Mtok**. Consistent across a 8.26x token delta, so it is a rate difference and not noise.

Not diagnosed here, and deliberately not chased: it is outside this design's scope and chasing it would be scope creep. What it means concretely is that clyde's reported cost for keyless enrichment will **understate** the CLI's own reported cost by ~1.5x on the cache-write component. Worth its own investigation, because it affects every cost number clyde prints for any cli-transport work, not just enrich.

### Finding 6: `--no-session-persistence` holds (R5 clear)

Baseline before probes: 3,247 `*.jsonl` and 0 `*.lock` under `~/.claude/projects`. After: 3,248 and 0.

The +1 is **not** a probe artifact. Attribution: the new file is `~/.claude/projects/-tmp-review-panel-r2-o4jA1B2n/`, the review panel's own round-2 Claude Code session, created by the `architect`/`staff-engineer` scripts running in `/tmp/review-panel/r2-o4jA1B2n` before the probes ran. Confirmed two ways:
- The probes ran with cwd = the repo, so a persisted session would land in `-home-saidler-repos-tatari-tv-clyde/`. The only recent file there is this session's own transcript.
- Grepping every session jsonl for the enrich `SYSTEM_PROMPT` returns 10 files, of which 9 are dated 2026-07-23/24 (pre-existing) and the tenth is this session's transcript, which contains the string because the prompt was printed into the conversation.

So no probe created a session. R5 is clear at this call volume. It is worth re-checking during Phase 3's live sweep, which will make hundreds of invocations rather than two.

### Finding 7: `tokens_in` must sum the cache buckets (confirms the design's R3)

Measured `input_tokens: 10` with `cache_creation_input_tokens: 4336`. A naive `usage.input_tokens` read would record **10** as `tokens_in` for a 12 KB payload.

This is exactly the failure the design's Data Model section predicted: `--budget-tokens` would become a no-op that silently never trips. The design's decisions both stand and are now measured rather than inferred:
- `tokens_in` sums `input_tokens + cache_creation_input_tokens + cache_read_input_tokens`.
- Absent `usage` is a hard error, never a zero.

Real values for the fixtures Phase 2 needs: A `tokens_in = 4346`, `tokens_out = 5798`. B `tokens_in = 35813`, `tokens_out = 678`.

### Finding 8: `api_error_status` is the structured auth discriminator, and the argv decides whether it appears

Probe C, a deliberate auth failure (`ANTHROPIC_API_KEY=sk-ant-bogus...`, which does not touch the OAuth login), run under the **exact final argv**. Verbatim classification fields:

```json
{ "is_error": true, "subtype": "success", "stop_reason": "stop_sequence",
  "terminal_reason": "api_error", "api_error_status": 401,
  "result": "Invalid API key · Fix external API key",
  "total_cost_usd": 0, "duration_ms": 280, "modelUsage": {} }
```

Three things follow.

**1. A typed discriminator exists: HTTP 401.** No prose matching is needed to classify auth. `Envelope` (`cli.rs:542-562`) does not currently deserialize `api_error_status`, so clyde is throwing the signal away.

**2. The argv decides whether the field is present.** The review panel probed the same failure with **bare** flags and got `subtype: "error_during_execution"` with `api_error_status` absent and `terminal_reason: "aborted_streaming"`, and concluded from the binary that the field is propagated only when `subtype === "success"` and therefore never survives an error. The inference from source was right; the conclusion was not, because under the final argv this failure sets `subtype: "success"` **and** `is_error: true` simultaneously, satisfying that gate. Any future probe of this envelope must use the argv `CliTransport` builds.

**3. `terminal_reason: "api_error"` is real.** It appears here and independently in clyde's own dated fixture (`cli/tests.rs:578`, "measured failure envelope, 2026-07-26"). Two measurements four days apart.

**Honest limit:** a bogus API key is not literally an expired OAuth token. Proving the OAuth-expiry shape would require logging Scott out, so the 401 path is measured and the expiry path is inferred from it. Relatedly, the expired-token fixture at `cli/tests.rs:558-566` is **hand-authored with no provenance comment**, unlike its measured neighbour at `:578`, so it is treated as unverified and Phase 3 re-captures or deletes it.

### Finding 9: a broken transport takes 179s per call (~14h sweep grind), and network is distinguishable from auth

Probe D, `ANTHROPIC_BASE_URL=http://127.0.0.1:9` under the final argv, completed:

```json
{ "is_error": true, "subtype": "success", "stop_reason": "stop_sequence",
  "terminal_reason": "api_error", "api_error_status": null,
  "result": "API Error: Unable to connect to API (ConnectionRefused)",
  "total_cost_usd": 0, "duration_ms": 176736 }
```

**179 seconds for a connection refused.** At 286 candidates in a sequential sweep, a dead transport grinds for **~14.2 hours** while failing every row. This is the circuit breaker's justification and it is independent of classification: a classifier fixes the accounting and does nothing about the wall clock.

**The classification table's belt-and-braces row is load-bearing, and is now measured rather than assumed.** Comparing the two failure classes under the final argv:

| | auth (probe C) | network (probe D) |
|---|---|---|
| `is_error` | true | true |
| `subtype` | `success` | `success` |
| `terminal_reason` | `api_error` | `api_error` |
| `api_error_status` | **401** | **null** |
| `result` | `Invalid API key · Fix external API key` | `API Error: Unable to connect to API (ConnectionRefused)` |
| `total_cost_usd` | 0 | 0 |

So the design's two sweep-fatal rows each catch one of them: the status row catches auth (401), and the `terminal_reason == "api_error"` with no status row catches network. Neither needs prose.

This also **disproves the review claim that auth and network failures are byte-identical**: under the final argv they differ in `api_error_status`. That claim came from bare-flag probes, which produce a different envelope shape entirely (Finding 8).

And it independently corroborates clyde's dated fixture at `cli/tests.rs:578` (2026-07-26), which carries `terminal_reason: "api_error"` with `result: "API Error: Unable to connect to API (ENOTIMP)"`. Different errno, same shape, four days apart. That fixture is real; it is the *other* one (`:558-566`, hand-authored) that remains unverified.

Both failure envelopes report `total_cost_usd: 0`, so a failed or tripped sweep is free.

### Finding 10: the 5,798 output tokens are reasoning, not content. The Non-Goal holds.

Review raised a fair worry: dropping Guard 6 removes the only remaining bound on output, because on the api path `MAX_OUTPUT_TOKENS` went out as a wire-level `max_tokens` and physically capped generation, while the cli path has no such lever. If enrich's `summary` (stored verbatim, `llm.rs:145`) or narrate's prose (returned unparsed and unclamped, `llm.rs:158-163`) ballooned, that would violate the Non-Goal "same prose contract."

Settled by reading the `result` text rather than the token count:

| probe | `output_tokens` | `result` chars | `result` tokens (approx) | reasoning not in `result` |
|---|---|---|---|---|
| A | 5,798 | 504 | ~126 | ~5,670 |
| B | 678 | 663 | ~165 | ~513 |

Both replies are schema-conformant: 5 and 7 tags (`MAX_TAGS = 7`), summaries of 395 and 532 characters, each 1 to 3 sentences exactly as `SYSTEM_PROMPT` asks.

**The content was never near the cap.** At ~126 and ~165 tokens, `result` sits far below 512, so the wire cap was not what kept these short on the api path either. The prompt is what constrains length, and it still does. The 5,798 figure is CLI-side reasoning that never enters `result`, and therefore never reaches `parse_enrich_json` or narrate's `first_text`. Dropping Guard 6 for these two kinds changes nothing clyde stores or displays.

Residual risk, stated rather than dismissed: without a wire cap there is no *hard* bound, only a prompt-shaped one. If a pathological session ever produced a multi-thousand-token `summary`, it would be stored verbatim. The cheap guard, if wanted later, is a post-parse clamp on `summary` mirroring the existing `MAX_TAGS` clamp on tags. Not adding it now: it is unrequested, and the measured behavior does not motivate it.

### Finding 11: output cost IS inside the recurring figure

Review questioned whether the $4.0-$6.6 per-100 figure was input-only, which would push the real number to $7-$9.5 and above the GO line. It is not input-only. Finding 4's per-session range is built as cache-write **plus** the measured output band:

```
$0.037 cache-write  +  $0.003 output  =  $0.040  (lower bound)
$0.037 cache-write  +  $0.029 output  =  $0.066  (upper bound)
```

The $6.6 upper bound decomposes as $3.70 cache-write + **$2.90 output** per 100 sessions, which is exactly the output figure review computed independently. Same number, already counted.

**What is true, and was already stated in Finding 4: the range straddles the GO line.** $4.0 to $6.6 against a ~$5 GO threshold and a ~$22 NO-GO threshold. Per the design's own criterion, that is Scott's ruling with the number in hand, not an automatic GO. The honest characterization stands: this is unambiguously the proportional case at roughly 2x to 3x the api path, and nowhere near the 11x fixed case that would have forced batching.

One lever worth knowing: output cost here is almost entirely reasoning overhead (Finding 10), not content. Probe A spent $0.029 on output to produce ~126 tokens of usable result. If reasoning were suppressible for these jobs, recurring drops to roughly $3.9 per 100, cleanly under the GO line. Whether a flag exists is unverified and would need its own probe.

### Finding 12: `MAX_THINKING_TOKENS=0` removes the reasoning tax. GO becomes unambiguous.

Finding 3 listed "suppress reasoning if a flag exists" as unverified. It exists. `claude` 2.1.220 reads `MAX_THINKING_TOKENS` from the environment and treats thinking as enabled iff the value is `> 0`, so `MAX_THINKING_TOKENS=0` disables it. (Mechanism located in the binary by review; measured here.)

Probes E and F re-run probes A and B on the **identical payloads** with the var set:

| | payload | output_tok (think) | output_tok (no-think) | cost (think) | cost (no-think) | elapsed |
|---|---|---|---|---|---|---|
| p50 | 12,320 B | 5,798 | **140** | $0.042303 | **$0.013960** | 52s -> **6s** |
| large | 125,267 B | 678 | **126** | $0.111149 | **$0.108301** | -> **6s** |

`cache_creation` is unchanged (4,336 -> 4,313 and 35,803 -> 35,780), confirming the saving is entirely output-side. `stop_reason: "end_turn"` on both.

**Quality does not degrade. It arguably improves.** Same payload, both schema-conformant, 5 tags each:

- with reasoning: `["security","code-review","github","rust","release-management"]`, 395-char summary.
- without: `["rust","github-api","security-review","authentication","binary-distribution"]`, 611-char summary that additionally names `browser_download_url`, the parent-directory-vs-target-file writability mechanism, and the redirect-auth-header suppression.

The no-reasoning summary is the more specific of the two. The large-payload run returned 4 tags (inside the prompt's 3-to-7 range) and a correct multi-phase summary.

**Three consequences.**

1. **The output-token variance that Finding 3 called "wildly variable" collapses.** 5,798 vs 678 becomes 140 vs 126. Output no longer tracks anything, it is just flat. This does not change the Finding 3 recommendation (`stop_reason` gating, no ceiling check) because output is still unsettable over the CLI, but it removes the pathology that made any ceiling indefensible.

2. **Cost clears the GO line.** Effective cache-write rate is consistent across both probes ($3.07 and $3.01 per Mtok). At the 40 KB candidate mean (~12,300 cache-write tokens):

   | | per session | 286-row backfill | per 100 new sessions |
   |---|---|---|---|
   | with reasoning | $0.040 to $0.066 | $11 to $19 | $4.0 to $6.6 (straddles GO) |
   | **without reasoning** | **~$0.038** | **~$11** | **~$3.8 (clears GO)** |

   The saving concentrates where output dominated: 67% cheaper on the p50 payload, only 2.6% on the large one, which was already output-light.

3. **The sweep gets ~9x faster.** 6s per call against 52s. A 286-row backfill goes from ~4.1 hours to **~29 minutes**, which also shrinks the exposure window the circuit breaker exists to bound.

**Design consequence, and a trap to avoid: this must be per-`Kind`, not global.** `child_env()` (`cli.rs:331`) is shared by every job on the transport. Setting `MAX_THINKING_TOKENS=0` there unconditionally would also suppress reasoning for `Kind::Slot` and `Kind::Judge`, silently changing what `report render` and `report eval` produce. That is unrequested scope and is not measured. So the variable is set only for `Kind::Enrich` and `Kind::Narrate`, and `Slot`/`Judge` keep today's behavior exactly.

Mechanically it is one conditional line in the built child environment, the same shape as the existing `NO_UPDATE_NOTIFIER=1` constant: a value clyde sets, not one forwarded from the parent, so the `env_clear()` allowlist and its fail-closed posture are untouched.

### Finding 13: `MAX_THINKING_TOKENS=0` flips narrate's verdict. Enrich ONLY.

Finding 12 extended the setting to `Kind::Narrate` on enrich-only evidence. That was asserted, not measured, and review was right to challenge it. Measured now, and the assertion was wrong.

Six runs against an identical, faithfully-formatted `format_facts` payload built from a real session (`ae31320f`, 193 turns, $48.56, cache-read share 93.6%, `flags: none`, worst signal 1h cache-write 90.7%), using the real `NARRATE_SYSTEM_PROMPT`:

| run | reasoning ON | reasoning OFF |
|---|---|---|
| 1 | "The session was **inefficient**..." | "This session was an **efficient** use of tokens." |
| 2 | "This session was **inefficient**..." | "...an **efficient** use of tokens overall..." |
| 3 | "This session was **inefficient**..." | "...an **efficient** use of tokens overall." |

**3/3 versus 3/3, cleanly separated.** The verdict is stable within each mode and opposite across them, so this is a deterministic effect of the flag, not run-to-run nondeterminism. A single sample each could not have distinguished those, which is why three were run.

That is a material change to what narrate produces, and the Non-Goal says "Excluded: changing what enrich or narrate produce."

**Resolved: set `MAX_THINKING_TOKENS=0` for `Kind::Enrich` only.** `Kind::Narrate`, `Kind::Slot`, and `Kind::Judge` all keep today's behavior. The asymmetry review named is the deciding argument: enrich runs 286 times so the 67% saving compounds, while narrate is a single interactive invocation whose cost saving ($0.025 to $0.002) is irrelevant. Narrate had nothing to gain and a user-facing verdict to lose. The latency win for narrate was real (36s to 8s) but does not justify silently changing an engineer-facing verdict.

Secondary cost measured for narrate: $0.025208 with reasoning, 4,764 output tokens, 36s.

**Observation, out of scope, worth a future ticket.** The two verdicts disagree about the same facts, and the no-reasoning one is arguably the more instruction-faithful: the payload carries `flags: none`, and `NARRATE_SYSTEM_PROMPT` rule 3 says "If the facts show no problems, say the session looks efficient." The reasoning path treats `worst signal` as a problem even when nothing is flagged, where `worst signal` only means worst *among* signals. That is pre-existing narrate behavior, unrelated to this design, and is not being changed here.

### Finding 14: `MAX_THINKING_TOKENS` is undocumented, so cost drift is the failure mode

`MAX_THINKING_TOKENS` is read straight from `process.env` in the `claude` binary and is not a documented public contract, the same category as `api_error_status` being marked `@internal`. If a future release renames it or stops honoring it, reasoning silently returns: enrichment still succeeds, `stop_reason` is still `end_turn`, nothing fails. Per-session cost simply goes back up ~3x and the sweep gets ~9x slower.

Because the failure mode is cost rather than correctness, it will not announce itself. The canary is already in these notes: **~140 output tokens and ~6s per enrich call** is the measured healthy band. A sweep whose per-call output tokens return to the thousands, or whose wall clock returns to ~52s, means the variable stopped working. Worth checking at the first Phase 3 live sweep and whenever `claude` is upgraded past the version floor.

### Artifacts

Raw envelopes, payloads, and the probe script under this session's scratchpad (`p0/`): `small.json`, `large.json`, `system.txt`, `probe.sh`, `payloads/`. Total measured spend for Phase 0: **$0.153452**.

## Phase 1: Move the transport to common

### Design decisions

- Moved `Transport`/`Kind`/`Job`/`check_stop_reason` from `report/src/summarize.rs` to `common/src/llm.rs`, `report/src/summarize/cli.rs` (+ its `cli/` dir) to `common/src/llm/cli.rs`, and `report/src/proc.rs` (both `run_bounded` and `run_with_payload`) to `common/src/proc.rs`, per the design's Architecture table. `report/src/summarize.rs::CliTransport` -> `common/src/llm/cli.rs::CliTransport` — `git mv`, no content edit inside the file.
- `check_stop_reason` (`common/src/llm.rs:check_stop_reason`) widened from a private `fn` to `pub fn`. Not in the doc's explicit re-export list (`Transport`, `Kind`, `Job`, `CliTransport`), but `report/src/summarize/api.rs` calls it via `use super::check_stop_reason` and that call must keep resolving until Phase 4 deletes `api.rs`. Same "no runtime behavior change, visibility only" reasoning the doc already applies to `run_bounded`/`run_with_payload`/`CLAUDE_TIMEOUT`/`SUBPROCESS_TIMEOUT`.
- `report/src/summarize.rs` (`report/src/summarize.rs`) rewritten as a thin shim: `pub mod api;` plus `pub use common::llm::{CliTransport, Job, Kind, Transport, check_stop_reason};`. Every existing `crate::summarize::*` call site in `report` (`render.rs`, `eval.rs`, `eval/judge.rs`, `render/slots.rs`, `summarize/api.rs`) keeps resolving unchanged.
- `common/src/llm/cli.rs`'s `use super::{Job, Transport};` and `use crate::proc;` needed NO edits: `super` still means `common::llm` (its new parent) and `crate::proc` still means `common::proc` (the sibling module also moved in this phase) — the relative module shape was preserved by moving both files into the same new tree.
- `report/src/render.rs`'s only import (`use crate::proc::run_bounded;` -> `use common::proc::run_bounded;`) repoints all three call sites (`pandoc --format pdf`, `marquee publish`, `marquee whoami`) at once; none of them reference `crate::proc` directly. Dropped `pub mod proc;` from `report/src/lib.rs` (`report/src/lib.rs`).
- `report/src/proc.rs`'s `run_bounded`/`run_with_payload`/`CLAUDE_TIMEOUT`/`SUBPROCESS_TIMEOUT` widened `pub(crate)` -> `pub` in `common/src/proc.rs`, exactly as the doc anticipates for the crate-boundary crossing.
- `common/Cargo.toml` gained `which`, `serde_json`, `thiserror` via `cargo add --package common` (workspace-pinned versions, matching `general.md`'s "never hand-edit Cargo.toml versions"). Verified already present before adding: `eyre`, `log`, `tempfile`, `dirs`. `wait-timeout = "0.2.1"` was ALREADY present in `common/Cargo.toml` as a bare version (not `.workspace = true`, since it is not a `[workspace.dependencies]` entry) — the doc's claim that it was already there is correct, but stated it would resolve to a workspace dep; it does not, and none was added since none was missing.
- Added `pub mod llm;` and `pub mod proc;` to `common/src/lib.rs`.

### Deviations

- **`report/src/summarize/tests.rs` moved to `common/src/llm/tests.rs`, which the design's Phase 1 bullet list does not name.** It tests `Kind::max_output_tokens_key` and `check_stop_reason` (`each_kind_names_its_own_ceiling_key`, `only_end_turn_is_accepted`, `a_truncation_error_names_the_stop_reason_and_a_remedy`, `check_stop_reason_missing_bails`), both of which this phase moves to `common/src/llm.rs`. Leaving the test file behind in `report` was not an option: `check_stop_reason` is private to its defining module, so a test file outside `common::llm` could not reach it at all after the move. `git mv`, no assertion changes. Consistent with the design's own instruction to move "the transport's tests with it."
- **`common` gained a crate-level `#[cfg(test)] pub(crate) static ENV_LOCK` (`common/src/lib.rs`), and `common/src/config/tests.rs` was repointed from its own local `ENV_LOCK` to the crate-level one.** Not named in the doc. Moving `llm::cli`'s env-touching tests into `common` recreates, inside `common`, the exact cross-module env-race hazard that `report/src/lib.rs`'s `ENV_LOCK` doc comment describes as the reason THAT lock exists (`summarize::cli`'s env-reading tests vs. `summarize::api`'s env-mutating tests, previously under separate locks). `common::config::tests` already mutates `XDG_CONFIG_HOME` under its own local `Mutex`; without unification, `config`'s tests and `llm::cli`'s tests could run in parallel, each under a different lock, while both touch the process environ block — a `set_var`/`remove_var` data race regardless of which variable names are involved (edition 2024 marks both functions `unsafe` for exactly this reason). Fix mirrors `report`'s existing pattern exactly: one crate-wide lock, every env-touching test in the crate takes it.
- **`common/src/llm.rs` and `common/src/proc.rs` gained new module-doc (`//!`) comments explaining the move and the visibility widening**; the design doc did not specify doc-comment content. Added so a reader of the moved file understands why `common` owns this code and why two functions/two consts that look private-shaped are `pub`, without needing to find this notes file first.
- `report/src/lib.rs`'s existing `ENV_LOCK` doc comment (the one that already told the `summarize::cli`-vs-`summarize::api` origin story) got one paragraph appended noting `summarize::cli` moved away, so it does not read as describing a module that no longer exists in this crate. Comment-only.

### Tradeoffs

- Widening `check_stop_reason` to `pub` (rather than, say, duplicating a private copy in `report`, or giving `report::summarize::api` its own truncation check) was chosen because a duplicate would immediately diverge from the design's own "one transport, one truncation contract" framing, and `api.rs`'s deletion is only three phases away — not worth inventing a second copy for.
- Kept the crate-level `common::ENV_LOCK` rather than leaving `config::tests`'s local lock alone and adding a second, separately-scoped lock for `llm::cli`'s tests. Two locks would technically also serialize `llm::cli`'s own tests against each other, but would NOT serialize them against `config::tests`'s env mutation, which is the actual hazard. One lock per crate is also exactly the precedent already shipped in `report`, so this keeps the two crates' test-isolation story symmetric rather than introducing a second convention.
- `git mv` was used for every file whose content did not need a full rewrite (`cli.rs`, `cli/tests.rs`, `tests.rs`, `proc.rs`, `proc/tests.rs`, and the initial `summarize.rs` -> `llm.rs` move), so `git diff --stat -M` reports them as renames. `report/src/summarize.rs` was then rewritten in place (via a fresh write, not a further `git mv`) into the thin shim, since its final content bears little resemblance to what moved out of it; git's similarity index correctly does NOT call that a rename, and that is the honest diff for what happened.

### Open questions

- None.

## Phase 2: Teach the transport input tokens

### Design decisions

- `Usage` (`common/src/llm/cli.rs::Usage`) gained `input_tokens`, `cache_creation_input_tokens`, and `cache_read_input_tokens`, all `Option<u64>` with `#[serde(default)]`, per the Data Model section verbatim. No `deny_unknown_fields`: `Usage` stays the same forward-compatible carve-out as `Envelope`, so a future CLI field lands unread instead of failing every call.
- `Usage::tokens_in()` (`common/src/llm/cli.rs::Usage::tokens_in`) sums `input_tokens + cache_creation_input_tokens + cache_read_input_tokens`, each defaulted to 0 for the sum. `Usage::tokens_out()` returns `output_tokens` defaulted to 0. Both are private methods on `Usage`, matching the struct's existing visibility; nothing outside `common::llm::cli` needs them yet (Phase 3 wires persistence).
- `check_envelope` (`common/src/llm/cli.rs::check_envelope`) gained a new Guard 6, inserted before the existing output-ceiling check: `envelope.usage` absent on an already-successful envelope (passed Guards 2-5) is now `bail!`ed with a message naming `job.kind` (`Slot`/`Judge` today), rather than silently continuing as the old code did. The old Guard 6 (output ceiling) and Guard 7 (model check) renumbered to 7 and 8; the module doc comment ("Guards 2-7") updated to "Guards 2-8".
- The renumbered ceiling guard (now Guard 7) reads `usage.tokens_out()` instead of `envelope.usage.as_ref().and_then(|u| u.output_tokens)`. Behaviorally identical for every envelope that reaches it: `usage` is now guaranteed `Some` by Guard 6, and `tokens_out()` on a `Usage` with `output_tokens: None` still evaluates to 0, same as the old code's "skip the check" branch when the sub-field was absent.
- Added one `debug!` at the top of the new Guard 6, logging `job.kind`, `tokens_in()`, and `tokens_out()` together, per the repo's function-level logging rule and to give `tokens_in()` a live (non-test) call site so `#![deny(dead_code)]` does not flag it while Phase 3 (the actual persistence wiring) is still two phases out.
- Unit tests (`common/src/llm/cli/tests.rs`) cover the four fixture shapes named in the design: `tokens_in_is_plain_input_tokens_when_no_cache_buckets_are_present` (input-only), `tokens_in_is_cache_write_alone_when_plain_input_tokens_is_absent` (cache-write-only), `tokens_in_sums_input_and_cache_write_probe_a` and `..._probe_b` (both, using Phase 0's verbatim Finding 7 fixtures and asserting the exact recorded sums: A `tokens_in=4346`/`tokens_out=5798`, B `tokens_in=35813`/`tokens_out=678`), and `check_envelope_bails_when_usage_is_absent_from_a_success_envelope_and_names_the_job` plus its `Kind::Judge` sibling (absent, hard error, message names the job).

### Deviations

- None. The struct shape, the `tokens_in`/`tokens_out` semantics, and the hard-error-on-absent-usage behavior all match the Data Model section and Finding 7 exactly.

### Tradeoffs

- Made the absent-usage hard error apply to every `Kind` (today `Slot` and `Judge`), not scoped to `Kind::Enrich`/`Kind::Narrate` (which do not exist until Phase 3). The Data Model section states the rule transport-wide, not per-kind, and every real `claude -p` success envelope measured in Phase 0 carried `usage`, so this is not expected to change observable behavior for `report render`/`report eval` in practice; it only replaces a silent skip with a loud failure on an edge case that was already unverified either way. Verified no existing fixture in `cli/tests.rs` exercises a success envelope with `usage` fully absent, so nothing needed to change or weaken to keep the suite green (see Interaction check below).
- Kept `tokens_in()`/`tokens_out()` as private `Usage` methods rather than exposing them on `Envelope` or widening visibility to `pub(crate)`/`pub`. Phase 3 is the phase that actually reads these values into `sessions::llm::ClaudeCli`, and doing so will require deciding how `Transport::complete`'s return value carries them out of `common::llm::cli` at all (it returns only `String` today); that decision belongs to Phase 3, not this one.
- Logged `tokens_in()`/`tokens_out()` at `debug!` inside Guard 6 rather than leaving `tokens_in()` unreferenced outside tests. The alternative (an `#[allow(dead_code)]`) is banned by the repo's Rust rules; a debug log is the smallest production call site that is both honest (it is genuinely useful operator information) and consistent with the crate's function-level logging convention, and it adds no behavior Phase 3 needs to undo.

### Open questions

- None. The task's flagged risk (absent-usage hard error breaking an existing `Kind::Slot`/`Kind::Judge` fixture) did not materialize: every existing test that reaches Guard 6 already supplies a `usage` object via the shared `envelope_json` test helper, and every raw-JSON fixture that omits `usage` bails at an earlier guard (`is_error`, missing `subtype`, or missing `stop_reason`) before Guard 6 is ever reached. No existing assertion was touched, weakened, or deleted.
