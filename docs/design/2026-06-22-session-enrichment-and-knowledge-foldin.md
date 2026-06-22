# Design Document: klod Session Enrichment & Knowledge Fold-In (Phases 2-4)

**Author:** Scott Idler
**Date:** 2026-06-22
**Status:** In Review
**Review Passes Completed:** 5/5 author self-review (Rule of Five); prior-session (507bbdaf) mined
for Phase 2-4 decisions; external design review folded in - Architect (Gemini) + Staff Engineer
(Codex), 2026-06-22, both verified reuse claims against live klod + second-brain code.

## Summary

This is the follow-on design to `2026-06-21-session-knowledge-catalog.md`, which shipped the
navigational layer (Phases 0, 1, 1.5) and deliberately deferred Phases 2-4 to "named so the seam
is right, designed later." This doc designs those three phases:

- **Phase 2 - Enrichment.** Fill the `tags` and `summary` (abstract) columns that Phase 1 created
  but left NULL, by running a cheap Haiku pass over dormant sessions. This is the first time klod
  ships session content off-machine, so it is gated on the redaction/scope decision the catalog
  doc parked.
- **Phase 3 - Knowledge layer.** Distill dormant sessions into discrete knowledge atoms in
  **second-brain** (not klod), served through oracle's existing MCP. This phase crosses the
  work/personal line and is the deepest of the three; it gets a fuller home in a second-brain doc
  but is specified here at the level this thread supports.
- **Phase 4 - Tool migration.** Fold `cr`, `ccu`, and `claude-permit` into klod as `report`,
  `cost`, and `permit` lib crates over the shared `session` core.

The central decision this doc forces to a head is the **Phase 2 execution environment**: a
**dormancy sweep** (cron-invoked CLI on desk.lan) over a Stop-hook. That choice settles key
*custody* (one key, one host) but not *which account* - and external review made clear that the
account choice is inseparable from a **scope gate that must classify work/personal and run before
the first off-machine call**, so that gate is pulled forward into Phase 2 rather than parked.

## Problem Statement

### Background

Phase 1 built `klod sessions` over a local `sessions.db`: a per-session record with dual FTS5
indexes (high-signal + body), `search`/`ls`/`open`/`reindex`, and the `session` shared core that
parses `~/.claude/projects/**/<uuid>.jsonl`. Phase 1.5 added `klod sessions stage`, which copies
the raw transcript of a dormant session (idle past `--dormant-after`, default 7d) under
`<xdg-data>/klod/staged/<session-id>/` to beat Claude's 30-day TTL.

Two columns exist but are unpopulated: `summary` (the catalog doc's `abstract`; renamed because
`abstract` is a Rust keyword) and `tags`. The Retrieval Decision in the catalog doc depends on
both: ranking uses a high-signal projection of `title + tags + summary`, so until enrichment runs,
ranking leans entirely on `title`, and tag search returns nothing.

Separately, three sibling tools (`cr`, `ccu`, `claude-permit`) already parse this same corpus with
their own parsers. The catalog doc designed the `session` seam specifically so they could later
share one parser, but deferred the migration.

### Problem

1. **Sessions cannot be searched by topic or content quality.** Ranking and tag search are inert
   until `tags`/`summary` are populated, and the only mechanism that can populate them well is an
   LLM pass - which means shipping session content off-machine, which is gated.
2. **The knowledge stored in past sessions is not recallable as knowledge.** Finding the *session*
   is solved; answering *"what did I decide about X"* is not.
3. **Three tools parse the same corpus three ways.** Divergent parsers mean divergent truth
   (notably the subagent-rollup contract) and triplicated maintenance.

### Goals

- Populate `tags` and `summary` for dormant sessions via a cheap, re-entrant LLM pass.
- Settle the execution environment (sweep vs. hook) and the identity/key it runs under.
- Make the redaction/scope gate concrete enough to pass before the first off-machine call.
- Specify the knowledge layer's write path, atom taxonomy, and dedup mechanism at the level this
  thread supports, while keeping its detailed design in second-brain.
- Migrate `cr`/`ccu`/`claude-permit` onto the shared `session` core without disturbing them until
  the seam is proven.

### Non-Goals

- **Live (in-session) tagging.** The catalog is for finding *past* sessions; enrichment at
  dormancy is sufficient and gives the LLM the complete session's text as signal.
