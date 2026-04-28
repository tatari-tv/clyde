# Design Document: cr (claude-report)

**Author:** Scott Idler
**Date:** 2026-04-27
**Status:** Implemented
**Review Passes Completed:** 5/5 + Architect review (1 round, 7 findings, 6 actioned, 1 conceded)

## Summary

`cr` is a Rust CLI that scans Claude Code's local session JSONL files under `~/.claude/projects/`, summarizes each session (token totals, time window, models, repo, cwd), titles each session with Haiku in 3-7 words, and emits a per-host YAML report. A separate `cr render` subcommand turns that YAML into a justification artifact: markdown by default, PDF on demand. The first concrete consumer is the author's monthly Claude Enterprise usage justification.

## Problem Statement

### Background

Claude Code writes one JSONL file per session under `~/.claude/projects/<encoded-cwd>/<session-id>.jsonl`, with subagent activity nested as `<encoded-cwd>/<session-id>/subagents/<agent>.jsonl`. Every assistant turn carries token usage in `message.usage`. `tatari-tv/claude-cost-usage` (`ccu`) already reads these files for daily/weekly/monthly cost summaries, but it does not preserve per-session metadata (which repo, when, which models) and has no notion of titling sessions.

The author wrote a Claude usage justification for March 2026 spend by hand and has now been asked again for April with finer per-session attribution. Hand-curating that report does not scale and the question keeps coming.

### Problem

There is no machine-readable inventory of Claude Code sessions on a developer machine. Without one:

- Token spend cannot be attributed to repos or projects, only to days.
- Sessions cannot be filtered, sorted, or summarized by what they were about.
- A justification report has to be hand-curated each time spend is questioned.

### Goals

- Discover every session JSONL under `~/.claude/projects/`, including subagent files.
- For each session, compute: token totals (input, output, cache 5m write, cache 1h write, cache read, total), begin and end timestamps, set of models used, cwd, reposlug, hostname, JSONL paths.
- Roll subagent sessions up under their parent session in the output.
- Emit a YAML report keyed by parent session id.
- Title each session with Haiku in 3-7 words; persist titles in the same YAML so reruns do not retitle existing sessions.
- Defaults: `--since` = first day of current month at local midnight; `--until` = now.
- Default output `./claude-report.yml`; overridable with `-o`.
- Use `git remote get-url origin` to resolve cwd to a reposlug; `null` when undetermined.
- Parallelize file parsing with rayon.
- Render the YAML into a justification document via `cr render`. Markdown is the default output; PDF is a `--pdf` option.

### Non-Goals

- Not computing dollar costs. ccu owns pricing and the live statusline.
- Not building a TUI, web UI, or daemon.
- Not a replacement for ccu. Complementary: ccu = $/day, cr = sessions/repos/titles/document.
- Not merging across machines in v1. That ships as `cr merge`.
- Not bundling a PDF engine. `cr render --pdf` shells out to `pandoc` and inherits its toolchain.

## Proposed Solution

### Overview

`cr` walks `~/.claude/projects/`, parses every JSONL in parallel, dedupes per `(message-id, request-id)`, folds entries into per-session summaries, rolls subagent files up under their parent session id (the directory name above `subagents/`), resolves each session's cwd to a reposlug via `git remote get-url origin`, calls Haiku to title sessions that do not already have a title in an existing report, and writes YAML to `./claude-report.yml` atomically.

Titling is a step inside the scan pipeline, not a separate subcommand. v1 ships the scan-and-emit path with `title: null`; the Haiku step lands in a follow-up revision behind `--no-title` for opt-out. The YAML schema is title-ready from day one so the change is additive.

### Architecture

Two pipelines, sharing only the YAML on disk:

