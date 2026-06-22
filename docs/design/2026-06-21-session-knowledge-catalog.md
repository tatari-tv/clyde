# Design Document: Claude Session Knowledge Catalog

**Author:** Scott Idler
**Date:** 2026-06-21
**Status:** Implemented (Phases 0, 1, 1.5; Phases 2-4 deferred per the plan)
**Review Passes Completed:** 0/5 author self-review (Rule of Five not yet run) — external design review folded in: Architect (Gemini) + Staff Engineer (Codex), 2026-06-21, incl. a 2nd round adjudicating Risk C

## Summary

Catalog Claude Code sessions so they can be **found, searched, resumed, and eventually
recalled as knowledge**. Today sessions are addressable only by slugified-CWD plus a UUID
filename, so a topic lives *inside* a file whose path reveals nothing — finding "that session
where I set up the Marquee S3 bucket" means grepping 800 transcripts by hand.

The system is **two layers, split along the line between *navigation* and *knowledge*:**

1. **Navigational layer (build first, safe, local):** a thin per-session index + a small CLI
   (`search` / `ls` / `open`) over a local SQLite store. Answers *"find/resume the session
   where I…"*. Touches no vault, no LLM-as-knowledge, no work/personal line.
2. **Knowledge layer (deferred until comfortable):** a borg distiller that turns *dormant*
   sessions into discrete **knowledge atoms** (decisions, facts, gotchas) served through
   oracle's existing MCP. Answers *"what did I decide about X"*. This is where every hard
   line-threading question lives, so it is deliberately postponed without blocking layer 1.

The retrieval engine is **not new** — second-brain already runs BM25 + Candle `bge-small-en-v1.5`
+ RRF hybrid on this exact (AVX2-less) CPU. The knowledge layer reuses it rather than
reinventing it.

**Home — `klod`.** The navigational layer ships as the first subcommand of **`klod`**
(`github.com/tatari-tv/klod`), a new `claude-*`-family Cargo workspace built second-brain-style:
a thin `clap` `main.rs` shim over subcommand **lib crates**. `cr`, `ccu`, and `claude-permit`
migrate into `klod` later (separate design doc). The knowledge layer is a *downstream consumer*
living in second-brain (borg/cortex), a different repo — **klod produces, second-brain consumes.**

## Problem Statement

### Background

Claude Code stores each session as a JSONL transcript at
`~/.claude/projects/<slugified-cwd>/<session-id>.jsonl`. On this machine: **97 project
directories, ~801 JSONL files, of which 385 are real top-level sessions** (the rest are
`agent-*` subagent transcripts). Claude auto-generates a per-session title (`ai-title` line,
written ~turn 2-3); **96% of real sessions have one**.

Scott already owns tooling that parses this corpus: `cr` (per-host session report), `ccu`
(per-session cost). second-brain (`borg`/`cortex`/`oracle`/`sb`) is a Rust workspace that
ingests sources into an Obsidian vault and serves them via an MCP knowledge server.

### Problem

Sessions cannot be located by name, topic, content, or tag. There is no catalog, no search,
and no way to treat the accumulated reasoning in past sessions as retrievable knowledge.

### Goals