- **A new retrieval engine.** Phase 3 rides oracle's existing BM25 + Candle bge-small + RRF stack.
- **Resolving the work/personal vault crossing in this doc.** Phase 3 surfaces it; the decision
  stays parked (Open Q from the catalog doc) and Phases 2 and 4 do not cross it.
- **Changing `cr`/`ccu` user-facing behavior in Phase 4.** Migration is internal; the seam is the
  point, not new features.

## Proposed Solution

### Phase 2 - Enrichment

#### Execution environment (the decision)

The catalog doc sketched Phase 2 as a "debounced `Stop` hook." This thread re-examined that and
chose a **dormancy sweep** instead. Both are short-lived `klod` processes that read a transcript,
make one Haiku API call, write `tags`/`summary` to `sessions.db`, and exit. The difference is what
spawns them and where:

- **Stop-hook model (rejected):** Claude Code runs `klod` as a hook subprocess when a turn ends,
  on whatever machine you are running Claude on (desk, mini, lappy). Each machine would need the
  hook configured and an Anthropic API key in its environment. This spreads the off-machine API
  surface across every host you use Claude on.
- **Dormancy sweep (chosen):** a cron job (or manual run) on the **one host that owns the
  catalog** - desk.lan, where `sessions.db` and the staged transcripts live. One place, one
  schedule, one key. This is the same posture as sb/borg/cortex, and it reuses the dormancy
  machinery Phase 1.5 already built.

Why the sweep wins:

1. **Key custody.** The hook spreads the Anthropic key across every machine; the sweep keeps it on
   desk alone. Whichever account is chosen (see the gate below), centralizing one key on one host
   is the cleaner security posture.
2. **Better signal, not worse.** The hook's original rationale was "first few exchanges are
   highest-signal." But a dormant session is *complete*, so the abstract and tags are drawn from
   the whole arc, not a guess at turn 2-3. The catalog is read after the fact; there is no value in
   tagging mid-session.
3. **Reuse.** Phase 1.5's `stage` already selects dormant sessions (`--dormant-after`, default 7d,
   well inside the 30-day TTL). Enrich rides the same selection.
4. **No new daemon.** There is no resident klod service - a transient process, spawned by cron,
   gone when done. Same shape as every other klod verb.

**No local-ML concern.** The inference is an API call to Haiku in Anthropic's cloud. The
no-AVX2/Candle-only constraint (`reference-desk-cpu-no-avx2`) belongs to **Phase 3**, where cortex
does local embeddings. Phase 2 needs only an API key and network egress - exactly what `cr`
already does for its titling, so there is family precedent.