```
cr (default):
+---------+    +---------+    +-----------+    +---------+    +---------+    +---------+
| scan::  |--> | parse:: |--> | session:: |--> | repo::  |--> | title:: |--> | report::|
| find    |    | (rayon) |    | fold,     |    | git     |    | haiku   |    | atomic  |
| files   |    | jsonl   |    | dedup,    |    | remote  |    | (later) |    | yaml    |
|         |    |         |    | rollup    |    |         |    |         |    | write   |
+---------+    +---------+    +-----------+    +---------+    +---------+    +---------+

cr render:
+----------+    +----------+    +-------------+    +-----------------+
| read::   |--> | render:: |--> | (markdown)  |--> | --pdf? pandoc   |
| yaml     |    | template |    | to stdout   |    | else: write .md |
| (input)  |    |          |    | or file     |    |                 |
+----------+    +----------+    +-------------+    +-----------------+
```

On-disk layout we walk:

```
~/.claude/projects/
  -home-saidler-repos-tatari-tv-claude-report/
    9d4c1f28-....jsonl                          # parent session
    9d4c1f28-.../
      subagents/
        agent-aabbccdd.jsonl                    # rolls up under 9d4c1f28-...
  -home-saidler-repos-scottidler-borg/
    8b21c3....jsonl
```

Module layout (single-word files, Rust 2018+ style):

```
src/
  main.rs       # thin shell: parse args, call lib, set exit code
  lib.rs        # public API: Cli -> Config -> run()
  cli.rs        # clap derive; subcommands
  config.rs     # Cli -> Config validation/defaults
  scan.rs       # discover JSONL files, classify parent vs subagent
  parse.rs      # JSONL -> AssistantEntry plus first cwd seen
  session.rs    # group, dedup, fold to SessionSummary
  repo.rs       # cwd -> Option<reposlug>
  report.rs     # SessionSummary list -> YAML on disk (atomic)
  title.rs      # placeholder for future Haiku titling step
  render.rs     # YAML -> markdown; --pdf shells out to pandoc
```

### Data Model

#### Internal: `SessionFile` (from `scan.rs`)

```rust
pub struct SessionFile {
    pub path: PathBuf,
    /// Effective session id used for grouping. For a parent file
    /// `<dir>/<uuid>.jsonl` this is `<uuid>`. For a subagent file
    /// `<dir>/<parent-uuid>/subagents/<agent>.jsonl` this is `<parent-uuid>`.
    pub group_id: String,
    pub kind: SessionFileKind,  // Parent or Subagent
}
```

#### Internal: `ParseResult` (from `parse.rs`)

```rust
pub struct ParseResult {
    pub entries: Vec<AssistantEntry>,
    /// First `cwd` field encountered on any line type. Captured during
    /// the same pass that pulls assistant entries, so it is free.
    pub cwd: Option<PathBuf>,
}
```

`AssistantEntry` mirrors `ccu::parser::AssistantEntry`: session-id, timestamp, model, usage, message-id, request-id.

#### Internal: `SessionSummary` (from `session.rs`)

```rust
pub struct SessionSummary {
    pub session_id: String,
    pub repo: Option<String>,
    pub cwd: Option<PathBuf>,
    pub begin: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub models: BTreeSet<String>,
    pub tokens: TokenTotals,
    pub jsonl_paths: Vec<PathBuf>,
    pub title: Option<String>,
}

pub struct TokenTotals {
    pub input: u64,
    pub output: u64,
    pub cache_5m_write: u64,
    pub cache_1h_write: u64,
    pub cache_read: u64,
    pub total: u64,
}
```

`host` is captured once at the top of the YAML, not per session. `cr merge` will stamp per-session host when it folds reports together.

