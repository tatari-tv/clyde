# Design Document: Excise the API Key

**Author:** Scott Idler
**Date:** 2026-07-29
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

clyde still requires `ANTHROPIC_API_KEY` for `session enrich` and `efficiency session --narrate`, still carries a redundant key-bearing transport in `report`, and still writes a systemd unit that loads a key file off disk. Remove all three. After this, clyde reads, stores, and transmits no credential: the locally installed `claude` binary owns auth end to end, for every LLM call in the workspace.

## Problem Statement

### Background

- #60 (`2026-07-24-report-render-claude-cli-transport`) added `CliTransport`: shell out to `claude -p`, piggy-back the Claude Code login the user already has, no key.
- That design scoped itself to `report render` on Scott's instruction. It recorded the boundary three times (`:254`, `:458`, `:595`) with an explicit revisit trigger: "the first time someone hits the keyless wall on one of those two commands."
- #76 (`render-inversion`) removed the html-source formats. `--format` is now `markdown | pdf | marquee-markdown`, and `CliTransport` serves all three. The "html stays api-only" carve-out that justified keeping the api path is gone.
- 2026-07-29, Slack `#claude-code`: Scott published the usage-report pipeline runbook and asked the team to run it. Keegan ran it and hit the wall on step one:

  ```
  warning: only 0 of 94 sessions in the window (0.0%) carry an enrich summary,
  below the 50.0% floor. Run `clyde session enrich` to raise coverage
  ```

  then, on running it: `GDI, enrich needs the API key`.

The revisit condition fired. This design is the follow-on #60 predicted.

### Problem

Four distinct surfaces, not one.

**1. Hard requirement. Two commands are dead without a key.**

| Site | Command |
|---|---|
| `sessions/src/llm.rs:29,30,81-101,169-230` (`x-api-key` at `:201`) | the single `AnthropicClient` behind both ports |
| `clyde/src/main.rs:840` | `clyde session enrich` (live pass) |
| `efficiency/src/lib.rs:122` | `clyde efficiency session --narrate` |

One client implements both `Completer` (structured enrichment JSON) and `Narrator` (prose). Both pin `claude-haiku-4-5-20251001` (`llm.rs:17,24`).

**2. Redundant second door. `report` still has a key path.**

| Site | What |
|---|---|
| `report/src/summarize/api.rs` + `api/tests.rs` | `ApiTransport`, `ENDPOINT:22`, `api_key_from_env:29`, `x-api-key:88` |
| `report/src/summarize.rs:4` | re-export |
| `report/src/render.rs:119,196,337` | constructs it, feeds key-presence into selection |
| **`report/src/eval.rs:43,448`** | **`report eval`'s judge: imports `ApiTransport` and dispatches `TransportKind::Api => judge::score(&ApiTransport::from_env()?, ...)`** |
| **`report/src/eval/tests.rs:409`** | **`ApiTransport::from_env().expect("ANTHROPIC_API_KEY must be set for this ignored test")`** |
| `report/src/config.rs:125,127,131` | the `--llm api` arm, and the error text advertising the key |
| `report/src/config.rs:244`, `report/src/cli.rs:265` | eval's own `--llm` surface and config fallback |
| `report/src/cli.rs` | `--llm api` flag value |
| `report/Cargo.toml:38` | `ureq`, whose ONLY use in `report/src/` is `summarize/api.rs` |

Nothing requires it post-#76. It is a second billing path and a second failure mode kept alive for a format that no longer exists.

**`report eval` is a second consumer, and an earlier draft of this doc missed it entirely.** The eval judge rides the same transport by design (`summarize.rs:29-32`: "it rides the existing transport rather than a second client, so the eval inherits `--llm` and needs no second credential"). That inheritance is exactly why deleting `ApiTransport` without touching `eval.rs` leaves `report` **not compiling**. Phase 4 covers render and eval together or it does not build.

**3. A key on disk, loaded by a timer.** The part that is not just an env read.

- `~/.config/clyde/enrich.env`: 127 bytes, mode 0600, present on desk.lan since 2026-06-22.
- `clyde/src/bootstrap.rs:1131`: the generated `clyde-enrich.service` ships `EnvironmentFile=%h/.config/clyde/enrich.env`.
- `clyde/src/bootstrap.rs:1101-1116`: `move_env_file` actively migrates the klod-era key file into clyde's config dir, preserving permissions.
- Live on desk.lan: `clyde-enrich.service` + `clyde-enrich.timer` consuming it.

clyde has custody of a credential file and installs a unit that reads it. Excision has to reach the deployed host, not just the source tree.

**And the `.env` shape was never legitimate.** `taste.md` states the rule flatly: "env vars never .env". So `enrich.env` is two violations stacked, independent of the key: a `.env` file at all, and a credential living under `~/.config`. Even if clyde kept an API key, this would be the wrong way to deliver it. Phase 5 is therefore not just "stop loading a credential," it deletes a pattern that should never have shipped. There is no replacement config file: the whole point is that the transport needs no value delivered to it.

**4. Docs asserting the key is required.** `README.md:145,171`, `report/README.md:101,148`, `efficiency/src/cli.rs:47` help text.

### Cleared, not touched

Checked and deliberately left alone, so the audit does not re-flag them:

- `session/src/redact.rs:41` + `session/src/redact/tests.rs:34`: the scrubber's pattern for `api_key:` appearing in session *content*. Keep.
- `permit/src/risk/tier.rs:741`: test asserting `Bash(API_KEY="abc" curl:*)` classifies `Dangerous`. Keep.
- `pricing/*`: `DEFAULT_FEED_URL` is `tatari-tv.github.io/clyde/pricing.json`, unauthenticated. Clean.
- `docs/design/*`, `docs/shakedown-*.md`: point-in-time records. They stay as history and are not rewritten.

The only two `x-api-key` POSTs in the tree are `sessions/src/llm.rs:201` and `report/src/summarize/api.rs:88`. Nothing else in the workspace carries a credential.

**Inventory verified by enumeration, not keyword search (2026-07-29).** A keyword grep is not sufficient here, because `sessions/src/llm.rs:91` reads the key through the `API_KEY_ENV` const (`llm.rs:29`) and never names the string at the read site. So the surface was confirmed by enumerating every env-var name read anywhere in the workspace:

```
rg -o -N 'env::var(_os)?\("([A-Z_]+)"\)' -r '$2' --glob '*.rs' --glob '!target/**' | sort -u
```

That returns only `GIT_DESCRIBE`, the four `XDG_*` vars, `RUST_LOG`, `PATH`, and `ANTHROPIC_API_KEY` (at `report/src/summarize/api.rs:29` and `api/tests.rs`), plus the const-indirected read in `sessions`. Three further checks came back empty: no `dotenv`/`envy`/`envmnt`/`figment` dependency in any `Cargo.toml`; no Rust code reads `enrich.env`'s **contents** (`bootstrap.rs` only moves the file, never opens it); no clap flag or config field carries a key, token, or secret. So there is no credential route other than the two named above.

### Goals

- G1 (Scott, 2026-07-29): no code path in the workspace reads `ANTHROPIC_API_KEY`.
- G2 (Scott, 2026-07-29): `clyde session enrich` and `clyde efficiency session --narrate` work on a keyless host with a logged-in `claude`.
- G3 (Scott, 2026-07-29): clyde installs no unit that loads a credential file, and stops migrating one.
- G4 (derived from G1): one transport for every LLM call in the workspace. Not two implementations of keyless.
- G5 (derived from R2 below, found in review pass 4): a systemic transport failure must consume at most a **bounded, small** amount of the enrich sweep's durable retry budget, never the whole candidate set. An API key never expired; a Claude Code login does, and the sweep runs unattended on a timer. Stated as bounded rather than zero because the circuit breaker deliberately charges its N observed failures to guarantee forward progress (see Sweep-fatal vs per-session failure).
- G6 (Scott, 2026-07-29): clyde ships no credential file for the enrich sweep. `taste.md` says "env vars never .env". clyde stops generating one, stops migrating one, and stops reading one, and it does not gain a replacement config key to carry a credential value. Nothing needs delivering, because the transport is keyless.

  **Scoped 2026-07-30 (Scott).** An earlier wording read "no `.env` file, anywhere, for any purpose," which overreached. `EnvironmentFile=` is the standard way to deliver a secret to a local `systemd --user` daemon, and this design must not be read as forbidding it for some future clyde daemon that has a real secret to receive. G6 is about *this* unit, which has no secret. If a later daemon needs one, `EnvironmentFile` from a `0600` file or systemd's `LoadCredential=` remains the right mechanism and nothing here precludes either. The doc's Security section cites only the K8s path (external-secrets to env vars) and is silent on local user daemons; that gap is inert for this design and is called out so a future reader does not mistake silence for prohibition.

### Non-Goals