**Multi-host coverage is a known hole, not a free virtue (review catch).** Centralizing the sweep on
desk also means desk's `sessions.db` only knows about desk's `~/.claude/projects`. Sessions created
on `mini` or `lappy` are never staged, enriched, or folded into Phase 3 - they are second-class.
The catalog doc deferred multi-host merge (Non-Goal; `host` column reserved), and this doc
**accepts that constraint explicitly**: enrich and Phase 3 cover *desk-originated* sessions in v1.
The Architect's "hardest question" is the same gap seen through Phase 4: once `cr` becomes `klod
report` running on all three hosts, non-desk hosts get a `klod` whose sessions never enrich. The
resolution is **not** in scope here, but the seam is named: a future host-sync/merge (rsync of raw
JSONL to desk before the sweep, or a multi-host `sessions.db` merge) is the unblock, and it is added
to Open Questions rather than left implicit.

#### The scope-and-redaction gate (the precondition)

Phase 2 is the first klod phase to send session content off-machine. Sessions contain tokens,
internal Tatari data, and secrets. Per the catalog doc's corrected Security section, the gate
belongs **before the first LLM call**. Both external reviews independently made this their #1
finding, and they corrected an error in the draft: the draft tried to **park work/personal scope to
Phase 3 while committing Phase 2 to the work key**. That is incoherent. The moment you point a
sweep at a scope-bound endpoint, you have made a scope decision implicitly, in the riskiest
direction - a blanket desk sweep would ship *personal*-repo session content to Tatari's enterprise
Anthropic account. That directly violates this doc's own Non-Goal ("Phases 2 and 4 do not cross the
work/personal line"). So scope determination is pulled forward into Phase 2 as a hard gate.

The gate has three parts, in order:

1. **Scope classification (must precede send).** Each session is classified `work` or `personal`
   from the `cwd` / `project_dir` already stored in `sessions.db`, using the repo identity
   convention (`~/repos/CLAUDE.md`): paths under `tatari-tv/` are work, under `scottidler/` (and
   other personal roots) are personal; anything unclassifiable defaults to **`personal`** (fail
   safe - an unknown session is never assumed shippable to the work account). Classification is a
   pure function of stored metadata, unit-testable, and runs *before* any payload is built. The
   `scope` column is added in Phase 2 (it was a Phase 3 concept in the catalog doc; it has to exist
   earlier because Phase 2 ships off-machine first).

2. **Account routing (the invariant).** The **work Anthropic key on desk** (klod is `tatari-tv`;
   `cr` titling already crosses this exact boundary) enriches **only `work`-scoped sessions**.
   Personal-scoped sessions are **not** sent to the work account. The invariant, stated plainly:

   > *No `personal`-scoped session content is ever sent to the work Anthropic account.*

   v1 simply **skips** personal sessions (leaves them un-enriched). Enriching personal sessions
   under a separate personal key is a future option, not a v1 commitment. The reviewers' point
   stands: this is the true trust boundary - the endpoint's data-retention agreement plus the scope
   filter - not the regex below.

3. **Secret scrub (defense-in-depth, not the boundary).** For the work-scoped sessions that *do*
   ship, a regex chokepoint strips high-confidence secret shapes (API keys, bearer/AWS tokens,
   private-key PEM blocks) before the payload leaves the process, logging a redaction count. Both
   reviewers correctly note a regex misses generic bearer tokens, DSNs, JWT variants, PII, and
   proprietary URLs. The design does **not** claim the scrub is the boundary - it is belt-and-
   suspenders against a live credential slipping into content already cleared (by step 2) to reach
   the work account. It is not a substitute for the scope gate.

This gate is a hard precondition: **Phase 2 does not ship until scope classification + the routing
invariant are implemented and tested**, with the secret scrub as a single chokepoint
(`session::redact::scrub(&body) -> (String, usize)`) the enrich path must call. The classification
and routing are the load-bearing controls; the scrub is the net beneath them.

#### What enrichment produces

For each work-scoped, dormant, un-enriched (or grown-since-enriched) session, one Haiku call returns:

- **`tags`** - 3-7 klod-owned search tags, deliberately independent of cortex's
  `canonical-tags.yml` (the catalog doc's Naming & Tagging decision; klod never reads that file).
- **`summary`** - the abstract: a 1-3 sentence durable description of what the session was about,
  feeding the high-signal FTS projection.

Titling stays with `cr` for now (the 4% without `ai-title` already fall back to first-prompt in
Phase 1); Phase 4 is where titling consolidates. Phase 2 owns only `tags` + `summary`.

#### Payload: what actually feeds Haiku

The enrich payload is the **parsed high-signal text** (`ParsedSession.body`: user+assistant text
only, no tool output, no thinking, capped), *not* the raw staged JSONL. This is deliberate and
resolves a draft ambiguity the Staff review flagged: "dormant sessions are complete" referred to
*lifecycle* (the session is done, so its arc is settled), not to sending every raw byte. Sending
the text-only body is cheaper, strips tool noise that would dilute a summary, and reuses the
existing parser. The Phase-1 body cap (currently 500K chars) is the send cap; if a transcript
exceeds it, the head+tail are sent and the truncation is logged (a 500K-char text body is already
far more than Haiku needs for a 3-sentence abstract). For an **archived** session, the same parser
runs over the staged copy - which requires a "parse from staged path" read path that does not exist
yet and is explicit Phase 2 work.

#### Enrichment state (schema)

Re-entrancy and idempotency need durable state; `summary IS NULL` alone is insufficient (manual
tags exist via `set_tags`, `--all` re-enrich exists, and a failed call would otherwise retry every
cron forever). Phase 2 adds an `enrichment` state, either as columns on `sessions` or a sidecar
table:

| field | purpose |
|-------|---------|
| `scope` | `work`/`personal` classification (also used by the routing invariant) |
| `enriched_at` | when enrichment last succeeded (NULL = never) |
| `enriched_modified` | the session `modified` mtime enrichment last ran against (grown-since detection) |
| `enrich_model` | model id used (e.g. `claude-haiku-4-5-20251001`) |
| `prompt_version` | enrichment prompt/schema version (bump = eligible for re-enrich) |
| `status` | `ok` / `skipped-personal` / `skipped-empty` / `failed` |
| `last_error`, `attempts` | failure diagnostics + backoff/max-attempt accounting |
| `redaction_count`, `tokens_in`/`tokens_out` | observability + cost accounting |

**Selection predicate** for the default sweep becomes: scope=`work` AND not archived-without-staged
AND (`enriched_at` IS NULL OR `modified` > `enriched_modified` OR `prompt_version` < current). This
makes failures retry with backoff (bounded by `attempts`), not forever.

**Write path (correction from review).** Enrichment is **not** written through `upsert_session` -
that function deliberately *preserves* `tags`/`summary` across reindex (`sessions/src/db.rs`), which
is exactly why the parser never clobbers enrichment, and exactly why it cannot be the enrichment
writer. There is currently a `set_tags` (tags + FTS, no summary) but no summary/state writer.
Phase 2 adds a single transactional **`set_enrichment(id, summary, tags, state…)`** that updates the
row and rebuilds the high-signal FTS in one transaction. Manual tags set via `set_tags`: by default
enrichment **does not overwrite** a manually-tagged session's tags (it still writes `summary` +
state); `--all` / `enrich <id>` override. (Whether that default is right is an Open Question.)

#### Phase 2 API surface

```
klod sessions enrich                      # enrich dormant, un-enriched sessions (cron entrypoint)
klod sessions enrich --all                # re-enrich every session (vocabulary refresh)
klod sessions enrich --dormant-after 3d   # override dormancy threshold (mirrors `stage`)
klod sessions enrich <id>                 # enrich one session by id/fuzzy (manual)
klod sessions enrich --dry-run            # per-session: scope, would-send y/n, redaction count, payload size
```

`--dry-run` is justified here (unlike the CLI rule's default no-dry-run-on-opt-in-flags stance)
because the destructive-ish action is *sending content off-machine*, and previewing the gate's
decisions is the operator's confidence check before it opens. To avoid contradicting "never log the
prompt body," `--dry-run` reports **decisions and metrics** (scope, would-send, redaction count,
payload byte size) by default; dumping the actual redacted payload requires an explicit
`--dry-run --show-payload` to a file the operator opts into, never to the normal log stream.

### Phase 3 - Knowledge layer (in second-brain, deferred)

This phase lives in **second-brain**, not klod (klod produces, second-brain consumes). It gets its
own design doc in that repo; this section fixes the seam and the invariants so klod's side is built
right.

#### Reuse vs. net-new (corrected by review)

The Staff review audited the reuse claim against second-brain code; the split is:

- **Genuinely reusable (verified):** oracle `knowledge_search` / `note_read`; the RRF/vector
  pipeline and `VaultWatcher` markdown indexing; the `trace` fields promoted into the index. The
  retrieval surface is real and does not need rebuilding. (Note again that oracle's default no-mode
  search is vector-first with BM25 off unless configured - "reuse hybrid" means *configuring* it.)
- **Net-new work, not reuse (must be built):** there is **no `IngestKind::ClaudeSession`** (current
  kinds: article/github/youtube/thread/image/voice/idea/vocab), **no session-atom distiller**
  (existing distillers are article/idea/repo/thread/video/voicenote/etc.), and **no session-specific
  `NoteType`/frontmatter schema**. Phase 3 is therefore *new distiller + new ingest kind + new note
  type riding existing retrieval*, not "wire klod into the existing pipeline." The catalog doc's
  framing undersold this; the second-brain Phase 3 doc must scope it as new construction.

#### Write path & ownership

The strict repo invariant holds: nothing in borg opens oracle's DB; cortex is the sole embeddings
writer; the two SQLite files never share a writer. So the distiller runs **cortex-side (or as an
`sb` batch verb)** and follows the existing data flow:

```
klod sessions.db + staged transcripts   (klod's output, on desk)
        │  (read-only consumer)
        ▼
cortex/sb distiller:  read JSONL → stage raw → emit vault markdown
        │
        ▼
VaultWatcher → cortex indexes/embeds → oracle MCP recall (knowledge_search / note_read)
```

No direct oracle DB write; no MCP dependency during a background sweep.

#### Atom taxonomy

The distiller emits **only durable output**, never process narration, and may emit **nothing** (a
typo-fix session yields no atoms - this is the volume control):

- **Decisions** - "chose X over Y because Z" (highest value)
- **Facts learned** - durable truths discovered
- **Reusable solutions / gotchas** - including negative results ("tried fastembed, dropped it
  because the CPU lacks AVX2")
- **Artifacts produced** - design docs, PRs, files (link, never duplicate)

Each atom carries `source: claude-session`, a `scope` tag (`work`/`personal`), and a `trace` handle
to the durably staged transcript copy.

#### Dedup / supersession

This is an **in-process cortex concern**, not a call to `find_similar`/`duplicate_groups` (those
are FTS5 term-extraction and frontmatter-state reporting, not semantic dedup). The mechanism is an
open Phase 3 decision: embedding-cosine threshold at index time, or a dedicated cortex pass.

#### What stays parked

The work→personal vault crossing (klod is `tatari-tv`, second-brain is `scottidler`) lands here.
Phase 3 routes work session knowledge into the personal vault, which is the parked work/personal
decision at repo/identity level. This doc flags it; it does not resolve it.

### Phase 4 - Migrate cr / ccu / claude-permit into klod

Fold the three tools in as sibling lib crates over the shared `session` core:

```
klod/
  klod/        thin clap shim (composition root, only crate that prints)
  session/     SHARED CORE (parse, scan, paths, redact)  ← already exists
  sessions/    nav layer (search/ls/open/stage/enrich)   ← already exists
  report/      ← cr's logic (per-host session report)
  cost/        ← ccu's logic (per-session cost)
  permit/      ← claude-permit's logic (permission hygiene)
```

Invocation becomes `klod report`, `klod cost`, `klod permit`. The migration's whole point is that
all four consume **one** parser, so the subagent-rollup contract (parent `<uuid>/subagents/*.jsonl`
rolled into the parent) lives in exactly one place. `cr`'s current rollup semantics are the
reference the `session` parser was built to honor in Phase 1.

**This is not a drop-in (correction from review).** The Staff review checked the parsers: `session`'s
`ParsedSession` carries no token usage, no request/message IDs, and no per-model totals or cache
tiers, whereas `ccu` is built on assistant usage fields + cache tiers and `cr` dedupes by
message/request ID and aggregates per model. Replacing those parsers with today's `session` would
**silently change `ccu`/`cr` output**. So Phase 4 has a prerequisite step: **first extend the
`session` core to expose a richer raw-event/usage API** (per-message usage, request/message IDs,
per-model rollups, cache tiers) that is a superset of what all three tools need; *then* migrate each
tool onto it. The migration of each tool's domain logic into a lib crate is mechanical once the core
exposes the data; building that richer core is not. User-facing behavior is preserved and proven by
golden-output comparison (below); the old standalone binaries can alias to the new subcommands during
transition, and the original tools stay untouched on `main` until each migration is proven, then the
standalone repo is archived (not deleted).

## Implementation Plan

Phases carry a `Model:` annotation per the create-design-doc template / `rwl-a-plan`.

#### Phase 2 - Enrichment
**Model:** opus
- **Scope gate (load-bearing, build first):** `session::scope::classify(cwd) -> Scope` from the repo
  identity convention, defaulting unknown to `personal`. Enforce the routing invariant - only
  `work`-scoped sessions reach the work account; personal sessions are skipped (`status =
  skipped-personal`). Unit-test the classifier and the invariant.
- **Enrichment state:** add the `scope`/`enriched_at`/`enriched_modified`/`enrich_model`/
  `prompt_version`/`status`/`last_error`/`attempts`/`redaction_count`/`tokens_*` state (columns or
  sidecar), with a guarded idempotent migration (mirrors the Phase 1.5 v2 `ALTER` pattern). Add the
  transactional **`set_enrichment(...)`** writer (row + high-signal FTS in one tx) - **not**
  `upsert_session`, which preserves these columns by design.
- `session::redact::scrub` - chokepoint stripping high-confidence secret shapes (API keys,
  bearer/AWS tokens, PEM blocks), returns `(redacted, count)`. Defense-in-depth, not the boundary.
- Parse-from-staged read path so archived sessions enrich from their staged copy.
- Anthropic Haiku client (`cargo add` latest, key from env, never inlined): one call per
  work-scoped session, body = parsed high-signal text (capped, head+tail on overflow), structured
  response `{tags, summary}`. Pin `enrich_model` + `prompt_version`.
- `klod sessions enrich` verb: selection predicate from the state table; `--all`, `--dormant-after`,
  `<id>`, `--dry-run [--show-payload]`. Re-entrant on grown sessions; manual tags preserved unless
  overridden.
- **Failure handling + observability are part of Phase 2, not deferred:** bounded retries with
  backoff (via `attempts`), rate-limit handling, a per-run token/cost budget, and a sweep exit
  summary (enriched / skipped-personal / skipped-empty / failed counts, redactions, tokens, cost).
  A `klod sessions doctor` (or status line) surfaces last-successful-sweep and stale rows.
- Function-level DEBUG logging per the repo rule (entry with session id + sizes; scope decision;
  redaction count; API outcome). Never log the prompt body or API key - length/preview only.
- **Gate:** scope classification + the routing invariant must be implemented and tested before this
  ships (the scrub policy rides along but is not the gate).

#### Phase 3 - Knowledge layer (second-brain repo, deferred)
**Model:** opus
- *In second-brain, own doc.* cortex-side / `sb` batch verb consumes klod's `sessions.db` + staged
  transcripts → stage raw → vault markdown → VaultWatcher → cortex index/embed.
- Atom taxonomy; in-process dedup + supersession; durable staging with the borg-retention
  exception; `scope` tagging.
- Resolves (or escalates) the work→personal vault crossing before any work atom is written.

#### Phase 4 - Migrate cr / ccu / claude-permit
**Model:** opus for the `session` core extension (the hard part), sonnet for the per-tool lifts
- **Prerequisite:** extend the `session` core to expose a richer raw-event/usage API - per-message
  token usage, request/message IDs, per-model rollups, cache tiers - a superset of what `cr`/`ccu`/
  `permit` need. Without this, migrating onto today's `ParsedSession` silently changes `ccu`/`cr`
  output.
- Lift `cr` → `report`, `ccu` → `cost`, `claude-permit` → `permit` lib crates over the extended
  `session`.
- Verify each migrated crate's output matches the standalone tool byte-for-byte on the live corpus
  (subagent-rollup, per-model usage, request-id dedup) before cutover.
- Wire subcommands into the `klod` shim; alias old binaries during transition; archive standalone
  repos after each is proven.

## Alternatives Considered

### Phase 2 execution: Stop-hook
- **Description:** Claude Code fires `klod` as a hook subprocess at turn end, on the active machine.
- **Pros:** enriches sessions promptly; no cron needed; tags available during the session's life.
- **Cons:** spreads the Anthropic key and off-machine API surface across every host (desk, mini,
  lappy); needs the hook configured everywhere; debounce/re-entrancy complexity; mid-session signal
  is weaker than a complete session.
- **Why not chosen:** key custody and signal both favor the sweep; the catalog is read after the
  fact, so there is no benefit to in-session tagging.

### Phase 2 redaction: ship raw (no scrub)
- **Description:** send transcript text to Haiku unmodified, accepting that it is work data going to
  Tatari's own Anthropic endpoint under the work key.
- **Pros:** simplest; same trust boundary `cr` titling already crosses.
- **Cons:** a live credential captured in a transcript would be shipped verbatim; no defense in
  depth.
- **Why not chosen:** the marginal cost of a regex scrub chokepoint is low and it removes the worst
  case; defaulting to scrub keeps the gate honest.

### Phase 2 scope handling: park it to Phase 3 (the original draft)
- **Description:** classify work/personal only at Phase 3 vault ingestion; let Phase 2 enrich
  everything under the work key.
- **Pros:** simplest Phase 2; one fewer thing to build now.
- **Cons:** incoherent - a sweep against a scope-bound (work) endpoint *is* a scope decision, made
  implicitly in the riskiest direction; it ships personal content to the work account and violates
  this doc's own Non-Goal. Both external reviews flagged it as the #1 issue.
- **Why not chosen:** scope classification is cheap (a pure function of stored `cwd`) and is the
  actual trust boundary; it must precede the first off-machine call. *Full routing* (e.g. enriching
  personal sessions under a personal key, work→personal vault rules) still lives later, but
  classification + the "work-key, work-scope-only" invariant are now Phase 2.

### Phase 4: leave cr/ccu/permit standalone
- **Description:** keep three parsers, three repos.
- **Pros:** zero migration risk now.
- **Cons:** divergent truth (subagent rollup), triplicated maintenance, defeats the seam the
  catalog doc built Phase 1 around.
- **Why not chosen:** the seam exists precisely to retire the divergence; the migration is the
  payoff, sequenced last so it carries no blast radius on the working tools.

## Technical Considerations

### Dependencies
Phase 2 adds an Anthropic client dep (via `cargo add`, latest) to `session`/`sessions`; everything
else reuses Phase 1 deps. Phase 3 lives in second-brain and reuses its candle/cortex/oracle crates
- klod does not depend on them. Phase 4 adds no new deps; it consolidates existing ones.

### Performance
385 sessions × one Haiku call each is a trivial one-time backlog; incremental thereafter (only
dormant, un-enriched, or grown sessions). The sweep is I/O- and network-bound, not CPU-bound; no
AVX2 concern. Cost is bounded by Haiku pricing × session count and is the operator's to watch
(Open Q on observability).

### Security
- Phase 2 is the first off-machine boundary. The load-bearing control is the **scope gate**:
  classify work/personal from `cwd` before building any payload, and send only work-scoped content
  to the work account (invariant: no personal content reaches the work account). The secret scrub is
  defense-in-depth beneath it, not the boundary; the true boundary is the endpoint's data-retention
  posture plus the scope filter. The key is read from env, never inlined, never logged.
- Phases 1/1.5 carry no off-machine risk; Phase 4 carries none beyond what `cr`/`ccu` already do.
- Phase 3 carries the work→personal crossing (work-session knowledge into the personal vault);
  parked and flagged, with the enforcement primitive (`scope`, now born in Phase 2) ready for it.

### Edge Cases
- **Empty / aborted session** (no user+assistant text body): skip enrichment, leave `tags`/`summary`
  NULL; nothing to summarize and an LLM call would burn cost for noise.
- **Haiku call fails** (network, rate limit, 5xx, malformed response): record `status=failed` +
  `last_error`, increment `attempts`, never abort the sweep. Retry on a later run with backoff up to
  a max-attempts cap (not "every cron forever" - the Staff review's catch); a bad/unparseable
  structured response is a skip, not a partial write. The per-run token/cost budget halts the sweep
  early if exceeded.
- **Personal-scoped session:** never sent to the work account; recorded `status=skipped-personal`,
  left un-enriched. This is the routing invariant, not a failure.
- **Archived (reaped) session with a staged copy:** enrich reads the staged transcript under
  `<xdg-data>/klod/staged/<id>/`, so a session past the 30-day TTL is still enrichable from its
  Phase 1.5 copy. Archived with no staged copy: not enrichable, skip-and-log.
- **Session grown since last enrichment** (resumed): re-staged and re-enriched; `tags`/`summary`
  overwritten with the fuller view (re-entrancy).
- **Redaction false positive** stripping real content: acceptable - the redacted text only feeds
  tag/summary inference, never replaces the stored body; over-scrubbing degrades a tag, it does not
  corrupt the record.

### Testing Strategy
- `session::scope::classify` + the routing invariant - **the safety-critical unit.** Table tests
  over work/personal/unknown `cwd` shapes; an explicit test asserting no `personal`-scoped session
  is ever handed to the work-account send path (the invariant, tested directly, not just inferred).
- `session::redact::scrub` - unit tests over known secret shapes (real-format synthetic tokens) and
  false-positive guards.
- Enrich - `--dry-run` over the live corpus to inspect per-session gate decisions before the first
  real call; mock the Haiku client for deterministic tag/summary parsing and failure/backoff tests.
- Phase 4 - golden-output comparison: each migrated crate's output (subagent-rollup, per-model
  usage, request-id dedup) must match the standalone tool byte-for-byte on the live corpus before
  cutover.

### Rollout
Phase 2 ships once the redaction decision is made; reversible (drop the cron entry, columns stay
NULL-tolerant). Phase 3 is independent and in another repo. Phase 4 cuts over one tool at a time
with the old binary aliased and the standalone repo archived only after proof.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| **Personal session shipped to work account** | Med | High | Scope gate before payload build; routing invariant (work-key enriches work-scope only); unknown defaults to personal; classifier unit-tested |
| Live credential shipped to Haiku | Med | High | Scope gate (primary); `session::redact::scrub` chokepoint (defense-in-depth); `--dry-run` decisions preview; unit-tested secret shapes |
| Failed enrich retries forever / cost runaway | Med | Med | `attempts`+backoff+max-attempts in enrichment state; per-run token/cost budget halts sweep; incremental selection predicate |
| Work key spread across hosts | Low | Med | Sweep-on-desk only; key never on mini/lappy for klod |
| **Non-desk sessions silently second-class** | High | Med | Accepted v1 constraint (desk-originated coverage); host-sync seam named in Open Questions, not left implicit |
| Phase 4 silent output drift in cr/ccu | High | High | Extend `session` core (usage/IDs/per-model) *before* migrating; byte-for-byte golden-output match on live corpus; old binaries aliased, repos archived not deleted |
| Work→personal vault leakage (Phase 3) | Med | High | `scope` enforcement primitive born in Phase 2; Phase 3 resolves routing before first work atom; Phases 2/4 don't cross it |
| Enrich quality (vague tags/summary) | Med | Low | Re-entrant refinement on grown sessions; `prompt_version` bump re-enriches; navigational record + staged copy never depend on it |

## Open Questions

- [ ] **Scope classification correctness (gates Phase 2):** is `cwd`/`project_dir` → work/personal
  sufficient, and is "unknown ⇒ personal (skip)" the right fail-safe? Are there work sessions run
  outside `tatari-tv/` paths that would be wrongly skipped?
- [ ] **Phase 2 account/identity (gates Phase 2):** the work key is recommended (`cr` precedent);
  confirm it, and confirm Tatari's Anthropic data-retention posture is acceptable for work session
  content (this, not the regex, is the real boundary). Is a separate personal key for personal
  sessions ever wanted, or do they stay un-enriched?
- [ ] **Redaction depth:** which exact secret shapes are in scope for the v1 scrub? (Secondary to
  the scope gate, but still needs a concrete list.)
- [ ] **Manual-tag overwrite:** should enrichment overwrite manually-set tags, or only fill
  `summary` + state? (Draft default: preserve manual tags.)
- [ ] **Sweep trigger/cadence (shared with Phase 1.5 Open Q2/Q6):** cron on desk, idle-daemon, or
  both, and at what schedule? Enrich and stage want the same trigger.
- [ ] **Multi-host coverage:** accept desk-only coverage for v1, or design the host-sync/merge
  (rsync raw JSONL to desk before the sweep, vs. multi-host `sessions.db` merge) that unblocks
  mini/lappy enrichment and Phase 4's `klod report` on non-desk hosts?
- [ ] **Phase 3 dedup mechanism:** embedding-cosine threshold at index time vs. dedicated cortex
  pass (carried from catalog doc Open Q5).
- [ ] **Work→personal vault crossing (Phase 3):** the parked repo/identity decision; is the crossing
  allowed at all, and what audit/rollback removes a wrongly-ingested work atom?
- [ ] **Phase 4 core API shape:** what exact raw-event/usage surface must `session` expose to be a
  superset of `cr`/`ccu`/`permit` needs (per-message usage, request/message IDs, per-model rollups,
  cache tiers)? Which tool migrates first once it exists (smallest blast radius - likely `ccu`)?

## References

- `docs/design/2026-06-21-session-knowledge-catalog.md` (parent design; Phases 0/1/1.5 shipped)
- `docs/design/2026-06-21-session-knowledge-catalog-implementation-notes.md` (what shipped)
- `cr` (claude-report; titling precedent for the off-machine Haiku call), `ccu`, `claude-permit`
- second-brain: `oracle` (MCP, BM25+vector+RRF), `cortex` (tagging/dedup/embeddings), `borg`
  (staged-source/trace), Candle `bge-small-en-v1.5`
- `reference-desk-cpu-no-avx2` (why Phase 3 local ML is Candle-only; Phase 2 has no such concern)