- Find, search, and resume sessions by **name (slug), session-id, content, topic, and tags**.
- Name sessions early in their life (Claude's `ai-title` largely already does this).
- Eventually expose sessions as **knowledge Claude itself can query** via oracle's MCP.
- **Reuse existing infra**: `cr`/`ccu` parsing, oracle retrieval (bge-small + BM25 + RRF),
  cortex tagging, borg staged-source/trace provenance.
- Adopt a **single, XDG-correct shared data location** for derived artifacts.

### Non-Goals (for now)

- **Secrets redaction** — parked; re-address if it becomes an issue. (Knowledge-layer concern only.)
- **Work/personal vault split** — intermix for now; tag notes so a future split is a clean filter.
- **Renaming Claude's sessions / fighting the `/resume` picker** — our catalog owns its own
  search; the picker's auto-name is fine.
- **Multi-host merge** — single host (desk) for now; schema reserves a `host` column.
- **A from-scratch FTS-vs-vector experiment** — oracle's `sb oracle eval` already settled
  bge-small + hybrid as eval-best.

## Key Findings (ground truth, verified on disk)

These corrected two earlier assumptions and must not be re-litigated:

1. **`sessions-index.json` is NOT a usable backbone.** Claude maintains a per-project index,
   but it is **stale and disjoint**: in `-home-saidler` it lists 146 sessions (transcripts
   already cleaned up) with **zero overlap** against the 79 current JSONL files, and it exists
   for only 26 of 97 project dirs. **The JSONL is the sole trustworthy source.** (96% carry
   `ai-title`; the 4% without fall back to first user prompt.)
2. **Transcripts have a 30-day TTL** (`cleanupPeriodDays`). They are Claude-owned and
   self-deleting — copying them wholesale fights that lifecycle.
3. **The retrieval stack already runs on this CPU.** second-brain uses **Candle** (pure-Rust,
   compiled-from-source) for `bge-small-en-v1.5` (384-dim, L2-normalized, CLS-pooled), *not*
   ONNX/`fastembed`. This matters because **this CPU has AVX + SSE4.2 but no AVX2/FMA** — a
   Python/torch/onnx embedding path would hit "illegal instruction"; Candle does not.
   `cortex.service` is active here and `~/.local/share/oracle/oracle.db` is live.
4. **cortex already solves tagging** — `canonical-tags.yml` + `tag-mapping` + `tag-proposals`
   (`max-per-note: 7`): an evolving controlled vocabulary, exactly what high-quality tag search needs.
5. **borg already solves durable provenance** — staged-source assets + oracle `trace` handles.

## Proposed Solution

### Overview

```
                          ~/.claude/projects/**/<uuid>.jsonl   (Claude-owned, 30-day TTL)
                                        │  (referenced, never relocated)
            ┌───────────────────────────┴───────────────────────────┐
            ▼                                                         ▼
  NAVIGATIONAL LAYER (phase 1)                          KNOWLEDGE LAYER (phase 3, deferred)
  thin per-session record                               borg distiller on DORMANT sessions
  sessions.db (SQLite + FTS5)                           → knowledge atoms (decisions/facts/…)
  CLI: search / ls / open                               → oracle notes (source:claude-session,
  "find / resume my session"                              scope tag, trace → staged copy)
                                                         → dedup (find_similar), supersession
                                                         MCP recall = oracle's existing surface
```

### Workspace & crates (`klod`)

A new Cargo workspace at `~/repos/tatari-tv/klod` (`clone` worktree layout: `.bare` + `main/`),
edition 2024, mirroring second-brain's shape — a thin binary over lib-only crates:

```
klod/                 workspace root
  klod/               thin clap main.rs shim (the only bin; composition root, the only crate that prints)
  sessions/           lib crate — navigational layer: index, FTS5, search/ls/open   ← built first
  session/            lib crate — SHARED CORE: locate ~/.claude/projects, parse the JSONL
                      into a typed model (messages, ai-title, usage, tool-calls, subagent
                      rollup), + klod path resolution. This is the integration seam.
```

Invocation is `klod sessions <verb>`. Only `klod` prints; lib crates return typed data (the
second-brain "libraries are lib-only" invariant). `cr`/`ccu`/`claude-permit` later become sibling
lib crates (`report`/`cost`/`permit`) over the same `session` core — that migration is a
**separate design doc**, out of scope here.

### Architecture

**Navigational layer.** One lightweight record per session: `session_id`, `cwd`,
`project_dir`, `title` (ai-title, else first-prompt), `first_prompt`, `abstract`, `tags`,
`git_branch`, `model`, `n_msgs`, `created`, `modified`, `cost`, `host`, `archived`. Stored in a
SQLite db with **two** FTS5 tables: a high-signal one (title + tags + abstract) for *ranking*,
and a body table for *content recall* — see Retrieval Decision. The record references the live
transcript path; if Claude has cleaned it (>30d) the row is flagged `archived` (its Phase-1.5
staged copy, if any, still resolves).

`sessions.db` follows the same DB discipline second-brain uses (`vault/src/search.rs`,
`borg/src/receipts.rs`): WAL, busy-timeout, foreign keys, a **single-writer model**, schema
versioning + migrations, and full rebuildability from the JSONL source (corruption recovery =
delete and reindex). Path resolution lives in `session::paths` (klod's analog to
`vault::paths`) — **never** `dirs::*_dir()` with a `~/` fallback; `.expect()` if `HOME`/`XDG_*`
are unset, per the repo rule that a fabricated `~/` path silently creates a literal `~` dir.

**Knowledge layer (deferred).** A borg distiller reads a session once it is *dormant* and emits
**only durable output**, never process narration:

- **Decisions** — "chose X over Y because Z" (highest value)
- **Facts learned** — durable truths discovered
- **Reusable solutions / gotchas** — including **negative results** ("tried fastembed, dropped
  it because the CPU lacks AVX2")
- **Artifacts produced** — design docs, PRs, files (link, never duplicate)

The distiller may emit **nothing** (a typo-fix session yields no atoms) — this is the volume
control. Each atom carries `source: claude-session`, a `scope` tag (`work`/`personal`), and a
`trace` handle to a **durably staged copy** of the transcript (see Phase 1.5 + the retention
exception below).

**Write path & ownership (resolves both reviews' top finding).** The repo invariant is strict:
*nothing in borg opens oracle's DB; cortex is the sole embeddings writer; the two SQLite files
never share a writer* (`CLAUDE.md`). So the distiller does **not** live in borg's pipeline
reaching into `oracle.db`. It runs **cortex-side (or as an `sb` batch verb)** — cortex already
legitimately owns embeddings and the oracle index — and follows the existing data flow:
read JSONL → stage raw transcript → **emit vault markdown** → VaultWatcher → cortex indexes/embeds.
No direct oracle DB write; no dependency on a live MCP server during a background sweep.

**Dedup/supersession** is therefore an **in-process cortex concern**, not a call to
`find_similar`/`duplicate_groups` as originally written — those are FTS5 term-extraction and
`duplicate_group` frontmatter-state reporting respectively, not semantic dedup. The real
mechanism (embedding-cosine threshold at index time, or a dedicated cortex pass) is an open
Phase-3 decision. Recall remains oracle's existing `knowledge_search`/`note_read`/`find_similar`
— no new MCP surface required.

### Naming & Tagging

- **Naming** is effectively free: Claude's `ai-title` lands ~turn 2-3 and covers 96% of sessions.
- **Early tagging** (phase 2): a debounced `Stop` hook fires a cheap Haiku pass over the first
  few exchanges (highest-signal input) and writes tags into `sessions.db`. Non-blocking,
  debounced, re-entrant (tags refine as the session grows). These are **klod's own search tags**,
  deliberately *independent* of cortex's `canonical-tags.yml` — klod never reads that file, so
  there is no cross-repo vocabulary-drift coupling. Canonical/vault tagging happens **cortex-side
  at knowledge ingestion** (Phase 3), where `canonical-tags` already lives and is version-locked
  to cortex. (This independence is what dissolved the standalone-vs-`sb` tension — see Open Q3.)

### Retrieval Decision

Empirically, **full-body FTS ranks poorly** on long transcripts: a 120K-char session mentions
everything, so BM25 term-frequency washes out (observed: `gated repo tagging release` returned
flat `-0.0` scores; the right session buried at #4). Therefore *ranking* uses a **high-signal
projection** (title + tags + abstract), while a **separate body-FTS table is retained for
content recall** — so a term appearing only mid-transcript (e.g. "Marquee S3 bucket", never in
the title) is still findable, just ranked below high-signal hits. Dropping body FTS entirely
would have failed the core "search by content" user story (review catch); the `abstract` is
produced by the Phase 2 enrich pass. The knowledge layer rides oracle's retrieval as-is — but
note its default `knowledge_search` is **vector-first with BM25 off by default**, so "reuse
hybrid" means *configuring* the hybrid mode, not assuming it.

### Data Model & Layout (XDG)

Derived artifacts live under one family namespace (name TBD — candidates `cct` / `ccx` /
`claude-tools`), resolved via the `dirs` crate (respects `XDG_*_HOME`, consistent with sb):

```
$XDG_DATA_HOME/<family>/        # ~/.local/share/<family>/  — authoritative
    sessions.db                 #   shared index — THE integration contract
    reports/                    #   cr output lands here (no more CWD clobber)
$XDG_CACHE_HOME/<family>/       # ~/.cache/<family>/  — regenerable parse/cost caches (rm-safe)
$XDG_CONFIG_HOME/<family>/      # ~/.config/<family>/ — shared config (mirrors ~/.config/sb)
```

Raw transcripts are **never** placed here — they stay Claude-owned in `~/.claude/projects` and
are referenced; the knowledge layer stages a durable copy only for atoms it actually creates.

### API Design (navigational CLI)

```
klod sessions search "terraform marquee"   # FTS over high-signal fields, ranked
klod sessions ls --repo loopr --since 7d    # metadata filters (project/repo/date/tag/model)
klod sessions open <id-or-fuzzy>            # prints the `claude --resume <uuid>` line (no auto-launch)
klod sessions tag <id> <tags…>              # manual tag override
klod sessions reindex                       # incremental, mtime-skip
```
(Per the CLI rules: space-separated or repeated multi-value flags, never comma-separated.)

## Implementation Plan

Phases carry a `Model:` annotation (per the create-design-doc template / `rwl-a-plan`).

#### Phase 0 — cr timestamped output (independent, no klod dependency)
**Model:** sonnet
- Change `cr collect` default `-o` from CWD to a timestamped
  `~/.local/share/claude-report/claude-report-%Y-%m-%d-%H%M%S.json` (rkvr's `chrono::Local`
  format), so re-runs don't clobber. Ships anytime, independent of the klod workspace.

#### Phase 1 — klod workspace + `sessions` navigational layer  ← first real build
**Model:** opus for the workspace/seam + parser design, sonnet for CLI wiring
- Scaffold the `klod` workspace: `klod` bin (clap shim) + `session` (shared core) + `sessions`
  lib crates; edition 2024; `.otto.yml`; `whitespace -r` in lint; `build.rs` GIT_DESCRIBE in the
  bin only.
- `session`: locate `~/.claude/projects`, parse the JSONL → typed model (reconcile the
  `agent-*` / subagent-rollup contract with `cr`), plus `session::paths`.
- `sessions`: ingest → `sessions.db` (high-signal + body FTS5) → `search`/`ls`/`open`;
  incremental reindex (lazy-on-query + optional debounced `Stop` hook pre-warm).
- *Delivers the original "I forget where my sessions are" need.*

#### Phase 1.5 — raw-transcript staging (TTL insurance)
**Model:** sonnet
- On dormancy, stage a durable copy of the transcript, **decoupled from distillation**, to beat
  the 30-day TTL before Phase 3 exists. Lives in `sessions`/`session`; commits to none of the
  knowledge-layer questions.

#### Phase 2 — early enrichment (tags + abstract)
**Model:** opus
- Debounced `Stop` hook → cheap Haiku tag/abstract pass; klod's own tag vocabulary (not
  canonical-tags); re-entrant refinement. **Gate:** this ships content off-machine, so the
  scope/redaction decision (parked) is the precondition *here*, before the first LLM call.

#### Phase 3 — knowledge layer (in second-brain, deferred)
**Model:** opus
- In **second-brain** (not klod): a cortex-side / `sb` batch verb consumes klod's
  `sessions.db` / staged transcripts → stage raw → vault markdown → VaultWatcher → cortex
  index/embed. Atom taxonomy; in-process dedup + supersession; durable staging with a
  borg-retention exception. *All secrets/work-personal + the work→personal-vault crossing live here.*

#### Phase 4 — migrate `cr`/`ccu`/`claude-permit` into klod (separate design doc)
**Model:** sonnet
- Fold the existing tools in as `report`/`cost`/`permit` lib crates over the shared `session`
  core. Out of scope here; called out so the seam is designed for it from day one. (`cr` already
  rolls nested `<uuid>/subagents/*.jsonl` up into the parent — `session`'s parser must honor
  that contract, which is why the seam is designed in Phase 1, not retrofitted.)

## Alternatives Considered

- **`sessions-index.json` as the structural backbone** — *rejected:* stale, disjoint from
  reality, partial coverage. JSONL is the only trustworthy source.
- **Python/torch/onnx local embeddings** — *rejected:* this CPU lacks AVX2/FMA; prebuilt ML
  binaries risk "illegal instruction." Candle (compiled from source) is the proven path.
- **Fold the navigational tool into `sb`/second-brain** — *rejected:* `sb` is a personal
  knowledge daemon (`scottidler`); a `claude-*` session tool is a different family and a different
  identity (work, `tatari-tv`). Chosen instead: a new `klod` workspace in the `claude-*` family.
  We *do* adopt the subcommand-tree/workspace shape — just as a fresh `klod`, deferring migration
  of the working `cr`/`ccu`/`claude-permit` so there's no blast radius on them now.
- **Standalone copy of the embedding/RRF recipe** — *rejected:* duplicates oracle for no benefit.
- **Whole-session note in the vault** — *rejected:* mushy retrieval and vault pollution; distill
  atoms, not transcripts.
- **ripgrep over JSONL** — *rejected:* no ranking, no tags, no metadata filters, degrades with corpus growth.

## Technical Considerations

### Dependencies
klod (Phase 1): `rusqlite` (bundled SQLite + FTS5), `chrono`, `clap`, `serde`/`serde_json`,
`walkdir`. **No `dirs`** with `~/` fallbacks — own `session::paths` with `.expect()`
discipline. Add deps via `cargo add` (latest), never from memory. Phase 3 lives in second-brain
and reuses its candle/cortex/oracle crates — klod does not depend on them.

### Performance
385 sessions is trivial for SQLite; corpus body ≈ 62M chars (~15.6M tokens). Reindex is
incremental via `fileMtime` skip. Brute-force cosine (oracle's existing approach) needs no ANN index.

### Security
Secrets handling is a **non-goal for now** (explicit). Sessions contain tokens and internal
Tatari data. **Correction from review:** the redaction/scope gate belongs *before the first LLM
call* — i.e. before **Phase 2's** Haiku tag/abstract pass, which ships content off-machine — not
only at Phase 3 vault ingestion. Phases 1 and 1.5 read only local data and never leave the
machine; Phase 2 onward do, and are gated on the parked redaction decision.

### Edge Cases
- **Subagent transcripts:** the prototype excludes top-level `agent-*` files, but `cr` instead
  scans nested `<uuid>/subagents/*.jsonl` and *rolls them up* into the parent. The parser
  contract must be reconciled with `cr`'s semantics before any shared-core work.
- **Parser robustness:** partial/last-line JSON, malformed lines, nested tool-result content,
  and Claude JSONL schema drift must be handled (skip-and-log, never crash the reindex).
- 4% of sessions lack `ai-title` → first-prompt fallback.
- Transcripts cleaned by the 30-day TTL → row flagged `archived`; if Phase 1.5 staged a copy,
  open/trace still resolve, else they report "transcript reaped".
- Resumed sessions are *assumed* to retain `session_id` (one growing row) — **unverified**; `cr`
  groups by file/path semantics, so this needs confirming.

### Rollout
Phase 0 and Phase 1 ship independently and reversibly. `cr`/`ccu` are untouched until Phase 4,
and even then only gain an opt-in fast path.

## Risks and Mitigations

- **30-day TTL race (review catch, high):** deferring Phase 3 means dormant sessions age out and
  Claude reaps the transcripts *before* durable staging exists — the backlog is silently lost.
  *Mitigate:* Phase 1.5 raw-transcript staging, decoupled from and shipped ahead of distillation.
- **Ownership-invariant violation (both reviews' #1):** a borg-pipeline distiller cannot open
  `oracle.db`. *Mitigate:* distiller lives cortex/`sb`-side and writes via vault markdown →
  watcher; never a second writer to oracle's DB, never an MCP dependency in a background sweep.
- **Secret/work-sensitivity leakage** — *mitigate:* redaction/scope gate before the **Phase 2**
  LLM call (corrected), not just Phase 3; Phases 1 and 1.5 carry no such risk.
- **Vault pollution** — *mitigate:* distill atoms not transcripts; in-process cortex dedup;
  `source` tagging; knowledge layer deferred.
- **Distillation quality (deep, inherent):** an LLM sweep emitting "nothing" for a subtle
  decision is silent knowledge loss. *Mitigate:* atoms augment the always-present navigational
  record + staged transcript; they are never the sole record.
- **Work→personal identity crossing (new, surfaced by klod):** klod is `tatari-tv` (work),
  second-brain is `scottidler` (personal); Phase 3 would route work session knowledge into the
  personal vault. *Mitigate:* parked with the rest of the work/personal decision (Open Q3/Q4);
  Phases 0–2 don't cross it. Flagged so it isn't discovered late.
- **Disturbing working tools** — *mitigate:* greenfield-first; `cr`/`ccu` opt in later.

## Open Questions

1. ~~Family name / home~~ — **RESOLVED:** workspace `klod` (`tatari-tv/klod`), XDG namespace
   `klod` (`~/.local/share/klod/`, `~/.config/klod/`, `~/.cache/klod/`); `claude-*` family, not `sb`.
2. **Dormancy trigger + threshold** — idle-time heuristic, end-of-day cron, or both, and the
   exact threshold? (Preference: dormant/EOD sweep, not live.) Given the 30-day TTL, an
   unreliable trigger means missing the staging/extraction window — so this gates Phase 1.5 too.
3. ~~Standalone vs. fold into `sb`~~ — **RESOLVED:** standalone `klod` workspace. The
   canonical-tags drift risk is moot — klod's search tags are independent of `canonical-tags.yml`
   (Naming & Tagging); canonical tagging happens cortex-side at Phase 3. *New, surfaced by this:*
   klod is *work* (`tatari-tv`) and second-brain is *personal* (`scottidler`), so Phase 3 routes
   work session knowledge into the personal vault — the parked work/personal concern is now a
   repo/identity-level crossing, not just a tag. Still parked, but flagged (see Risks).
4. How is the `scope` (`work`/`personal`) tag set — auto-inferred from repo/CWD, or manual?
5. **Dedup mechanism** for Phase 3 (since `find_similar`/`duplicate_groups` aren't semantic
   dedup): embedding-cosine threshold at index time, or a dedicated cortex pass?
6. **Observability/doctor:** reindex counts, hook failures, stale rows, tag-pass latency/cost,
   last successful scan — integrate with `sb doctor`/status?
7. **Phase 3 rollback:** how to remove/supersede a wrong atom and undo accidental work/personal
   leakage, then safely reindex oracle?
8. ~~Shared-core crate name~~ — **RESOLVED:** `session` (the shared core); `sessions` (plural) is
   the subcommand crate that builds on it. Singular core / plural subcommand is intentional.
9. ~~Doc home~~ — **RESOLVED:** moved to `tatari-tv/klod/main/docs/design/` (the project's home).
   The Phase 3 second-brain side gets its own future doc.

## Review Notes — Pass 1 (Architect + Staff Engineer, 2026-06-21)

Both reviewers verified the reuse claims against the live repo (oracle's MCP surface, cortex
canonical-tags, borg staged-source, Candle/no-AVX2) — the design is **not built on phantom
infrastructure**. They **independently converged on the same #1 issue:** a borg-orchestrated
distiller would open `oracle.db`, violating the strict "two SQLite files never share a writer"
invariant. Resolution adopted: the distiller lives **cortex/`sb`-side** and writes via vault
markdown → watcher, which also makes dedup an in-process cortex concern.

Incorporated this pass: (A) write-path/ownership fix; (B) redaction gate moved before the Phase 2
LLM call; (C) **new Phase 1.5** raw-transcript staging to beat the 30-day TTL on the backlog
(architect catch); (D) corrected "reuse hybrid" (oracle defaults vector-first, BM25 off);
(E) restored body-FTS for content recall + added `abstract`; (F) `sessions.db` writer/PRAGMA/
migration discipline; (G) `vault::paths` over `dirs`; (H) `cr` subagent-rollup parser contract.

Pushed back on Risk C (cross-workspace coupling) and ran a second round with the Architect.
**Adjudicated outcome:** the *file-sharing* half is withdrawn — `~/.config/sb/canonical-tags.yml`
is shared config by construction (verified: `include_str!`'d into `sb`, written out by
`sb bootstrap`, and *both* borg and cortex refuse to start without it), so a Phase-2 tagger
reading it matches the established pattern. The *drift* half holds but is **reclassified as a
consequence of Open Q3**, not an independent risk: fold into `sb` → the vocabulary is
binary-version-locked via `include_str!` / `bootstrap --force`, so drift is structurally
impossible (same as borg/cortex today); stay standalone → an out-of-workspace consumer parses the
YAML with no compile-time link, so a future schema change silently breaks it. Mitigation for the
standalone branch: a **versioned vocabulary contract** — lift `vault::canonical` parsing + a
schema-version constant into a thin published crate the tool depends on via `cargo add`, so a
schema change fails the build instead of misreading at runtime. Either branch leaves Phase 1
untouched.

The "unverified" flags on `ai-title` coverage and the 30-day TTL are empirical findings measured
on disk, not repo-derived — they stand.

## References

- `cr` (claude-report), `ccu` (cost), `cs` (RLM context store)
- second-brain: `oracle` (MCP, BM25+vector+RRF), `cortex` (tagging/governance), `borg`
  (ingestion, staged-source/trace), Candle `bge-small-en-v1.5`
- `rkvr` timestamp convention (`chrono::Local`, `%Y-%m-%d-%H%M%S`)
- `docs/design/2026-03-21-cortex-classify-promote.md` (pipeline structure precedent)
- Prototype: `~/scratch/sx-proto/ingest.py` (JSONL parsing reference; rest superseded)
