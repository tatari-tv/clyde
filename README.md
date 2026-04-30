# claude-report

Rust CLI that scans Claude Code's session JSONL files and emits a queryable JSON report, plus a synthesized markdown writeup rendered via Opus. The binary is named `cr` (the package is `claude-report`).

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/tatari-tv/claude-report/main/install.sh | bash
```

Installs to `~/.local/bin` by default. Override with `INSTALL_DIR`:

```bash
INSTALL_DIR=/usr/local/bin curl -fsSL https://raw.githubusercontent.com/tatari-tv/claude-report/main/install.sh | bash
```

### From Source

```bash
cargo install --git https://github.com/tatari-tv/claude-report
```

---

## Problem

Claude Code accumulates session history in `~/.claude/projects/` as one JSONL file per session. Each line carries token usage, model, repo context, and timestamps. After a month of work you have hundreds of sessions across dozens of repos, and answering basic questions takes a custom script every time:

- What was my total spend this month, per model?
- Which repos absorbed the most spend?
- What are the top 10 sessions by cost?
- Can I produce a writeup explaining what the spend funded, suitable for management or finance review?

`ccu` covers the dollar number. `cr` goes further: it folds the JSONL into a structured report you can query with `jq`, and renders an Opus-synthesized markdown writeup that names what the work produced - repos, themes, outliers - without enumerating every session as a dollar-tagged bullet.

---

## Setup

### Required tools

`cr --help` shows which of these are present on your system:

| Tool | Used for |
| --- | --- |
| `jq` | required - queries the JSON report. `cr collect` refuses to run without it. |
| `git` | `cr collect` repo detection (resolves session `cwd` to a `<owner>/<repo>` slug). |
| `persona` | `cr render` includes a persona block in the Opus context. Optional - falls back to anonymous. |
| `pandoc` | `cr render --pdf` only. Skip if you only produce markdown. |

### API key

`cr render` (Opus) and `cr collect`'s session titling step (Haiku) call the Anthropic API. Set:

```bash
export ANTHROPIC_API_KEY=sk-ant-...
```

To skip the API entirely, pass `--skip-title` to `collect` and `--template <path>` to `render` (offline markdown via placeholder substitution).

---

## Commands

### collect

Scan `~/.claude/projects/`, fold subagents into parent sessions, compute per-model spend, and write the JSON report.

```bash
# Default: ~/.claude/projects -> ./claude-report.json, current month to now
cr collect

# Explicit window and output path
cr collect --since 2026-04-01 --until 2026-04-30 -o ./april.json

# Offline (no Haiku titling pass)
cr collect --skip-title

# Keep subagents as their own sessions instead of rolling them up
cr collect --no-rollup

# Override the projects directory (e.g. a snapshot)
cr collect --projects-dir /tmp/cr-baseline-snap
```

**Title preservation.** `cr collect` reads the existing report at the output path and carries forward any titles already present, including titles you've hand-edited. Re-running on the same path is idempotent for titles.

**Untracked models.** When the embedded `claude-pricing` table doesn't recognize a model, the session and the totals each surface a `untracked-models` array, and `spend-usd` is `null` for that model. The markdown render flags this in a single bolded sentence below the Cost Summary table.

### render

Read the JSON report, produce markdown (default) or PDF.

```bash
# Default: read ./claude-report.json -> ./<YYYY-MM>-claude-report.md via Opus
cr render

# Specific input
cr render -i ./april.json

# Stdout
cr render -i ./april.json -o -

# Offline: custom template with placeholder substitution
cr render -i ./april.json --template ./mytemplate.md -o ./out.md

# Custom prompt instead of the bundled templates/report.pmt
cr render -i ./april.json --prompt ./myprompt.pmt

# Include the optional Tradeoffs section
cr render -i ./april.json --include-tradeoffs

# PDF (requires pandoc + a PDF engine)
cr render -i ./april.json --pdf -o ./april.pdf --pdf-engine wkhtmltopdf
```

**Custom-template placeholders:** `{{host}}`, `{{since}}`, `{{until}}`, `{{session-count}}`, `{{total-tokens}}`, `{{total-spend}}`.

**Default output filename** is derived from the report's `since` field: `./<YYYY-MM>-claude-report.{md,pdf}`. Override with `-o`.

---

## Pipeline Recipes

The JSON report uses kebab-case keys throughout. All examples below run on `./claude-report.json`:

```sh
# Top 10 sessions by spend
jq '.sessions | to_entries | sort_by(.value."spend-usd") | reverse | .[:10]
    | map({sid: .key, spend: .value."spend-usd", title: .value.title})' \
    claude-report.json

