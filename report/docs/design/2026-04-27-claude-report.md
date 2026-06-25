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

- Not depending on the `ccu` binary at runtime. cr and ccu are separate CLIs with separate UX; cr does not shell out to ccu. They share a pricing library (see Phase 1b) but are otherwise independent.
- Not building a live statusline. ccu owns that surface.
- Not building a TUI, web UI, or daemon.
- Not a replacement for ccu. Complementary: ccu = $/day live, cr = sessions/repos/titles/document.
- Not merging across machines in v1. That ships as `cr merge`.
- Not bundling a PDF engine. `cr render --pdf` shells out to `pandoc` and inherits its toolchain.

## Proposed Solution

### Overview

`cr` walks `~/.claude/projects/`, parses every JSONL in parallel, dedupes per `(message-id, request-id)`, folds entries into per-session summaries, rolls subagent files up under their parent session id (the directory name above `subagents/`), resolves each session's cwd to a reposlug via `git remote get-url origin`, calls Haiku to title sessions that do not already have a title in an existing report, and writes YAML to `./claude-report.yml` atomically.

Titling is a step inside the scan pipeline, not a separate subcommand. v1 ships the scan-and-emit path with `title: null`; the Haiku step lands in a follow-up revision behind `--no-title` for opt-out. The YAML schema is title-ready from day one so the change is additive.

### Architecture

Two pipelines, sharing only the YAML on disk:

```
cr collect (default when bare):
+---------+    +---------+    +-----------+    +---------+    +---------+    +---------+
| scan::  |--> | parse:: |--> | session:: |--> | repo::  |--> | title:: |--> | report::|
| find    |    | (rayon) |    | fold,     |    | git     |    | haiku   |    | atomic  |
| files   |    | jsonl   |    | dedup,    |    | remote  |    | (later) |    | yaml    |
|         |    |         |    | rollup    |    |         |    |         |    | write   |
+---------+    +---------+    +-----------+    +---------+    +---------+    +---------+

cr render:
+----------+    +----------+--branch--+    +----------+    +-------------+    +-----------------+
| read::   |    | --template?         |--> | render:: |--> | (markdown)  |--> | --pdf? pandoc   |
| yaml     |--> +----------+----------+    | (markdown|    | to stdout   |    | else: write .md |
| (input)  |    | else                |--> |  or pmt) |    | or file     |    |                 |
+----------+    +----------+----------+    +----------+    +-------------+    +-----------------+
                           |
                           v (Opus path only)
                +-------------------+
                | persona whoami    |
                | (5s wait-timeout) |
                +-------------------+
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
  pricing.rs    # model pricing table; calculate_usd -> Result<f64, UnknownModel>
  persona.rs    # shells out to `persona whoami --json` with 5s timeout
  session.rs    # group, dedup, fold to SessionSummary
  repo.rs       # cwd -> Option<reposlug>
  report.rs     # SessionSummary list -> YAML on disk (atomic)
  summarize.rs  # Anthropic API client for pmt-driven Opus rendering
  title.rs      # Haiku titling step
  render.rs     # YAML -> markdown via Opus pmt (default) or custom template; --pdf shells to pandoc
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
    pub models: BTreeMap<String, TokenTotals>,  // per-model token breakdown
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

Per-model totals (rather than a single `tokens` plus `BTreeSet<String>` of model names) is what the report layer needs to compute per-model spend; collapsing to one total would have to be unrolled before pricing.

`host` is captured once at the top of the YAML, not per session. `cr merge` will stamp per-session host when it folds reports together.

`cwd` is held in the internal `SessionSummary` struct (it's the input to `repo::detect`) but is **not** serialized into the YAML. For repo-detected sessions it's redundant with the slug; for `repo: null` sessions, `jsonl-paths` is sufficient for debugging. Keeping it out of the output keeps the report machine-portable (no host-specific paths leaking) and tighter to read.

#### YAML output

Keyed by session-id (per `yaml.md`: keyed maps, kebab-case keys, prefer keyed maps over list-of-dicts).

`schema-version: 1` is the first stable schema; v2 is reserved for the next breaking shape change. The project hadn't shipped externally when the version label was reset, so collapsing the local v2 work back to v1 was a no-op for any consumer.

Per-session and per-model `spend-usd` are `Option<f64>`: a number means tokens were priced, `null` means tokens were counted but the model isn't in the embedded pricing table. Per-session `untracked-models` lists exactly which models contributed unpriced tokens, so a 1M-untracked / 1-priced session can't pass for a real `$0.01` charge. `totals.untracked-models` is the deduped union across all sessions.

```yaml
schema-version: 1
generated: 2026-04-27T19:42:08Z
host: desk
since: 2026-04-01T00:00:00-07:00
until: 2026-04-27T19:42:08-07:00
totals:
  sessions: 287
  spend-usd: 1234.56          # sum of priced models only
  untracked-models:
    - claude-some-new-model
  models:
    claude-opus-4-7:
      input: ...
      output: ...
      cache-5m-write: ...
      cache-1h-write: ...
      cache-read: ...
      total: ...
      spend-usd: 1.23
    claude-some-new-model:
      input: 500
      output: 200
      # ... cache fields ...
      total: 700
      spend-usd: null         # tokens counted, price unknown