- **Excluded: changing what enrich or narrate produce.** Same prompts, same model pin, same `LlmEnrichment` schema, same prose contract. Transport swap only.
- **Excluded: clyde deleting `~/.config/clyde/enrich.env` itself.** The file must go (G6), but clyde does not remove it: it holds a live credential, and destroying an operator's secret is not a bootstrap's job, per `secrets.md` (custody is the operator's channel). What clyde owes instead is not leaving it to rot silently, so Phase 5 makes bootstrap **detect and warn** when the stale file is present, naming its path. Deletion is a required Rollout step, run by Scott with `rkvr rmrf` per `safety.md`, not an eventually.
- **Excluded: rewriting historical design docs and shakedowns** that mention the key. They record what was true.
- **Parked, revisit condition named: batching multiple sessions per `claude` invocation.** Only becomes necessary if Phase 0 shows per-session invocation cost is untenable. Revisit condition: Phase 0 measures a per-session cost that makes a full-catalog backfill cost more than the artifact is worth. See Phase 0 and Risk R1.

## Proposed Solution

### Overview

Move the keyless transport down the dependency graph so every crate can reach it, point the two remaining LLM call sites at it, then delete the api path and the key file plumbing.

Order matters: the transport has to move before anything can depend on it, and the api path cannot be deleted until the callers that would otherwise regress are already keyless.

### Architecture

`report` depends on `sessions` and `efficiency` (`report/Cargo.toml`). So `CliTransport` cannot stay in `report` and be reachable from `sessions`. It moves to `common`, which both already depend on.

Before:

```
common
  ^        ^
sessions  efficiency          report ── summarize/{Transport, CliTransport, ApiTransport}
  │  AnthropicClient (x-api-key)  ^
  └──────── Completer/Narrator ───┘
```

After:

```
common ── llm/{Transport, Job, CliTransport}   (the ONE keyless transport)
  ^                 ^                  ^
sessions            efficiency         report
  CliCompleter      (uses sessions'    summarize/ ── Job::{Markdown, Slots, Eval}
  CliNarrator        Narrator port)     (ApiTransport deleted)
```

What moves, verbatim where possible:

| From | To | Note |
|---|---|---|
| `report/src/summarize.rs` (`Transport`, `Kind`, `Job`, `check_stop_reason`) | `common/src/llm.rs` | |
| `report/src/summarize/cli.rs` (582 lines) + `cli/` | `common/src/llm/cli.rs` | |
| `report/src/proc.rs` (**both** `run_bounded` and `run_with_payload`) | `common/src/proc.rs` | `report/src/lib.rs:16` declares `pub mod proc`, so this changes report's public surface. `report/src/render.rs:7` imports `run_bounded` and becomes a `common::proc` import |

`common/Cargo.toml` gains `which`, `serde_json`, `eyre`, `log`. It already has `wait-timeout`.

`report/src/summarize.rs` becomes a thin re-export of `common::llm` plus report's own prompt assembly, so `render.rs` call sites do not churn.

`check_stop_reason`'s error text says "Anthropic API stopped with stop_reason=" (`summarize.rs:81`). Once the api transport is gone that names a path that no longer exists, so Phase 4 rewords it to name `claude -p`.

The transport's stated policies come along unchanged, because they are the reason it exists: `env_clear()` plus an explicit allowlist (`PATH`, `HOME`, `NO_UPDATE_NOTIFIER=1`), fail loud, never retry, never fall back.

### Data Model

Two changes, both in `common/src/llm/cli.rs` after the move.

**1. `Usage` must decode input tokens.** Today it decodes only `output_tokens` (`report/src/summarize/cli.rs:564-567`), because render only ever needed the output ceiling check. Enrich persists both: `sessions/src/db.rs:131-132` (`tokens_in`, `tokens_out`), fed from `LlmEnrichment` (`llm.rs:57-58`).

```rust
struct Usage {
    #[serde(default)] input_tokens: Option<u64>,
    #[serde(default)] output_tokens: Option<u64>,
    #[serde(default)] cache_creation_input_tokens: Option<u64>,
    #[serde(default)] cache_read_input_tokens: Option<u64>,
}
```

`tokens_in` sums the three input buckets. Per #60's measurement (`:510`), the CLI bills the payload as a 1-hour cache **write**, so a naive read of `input_tokens` alone records near-zero and `--budget-tokens` becomes a no-op that never trips. That is a fail-quietly bug in a budget gate, so:

**Absent usage is a hard error, not a zero.** If the envelope carries no `usage`, the call fails loudly rather than recording `tokens_in: 0`. A silently-zero budget accountant is worse than a failed sweep (`taste.md`: fail loudly, fail closed).

Phase 0 captures the exact field set and settles what the three buckets actually contain.

**No schema migration.** `tokens_in`/`tokens_out` columns and types are unchanged. Only the source of the numbers changes, and their billing meaning (see Performance).

### Sweep-fatal vs per-session failure (G5)

The enrich sweep is sequential (`enrich.rs:90`, `for rec in &records`) and charges every `Err` from the port to that session's durable `attempts` counter (`db.rs:568`, `attempts=attempts+1`). The candidate predicate then excludes any row where `attempts >= max_attempts` (`db.rs:593`, default 5).

That was safe against an API key, which does not expire. It is **not** safe against a Claude Code login, which does. A logged-out `claude` fails every call in the sweep, so one unattended run charges an attempt to all 94 (or 1,300) sessions, and five daily timer runs with an expired login permanently drop the entire catalog out of the candidate set. The failure is silent, it looks like "enrich has nothing to do," and it does not self-heal after the user logs back in.

So failures split by layer:

**Auth fails in TWO shapes, and only one of them is a non-zero exit.** This is the finding that G5 turns on, and an earlier draft of this section got it wrong by collapsing them. The transport's own guards distinguish them explicitly:

- **Guard 1** (`cli.rs:149-155`): *"A **logged-out** `claude` exits non-zero and prints to stderr WITHOUT emitting a JSON envelope."* Non-zero exit, no envelope.
- **Guard 2** (`cli.rs:174-179`): *"An **expired token** produces a perfectly well-formed envelope saying exactly what is wrong."* Exit 0, `is_error: true`, and today that becomes an ordinary `bail!`. Pinned by `cli/tests.rs:558-566`, whose fixture is literally `{"is_error":true,...,"error":{"message":"OAuth token has expired. Please run /login."}}`.

Classifying on exit status alone therefore catches the never-logged-in case and **misses token expiry**, which is the exact scenario G5 exists for: G5's rationale is "an API key never expired; a Claude Code login does." G2's premise is a user who *was* logged in, so expiry is the dominant unattended-timer failure, not never-logged-in. Classification must read envelope **content**, not exit shape.

| Failure | Detection | Class | Behavior |
|---|---|---|---|
| `claude` not on PATH | `which` fails | sweep-fatal | `ClaudeCli::resolve()` fails before the loop. Already correct |
| logged out | non-zero exit, no envelope (Guard 1) | **sweep-fatal** | abort, charge NO attempts, exit non-zero naming the login remedy |
| **token expired / invalid login** | **exit 0, `is_error: true`, auth-shaped detail (Guard 2)** | **sweep-fatal** | **same. This is the row the earlier draft omitted** |
| transport/API failure | `terminal_reason: "api_error"` (`cli.rs:481`) | **sweep-fatal** | same. No per-session retry can fix a dead transport |
| version below the floor | unsupported-flag exit | **sweep-fatal** | same |
| malformed envelope, bad schema, empty result, model mismatch | envelope parse/validate | per-session | charge one attempt, continue, as today |
| ceiling exceeded / non-`end_turn` stop | `stop_reason` | per-session | charge one attempt, continue, as today |

**There IS a structured discriminator, and clyde does not currently read it.** Measured 2026-07-29 under the exact final argv (Phase 0, probe C). A bogus-key auth failure returns:

```json
{ "is_error": true, "subtype": "success", "terminal_reason": "api_error",
  "api_error_status": 401, "result": "Invalid API key · Fix external API key",
  "total_cost_usd": 0, "duration_ms": 280 }
```

`api_error_status` is an HTTP status code. That is the typed auth signal: no prose, no string matching.

**The argv determines whether the field appears, which is why an earlier draft concluded it does not exist.** Under bare flags the same failure returns `subtype: "error_during_execution"` with `api_error_status` absent. The CLI propagates that field only when `subtype == "success"`, and under the final argv this failure sets `subtype: "success"` **and** `is_error: true` at once, so the field survives. Any probe of this envelope must use the argv `CliTransport` actually builds or it measures a different code path.

**Resolved: deserialize `api_error_status` and classify on it.** `Envelope` (`cli.rs:542-562`) gains `api_error_status: Option<u16>`. Classification, all structured:

| signal | class | rationale |
|---|---|---|
| `api_error_status` 401 / 403 | **sweep-fatal** | auth. No per-session retry fixes a bad credential |
| `api_error_status` 429 | **sweep-fatal** | rate limit. Not auth, equally not retry-fixable this run |
| `api_error_status` 5xx | **sweep-fatal** | upstream down |
| `terminal_reason == "api_error"` with no status | **sweep-fatal** | belt-and-braces for a status-less API failure. Independently measured in clyde's own dated fixture, `cli/tests.rs:578` (2026-07-26) |
| non-zero exit, no envelope (Guard 1) | **sweep-fatal** | logged out. Already correct |
| binary missing / version floor | **sweep-fatal** | `ClaudeCli::resolve()`, before the loop |
| `api_error_status` other 4xx | per-session | a malformed request is about this payload |
| malformed envelope, bad schema, empty result, model mismatch | per-session | as today |
| ceiling exceeded / non-`end_turn` stop | per-session | as today |

Two structural repairs to `Envelope` come with it, both found by review:

- It reads `error` **singular** (`cli.rs:556`) and would silently drop an `errors` **array**, which some failure shapes emit. Deserialize both, preferring whichever is present.
- The `TransportError::Unavailable` variant is named for the broad meaning (*the transport cannot serve requests right now*), not for auth specifically, because 429 and 5xx classify the same way and are not authentication problems.

**Treat `cli/tests.rs:558-566` as unverified.** That expired-token fixture is hand-authored with no provenance comment, unlike its neighbour at `:578` which is explicitly dated and measured. It may not match what 2.1.220 emits for a real OAuth expiry. Phase 3 re-captures it rather than building on it. Honest limit: a bogus API key is not literally an expired OAuth token, and proving the latter would require logging Scott out, so the 401 path is measured and the OAuth-expiry path is inferred from it.

**If a case escapes the table, over-classify.** A false sweep-fatal aborts one run; a false per-session charge burns durable budget and retires rows permanently. One costs a re-run, the other silently costs the catalog.

### The circuit breaker (the part that does not depend on classification)

Classification handles the failures we can name. A **consecutive-failure circuit breaker** handles the rest, and it earns its place on an argument that has nothing to do with error taxonomy.

**Measured: a broken transport does not fail fast.** With the base URL pointed at a dead port, a single invocation ran past **150 seconds** without returning (two independent runs). At 286 candidates that is a timer sweep grinding for **~12 hours** while failing every row. Classification fixes the accounting and does nothing about the wall clock. The breaker fixes both, and it covers failure modes nobody has enumerated, including ones that do not exist yet.

**Rule: if the first 3 sessions of a sweep fail, abort the sweep.** One bad session cannot trip it. Anything systemic trips it immediately.

**Charge `attempts` to the 3 observed failures only, not to the whole candidate set.** This is the detail that makes the breaker safe. Abort-with-no-charge livelocks: the candidate order is deterministic (`ORDER BY s.modified DESC`, `db.rs:606`), so three genuinely-bad sessions at the head would trip the breaker every run forever and nothing would ever enrich. Consistently-failing clusters demonstrably exist, which is what the 46 retired rows are. Charging only the observed failures bounds damage at 3 rows per run instead of 286, guarantees forward progress (a genuinely-bad row exhausts in 5 runs and drops out), and still protects the other 283.

Failure envelopes report `total_cost_usd: 0` (measured, both failure classes), so a tripped sweep is free.

**Rejected alternative: probe a tail candidate before aborting.** On trip, run one candidate from the far end of the order; if it also fails the failure is systemic and nothing is charged, if it succeeds the head rows are genuinely bad and those N are charged. This preserves the stricter "charge nothing on systemic failure" invariant. Not taken: it costs an extra invocation on every tripped run, which at 179s for a dead transport is 179 seconds of additional grind precisely when the system is already broken, and it adds a second failure mode (the tail row is also genuinely bad). Charging N is simpler, bounds damage to 3 rows, and the recovery path for those rows already exists (`--max-attempts`).

**How the sweep matches it, given the workspace uses `eyre`.** `Completer::enrich` returns `eyre::Result`, and eyre carries no variants. So `common::llm` defines a minimal `thiserror` type and the transport attaches it as the error's source:

```rust
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// `claude` is present but cannot serve a request at all: logged out, or below the version
    /// floor. Sweep-fatal. Charging a retry attempt for this would burn durable budget on a
    /// condition no retry can fix.
    #[error("the `claude` CLI is unavailable: {0}")]
    Unavailable(String),
}
```

`sessions::enrich` matches with `err.downcast_ref::<TransportError>()`. That is the standard eyre pattern, keeps `Completer`'s signature unchanged, and satisfies `rust.md`'s typed-variant rule without converting the workspace off eyre. `common/Cargo.toml` gains `thiserror` (a lib crate, which is where `rust.md` puts it).


### The 46 already-retired rows

Verified on desk.lan, 2026-07-29, `~/.local/share/clyde/sessions.db`:

```
attempts | rows
       0 | 2023
       1 |    1
       5 |   46
```

All 46 are `enrich_status = failed`. The candidate predicate is `s.attempts < ?1` (`db.rs:593`) against a default cap of 5, so **those rows are outside every future sweep under the default cap**, and G5 does not retroactively free them. (Recoverable by raising the cap, see below. "Permanently" would be wrong: the cap is a bound, not a tombstone.) Fixing the classifier stops the bleeding; it does not heal what already bled.

**Resolved: no new code. The recovery path already exists.**

`--max-attempts` is already a CLI flag (`clyde/src/cli.rs:286-287`, `#[arg(long, default_value_t = DEFAULT_MAX_ATTEMPTS)]`), and the predicate is `s.attempts < ?1` bound to it. So `clyde session enrich --max-attempts 6` makes `attempts < 6` true for every row sitting at 5 and sweeps them immediately. Verified: the `attempts < ?1` clause sits OUTSIDE the `if !all` block (`db.rs:593-596`), so `--all` does NOT free them, but raising the cap does.

An earlier draft of this section proposed adding a `--reset-attempts` flag. **Cut as redundant scope**: nobody asked for it, and a flag that duplicates an existing tunable is exactly the unrequested scope the house rules forbid. `taste.md` also prefers the simple direct mechanism, and a tunable that already exists beats a new one.

Two properties make the existing flag sufficient rather than merely usable:

- `set_enrichment` already resets `attempts=0` on success (`db.rs:521`), so one successful pass under a raised cap returns each row to a clean state permanently. The raise is a one-time recovery invocation, not a permanent config change.
- Ordering matters, and it falls out correctly: run the raised-cap sweep only AFTER G5 ships. Before the fix, a still-broken transport would charge all 46 again and push them to 6. After it, transport failures charge nothing, so the sweep either enriches a row or charges it for a genuine per-session reason, which is the correct accounting.

This is a Rollout step, not a phase. It is recorded here because "the excision shipped and 46 sessions are still silently uncatalogued" is the same class of quiet failure G5 exists to prevent, and nothing in the code will remind anyone.

Narrate needs none of this: it is a single interactive call with no durable retry state, so it just fails.

### API Design

`sessions/src/llm.rs` keeps both ports exactly as they are. `Completer` and `Narrator` are the seam that makes this a wiring change:

```rust
pub trait Completer { fn enrich(&self, payload: &str) -> Result<LlmEnrichment>; }
pub trait Narrator  { fn narrate(&self, system: &str, user: &str) -> Result<String>; }
```

`AnthropicClient` is deleted. One replacement implements both over the moved transport:

```rust
/// The keyless enrichment/narration client: both ports over `common::llm::CliTransport`.
pub struct ClaudeCli { transport: common::llm::CliTransport }

impl ClaudeCli {
    /// Resolve `claude` off PATH and log its version. Errors name the install-and-login remedy.
    pub fn resolve() -> Result<Self>;
}

impl Completer for ClaudeCli { /* Kind::Enrich,  ENRICH_MODEL,  MAX_OUTPUT_TOKENS */ }
impl Narrator  for ClaudeCli { /* Kind::Narrate, NARRATE_MODEL, NARRATE_MAX_OUTPUT_TOKENS */ }
```

The transport's job type is a **struct** `Job { kind: Kind, model: &str, max_output_tokens: u32 }` with `Kind` the enum (`Slot`, `Judge` today, after render inversion deleted `Kind::Markdown`). So this adds two `Kind` variants, `Enrich` and `Narrate`, and constructs `Job` with the pins from `sessions/src/llm.rs` (`ENRICH_MODEL` / `NARRATE_MODEL`, both `claude-haiku-4-5-20251001`; both ceilings 512).

**One decision this forces.** `Kind::max_output_tokens_key()` (`summarize.rs:46-51`) returns the `clyde.yml` key that governs each kind's ceiling, so a ceiling failure can name the one line that fixes it. Enrich and narrate ceilings are compile-time consts, not config. Options were: invent `enrich-max-output-tokens` / `narrate-max-output-tokens` config keys, or make the method total over a non-configurable kind.

**Resolved: the method returns `Option<&'static str>`, `None` for the two new kinds.** Adding config keys nobody asked for is unrequested scope, and returning a key that does not govern the failing job is exactly the "remedy that cannot remedy" the method's own doc comment forbids. A `None` arm makes the ceiling failure say the ceiling is fixed in code, which is true. If either ever needs tuning, it becomes a config key then.

**How the payload reaches the model.** `Transport::complete(job, system, prompt, json_body)` puts `system` and `prompt` on argv and streams `json_body` on stdin, and `complete` currently hardcodes the fence as ```` ```json ```` (`cli.rs:129`) because both existing kinds send JSON facts.

Enrich sends a session's redacted **prose**, not JSON. Labeling prose as `json` misdescribes the payload to the model, and routing it through `prompt` on argv is not an option: enrich payloads are large and that is the exact `ARG_MAX` hazard `summarize.rs:13-16` documents.

**Resolved: the fence label becomes a `Kind` property.** `Kind::fence()` returns `"json"` for `Slot`/`Judge` and `"text"` for `Enrich`/`Narrate`. One method, one line per arm, and the mapping is:

| port call | system | prompt | json_body (stdin) |
|---|---|---|---|
| `Completer::enrich(payload)` | `SYSTEM_PROMPT` | empty | the redacted session text |
| `Narrator::narrate(system, user)` | `system` | empty | `user` |

**The reply shape needs no work.** `parse_enrich_json` (`llm.rs:264-276`) already tolerates fences and surrounding prose by falling back to the outermost `{...}` span, which is exactly the shape a CLI reply can arrive in. That tolerance was written for the api path and carries over unchanged.

Call sites, both one-liners:

- `clyde/src/main.rs:840`: `AnthropicClient::from_env()?` -> `ClaudeCli::resolve()?`. The `sessions::enrich::<AnthropicClient>(db, None, &opts)` dry-run turbofish becomes `::<ClaudeCli>`.
- `efficiency/src/lib.rs:122`: same swap, same `.context(...)`.

Deleted: `report/src/summarize/api.rs` (whole module), `api_key_from_env`, the `ApiTransport` re-export, `report/src/cli.rs`'s `Llm::Api`, and the `key_present` parameter on `config::resolve_transport`.

**The config enum is in `common`, not `report`.** `common/src/config.rs:59` owns `LlmConfig { Auto, Api, Cli }`, with the `render.llm` field at `:171-173`, its default at `:198`, and the `render_llm()` accessor at `:485`. `report/src/cli.rs:40` maps `LlmConfig -> Llm`. Removing the transport selection touches both crates, and `common` is the one that owns the serde view.

`resolve_transport` collapses from a four-arm matrix to a presence check:

```rust
pub fn resolve_transport(claude_present: bool, format: Format) -> Result<TransportKind> {
    if claude_present { return Ok(TransportKind::Cli); }
    bail!("no LLM transport available for --format {}: install the `claude` CLI and log in once",
          format_name(format));
}
```

`--llm` loses its reason to exist once there is one transport. It is **removed**, not kept as a one-value flag. `llm:` in `clyde.yml` is removed with it, and an unknown key there is already a loud parse error, so a stale config fails visibly instead of being ignored.

### Implementation Plan

#### Phase 0: Measure keyless enrich cost. Zero code.

**Model:** opus

The unproven assumption: that per-session `claude -p` invocation is affordable at catalog scale. #60 measured a non-suppressible internal haiku sub-call at ~$0.19 per invocation on a 513KB payload, plus payload billed as a 1h cache write at 2x input rate. Enrich invokes **once per session**. Keegan has 94 dormant sessions; Scott's catalog is 1,300+. If the sub-call tax is fixed rather than proportional to payload, a backfill is unaffordable and the design needs batching (the parked Non-Goal).

- Take one real redacted enrich payload via `clyde session enrich --dry-run --show-payload`.
- Invoke `claude -p` by hand with #60's FINAL argv, `--model claude-haiku-4-5-20251001`, `--output-format json`, keyless (`env -u ANTHROPIC_API_KEY`).
- Capture the full `usage` and `modelUsage` objects verbatim into the implementation notes.
- Repeat on the smallest and largest payloads in the catalog to separate fixed cost from proportional cost.
- Price both against `pricing/data/pricing.json` and against today's api-path cost for the same payload.

**It collapses to two invocations.** The fixed-vs-proportional question is answered by measuring at two payload sizes, so this is cents, not a study: one at the catalog's p50 (**15 KB**) and one at the `SEND_CAP_CHARS` ceiling (**500 KB**, `sessions/src/enrich.rs:25`).

**Catalog shape, measured read-only on desk.lan 2026-07-29** (`~/.local/share/clyde/sessions.db`): 2,061 non-empty bodies, 73.35 MB total, mean 35.6 KB, **p50 15 KB**, max 520 KB, 51 over 200 KB.

**Default-filter candidates: 286.** An earlier draft said 1,003, which was wrong: it came from guessing `prompt_version < 3` instead of reading `ENRICH_PROMPT_VERSION = 1` (`sessions/src/llm.rs:19`, verified). At the real value the 722 already-enriched rows stop re-qualifying. The 1,003 was a coincidence, it is exactly the `skipped-personal` count that the predicate excludes. Re-measured against the real predicate (`prompt_version=1`, `max_attempts=5`): **286** candidates, 284 with non-empty bodies, 11.37 MB, mean 40 KB, max 500 KB. `--all` scope is 1,960, still `attempts`-capped.

Projected 286-row backfill, at 2.118 B/tok (predecessor `:510`) on haiku-4-5:

| path | backfill (286 rows) | per 100 new sessions (recurring) |
|---|---|---|
| api (today) | ~$6 | ~$2 |
| cli, sub-call **proportional** to payload | ~$16 | ~$5.5 |
| cli, sub-call **fixed** per invocation | ~$66 | ~$23 |

The GO/NO-GO thresholds below are unaffected by the correction: per-100 depends on mean payload size (40 KB for candidates), not on candidate count.

**The recurring number decides, not the backfill.** Backfill is one-time; steady state is forever, and the doc previously gated on the wrong figure.

**Success criteria (falsifiable):**
- The verbatim `usage` object is recorded for both probes, and it is settled which buckets carry the payload's input tokens.
- The two measurements determine fixed vs proportional: assert whether sub-call input tokens scale with payload size across a 33x payload delta.
- Session-JSONL and lock-file counts under `~/.claude/projects` are unchanged across the probe invocations, re-proving `--no-session-persistence` at enrich's call volume (R5).
- **Numeric GO/NO-GO, decided by Scott on the measured recurring cost:** GO if projected steady-state cost per 100 newly-dormant sessions is **at or under ~$5** (the proportional case, ~2.5x the api path). NO-GO if it lands near **~$22** (the fixed case, ~11x), which triggers the batching alternative. Between the two, Scott rules with the number in hand.

#### Phase 1: Move the transport to `common`. No behavior change.

**Model:** sonnet

- Move `Transport`/`Kind`/`Job`/`check_stop_reason` -> `common/src/llm.rs`, `summarize/cli.rs` + `cli/` -> `common/src/llm/cli.rs`, `proc.rs` (both fns) -> `common/src/proc.rs`.
- Add `which`, `serde_json`, `thiserror` to `common/Cargo.toml`. Verified already present: `eyre`, `log`, `wait-timeout`, `dirs`, `tempfile`.
- `report/src/proc.rs`'s `run_bounded` and its consts (`CLAUDE_TIMEOUT:28`, `SUBPROCESS_TIMEOUT:36`) are `pub(crate)`; crossing the crate boundary widens them to `pub`. That is a real public-API change to `common`, so "no behavior change" means runtime behavior, not visibility.
- Note for the implementer: `common/src/tools.rs:63-89` already spawns a `Command`, drains stdout on a thread, and calls `child.wait_timeout(PROBE_TIMEOUT)`. Same bounded-subprocess pattern, and the fact that settles the destination question: `common` already does this kind of work. No `common/src/proc.rs` exists, so there is no collision, but the move lands a near-duplicate beside it. Consolidating the two is explicitly OUT of scope here (unrequested), just do not be surprised by it.
- `report/src/summarize.rs` re-exports `common::llm::{Transport, Kind, Job, CliTransport}`; report keeps its own prompt assembly.
- Repoint `report/src/render.rs:7` (`crate::proc::run_bounded` -> `common::proc::run_bounded`) and drop `pub mod proc` from `report/src/lib.rs:16`.
- Move the transport's tests with it. They are the proof the move was verbatim.

**Success criteria:**
- `otto ci` green.
- `git diff --stat` shows the moved files as moves: no logic change to `CliTransport` in this phase.
- `env -u ANTHROPIC_API_KEY clyde report render -i <real>.json --format markdown -o -` still exits 0 and emits a model-authored report (byte-comparable to a pre-move render of the same input, modulo model nondeterminism: assert >5,000 bytes, >=3 `^## ` headers, no `Generated offline via`).

#### Phase 2: Teach the transport input tokens.

**Model:** sonnet

- Extend `Usage` per Data Model, using Phase 0's verbatim field set.
- Add a `tokens_in()` helper summing the input buckets, and `tokens_out()`.
- Absent `usage` on a success envelope is a hard error naming the job.
- Unit tests over fixture envelopes: input-only, cache-write-only, both, and absent.

**Success criteria:**
- A fixture envelope shaped exactly like Phase 0's real capture yields the token counts Phase 0 recorded.
- A success envelope with `usage` absent returns `Err`, and the message names the job.

#### Phase 3: Rewire enrich and narrate. The keyless flip.

**Model:** opus

- Add `Kind::Enrich` and `Kind::Narrate`; make `max_output_tokens_key` return `Option<&'static str>` with `None` on both.
- Add `ClaudeCli` in `sessions/src/llm.rs` implementing `Completer` + `Narrator`.
- Delete `AnthropicClient`, `API_KEY_ENV`, `API_URL`, `ANTHROPIC_VERSION`, `MAX_HTTP_RETRIES`, `RETRY_BACKOFF_MS`, `error_snippet`, and `reqwest` from `sessions/Cargo.toml`. Verified droppable: `reqwest` appears in `sessions/src` only at `llm.rs:82,96,238`, all inside `AnthropicClient`.
- Swap the two call sites (`clyde/src/main.rs:840`, `efficiency/src/lib.rs:122`).
- Reword `efficiency/src/cli.rs:47` help text: needs a logged-in `claude`, not a key.
- **Implement G5, structured classification:** add `api_error_status: Option<u16>` to `Envelope`, plus `errors` (array) alongside the existing singular `error`. Add `TransportError::Unavailable` and return it for every sweep-fatal row of the classification table. Make `enrich`'s match arm abort the sweep on it without calling `record_enrich_failure`.
- **Implement the circuit breaker:** 3 consecutive failures from the start of a sweep aborts it, charging `attempts` to those 3 observed failures ONLY. This is what covers unenumerated failures and the ~12-hour grind a dead transport otherwise causes.
- **Re-capture the expired-token fixture.** `cli/tests.rs:558-566` is hand-authored with no provenance; replace it with a measured envelope or delete it. Do not build the classifier on it.
- **Set `MAX_THINKING_TOKENS=0` in the child env for `Kind::Enrich` ONLY (Phase 0 Findings 12 and 13).** Measured on enrich: 67% cheaper on a p50 payload, ~9x faster (6s vs 52s), output collapses from 5,798 to 140 tokens, tags and summary equal or better.
  **Not `Kind::Narrate`.** Measured separately, 3 runs per mode on identical facts: reasoning ON returns "inefficient" 3/3, reasoning OFF returns "efficient" 3/3. The flag deterministically flips the verdict, which is precisely what the Non-Goal excludes. Narrate is one interactive invocation, so it had no cost case to justify the risk.
  **Not `Kind::Slot` or `Kind::Judge`** either: `child_env()` is shared, and setting this unconditionally would silently change what `report render` and `report eval` produce. Unrequested and unmeasured.
  One conditional line keyed on `Kind`, same shape as the existing `NO_UPDATE_NOTIFIER=1` constant, so the allowlist posture is unchanged.
- **Resolve the output-ceiling blocker (Phase 0 Finding 3):** drop the output-token ceiling check for `Kind::Enrich` and `Kind::Narrate` and gate on `stop_reason == "end_turn"` alone. Measured output was 5,798 and 678 tokens against a 512 ceiling, so the check as written rejects 100% of enrich calls; and the value does not track payload size, so no low ceiling is safe. `stop_reason` is the direct truncation signal the ceiling was only ever a proxy for.
- Existing `Fake`/`FakeNarrator` test doubles are untouched. That they still compile and pass is the proof the ports did not move.

**Success criteria:**
- `env -u ANTHROPIC_API_KEY clyde session enrich --only <one-session-id>` exits 0 and the row gains non-empty `tags` and `summary`, with `tokens_in > 0` and `tokens_out > 0`.
- `env -u ANTHROPIC_API_KEY clyde efficiency session <id> --narrate` exits 0 and prints a prose verdict.
- `rg 'ANTHROPIC_API_KEY|x-api-key' sessions/ efficiency/ clyde/src --glob '*.rs'` returns nothing.
- On a host with no `claude` on PATH, both commands fail loudly naming the install-and-login remedy.
- **G5 bites at the sweep layer:** a `Fake` completer returning `TransportError::Unavailable` on the first record leaves `attempts` unchanged on **every** candidate row and returns `Err`; the same fake returning an ordinary error charges exactly one attempt to that row and continues to the next. Both asserted against a seeded multi-row db.
- **G5 bites at the TRANSPORT layer** (the criterion above cannot catch this, and it is where the bug would actually live). A `Fake` that *returns* the variant proves only that the sweep honors it, not that the transport ever produces it. So: feed `CliTransport`'s envelope checker the **measured** 401 envelope from Phase 0 (`{"is_error":true,"subtype":"success","terminal_reason":"api_error","api_error_status":401,...}`) and assert the error downcasts to `TransportError::Unavailable`. Same for the dated `terminal_reason: "api_error"` envelope at `cli/tests.rs:578`. A malformed-envelope, a 400, and a `stop_reason: max_tokens` fixture must NOT downcast to it.
- **The breaker bites:** a `Fake` failing the first 3 records aborts the sweep, and `sum(attempts)` rises by exactly 3, not by the candidate count. A `Fake` failing only record 2 does not abort.
- **The ceiling no longer rejects valid work:** an envelope with `output_tokens: 5798` and `stop_reason: "end_turn"` succeeds for `Kind::Enrich`; one with `stop_reason: "max_tokens"` still fails.
- **Reasoning is suppressed for exactly one kind:** the built child `Command` carries `MAX_THINKING_TOKENS=0` for `Kind::Enrich`, and does NOT carry it for `Kind::Narrate`, `Kind::Slot`, or `Kind::Judge`. Asserted over the built command, enumerated by kind, so a future refactor cannot silently extend it. The Narrate exclusion is load-bearing: the flag flips its verdict (Finding 13).
- **Recovery path needs no new code:** against a seeded db with rows at `attempts=5`, `enrich_candidates(.., max_attempts=6, ..)` returns them and `max_attempts=5` does not. Asserts the existing `--max-attempts` flag is sufficient, so no reset flag is added.

#### Phase 4: Delete the api transport.

**Model:** sonnet

- Delete `report/src/summarize/api.rs` **and `report/src/summarize/api/tests.rs`**, its re-export, `api_key_from_env`, and `ApiTransport`. Deleting that test dir also closes a documented test-isolation hazard: `api`'s tests mutate the process-wide `ANTHROPIC_API_KEY` while another test reads the entire parent environment (`docs/design/2026-07-24-report-render-claude-cli-transport-implementation-notes.md:329`). Removing the only env-mutating tests in the crate makes that class of interference impossible rather than merely serialized.
- **Convert `report eval` in the same phase.** `eval.rs:43` drops `ApiTransport` from its import, `eval.rs:448`'s `TransportKind::Api` arm goes with the enum variant, and `eval/tests.rs:409` switches to `CliTransport::resolve()` (dropping its `ANTHROPIC_API_KEY must be set` expect message). Remove eval's own `--llm` surface at `report/src/cli.rs:265` and the config fallback at `report/src/config.rs:244`, and update `report/README.md:212`. Non-negotiable ordering: this lands in the same commit as the `ApiTransport` deletion, because `report` does not compile between the two.
- **Fix the neither-door error and its test.** `report/src/config.rs:131` reads `"...set ANTHROPIC_API_KEY (--llm api), or install the `claude` CLI..."`, and `report/src/config/tests.rs:404` asserts `err.contains("ANTHROPIC_API_KEY")` with the message `"must name the key"`. That is a **hard test failure**, not a stale string: the test must be rewritten to assert the single remaining remedy, not deleted.
- **Drop `ureq` from `report/Cargo.toml:38`.** Verified: after `summarize/api.rs` is deleted there are **zero** `ureq` uses left anywhere in `report/src/`. Leaving the declaration is precisely the "we shipped *the key is gone* and the HTTP client is still wired in" outcome, one `use` away from a regression. (`pricing` keeps its `ureq`: it fetches the public GitHub Pages feed, no credential.)
- Remove `report/src/cli.rs`'s `Llm` enum and the `--llm` flag entirely, plus the `From<LlmConfig> for Llm` impl at `:40`.
- Remove `LlmConfig` from `common/src/config.rs:59`, the `render.llm` field (`:171-173`), its default (`:198`), and `render_llm()` (`:485`).
- Collapse `resolve_transport` per API Design; drop the `key_present` parameter and rewrite its precedence tests.
- Rewrite the neither-door error to name the one remaining remedy, and reword `check_stop_reason`'s "Anthropic API stopped with" to name `claude -p`.
- **Scrub the transport's own escape hatch.** `ESCAPE_HATCH` (`cli.rs:249-250`) advises `--llm api`, and with one transport there is nowhere to escape to. Rewrite it to the install-and-login remedy only. It is interpolated at `cli.rs:179,186,206,284,425,462,470` and inlined again at `:67,144`. Test updates, all rewritten against the new remedy string rather than deleted:
  - positive assertions `cli/tests.rs:397,499,566,861` (four, not three)
  - **negative assertions `cli/tests.rs:875,881`** (`assert!(!err.contains("--llm api"))`). These pin the remedy-less-error doctrine at `cli.rs:242-247`, and after excision they go **trivially true and silently stop biting**. A test that cannot fail is worse than no test: re-point them at the new string so they still prove which failures deliberately carry no remedy.
- Also update the `:326` doc comment: the child no longer excludes `ANTHROPIC_API_KEY` because "`--llm cli` must mean what it says" but because clyde handles no key at all.
- Update the `llm:` line in the annotated config block in `README.md` and `report/README.md`, plus `README.md:143-145,171` and `report/README.md:91,101,148`. There is no standalone example `clyde.yml` in the repo (only `permit/claude-permit.yml`), so those two README blocks are the annotated example.

**Success criteria:**
- `rg 'api_key_from_env|ApiTransport|Llm::Api|LlmConfig' --glob '*.rs' --glob '!target/**'` returns zero hits.
- `clyde report render --llm api` fails as an unknown argument, and so does `--llm cli`.
- No error string in the workspace advises `--llm api`, asserted by the rewritten `cli/tests.rs` assertions.
- `otto ci` green, and the removed precedence-matrix tests are replaced, not just deleted.

#### Phase 5: Stop installing a credential file.

**Model:** sonnet

- Remove `EnvironmentFile=%h/.config/clyde/enrich.env` from the generated unit (`bootstrap.rs:1131`).
- **Rewrite EXISTING units, not just the generated body.** `refresh_clyde_unit` (`bootstrap.rs:1082`) returns `Ok(false)` unless the unit text contains `"sessions enrich"` (`:1086`), and every already-migrated install is on `session enrich` (verified on desk.lan). So re-running bootstrap today **no-ops and leaves the `EnvironmentFile` line in place**. Broaden its trigger: rewrite a clyde unit that carries an `EnvironmentFile` line as well as one carrying the stale subcommand spelling. Without this, Phase 5 changes only fresh installs and the deployed timer keeps naming a file the operator is about to delete, at which point systemd fails the unit (`EnvironmentFile=` without `-` is mandatory).
- **`rewrite_unit` itself has to change shape.** It is currently two string substitutions, `text.replace("klod","clyde").replace("sessions enrich","session enrich")` (`bootstrap.rs:1071-1074`). Removing a whole directive needs **line filtering**, not `.replace`. Keep the existing `backup(svc)` call (`:1093`), which already gives the recovery path if a rewrite goes wrong.
- **Do NOT add `EnvironmentFile=-`.** Stripping the line is sufficient, and an optional-file directive pointing at a path clyde no longer uses is the orphan this phase exists to delete. The mandatory-vs-optional semantics are cited above only to explain why the un-stripped line breaks the unit, not as a proposed fix.
- Delete `move_env_file` (`:1101-1116`) and its call site; drop the "move the API-key env file" step from the oneshot's doc comment (`:994`) and from the `completed`/`failed` tallies.
- Delete the three bootstrap tests that write a fake key file (`bootstrap/tests.rs:56,455,803`), replaced by one asserting the generated unit contains **no** `EnvironmentFile` line.
- **Warn on a stale `.env` (G6).** Bootstrap checks for `~/.config/clyde/enrich.env` and, if present, emits a `warn!` plus an operator-visible line naming the full path and saying it is no longer read and should be removed. It does NOT delete it. This is the "degrade visibly" rule: an orphaned credential that nothing reads is exactly the kind of thing that sits on disk for a year.
- **Resolve `claude` explicitly in the unit. Do not defer this to implementation.** The unit already hardcodes `%h/.cargo/bin/clyde` (`bootstrap.rs:1132`) precisely because `systemctl --user` does not inherit an interactive `PATH`. The flip makes clyde spawn `claude` via `which::which` at runtime, so the unit must be able to see it.

  **It works today, and that is exactly the problem.** Measured on desk.lan 2026-07-29 with a real transient unit, which is ground truth rather than inference:

  ```
  systemd-run --user --wait --pipe --quiet /bin/sh -c 'echo $PATH; command -v claude'
  PATH=/home/saidler/.local/bin:/home/saidler/.local/share/mise/bin:/home/saidler/.cargo/bin:...
  claude: /home/saidler/.local/bin/claude
  ```

  Two earlier drafts of this bullet got this wrong in opposite directions, so the corrections are recorded: `claude` is **not** an npm global (it is `~/.local/bin/claude`, a symlink to `~/.local/share/claude/versions/2.1.220`, the native installer), and the user manager **does** carry a `PATH` that includes `~/.local/bin`, so a user unit resolves `claude` today. The second draft's "will fail today, this is not a maybe" was false. Method note for whoever audits this: the false reading came from running `systemctl --user show-environment` inside a sandbox that blocked the user bus, so it emitted nothing and the empty output was misread as "no PATH." A blocked command's silence is not evidence.

  **The latent failure is real even though the current behavior is fine.** That PATH is inherited from the login session, not owned by the unit: there is no `import-environment` in dotfiles and no `PATH` in `~/.config/environment.d/`. So it is present because Scott is logged in. A headless boot with lingering, or any change to the login path, drops the manager to systemd's built-in default, which excludes `~/.local/bin`, and every timer run then fails.

  **Resolved: resolve `claude` at install time and write it into the unit. Do not hardcode a PATH list.** `bootstrap` runs with the full interactive PATH, so it can resolve `claude` and record it. Two constraints:
  - Write the **symlink's directory** (`~/.local/bin`), not the versioned target (`.../versions/2.1.220`), or the unit goes stale on the next `claude` self-update.
  - Prepend that directory to the inherited PATH rather than replacing it. A hand-written list drops `~/.local/share/mise/bin`, `/usr/local/sbin`, `/usr/sbin`, `/sbin`, and snap, all present in the live PATH, and `child_env()` forwards PATH to the child `claude` (`cli.rs:~342`), so a trimmed list becomes the child's PATH too.

  This also survives an npm-installed `claude` on someone else's host, which a hardcoded native-installer path would not. Assert resolution in Phase 5's live check.
- This interacts with G5: if the unit cannot find `claude`, **every** timer run fails, and whether that silently burns the `attempts` budget depends entirely on the classifier from Phase 3 being right. `claude`-not-on-PATH is sweep-fatal by construction (`ClaudeCli::resolve()` fails before the loop), so the two fixes have to both hold or the timer quietly retires the catalog.

**Success criteria:**
- The unit body generated by `install_clyde_timer` contains no `EnvironmentFile`, asserted by a unit test.
- `rg -i 'enrich\.env|EnvironmentFile' --glob '*.rs' --glob '!target/**'` returns only the stale-file warning added by this phase. No code generates, moves, or reads a `.env` (G6).
- With a stale `enrich.env` planted in a temp XDG config dir, bootstrap warns and names the path, and the file still exists afterward (clyde must not delete it).
- On desk.lan, `systemctl --user start clyde-enrich.service` after a reinstall completes with the key file absent from the environment, and the sweep enriches at least one session.

## Acceptance Criteria

- [x] AC1: no Rust file in the workspace **reads** `ANTHROPIC_API_KEY` or sends `x-api-key`. Mechanically, BOTH halves must hold:
  - **Negative:** use the **enumeration** form from the inventory section, not a keyword grep: `rg -o -N 'env::var(_os)?\("([A-Z_]+)"\)' -r '$2' --glob '*.rs' --glob '!target/**' | sort -u` must not contain `ANTHROPIC_API_KEY`, AND `rg 'x-api-key' --glob '*.rs' --glob '!target/**'` must return zero hits. A keyword grep for `env::var.*ANTHROPIC_API_KEY` is NOT acceptable here: it cannot see `std::env::var(API_KEY_ENV)` (`sessions/src/llm.rs:91`, const at `:29`), which is the exact indirection this doc's inventory section calls out as the reason keyword search is insufficient. An earlier draft's AC1 contradicted the doc's own method four hundred lines earlier.
  - **Positive (the guard against satisfying AC1 by deletion):** the two child-env allowlist tests must still exist and still plant the variable. Assert by **test name**, not by a string count: `rg -c 'fn (env_is_cleared_and_only_allowlisted_vars_reach_the_child|planted_secrets_never_reach_the_child)' common/src/llm/cli/tests.rs` returns 2, with the real function names substituted at implementation time. Do NOT pin a sentinel-string count: measured today, `rg -c 'planted-api-key-must-not-leak' report/src/summarize/cli/tests.rs` returns **1** (only `:320`; `:155` asserts on the bare name `"ANTHROPIC_API_KEY"`), so an earlier draft's "returns 2" failed on day one, and an exact integer also breaks when someone legitimately adds a third planted-var test.
  - Why the positive half exists at all: those tests (`cli/tests.rs:155,320`, moved from `report` in Phase 1) plant `ANTHROPIC_API_KEY` in the parent environment and assert it never reaches the child. They are array-literal strings, so the negative half cannot see them, and without a positive assertion an implementer could delete both and AC1 would still pass green. They must stay: a user may still have the var set for other tools, and these are the proof clyde does not forward it.
- [x] AC2: on a keyless host with a logged-in `claude`, all three LLM commands exit 0 and produce model-authored output: `env -u ANTHROPIC_API_KEY clyde session enrich <session-id>` (row gains tags + summary + non-zero token counts), `... clyde efficiency session <session-id> --narrate` (prose verdict), `... clyde report render -i <real>.json --format markdown -o -` (>5,000 bytes, >=3 `^## ` headers, no `Generated offline via`).

  Verified 2026-07-30. render: 6,173 bytes, 9 `^## ` headers, no offline marker. enrich: exit 0, gained 5 tags and a 356-char summary, `tokens_in=11817`, `tokens_out=125`. narrate: prose verdict in 13s. The enrich and narrate probes ran against a **copy** of the live catalog via `--db`, so the real `sessions.db` was not mutated to satisfy an AC.

  Two corrections to this criterion's own wording, both found by running it: there is no `--only` flag (enrich takes a positional `[ID]`, per `clyde session enrich --help`), and the id is the `session_id` uuid column, not the `id` integer primary key. The original text specified an invocation that does not parse.

  `tokens_out=125` also reads Finding 14's cost canary as healthy (band is ~140), confirming `MAX_THINKING_TOKENS=0` is still honored and enrich is not silently paying the ~3x reasoning tax.
- [x] AC3: there is exactly one **production** `Transport` implementation in the workspace: `rg 'impl Transport for' --glob '*.rs' --glob '!**/tests.rs' --glob '!target/**'` returns one hit, in `common/src/llm/cli.rs`. Test doubles are excluded deliberately and must NOT be deleted to satisfy this. Measured today: five impls exist (`summarize/api.rs:70`, `summarize/cli.rs:125`, `eval/judge/tests.rs:33` `Canned`, `render/slots/tests.rs:53` `Scripted`, `:74` `Exploding`); Phase 4 removes exactly one of them, the api transport, leaving one production impl and three doubles.
- [x] AC4 (G6): the systemd unit `install_clyde_timer` generates contains no `EnvironmentFile` line, `move_env_file` no longer exists in the tree, and no code path generates, moves, or reads a `.env` file. In **non-test** Rust the only mentions of `enrich.env` are the stale-file warning (`check_stale_env_file`) and the `Outcome.stale_env_file` field that carries it. Verified 2026-07-30: `rg 'fn move_env_file'` returns nothing, and `check_stale_env_file` only builds the path to test existence, never opens it.

  **Amended 2026-07-30: the `enrich.env` clause is scoped to non-test code, because the original wording was unsatisfiable.** Proving Phase 5's strip works requires fixture unit bodies that literally contain `EnvironmentFile=%h/.config/clyde/enrich.env` -- that string is the *input* to the test. Eleven such fixtures exist in `clyde/src/bootstrap/tests.rs`, plus the planted stale file in the warning test. Satisfying the clause literally would mean deleting the tests that prove the phase works, which is the same defect class as the AC3 and AC7 problems recorded elsewhere in this doc: an acceptance criterion that rejects the thing the design asks for.

  **Scope note on `EnvironmentFile`, raised by Scott 2026-07-30.** This AC is about clyde's OWN enrich unit and nothing else. `rewrite_unit` is reachable only from `clyde-enrich.service` and `clyde-enrich.timer`; every unit path in `bootstrap.rs` is hardcoded and there is no `read_dir`, glob, or walk over the systemd directory, so the strip cannot touch another daemon's unit. The directive was removable here for one reason only: after Phase 3 this unit has no secret to receive. Keeping it would be the dangerous option, because a bare `EnvironmentFile=` (no `-` prefix) is mandatory in systemd, so the unit would fail to start the moment Rollout deletes the file.
- [x] AC5: with `claude` absent from PATH, every one of the three commands surfaces the absence loudly, naming the install-and-login remedy, and none of them mentions `ANTHROPIC_API_KEY`. Verified 2026-07-30. The two commands whose whole purpose is an LLM call fail non-zero: `session enrich` and `efficiency session --narrate` both exit 1 with `install Claude Code and log in once (`claude`, then /login); clyde needs no API key for this`. `report render` **exits 0 by design**, warning on stderr and in the log before emitting a prose-free artifact.

  **Amended 2026-07-30 (Scott), because the original wording asserted the wrong thing about `report render`.** The first draft required all three to "fail non-zero". That contradicts `report/src/render.rs::no_transport`, whose own doc comment reads "Degrade to a prose-free artifact, loudly. Never silent: an operator reading stderr must be able to tell a thin report from a broken one." That function predates this design (verified present on `main`), so the criterion was describing behavior the product never had. It also contradicted `taste.md`'s own "degrade visibly" instinct, which `no_transport` is the implementation of: a report's data sections are computed in Rust and stay correct with no model available, so discarding them to honor an exit code would destroy working output. What AC5 actually cares about is that the failure is never silent and never blames a missing API key, and all three satisfy that. Rejected alternative: change `render` to fail non-zero. Not taken. It is a real behavior change nobody asked for, it would break the "the data sections are unaffected" promise, and it would need its own design decision rather than riding an acceptance criterion.
- [ ] AC6: Keegan's original failure is closed. A teammate with no key runs the published runbook end to end on their own catalog and gets a report whose enrichment coverage clears the 50% floor.
- [x] AC7 (G5): a sweep against an unavailable `claude` charges `attempts` to at most the rows the breaker observed failing, never to the whole candidate set, and exits non-zero. Mechanically, against a seeded db of >= 20 candidates:
  - classifier path: a `Fake` returning `TransportError::Unavailable` on the first record leaves `SELECT sum(attempts)` **identical** before and after (it aborts before charging).
  - breaker path: a `Fake` returning ordinary errors for every record aborts after 3 and raises `sum(attempts)` by **exactly 3**, not by the candidate count.
  - control: a `Fake` failing only record 2 does not abort, and raises `sum(attempts)` by exactly 1.
  An earlier draft asserted `sum(attempts)` unchanged in all cases, which a correct implementation of the breaker fails. Same defect class as the AC3 problem: an acceptance criterion that rejects the thing the design asks for.

## Resolved Decisions

- **2026-07-30 (Scott, post-implementation): three acceptance criteria were mis-specified and are amended in place, with the reasoning recorded rather than the criteria quietly dropped.** All five phases shipped `otto ci` green before this was noticed, and in each case the implementation was correct and the criterion was wrong. (a) AC5 required `report render` to fail non-zero with no `claude`; it exits 0 by deliberate pre-existing design (`render.rs::no_transport`, present on `main` before this work), so the criterion described behavior the product never had. Amended to require a loud, remedy-naming, key-free failure surface, which all three commands satisfy. Rejected: changing `render` to fail non-zero, an unrequested behavior change that would break the "data sections are unaffected" promise. (b) AC4's `enrich.env` clause was unsatisfiable: proving Phase 5's strip works requires fixtures containing the string, so satisfying it literally meant deleting the tests that prove the phase. Scoped to non-test code. (c) AC2 specified `clyde session enrich --only <id>`; no `--only` flag exists and the id is the `session_id` uuid, not the `id` integer PK, so the criterion named an invocation that does not parse. Corrected. This is the same defect class the doc already records for its own earlier AC3 and AC7 drafts: an acceptance criterion that rejects the thing the design asks for.
- **2026-07-30 (Scott): G6 is scoped to this unit, not a blanket ban on `EnvironmentFile`.** Scott raised that `EnvironmentFile=` is the only practical way to deliver a secret to a local `systemd --user` daemon and must stay supported. Confirmed by reading the code that nothing generic was removed: `rewrite_unit` is reachable only from `clyde-enrich.service` and `clyde-enrich.timer`, every unit path in `bootstrap.rs` is hardcoded, and there is no `read_dir`/glob/walk over the systemd directory, so the strip cannot reach another daemon's unit. The directive was removable here solely because Phase 3 left this unit with no secret to receive, and keeping it would be the actively dangerous choice: a bare `EnvironmentFile=` is mandatory in systemd, so the unit would fail to start the moment Rollout deletes the file. G6's wording is narrowed accordingly.
- **2026-07-30: retiring the klod-era migration is explicitly NOT part of this design.** 79 of the ~96 `klod` references in non-doc code are the migration and legacy-detection machinery itself (`bootstrap.rs` moving the data dir, config dir, and systemd unit; `doctor.rs` reporting legacy klod state as unhealthy). That code exists to remove klod from a machine, so deleting it does not finish the rename, it abandons any host not yet migrated. desk.lan is migrated; `ltl-7007` and `mini` are unverified. Three genuinely stale mentions remain (`Cargo.toml`'s provenance comment, `session/src/scope/tests.rs`'s path fixture, `README.md`'s link to a historical design-doc filename). Retiring the migration is its own targeted change gated on checking the other hosts, and is recorded here so it is not mistaken for something this design left half-done.

- **2026-07-29, Scott:** excise, do not deprecate. `--llm api` and `ApiTransport` are deleted outright, not kept behind a flag. Rationale: post-#76 nothing needs them, and a second billing path kept "just in case" is the thing that let the keyless hole survive #60.
- **2026-07-29:** the transport moves to `common` rather than being reimplemented in `sessions`. `report` depends on `sessions`, so it cannot stay put. Reimplementing would fork the `env_clear()` secret allowlist, and that allowlist is what keeps `CLAUDE_COST_ANTHROPIC_API_ADMIN_KEY` and two Slack tokens out of the child process (#60 `:325`). One copy, one place to fix it.
- **2026-07-29 (review pass 3):** `Kind::max_output_tokens_key` returns `Option<&'static str>` and `Kind` gains a `fence()` method. Rejected: inventing `enrich-max-output-tokens` / `narrate-max-output-tokens` config keys nobody asked for, and labeling enrich's prose payload as ```` ```json ````. Detail in API Design.
- **2026-07-29 (review pass 4):** sweep-fatal failures abort the enrich sweep without charging `attempts` (G5), matched via `downcast_ref::<TransportError>()`. Rejected: treating a logged-out `claude` as an ordinary per-session failure, which lets five unattended timer runs permanently drop the whole catalog out of the candidate set. Detail in Sweep-fatal vs per-session failure.
- **2026-07-29 (Phase 0 probes C and D, superseding a round-2 review conclusion):** the CLI **does** expose a structured discriminator under the final argv: `api_error_status` (401 on auth) plus `terminal_reason: "api_error"` (present on both auth and network). Classification is typed; no prose matching. A round-2 review concluded no structured signal existed and that auth and network envelopes were byte-identical, but that came from bare-flag probes: the argv changes the envelope shape, because the CLI propagates `api_error_status` only when `subtype == "success"`, and under the final argv a failure sets `subtype: "success"` and `is_error: true` at once. Measured: auth `api_error_status: 401`, network `api_error_status: null` with `terminal_reason: "api_error"`. Both sweep-fatal, and distinguishable.
- **2026-07-29 (review panel, accepted):** a consecutive-failure circuit breaker (N=3) rides alongside the classifier, charging `attempts` to its N observed failures only. It covers unenumerated failures and, more importantly, the wall clock: a dead transport takes **179s per call**, so an unbroken sweep grinds ~14 hours over 286 candidates. Charging N rather than zero is the fix for a livelock the reviewer's original formulation had: the candidate order is deterministic, so three bad head rows would otherwise trip the breaker forever and nothing would ever enrich.
- **2026-07-29 (review panel round 2, HIGH):** the systemd `claude` resolution is done by **install-time resolution written into the unit**, prepending the resolved symlink directory to the inherited PATH. Rejected: hardcoding a PATH list (drops mise/sbin/snap entries that `child_env()` forwards to the child, and assumes the native installer over an npm install). Two prior drafts stated the current behavior wrong in opposite directions; the measured truth is that a user unit resolves `claude` today via session-inherited PATH, and the fix exists because that inheritance is not the unit's to rely on.
- **2026-07-29 (review panel round 2, MEDIUM):** default enrich candidates are **286**, not the 1,003 an earlier draft carried. Root cause: a guessed `prompt_version < 3` instead of the real `ENRICH_PROMPT_VERSION = 1`. Backfill corrected to ~$6 api / ~$16 cli-proportional / ~$66 cli-fixed. GO/NO-GO thresholds unaffected, because per-100 tracks mean payload size, not candidate count.
- **2026-07-29 (review panel, convergent Architect + Staff, CRITICAL):** G5 classifies on envelope **content**, not exit status. An earlier draft's table had one sweep-fatal row for "logged out (non-zero exit, no envelope)", which catches never-logged-in and misses **token expiry**: `cli.rs:174-179` Guard 2 returns exit 0 with a well-formed `is_error` envelope, pinned by `cli/tests.rs:558-566`. Expiry is the exact case G5 exists for, so the draft contradicted itself. Also added: a transport-layer test, because AC7's `Fake` returns the variant and therefore cannot catch a misclassification inside the transport.
- **2026-07-29 (review panel, CRITICAL):** Phase 5 must rewrite EXISTING units. `refresh_clyde_unit` (`bootstrap.rs:1082`) no-ops unless the unit contains `"sessions enrich"`, and migrated installs are on `session enrich`, so the previous rollout step ("re-run bootstrap so the deployed unit loses its `EnvironmentFile`") asserted an outcome the code cannot produce. Verification is now reading the unit, not trusting a clean exit.
- **2026-07-29 (review panel, convergent, HIGH):** `report eval` is a second `ApiTransport` consumer (`eval.rs:43,448`, `eval/tests.rs:409`) and the doc had zero mentions of eval. It converts in the SAME commit as the deletion, because `report` does not compile between the two.
- **2026-07-29 (review panel):** Phase 0 is KEPT, against one reviewer's recommendation to strike it as already-settled arithmetic. That recommendation cited predecessor `:509`'s "$0.0024 per call", but predecessor `:507` explicitly disclaims probes 7/8 as trivial-payload figures describing only the preamble's absence; the haiku sub-call was a later real-payload finding. The fixed-vs-proportional question is genuinely open at n=1, the swing is 7x on backfill ($49 vs $227), and the probe costs cents. Reviewers advise, the owner decides.
- **2026-07-29 (owner, self-caught before round 2 reported):** the 46 rows already at `attempts=5` are recovered with the EXISTING `--max-attempts` flag (`clyde/src/cli.rs:286`), run once after G5 ships. A proposed `--reset-attempts` flag was CUT as redundant unrequested scope: `--max-attempts 6` already satisfies the `attempts < ?1` predicate, and `set_enrichment` already zeroes `attempts` on success. G5 stops future bleeding; this Rollout step heals the past.
- **2026-07-29:** within-call HTTP retry is dropped, not ported. `AnthropicClient` retries 3x on 429/5xx inside a call (`llm.rs:218-226`); `CliTransport` is fail-loud-never-retry by design. Enrich already has the durable cross-run layer (`attempts` column, `max_attempts` default 5, `db.rs:568`), so a transient failure costs one `attempts` increment instead of being absorbed, and still gets 5 chances across sweeps. Accepted: the transport's stated policy wins, and the durable layer covers the gap. Narrate has no durable layer and simply fails, which matches `report render`'s behavior today.

## Alternatives Considered

### Alternative 1: reimplement a keyless client inside `sessions`
- **Description:** leave `report/src/summarize/cli.rs` where it is, write a second `claude -p` shell-out in `sessions`.
- **Pros:** no crate moves, `report` untouched, smaller diff.
- **Cons:** duplicates 582 lines of argv construction, version floor, envelope parsing, and the `env_clear()` allowlist. Two places to fix when a `claude` flag moves. The next secret-bearing env var leaks from whichever copy someone forgets.
- **Why not chosen:** the allowlist is a security control, and forking a security control to avoid a file move is the wrong trade.

### Alternative 2: keep `ApiTransport` as an opt-in escape hatch
- **Description:** rewire enrich and narrate to keyless, leave `--llm api` in `report`.
- **Pros:** a key holder keeps the ~1.9x-cheaper render path (#60 `:515-516`).
- **Cons:** violates G1 outright. Keeps a second billing path and a second failure mode. Keeps `api_key_from_env` alive, which is what `resolve_transport` reads, which is what makes the precedence matrix four arms instead of one.
- **Why not chosen:** Scott asked for excision. The cost delta is real and is recorded in Performance as an accepted cost, not hidden.

### Alternative 3: `apiKeyHelper` in Claude Code settings
- **Description:** already rejected in #60 (`:494`).
- **Why not chosen:** it makes Claude Code fetch a key, which is more credential plumbing, not less. Rejected there, rejected here, not re-litigated.

### Alternative 4: batch N sessions per `claude` invocation
- **Description:** amortize per-invocation overhead by enriching several sessions in one call.
- **Pros:** cuts the fixed per-invocation tax by a factor of N.
- **Cons:** changes the prompt and response shape (a JSON array keyed by session id), changes the redaction payload boundary, and makes one bad session poison a batch. That is a real redesign of enrich, not a transport swap.
- **Why not chosen now:** parked with a revisit condition, triggered only by a Phase 0 NO-GO. Building it speculatively is unrequested scope.

## Technical Considerations

### Dependencies

- `common/Cargo.toml` gains `which`, `serde_json`, `eyre`, `log`, `thiserror`.
- `sessions/Cargo.toml` drops `reqwest`. Verified: it appears in `sessions/src` only at `llm.rs:82,96,238`, all inside `AnthropicClient`. That removes the workspace's last `reqwest`-based HTTP client outside `pricing`.
- No new external crates enter the workspace. Everything moving is already vendored.
- **Cross-repo blast radius: none.** Single repo, single flat version, one PR chain. No ship-order constraint on other repos.
- **New runtime prerequisite:** `claude` on PATH, logged in, at or above the #60 version floor. It was already required for `report render` under `llm: auto`; this extends it to enrich and narrate. For the target user that is not a new install: they have Claude Code, which is why they have sessions to catalog.

### Performance

The honest accounting, from #60's measurements (`:509-522`).

- Per invocation, the CLI path bills the payload as a **1-hour cache write** at 2x the input rate, and adds a non-suppressible internal haiku sub-call. Nothing reads that cache back (one turn, `--no-session-persistence`), so it is 2x for zero reuse.
- For `report render` that measured ~1.9x the api cost: about +$1.40 per render. Accepted at the time as the price of reaching keyless users.
- **Enrich is different in kind, and this is the design's main risk.** Render is one invocation per report. Enrich is one invocation per session. Keegan: 94. Scott: 1,300+. If the fixed per-invocation tax is anywhere near the ~$0.19 #60 measured, a full backfill is dominated by fixed cost, not payload cost.
- Enrich payloads are far smaller than a 513KB report context, so the sub-call tax may scale down proportionally. May. That is exactly why Phase 0 measures a small and a large payload separately and reports the fixed/proportional split, instead of this doc assuming it.
- Subprocess spawn cost per session is real but second-order next to the model call.
- **Cost attribution shifts** from the API key to the user's Claude Code seat. That is the intended outcome (it is how `report render` already bills) and it has a mild self-reference: enrichment now lands inside the usage numbers clyde reports on.

### Security

Strictly less credential handling than today, on every axis:

- clyde reads no key, holds none in memory, transmits none. `claude` owns auth end to end.
- clyde stops installing a systemd unit that loads a credential file, and stops migrating one into place.
- The `env_clear()` plus explicit-allowlist child environment is preserved by the move and now covers three call sites instead of one. Its enumerate-by-name test (#60 AC4) moves with it, so a future secret-bearing variable still fails loudly rather than leaking.
- **Measured confirmation that the allowlist is load-bearing for enrich, not just render (2026-07-29).** `systemctl --user show-environment` on desk.lan carries roughly a dozen live credentials that user units inherit, including the `CLAUDE_COST_ANTHROPIC_API_ADMIN_KEY` and two Slack tokens the allowlist rationale already names (`cli.rs:325-327`). No values are reproduced here or anywhere in this doc. This is not a new defect: it is exactly why `child_env()` is an allowlist rather than a denylist, and it is now confirmed by measurement rather than assumed. The consequence for this design: after the flip, **the enrich timer spawns `claude` from inside that secret-bearing environment**, so the allowlist protects `session enrich` too. That raises the stakes on the Resolved Decision not to fork it, and it is a further argument against the rejected "reimplement a keyless client inside `sessions`" alternative.
- Redaction is unchanged: enrich still scrubs payloads before they leave the process (`session/src/redact.rs`), and the scrubber's own `api_key:` pattern stays.
- After merge, `~/.config/clyde/enrich.env` is inert. Deleting it and revoking the key is Scott's action, in Rollout.

### Testing Strategy

- **The ports are the safety net.** `Completer` and `Narrator` do not change, so the existing `Fake` (`sessions/src/enrich/tests.rs:54`, `sessions/tests/export.rs:157`) and `FakeNarrator` (`efficiency/src/narrate/tests.rs:28`) compile and pass untouched. If they need edits, the ports moved and the phase is wrong.
- Phase 1 moves `CliTransport`'s tests with it, unchanged. That is the proof the move was verbatim.
- Phase 2 adds fixture-envelope tests over the real Phase 0 capture, including the absent-`usage` error case.
- Phase 4 replaces the `resolve_transport` precedence tests rather than deleting them: one transport still has a matrix (present/absent x format).
- Phase 5 adds a unit test asserting the generated unit body has no `EnvironmentFile`.
- The G5 test is the one that matters most, because the condition it guards is silent in production. It asserts on `sum(attempts)` across the whole candidate set, not on a single row, so a partial-abort regression fails it.
- **Break it to prove it bites:** the absent-`usage` test must fail if the error is softened to a zero, the no-`EnvironmentFile` test must fail if the line is restored, and the G5 test must fail if the sweep-fatal arm is changed back to `record_enrich_failure`.
- Live verification is in the ACs, not in CI: CI has no logged-in `claude`.

### Rollout Plan

- One PR per phase, each `otto ci` green, each one commit, per `general.md` and the phasing rule.
- Phase 0's findings land in the implementation notes before Phase 1 opens.
- After merge: `bump`, `cargo install --path .`, then live-verify AC2 keyless on desk.lan.
- Then re-run `bootstrap` on desk.lan and **verify by reading the unit** that `EnvironmentFile` is gone: `grep EnvironmentFile ~/.config/systemd/user/clyde-enrich.service` must return nothing. Do not treat a clean bootstrap exit as proof, because the pre-fix `refresh_clyde_unit` exits clean while changing nothing. Then `systemctl --user daemon-reload` and confirm the timer still fires.
- **Recover the 46 retired rows, once, AFTER G5 ships:** `clyde session enrich --max-attempts 6`. Uses the existing flag (`clyde/src/cli.rs:286`); no new code. Order matters: run it before G5 and a still-broken transport charges all 46 again and pushes them to 6. Verify afterward with `sqlite3 -readonly ~/.local/share/clyde/sessions.db "select attempts, count(*) from sessions group by attempts;"` and expect the 46 to have moved off 5.
- Then Scott removes the `.env` and revokes the key. Required, not optional (G6): `rkvr rmrf ~/.config/clyde/enrich.env` per `safety.md` (never `rm`), then revoke the key in the Anthropic console so any copy elsewhere is inert.

**The exact host state, verified on desk.lan 2026-07-29** (so the cleanup is a checklist, not a hunt):

| Path | State |
|---|---|
| `~/.config/clyde/enrich.env` | the credential. 127 bytes, mode 0600, dated 22 Jun. **The one file to remove** |
| `~/.config/klod/enrich.env` | gone. `move_env_file` relocated it as designed, so there is exactly one credential file, not two |
| `~/.config/systemd/user/clyde-enrich.service` | LIVE. `EnvironmentFile=` at line 11, `ExecStart ... session enrich` at 13. Rewritten by Phase 5's bootstrap re-run |
| `...clyde-enrich.service.clyde.bak` | backup, still carries `EnvironmentFile=` at line 11 and the pre-rename `sessions enrich` |
| `...klod-enrich.service.clyde.bak` | backup, `EnvironmentFile=%h/.config/klod/enrich.env`, a path that no longer exists. Inert, but it is a credential reference in a file nobody looks at |
| `...klod-enrich.timer.clyde.bak` | no `EnvironmentFile`. Nothing to do |

The two `.bak` service units are the same orphan in a different costume: bootstrap will not rewrite them, so they keep naming an env file after the live unit stops. Remove them in the same pass.

- The excision is not finished while that file exists. Bootstrap's warning is the reminder, not the fix.
- Then repost the runbook in `#claude-code` and have Keegan re-run it. AC6 is the close.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| R1: per-session invocation tax makes a catalog backfill unaffordable | Med | High | Phase 0 measures it before any code. NO-GO triggers the batching alternative, which is already scoped |
| R2: an expired `claude` login on the unattended timer burns the durable `attempts` budget and permanently drops the catalog from the candidate set | **High** | **High** | G5, classifying on envelope **content** (an expired token returns a well-formed `is_error` envelope at exit 0, so exit-status classification misses it). Asserted at BOTH the sweep layer and the transport layer in Phase 3. The 46 rows already retired are recovered by a one-time `--max-attempts 6` sweep after G5 ships (Rollout), no new flag. Not hypothetical: the mechanism has already retired 46 sessions on desk.lan |
| R3: `usage` buckets differ from #60's render measurement, so `tokens_in` records ~0 and `--budget-tokens` silently never trips | Med | Med | Phase 0 captures the verbatim object; absent/unrecognized usage is a hard error, never a zero |
| R4: a 1,300-session sweep runs one subprocess at a time and takes far longer than the api path | High | Low | Accepted. The sweep is incremental (dormancy-gated candidates) and bounded by `--budget-tokens`, so steady state is a handful of sessions per run. Only the first backfill is long, and it is resumable: `attempts` and `enriched_at` make a killed sweep pick up where it stopped |
| R5: `--no-session-persistence` regresses and enrich starts creating sessions clyde then catalogs and enriches, recursively | Low | High | The flag is in the argv and #60 verified session-JSONL counts unchanged across a render. Phase 0 re-verifies at enrich's much higher invocation count: session and lock-file counts before and after a multi-session sweep |
| R6: the `common` move churns `report` more than expected and the "verbatim move" claim quietly stops being true | Med | Med | Phase 1 gates on `git diff --stat` showing moves plus a keyless render matching pre-move behavior. Transport tests move unchanged |
| R7: the systemd timer's environment cannot find `claude` on PATH once the unit stops loading an env file | Med | Med | Phase 5 confirms the unit's PATH resolves `claude`, and sets it explicitly if not. Caught by the live start in Phase 5's criteria |
| R8: losing within-call retry makes enrich sweeps flakier against transient failures | Low | Low | Durable `attempts` layer already gives 5 chances across sweeps. Recorded as an accepted decision, not a surprise |
| R9: a caller somewhere pins `--llm api` and breaks on the flag's removal | Low | Low | `rg` the tree and desk.lan units for `--llm` before Phase 4. Only known caller is the operator's own shell history |

## Open Questions

None.

- Phase 0 is a measurement, not an open question: it now carries a **numeric** GO/NO-GO (GO at or under ~$5 recurring per 100 sessions, NO-GO near ~$22) and a named contingency. An earlier draft said "Scott decides on the measurement" with no threshold, which the review panel correctly called not-an-assertion.
- Every panel finding is dispositioned in-doc: F1, F2, F3a, F3b, F3e, F4a, F4b, F5, F6, F7a, F7b, F7c folded in; F3c and F3d were already closed by edits made before the panel reported. Nothing was deferred and nothing is disputed.
- The one place the two reviewers disagreed (strike Phase 0 vs keep it) is resolved in favor of keeping it: the argument for striking rested on predecessor `:509`, but predecessor `:507` explicitly disclaims those probe-7/8 figures as trivial-payload measurements that describe only the preamble's absence. The sub-call was a later real-payload finding. See Resolved Decisions.

## References

- `docs/design/2026-07-24-report-render-claude-cli-transport.md`: the keyless transport, the scope boundary at `:254`/`:458`/`:595`, the cost measurements at `:509-522`, the child-env decision at `:325`
- `docs/design/2026-07-29-render-inversion.md`: removed the html-source formats, which is what makes `ApiTransport` fully redundant
- Slack `#claude-code`, 2026-07-29, thread `1785365586.176949`: Keegan's `enrich needs the API key`, the report that triggered this
- `~/repos/.claude/rules/taste.md`: fail loudly/fail closed, phase-0 spikes, copy the proven in-house pattern
- `~/repos/.claude/rules/secrets.md`: credential custody is the operator's channel