`cwd` is held in the internal `SessionSummary` struct (it's the input to `repo::detect`) but is **not** serialized into the YAML. For repo-detected sessions it's redundant with the slug; for `repo: null` sessions, `jsonl-paths` is sufficient for debugging. Keeping it out of the output keeps the report machine-portable (no host-specific paths leaking) and tighter to read.

#### YAML output

Keyed by session-id (per `yaml.md`: keyed maps, kebab-case keys, prefer keyed maps over list-of-dicts):

```yaml
schema-version: 1
generated: 2026-04-27T19:42:08Z
host: desk
since: 2026-04-01T00:00:00-07:00
until: 2026-04-27T19:42:08-07:00
session-count: 287
sessions:
  9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042:
    repo: tatari-tv/claude-report
    begin: 2026-04-27T18:11:32Z
    end: 2026-04-27T19:42:08Z
    models:
      - claude-opus-4-7
      - claude-sonnet-4-6
    tokens:
      input: 12345
      output: 6789
      cache-5m-write: 0
      cache-1h-write: 0
      cache-read: 84321
      total: 103455
    jsonl-paths:
      - /home/saidler/.claude/projects/-home-.../9d4c1f28-...jsonl
      - /home/saidler/.claude/projects/-home-.../9d4c1f28-.../subagents/agent-aabb.jsonl
    title: null
```

Subagent JSONL files appear under their parent session's `jsonl-paths` and their tokens fold into the parent's totals.

### API Design

CLI:

```
cr [OPTIONS] [SUBCOMMAND]

  Subcommands:
    (none)   Default: scan, fold, title, write report
    render   Render YAML report to markdown (default) or PDF (--pdf)
    merge    Combine per-host reports into one (NOT YET IMPLEMENTED in v1)

  Global options:
    -l, --log-level <LEVEL>     error|warn|info|debug|trace [default: info]
    -h, --help
    -V, --version

  Default-command options:
        --since <DATETIME>      include sessions whose window touches this time or later
                                [default: first day of current month, local midnight]
        --until <DATETIME>      include sessions whose window touches this time or earlier
                                [default: now]
    -o, --output <PATH>         output YAML path [default: ./claude-report.yml]
        --projects-dir <PATH>   override scan root [default: ~/.claude/projects]
        --no-rollup             keep subagent sessions as separate top-level entries
                                (debug only)
        --skip-title            skip the Haiku titling step

  render options:
    -i, --input <PATH>          input YAML path [default: ./claude-report.yml]
    -o, --output <PATH>         output document path; "-" for stdout (markdown only)
                                [default: ./claude-report.md, or ./claude-report.pdf with --pdf]
        --pdf                   render PDF instead of markdown (requires pandoc on PATH)
        --template <PATH>       override the built-in markdown template
```

Date-only forms (`2026-04-01`) are accepted for `--since`/`--until` and treated as local midnight. RFC 3339 forms are accepted as written.

Rust core API (in `lib.rs`):

```rust
pub fn run(config: &Config) -> Result<RunResult>;

pub struct RunResult {
    pub sessions_emitted: usize,
    pub output_path: PathBuf,
}
```

`main.rs` parses args, calls `run`, prints a one-line summary, maps errors to exit codes. No business logic in `main.rs`.

### Implementation Plan

#### Phase 1: Scaffold and dependencies
**Model:** sonnet

- Add deps via `cargo add`: chrono (with `serde`), rayon, serde_json, gethostname.
- Confirm crate-root `#![deny(...)]` is in place (already from scaffold).
- Stub modules: empty `lib.rs`, `scan.rs`, `parse.rs`, `session.rs`, `repo.rs`, `report.rs`, `title.rs`. Re-export from `lib.rs`. Adapt existing `cli.rs` and `config.rs`.
- Verify `otto ci` passes on the empty skeleton.

#### Phase 2: File discovery
**Model:** sonnet

- Port the walker from `ccu/src/scanner.rs`. Two shapes:
  - `<projects>/<encoded-cwd>/<sid>.jsonl` (parent)
  - `<projects>/<encoded-cwd>/<sid>/subagents/<agent>.jsonl` (subagent)
- Each file gets `SessionFile { path, group_id, kind }`. Parent files: `group_id` = file stem. Subagent files: `group_id` = name of the directory containing `subagents/`.
- **Validate the parent shape, not just the subagent shape.** Before tagging a `<dir>/<stem>.jsonl` as `kind=Parent`, require the stem to match a UUID-v4 regex (`[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}`). If a JSONL appears at that depth with a non-UUID stem, fail the scan loudly. Rationale: if Anthropic renames `subagents/` to `workers/` or flattens the hierarchy, files that used to be subagents will start appearing alongside parents. Without parent-shape validation, they get silently promoted to independent sessions and token counts get split or doubled. Asserting only on the `subagents/` segment doesn't catch this — that branch never executes when the directory is renamed.
- Skip empty files.
- Tests: empty dir, parent-only, parent+subagents, subagent without sibling parent, non-jsonl ignored, **non-UUID stem at parent depth fails loud.**

#### Phase 3: JSONL parsing
**Model:** sonnet

- Port `ccu/src/parser.rs::parse_jsonl_file` with one extension: also capture the first `cwd` field encountered on any line type. The `cwd` field appears on user/assistant/attachment lines (verified empirically).
- Signature: `fn parse_jsonl_file(path: &Path) -> Result<ParseResult>`.
- Tests: cwd captured from a non-assistant line, cwd absent gives `None`, malformed lines skipped, streaming partial duplicates returned (dedup is downstream).

#### Phase 4: Reposlug detection
**Model:** opus

- `repo::detect(cwd: &Path) -> Option<String>`.
- Pre-check: if `cwd` no longer exists on disk (ENOENT), return `None` and log at `debug` (distinguishes a deleted project dir from a real git failure).
- Run `git -C <cwd> rev-parse --show-toplevel`. **Reject the climb:** if the returned toplevel is not equal to or a prefix of `cwd`, treat as not-a-repo and return `None`. Without this guard, a session in `~/scratch/foo` whose parent `~` is itself tracked (e.g. dotfiles repo) would be falsely attributed to the dotfiles slug.
- Run `git -C <cwd> remote get-url origin`. **`origin` only by policy** — no `upstream` / `deploy` fallback. If `origin` does not exist, the session is reported as `repo: null`. Parse SSH (`git@github.com:org/repo.git`), HTTPS (`https://github.com/org/repo.git`), and `git://` forms. Strip optional trailing `.git`. Return `<org>/<repo>`.
- Cache results in `HashMap<PathBuf, Option<String>>` so we don't shell out repeatedly for repos with multiple sessions.
- Tests: SSH form, HTTPS form, with and without `.git`, non-repo dir, missing dir (ENOENT), repo with no origin, dotfiles-climb scenario (cwd is unversioned subdir of a tracked parent).

#### Phase 5: Session folding, dedup, rollup
**Model:** opus

- `session::fold(files: &[SessionFile], parsed: &HashMap<PathBuf, ParseResult>, cfg: &Config) -> Vec<SessionSummary>`.
- For each file, set every entry's grouping key to `SessionFile::group_id` (this is the rollup: subagent entries inherit the parent session-id).
- Dedup within each group: bucket by `(message-id, request-id)`. From each bucket keep the entry with the largest `output_tokens` (matches ccu's streaming-partial heuristic; some entries are rewritten as the model streams).
- For each group:
  - `begin` / `end` = min/max timestamp of kept entries (assistant only; the "easiest" choice we agreed on).
  - `models` = `BTreeSet` of distinct `model`.
  - `tokens` = component sums; `total` = sum of the five components.
  - `cwd` = first non-`None` `cwd` from any source file in the group.
  - `jsonl_paths` = source files contributing, sorted (parent first by convention).
  - `repo` = `repo::detect(cwd)`.
  - `title` = inherited from prior YAML if present (Phase 6); else `None`.
- Drop sessions with zero assistant entries (no tokens, no timestamps).
- Filter: keep sessions whose `[begin, end]` overlaps `[since, until]` from config.
- Tests: dedup-by-(mid,rid) keeping max-output, multi-model session, subagent rollup, since/until window, no-cwd session, zero-assistant session dropped.

#### Phase 6: CLI, config, report emission, title-preservation
**Model:** sonnet

- `cli::Cli` with clap derive: default command + `Merge` subcommand stub.
- `config::Config` built via `TryFrom<Cli>`. Validate `since <= until`, resolve defaults (start-of-month local midnight, now), parse date-only forms.
- Title preservation: before fold, if the output path already exists, deserialize it and seed a `HashMap<session-id, title>` to copy forward.
- `report::write_yaml(summaries, cfg)`:
  - Build `BTreeMap<String, SessionEntry>` for stable order.
  - Write to `NamedTempFile` next to the destination, then rename. Atomic against concurrent runs and against ctrl-c during write.
- `lib::run` orchestrates phases 2-6 with rayon over file parsing.
- `main.rs` stays thin.
- Tests: end-to-end with fixture `~/.claude` tree under `tempfile::TempDir`, including title-preservation across two runs.

#### Phase 7: Subcommand stubs
**Model:** sonnet

- `cr merge <input>...` returns "not implemented in this release" with exit 2. Keeps the surface fixed so future work doesn't break callers.

#### Phase 8: Haiku titling (follow-up revision)
**Model:** opus

- `title::haiku(summary, jsonl_paths) -> Result<Option<String>>`. Reads a bounded prefix of the parent JSONL (first user prompt + first assistant reply), sends to Haiku with a strict prompt for "3-7 words, lowercase, no punctuation, summarize the task." Persists the result in the YAML.
- Skip when `--skip-title` is set or when the session already has a title.
- Reads the API key from `ANTHROPIC_API_KEY`; skips silently if absent.

#### Phase 9: Render to markdown / PDF
**Model:** opus

- `cr render` reads `claude-report.yml` and produces a justification document. Markdown is the default; `--pdf` shells out to `pandoc` to convert.
- `render::to_markdown(report: &Report, template: &Template) -> String`. Built-in template renders:
  - Header: period (`since`-`until`), host, total sessions, total tokens.
  - "By repo" rollup table: reposlug, sessions, total tokens, top models. Sessions with `repo: null` grouped under "(no repo)".
  - "Sessions" detail section: grouped by repo, each entry shows title (or `<untitled>`), session-id (short), begin-end window, models, total tokens.
- Templating engine: hand-rolled `format!` with a `Template` enum for the v1 default; switch to `tera` only if a custom template is genuinely needed (`--template <path>`).
- PDF path: write markdown to a `NamedTempFile`, run `pandoc <tmp> --pdf-engine=<engine> -o <output>.pdf`. Default engine is `wkhtmltopdf` (lighter than LaTeX, no 1GB+ texlive install required). User can override with `--pdf-engine=<name>`. Detect missing `pandoc` and surface a clear error directing the user to install it. If the chosen engine is missing, surface pandoc's error verbatim and point at the install path. PDF output path defaults to `./claude-report.pdf`.
- Output `-` (dash) goes to stdout (markdown only; PDF binary on stdout is rejected).
- Tests: golden-file markdown rendering against a fixture YAML; PDF test gated on pandoc availability (skipped on CI without pandoc).

## Alternatives Considered

### Alternative 1: Extend ccu instead of building cr
- **Description:** Add a `ccu sessions` subcommand that emits the same YAML.
- **Pros:** One binary, shared parsing code.
- **Cons:** ccu is firmly framed as a *cost* tool with embedded pricing tables; sessions-as-data is a different concern. Mixing them couples release cycles and grows ccu's scope. ccu must never need network access for the statusline; `cr title` will eventually call Anthropic.
- **Why not chosen:** Two tools, two concerns. Keep them separate.

### Alternative 2: Decode cwd from the project-dir name
- **Description:** Reverse `~/.claude/projects/-home-foo-bar` to `/home/foo/bar` by replacing `-` with `/`.
- **Pros:** No JSONL line read needed.
- **Cons:** Lossy: `~/repos/tatari-tv/claude-report` and `~/repos/tatari/tv/claude/report` encode identically.
- **Why not chosen:** JSONL files contain a literal `cwd` field. Use it.

### Alternative 3: List-of-dicts YAML
- **Description:** `sessions: [- session-id: x, ...]`.
- **Pros:** Preserves insertion order trivially.
- **Cons:** Violates `yaml.md` convention. Forces O(n) lookup by session-id during titling.
- **Why not chosen:** Keyed map maps directly to `BTreeMap<String, SessionEntry>` and is what every consumer wants.

### Alternative 4: Title as a separate subcommand
- **Description:** `cr scan` emits without titles; `cr title` reads the YAML, calls Haiku, writes back.
- **Pros:** Cleanly separates network calls from local scanning.
- **Cons:** It's not a different operation, it's a step. Two commands means two mental models, two argument sets, and a chance to forget step two.
- **Why not chosen:** Title persistence in the YAML already gives idempotency; the scan pipeline only titles what's missing. `--no-title` covers the offline case.

### Alternative 5: Subagents as separate top-level sessions
- **Description:** Subagent files become top-level entries with `parent-session: <uuid>`.
- **Pros:** Per-subagent token accounting.
- **Cons:** Downstream report wants per-session totals. Rolling up matches the user's mental model.
- **Why not chosen:** Per stated requirement (rollup). Kept `--no-rollup` as a debug flag.

### Alternative 6: Render as a flag on the default command
- **Description:** `cr --render md` or `cr --render pdf` after the scan completes.
- **Pros:** One invocation produces both YAML and document.
- **Cons:** Couples a network-touching pipeline (Haiku titling) to document rendering. Harder to re-render after editing the YAML by hand. Bigger CLI surface on the default command.
- **Why not chosen:** Render is a different operation (YAML in, document out). Subcommand keeps each command tightly scoped. Re-rendering is one command away.

### Alternative 7: Bundle a PDF engine
- **Description:** Use `wkhtmltopdf`, `weasyprint`, or `typst` directly from Rust instead of shelling out to pandoc.
- **Pros:** No external binary dependency.
- **Cons:** All options are heavy: wkhtmltopdf is a chromium-class browser; weasyprint is a Python runtime; typst-rust is closer but still megabytes of crates. Pandoc is already on every developer machine in this org.
- **Why not chosen:** Shell out. Document the dependency in README.

## Technical Considerations

### Dependencies

- `clap` (present): CLI parsing.
- `eyre` (present): error handling.
- `serde`, `serde_yaml` (present), `serde_json` (added): JSONL/YAML.
- `chrono` (added, with `serde`): timestamps.
- `rayon` (added): parallel parsing.
- `gethostname` (added): host field.
- `dirs` (present): `~/.claude` discovery.
- `log` + `env_logger` (present): logging.

No `git2`/`gix`. We shell out to `git remote get-url origin`: one call per distinct cwd, result cached. Pulling a 1MB+ git library for one URL would be disproportionate.

### Performance

Inputs scale with developer activity: a heavily-used machine has on the order of 1k-3k JSONL files totaling tens to low hundreds of MB.

- Discovery: O(files) directory walk. Negligible.
- Parsing: rayon `par_iter` over the file list. JSON parsing of ~50 MB of JSONL is sub-second on modern hardware.
- Folding: O(entries) hash group + sort. Trivial single-threaded.
- Repo lookup: one `git` fork+exec per distinct cwd, cached. Bounded by tens of repos.

A first run on a fully populated `~/.claude` should complete in 1-3 seconds wall-clock.

### Security

- Reads only. We open JSONL files and run `git remote get-url origin`. We do not read message content (avoids accidentally surfacing prompt text or secrets). The Haiku step in the follow-up phase will read a bounded prefix and is documented in its own design.
- Output YAML contains paths, session ids, repo slugs, timestamps, models, token counts. No prompt text. `cwd` may include private repo paths. Intended for personal usage justification reports; documented in README.

### Testing Strategy

- Unit tests per module, in `src/<mod>/tests.rs` (Rust 2018+ submodule pattern; never inline `mod tests` blocks).
- Fixture-based end-to-end test: build a fake `~/.claude/projects` tree under `tempfile::TempDir` with parent + subagent JSONL, run scan + fold + write, assert YAML.
- Repo detection test against a real `git init`'d temp dir.
- No live network, no live `~/.claude`. CI-safe.

### Rollout Plan

- Local install via `cargo install --path .`. This is a personal tool; no homebrew, no CI publishing.
- README documents:
  - Quickstart.
  - YAML schema with field meanings and the `title: null` placeholder.
  - Subcommand status table (`cr` ready, `cr merge` stub).
- Versioning via the `bump` tool; first release tag created after the first successful end-to-end run produces a sane YAML.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Subagent path heuristic mismatches reality (rename/flatten) | Med | High (rollup wrong) | Validate the *parent* shape (UUID-v4 stem) at scan time, not just the subagent shape. A renamed-subagents-folder failure mode that would silently promote subagent files to parents now fails the scan loudly. `--no-rollup` available as debug fallback. |
| Streaming-partial dedup picks the wrong entry on rare shapes | Low | Med (token totals off) | Reuse ccu's max-output-tokens heuristic, in production. Regression fixture covers duplicated streaming case. |
| `git remote get-url` slow on a machine with many repos | Low | Low | Cache by cwd; tens of unique cwds at most. |
| Sessions outside `~/repos/` | Med | Low | `repo: null` is documented; cwd is preserved. |
| YAML grows large | Low | Low | ~300 sessions/month, a few hundred bytes each = tens of KB. No streaming writer needed. |
| Two machines reuse a session-id | Very Low | Med (merge collision) | Top-level `host` field; `cr merge` namespaces on collision. |
| Concurrent `cr` runs corrupt the YAML | Low | Low | Atomic write via `NamedTempFile` + rename. |
| Haiku titles drift across reruns | Med (post-v1) | Low | Title persistence in the YAML; only call Haiku for sessions without an existing title. |
| `pandoc` not installed when `--pdf` is invoked | Med | Low | Detect missing binary, return a clear error pointing at install instructions. Markdown path is unaffected. |
| `--pdf` works with pandoc but PDF engine (LaTeX/wkhtmltopdf) is missing | Med | Low | Default engine is `wkhtmltopdf` (cheap to install). Expose `--pdf-engine=<name>` for users who already have LaTeX. Surface pandoc's engine-missing error verbatim. |
| Markdown template diverges from what management expects | Med | Med | Built-in template mirrors the structure of the existing March justification doc; `--template <path>` provides an escape hatch. |
| Memory blow-up at extreme scale (gigabytes of JSONL) | Low | Med | At the target scale (~3k files / tens of MB) `par_iter` returning all entries is fine. Documented limitation: if a user accumulates >1 GB of session history, switch the rayon stage to fold-into-running-totals per file (constant memory per file). Not a v1 concern. |

## Open Questions

All resolved (2026-04-27):

- [x] **`cwd` in YAML?** Dropped entirely. Used internally during scan, not persisted. For repo-detected sessions it's redundant; for `repo: null` sessions, `jsonl-paths` is enough.
- [x] **Parent JSONL missing, subagents/ present?** Yes, emit a session entry under the parent uuid; the subagent files contain valid assistant entries.
- [x] **Top-level "(no repo)" rollup in the rendered report?** Deferred. v1 leaves `repo: null` sessions in per-session detail only. Add a "(no repo)" group later if the volume warrants it.
- [x] **`cr render` date filter?** No. Always renders the whole YAML; no `--since`/`--until` on `render`. The scan command is where the date window lives.

## References

- `tatari-tv/claude-cost-usage` (`src/parser.rs`, `src/scanner.rs`)
- March justification: `tatari-tv/thoughts/blob/main/directors/scott.idler/claude-usage-justification-scott-idler-2026-03.md`
- April 24 follow-up callout (engineering-leadership): https://tatari.slack.com/archives/C089C6Y41ND/p1777067425284179