sessions:
  9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042:
    title: ship the report tool
    repo: tatari-tv/claude-report
    begin: 2026-04-27T18:11:32Z
    end: 2026-04-27T19:42:08Z
    spend-usd: 3.65            # null when every model in the session is untracked
    untracked-models: []
    jsonl-paths:
      - /home/u/.claude/projects/-home-u-r/9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042.jsonl
    models:
      claude-opus-4-7:
        input: 12345
        output: 6789
        cache-5m-write: 0
        cache-1h-write: 0
        cache-read: 84321
        total: 103455
        spend-usd: 1.23
      claude-sonnet-4-6: { ... }
```

Subagent tokens fold into the parent session's per-model totals. `jsonl-paths` lists the source files contributing to each session (parent first, then subagents) so debugging and future re-titling can find the original data without re-deriving the encoded path.

### API Design

CLI:

```
cr [OPTIONS] [SUBCOMMAND]

  Subcommands:
    collect  Walk ~/.claude/projects, fold, title, write YAML report (default)
    render   Render YAML report to markdown (default) or PDF (--pdf)
    merge    Combine per-host reports into one (NOT YET IMPLEMENTED in v1)

  Global options:
    -l, --log-level <LEVEL>     error|warn|info|debug|trace [default: info]
    -h, --help
    -V, --version

  collect options (also used when no subcommand is given):
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
        --template <PATH>       force the markdown-template path; suppresses the pmt+Opus default
        --prompt <PATH>         override the baked-in default report.pmt
        --include-tradeoffs     emit the optional Tradeoffs section in the rendered writeup
        --pdf-engine <NAME>     pandoc PDF engine [default: wkhtmltopdf]
```

Bare `cr` is sugar for `cr collect` with all defaults. The `Collect` subcommand is the default-when-bare; `cr merge` is a stub that returns "not implemented".

`cr render` defaults to the pmt-via-Opus path: prompt resolution is `--prompt <path>` > workspace `./templates/report.pmt` > the binary's baked-in default (`include_str!("../templates/report.pmt")`). Passing `--template <path>` forces the older markdown-template path instead.

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

- Add deps via `cargo add`: chrono (with `serde`), rayon, serde_json, gethostname, wait-timeout, ureq.
- Confirm crate-root `#![deny(...)]` is in place (already from scaffold).
- Stub modules: empty `lib.rs`, `scan.rs`, `parse.rs`, `session.rs`, `repo.rs`, `report.rs`, `title.rs`, `pricing.rs` (carries the embedded JSON pricing table; see Phase 1b), `persona.rs`, `summarize.rs`, `render.rs`. Re-export from `lib.rs`. Adapt existing `cli.rs` and `config.rs`.
- Verify `otto ci` passes on the empty skeleton.

#### Phase 1b: Pricing
**Model:** opus