# Total spend per model
jq '.totals.models | to_entries | map([.key, .value."spend-usd"])' \
    claude-report.json

# Sessions per repo, sorted desc
jq '.sessions | to_entries | group_by(.value.repo)
    | map({repo: .[0].value.repo, count: length})
    | sort_by(.count) | reverse' \
    claude-report.json

# Total spend, just the number
jq '.totals."spend-usd"' claude-report.json

# All sessions in a specific repo
jq '.sessions | to_entries
    | map(select(.value.repo == "tatari-tv/claude-report"))
    | map({title: .value.title, spend: .value."spend-usd"})' \
    claude-report.json
```

---

## Report Shape

```json
{
  "schema-version": 1,
  "generated": "2026-04-29T03:14:00Z",
  "host": "desk",
  "since": "2026-04-01T07:00:00Z",
  "until": "2026-04-29T07:00:00Z",
  "totals": {
    "sessions": 429,
    "spend-usd": 4437.78,
    "untracked-models": [],
    "models": {
      "claude-opus-4-7": {
        "input": 12345,
        "output": 67890,
        "cache-5m-write": 0,
        "cache-1h-write": 0,
        "cache-read": 0,
        "total": 80235,
        "spend-usd": 2108.63
      }
    }
  },
  "sessions": {
    "<session-uuid>": {
      "title": "...",
      "repo": "tatari-tv/...",
      "begin": "...",
      "end": "...",
      "spend-usd": 12.34,
      "untracked-models": [],
      "jsonl-paths": ["..."],
      "models": { "...": { "..." : "..." } }
    }
  }
}
```

The `Report`, `Totals`, `SessionEntry`, and `ModelTokens` structs round-trip cleanly via `serde_json`. Consumers should gate on `schema-version`.

---

## How It Works

- **Scan.** `cr collect` walks `~/.claude/projects/**/*.jsonl` and parses each file in parallel via rayon. Subagent files (under a `subagents/` subdirectory of the parent session) are folded into the parent session by default, or kept separate with `--no-rollup`.
- **Pricing.** Token counts are converted to spend using the embedded [`claude-pricing`](https://github.com/tatari-tv/claude-pricing) table. Models not in the table report `null` spend and surface in `untracked-models`.
- **Repo detection.** Each session's `cwd` is resolved to a `<owner>/<repo>` slug by walking up to a `.git` directory and parsing its `origin` remote. Cached per-cwd within a run.
- **Titling.** Untitled sessions get a 3-7 word lowercase title from a single Haiku call seeded with the first user prompt and the first assistant response. Hand-edited titles persist across re-runs because `cr collect` reads the prior report's titles before rewriting.
- **Rendering.** `cr render` ships a single prompt at [`templates/report.pmt`](templates/report.pmt) that enforces synthesis over enumeration: per-session dollar amounts are restricted to the per-repo summary line and the Outlier Sessions table, and quantification claims (cost-vs-engineer, hours-saved) are forbidden outright. Workspace edits to that file propagate immediately - the binary reads the on-disk template when present and falls back to the baked-in copy otherwise.

---

## Logs

`cr` writes to `~/.local/share/claude-report/logs/claude-report.log`. The CLI flag is `--log-level` / `-l` with values `error|warn|info|debug|trace`; `RUST_LOG` is intentionally not honored.

---

## Tips

- **Run `cr collect` first.** Every other command depends on the JSON report. Re-run any time you want fresh data.
- **`cr render --template <path>` for offline.** No API key needed, no network, no cost. Use the placeholders to extract a one-line summary into your statusline or shell prompt.
- **Hand-edit titles in the JSON.** Bad Haiku titles are routine for low-content sessions. Open the JSON, fix them, and re-run `cr collect` - your edits persist.
- **Pin a baseline.** Copy `claude-report.json` to a snapshot path before a month boundary so you can run `cr collect` against the snapshot for historical reproducibility.
- **`jq` is the API.** The markdown writeup is for humans; the JSON is for everything else. Build pipelines on top of `jq`, not on top of the markdown.
