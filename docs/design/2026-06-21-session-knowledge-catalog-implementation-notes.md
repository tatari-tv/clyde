# Implementation Notes: Claude Session Knowledge Catalog

Running, append-only record of how the implementation diverges from or interprets
`2026-06-21-session-knowledge-catalog.md`. One section per phase. Append, never edit.

Scope executed by this pass: **Phase 0, Phase 1, Phase 1.5**. Phase 2 is gated on the
parked redaction/scope decision (ships content off-machine to an LLM), Phase 3 lives in a
different repo (second-brain) and is deferred, and Phase 4 is a separate design doc — all
explicitly out of scope here.

## Phase 0: cr timestamped collect output

(Implemented in `tatari-tv/claude-report`, not `klod` — Phase 0 is independent of the workspace.)

### Design decisions
- Default `cr collect` output — `claude-report/src/config.rs:default_collect_output` — resolves
  to `<xdg-data>/claude-report/claude-report-%Y-%m-%d-%H%M%S.json` via the existing
  `xdg_data_dir()` helper and `chrono::Local`, matching the rkvr stamp the doc cites.
- Title-reuse preservation — `claude-report/src/lib.rs:latest_prior_report` + `run_collect` —
  cr caches per-session titles in the report file and re-loads them each run via
  `load_existing_titles(cfg.output)`. A timestamped output never pre-exists, so a naive change
  would make every run re-Haiku-title all sessions (cost + nondeterministic churn) or, with no
  `ANTHROPIC_API_KEY`, lose titles entirely. `run_collect` now seeds titles from the newest
  prior `claude-report-*.json` in the output dir when the exact output is absent. The
  `%Y-%m-%d-%H%M%S` stamp is lexically ordered, so newest = greatest filename.

### Deviations
- None. The render default *input* (`./claude-report.json`) is intentionally untouched —
  Phase 0 scopes only the collect default output.

### Tradeoffs
- Lexical filename sort for "newest prior report" vs. mtime — chose filename sort: deterministic,
  no syscall-per-file, and the timestamp is already in the name. Folding mtime into selection
  would reintroduce wall-clock nondeterminism the repo rules warn against.
- Preserving title-reuse is mild scope expansion beyond the one-line doc spec, but the
  alternative is a silent cost/determinism regression — judged a faithful gap-fill, flagged here
  so it can be reverted if considered out of scope.

### Open questions
- `cr render` with no `-i` still defaults its *input* to `./claude-report.json`, which collect no
  longer writes by default. Should render's default input track the newest timestamped report in
  `<xdg-data>/claude-report/`? Left out of Phase 0 deliberately (would change render behavior).

## Phase 1: klod workspace + sessions navigational layer

### Design decisions
- Workspace shape — `klod` (bin) + `session` (core) + `sessions` (nav), edition 2024, mirrors
  second-brain. Built by hand (not `scaffold`) because `scaffold` produces a single-crate CLI, not
  a workspace; conventions (build.rs GIT_DESCRIBE, eyre, deny attrs, XDG `paths`, `.otto.yml`) are
  carried over manually.
- `session::scan` — adapts `cr`'s parent/`<uuid>/subagents/*.jsonl` rollup contract verbatim, but
  **warns-and-skips** non-UUID names where `cr` `bail!`s. The design doc's edge-case contract is
  "skip-and-log, never crash the reindex," so one malformed dir cannot abort a scan.
- `session::parse` — line-parsed from raw bytes (`serde_json::from_slice` per line) so a non-UTF-8
  byte or malformed line skips just that line, never truncating the transcript. `first_prompt`
  skips slash-command/caveat/hook/system-reminder wrappers; `title()` = ai-title else first-prompt.
  Body = user+assistant **text** blocks only (thinking and tool noise excluded), capped at 500K
  chars; first_prompt capped at 2000.
- Dual FTS5 — `sessions_fts(title, tags, summary)` for ranking, `sessions_body_fts(body)` for
  content recall, both keyed by `rowid = sessions.id`, rebuilt in lockstep on upsert. `search`
  returns high-signal hits first, then body-only hits not already surfaced (the Retrieval Decision).
  Verified live: `search terraform marquee` ranks "Set up S3 bucket for Marquee with Terraform"
  first as high-signal — the design doc's headline user story.
- `db` discipline — copied from `borg::receipts`: WAL + synchronous=NORMAL + busy_timeout=5000 +
  foreign_keys=ON, schema versioned via `PRAGMA user_version`, migration+bump in one transaction.
  Upsert preserves `tags`/`summary`/`cost` across reindex (parse never owns those); incremental
  skip compares stored vs. parsed parent-file mtime.