- `cr/src/pricing.rs` carries the pricing table directly (no shared crate yet). `data/pricing.json` is the canonical embedded form and is consumed via `include_str!`; the binary stays self-contained with no runtime fetch.
- Extracting pricing into a shared `claude-pricing` crate (so cr and ccu can both consume one source) remains a follow-up: the cost of two parallel scrapers without a sync script outweighs the cost of two parallel pricing tables today. Defer until either binary actually produces a duplicate-fix incident.
- API: `calculate_usd(model: &str, usage: &TokenUsage) -> Result<f64, UnknownModel>`. The `Err` branch keeps unknown-model handling explicit; consumers cannot silently collapse to `0.0`.
- **Unknown model handling.** cr surfaces the gap in two layers:
  1. Log a warning at `warn` level: `unknown model '<name>'; spend reported as untracked`.
  2. Surface the gap in the report itself, not just stderr. Schema (tristate):
     - `totals.models` stays canonical: every model the user ran appears here with its full token breakdown (input / output / cache).
     - For untracked models, `models.<name>.spend-usd` is `null` (number = priced, `null` = tokens counted but unpriced, missing = model didn't appear).
     - `totals.untracked-models: [name, ...]` is the deduped union of per-session unknowns. Per-session `untracked-models` flags exactly which models contributed unpriced tokens in that session.
     - Per-session `spend-usd` is `Some(partial)` when at least one model is priced and `None` when every model is untracked, so a partial price can never silently look like a zero charge.
- Tests: known-model pricing math, unknown-model returns `Err`, mixed-session shape (partial spend + per-session `untracked-models`), `Totals.untracked-models` is the deduped union across sessions, YAML round-trip with `null` reads back as `Option::None`.

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
- Run `git -C <cwd> rev-parse --show-toplevel`. **Reject blocked-roots climbs:** the implementation maintains a list of blocked toplevel roots (default: `$HOME`). If the toplevel matches a blocked root, treat as not-a-repo and return `None`. Without this guard, a session in `~/scratch/foo` whose parent `~` is itself tracked (e.g. dotfiles repo at `$HOME`) would be falsely attributed to the dotfiles slug. The blocked-roots approach catches dotfiles climbing without rejecting legitimate climbs (e.g. cwd in a deep subdir of a tracked repo).
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

- `cr render` reads `claude-report.yml` and produces a justification document. The default flow wraps the report in a context block (persona + options + report) and sends it to Opus with the baked-in `templates/report.pmt`. `--pdf` then shells out to `pandoc` to convert the markdown to PDF.
- Persona block: `cr render` shells out to `persona whoami --json` with a 5s timeout (`wait-timeout` crate). On success the parsed fields go under `persona:` in the context block; on missing binary, timeout, non-zero exit, or JSON parse error, render emits `persona: {}` and prints a single stderr line ("persona whoami failed; rendering anonymously"). The pmt is written to handle both shapes.
- Options block: `--include-tradeoffs` becomes `options.include-tradeoffs: <bool>` in the context block. When `true` the pmt emits its optional Tradeoffs section; when `false` (the default) the section is dropped.
- Prompt resolution when sending to Opus: `--prompt <path>` > workspace `./templates/report.pmt` > the binary's baked-in copy (`include_str!("../templates/report.pmt")`). A build-time invariant test asserts the embedded copy is byte-identical to the workspace template at compile time. The workspace override exists so local edits to the pmt take effect at runtime without rebuilding.
- Custom markdown template (legacy path): `--template <path>` forces the older Tera-style template substitution and skips Opus. Useful for quick offline renders without API access.
- PDF path: write markdown to a `NamedTempFile`, run `pandoc <tmp> --pdf-engine=<engine> -o <output>.pdf`. Default engine is `wkhtmltopdf` (lighter than LaTeX, no 1GB+ texlive install required). User can override with `--pdf-engine=<name>`. Detect missing `pandoc` and surface a clear error directing the user to install it. PDF output path defaults to `./claude-report.pdf`.
- Output `-` (dash) goes to stdout (markdown only; PDF binary on stdout is rejected).
- Tests: context-block construction (persona present/absent, options.include-tradeoffs true/false), prompt-resolution order, baked-in default == workspace template invariant, custom-template substitution, PDF test gated on pandoc availability.

## Alternatives Considered

### Alternative 1: Extend ccu instead of building cr
- **Description:** Add a `ccu sessions` subcommand that emits the same YAML.
- **Pros:** One binary, no second CLI to install or document.
- **Cons:** ccu is firmly framed as a *cost* tool with a live-statusline performance budget (no network calls, sub-100ms). Sessions-as-data is a different concern: cr will eventually call Anthropic for titling and shell out to pandoc for rendering, neither of which is acceptable on ccu's hot path. The CLI surfaces are different products even when the underlying data is the same.
- **Why not chosen:** Two CLIs, two UX concerns. Code reuse for shared logic, when it eventually pays off (a `claude-pricing` crate and possibly a `claude-jsonl` parser crate), happens at the library layer without merging the binaries.

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
- `ureq` (added): Anthropic API HTTP client (Haiku titling, Opus rendering).
- `wait-timeout` (added): bound the `persona whoami` shell-out so an expired Okta session can't hang `cr render`.

No `git2`/`gix`. We shell out to `git remote get-url origin`: one call per distinct cwd, result cached. Pulling a 1MB+ git library for one URL would be disproportionate. Same for `persona`: `Command::spawn()` with a `wait_timeout` is simpler than reimplementing the persona client.

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
| Stale binary on user's machine encounters a model released after install | Med | Med (silent under-counting if mishandled) | Two-layer surface: `pricing::calculate_usd` returns `Err(UnknownModel)` rather than `0.0`; cr logs a `warn` AND emits an `untracked-models` block (per-session and totals) in the YAML so the rendered report explicitly calls out the gap instead of burying it. A stale binary cannot silently produce an undercounted total. |
| Anthropic redesigns their pricing page (DOM/format change) | High over multi-year | Med | Pricing data lives in `data/pricing.json` and is updated by hand for now. When two binaries (cr + ccu) actually drift, factor pricing into a shared crate with one scraper. Until then the duplication is small and the sync cost is "edit one JSON file". |

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
