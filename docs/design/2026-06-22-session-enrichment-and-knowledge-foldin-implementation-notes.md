# Implementation Notes: Session Enrichment & Knowledge Fold-In

Companion to `2026-06-22-session-enrichment-and-knowledge-foldin.md`. Append-only.

**Scope of this execution:** Phase 2 (Enrichment) only. Phase 3 lives in the **second-brain**
repo (its own doc) and Phase 4 (cr/ccu/permit migration) was deliberately deferred. The design
doc's status remains "In Review" — it is **not** fully implemented; only Phase 2 shipped.

Execution decisions confirmed with the user before coding:
- **Phase 2 only** in this run.
- **Work key, work-scope-only** routing: only `work`-scoped sessions reach the work Anthropic
  account; personal sessions are skipped. Live off-machine calls are enabled.

## Phase 2: Enrichment

### Design decisions
- **Anthropic client built directly on `reqwest` blocking, not an SDK crate** —
  `sessions/src/llm.rs::AnthropicClient`. The rust rules mandate `reqwest` with an explicit
  `.timeout(...)` and `error_for_status()`; a hand-built client over the Messages API avoids
  pulling an unmaintained third-party SDK and keeps full control of retries/headers. A
  `Completer` trait is the DI seam so the orchestrator is generic and tests inject a fake.
- **Module placement** — `scope` and `redact` live in the pure `session` core
  (`session/src/scope.rs`, `session/src/redact.rs`); the `Completer` trait, `AnthropicClient`, and
  the orchestrator live in `sessions` (`llm.rs`, `enrich.rs`) because the orchestrator touches
  `Db`. Putting the trait in `sessions` (not `session`) keeps the dependency arrow one-way and
  avoids a cycle.
- **Enrichment state as columns on `sessions`** (not a sidecar table) — schema bumped v2→v3 with
  an idempotent `ensure_column` migration mirroring the Phase 1.5 v2 `ALTER` pattern
  (`sessions/src/db.rs`). Columns: `scope`, `enriched_at`, `enriched_modified`, `enrich_model`,
  `prompt_version`, `enrich_status`, `last_error`, `attempts`, `redaction_count`, `tokens_in`,
  `tokens_out`.
- **`set_enrichment` is the sole enrichment writer** (`db.rs`), transactional (row + high-signal
  FTS rebuild in one tx), distinct from `upsert_session` which *preserves* tags/summary by design.
  `record_enrich_skip` / `record_enrich_failure` handle the non-success paths; `enrich_candidates`
  is the selection predicate; `enrich_summary` backs `doctor`.
- **Manual-tag preservation** (`enrich.rs` + `db.is_enriched`) — tags are overwritten only when
  forced (`--all` / `enrich <id>`), when the session has no existing tags, or when it was
  previously enriched (so klod-written tags refresh on grown sessions). Otherwise existing
  (manual) tags are preserved and only `summary` + state are written.
- **`ANTHROPIC_API_KEY`** is the env handle for the work key (standard name); `from_env` errors
  without echoing the key, and the key is never logged.
- **`--show-payload <DIR>`** writes one redacted `<session_id>.txt` per session under the operator
  dir (dry-run only) — never to the log stream, honoring "never log the prompt body."
- **`klod sessions doctor`** implements the "doctor or status line" option: counts plus the
  last-successful-enrichment timestamp.
- **Token budget** — `--budget-tokens` halts the sweep before a send that would cross the
  cumulative (in+out) budget.

### Deviations
- **Selection predicate omits `scope` in SQL.** The doc wrote the predicate as
  `scope='work' AND …`, but `scope` is a pure Rust classification of stored `cwd`, not a reliable
  stored column at selection time. So `enrich_candidates` returns work+personal eligible rows and
  the **orchestrator** enforces the routing gate, recording `skipped-personal` once (then excluding
  those rows on later sweeps via `enrich_status`). Net behavior matches the doc; the gate still
  runs before any payload is built, and the invariant test asserts the orchestrator never hands a
  personal session to the completer.
- **Body cap head+tail rarely triggers.** Phase-1 `parse` already bounds the body at 500K chars
  (head-only). `SEND_CAP_CHARS` is also 500K, so the enrich-side `head_tail` is a correctness
  guard that effectively never fires on today's parser output. Kept it as the explicit send-side
  cap the doc specified.

### Tradeoffs
- **`reqwest` blocking vs async/SDK** — blocking fits the otherwise-sync CLI (no tokio elsewhere);
  an SDK was rejected to avoid an unmaintained dependency and to keep timeout/retry control.
- **Columns vs sidecar table** for enrichment state — columns chosen: the session row is the
  natural owner and a sidecar would add a join for every candidate query.
- **`is_enriched` heuristic vs a `tags_source` column** — the heuristic avoids another column and
  matches the doc's stated default; the precise manual-vs-auto distinction is itself an Open
  Question in the doc.

### Open questions
- **Data-retention posture (the real boundary).** Confirm Tatari's Anthropic account
  data-retention terms are acceptable for work session content. This run wired live calls under
  the work key; the scope filter + endpoint posture is the trust boundary, not the scrub.
- **Key env var.** Is `ANTHROPIC_API_KEY` the right handle on desk, or should klod use a
  dedicated var so it can't pick up another tool's (possibly personal) key?
- **Personal sessions.** v1 skips them (`skipped-personal`). Confirm they should stay un-enriched
  rather than enriched under a separate personal key.
- **Redaction depth.** v1 strips: Anthropic/OpenAI keys, GitHub/Slack tokens, AWS access-key IDs,
  bearer tokens, PEM private-key blocks, and `secret|token|api_key|access_key|password = …`
  assignments. Confirm this set is sufficient.
- **Sweep trigger/cadence.** `klod sessions enrich` is the cron entrypoint; the cron entry itself
  (schedule, host) is operator setup and was not added here.
- **Multi-host coverage.** Unchanged known v1 hole: desk-originated sessions only.
- **Phases 3 & 4 not implemented.** Phase 3 is second-brain work (own doc); Phase 4 (the
  `session` core usage/IDs extension + cr/ccu/permit migration) is deferred. Do not read the
  design doc as fully implemented.