- FTS query safety — user query tokens are individually double-quoted and AND-joined, so FTS5
  operators in user input cannot inject or error (tested with `" OR 1=1 --`).
- Lazy reindex — search/ls/open reindex first by default (incremental, cheap), `--no-reindex` to
  skip; a reindex failure during a query warns and falls back to stored data, never aborts.
- `summary` column = the doc's `abstract` (renamed: `abstract` is a Rust keyword). Phase-2 enrich
  populates it; NULL for now. `cost` column reserved, NULL until the Phase-4 cr migration.

### Deviations
- None from the navigational-layer spec. Phase 2 (enrich), 3 (knowledge layer), and 4 (cr/ccu/
  permit migration) are intentionally not built — see scope note at top.

### Tradeoffs
- Standalone FTS5 tables with explicit rowid management vs. external-content FTS5 — chose
  standalone: simpler to reason about and fully rebuildable, at the cost of storing the body text
  twice (once in the FTS index; the main table does not store body). For a 385-session corpus the
  storage cost is negligible.
- First reindex of the live corpus parses 62M chars single-threaded (~28s). Acceptable as a
  one-time cost (incremental thereafter), but `parse_sessions` is an obvious `rayon` candidate if
  it ever bites — left sequential for Phase 1 simplicity.
- `model` stores the most-recent assistant model (one string) rather than the full per-model set
  `cr` tracks; the navigational record only needs a dominant model for filtering.

### Open questions
- Resumed-session identity (design doc edge case) is still **unverified** — we assume one session
  id maps to one growing transcript. If Claude ever splits/renames, the mtime-skip and the
  one-row-per-session model would need revisiting.
- First-index latency (~28s) — parallelize `parse_sessions` with rayon, or accept it as one-time?

## Phase 1.5: raw-transcript staging (TTL insurance)

### Design decisions
- Staging mechanics live in `session::stage` (copy files), orchestration in `sessions::stage`
  (which sessions, bookkeeping) — matching the doc's "lives in sessions/session" and the
  lib-only/return-data invariant. The `klod sessions stage` verb is the only printing site.
- Dormancy threshold (resolves the buildable half of Open Q2) — a session is dormant once idle
  `--dormant-after` (default **7d**), well inside the 30-day TTL so staging has ~3 weeks of
  slack. `--all` stages every non-archived session. The *trigger* (cron vs. idle-daemon) is
  still open (Q2/Q6) and intentionally not wired here — `stage` is a manual/cron-able verb.
- Idempotent + self-healing — `copy_if_newer` skips a destination already ≥ its source mtime, so
  re-running a sweep is cheap and a grown (resumed) session re-stages automatically. This avoids
  needing a `staged-at` column; freshness is judged from filesystem mtimes.
- Atomic copies — temp-in-dest + `persist` (rename), mirroring the parent + `subagents/` layout
  under `<xdg-data>/klod/staged/<session-id>/`. No knowledge-layer commitments: pure local file
  insurance, no LLM, no vault, no work/personal crossing (satisfies the Security gate for 1.5).
- `open` on an archived (reaped) session now prints the staged copy path when one exists, else
  reports "transcript reaped … and no staged copy exists" — the doc's open/trace contract.
- Schema migration to v2 adds `staged_path` via a guarded idempotent `ALTER` (probe
  `PRAGMA table_info` first) inside the migration transaction, so a v1 DB created between phases
  upgrades cleanly; fresh DBs get the column from `CREATE TABLE`.

### Deviations
- None.

### Tradeoffs
- Staging copies the *entire* raw JSONL (tool output, attachments, etc.), not just the
  conversational text — verified 691 files / 581 MB for 386 sessions. That is the point (a
  durable, faithful copy to beat the TTL), and it is opt-in, but it is heavier than the indexed
  body. A future compaction pass could trim tool noise if size becomes a problem.
- Dormancy is computed from the stored `modified` (parent mtime) rather than a separate
  last-activity signal — consistent with the incremental-reindex key, but it inherits the same
  "resumed session keeps one growing id" assumption flagged in Phase 1.

### Open questions
- Trigger + cadence for the sweep (Open Q2/Q6) — cron, idle-daemon, or `Stop`-hook? Left to the
  operator; `klod sessions stage` is ready to be driven by any of them.
- Staging size (581 MB for the current backlog) — acceptable, or worth a tool-output-trimming
  pass before staging?
