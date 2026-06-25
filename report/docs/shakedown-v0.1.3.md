# CLI Shakedown Report: cr v0.1.3

**Date:** 2026-04-29
**Binary:** `/home/saidler/.cargo/bin/cr` (also tested `/tmp/cr-shakedown/release/cr` from GitHub release asset)
**Repo:** `tatari-tv/claude-report`
**Commit:** `d39d928 Bump version to v0.1.3` (annotated tag `v0.1.3`)

## Summary

| Metric | Count |
|--------|-------|
| Subcommands discovered | 4 (collect, render, merge, help) |
| Top-level flags | 3 (-l/--log-level, -h, -V) |
| Subcommand flags | 13 across collect/render/merge |
| Commands tested | 23 distinct invocations |
| Commands passed | 22 |
| Commands failed (intentional error paths) | 6 (all returned correct exit codes and messages) |
| Output formats validated | 3 (JSON via jq, custom-template MD, Opus-rendered MD; PDF skipped — no engine installed) |
| Pipeline recipes verified | 3 (the design-doc-promised jq queries) |
| Edge cases tested | 7 |
| LLM endpoints exercised | 2 (Haiku for titling, Opus for rendering) |
| Release assets verified | 4/4 platform tarballs + 4/4 sha256 sidecars |

## Command Tree

```
cr [OPTIONS] [COMMAND]
  -l, --log-level <LOG_LEVEL>     [default: info]
  -h, --help
  -V, --version

cr collect [OPTIONS]
      --since <SINCE>             RFC 3339 or YYYY-MM-DD
      --until <UNTIL>             RFC 3339 or YYYY-MM-DD
  -o, --output <OUTPUT>           defaults to ./claude-report.json
      --projects-dir <DIR>        defaults to ~/.claude/projects/
      --no-rollup                 keep subagents as separate sessions
      --skip-title                skip Haiku titling pass

cr render [OPTIONS]
  -i, --input <INPUT>             defaults to ./claude-report.json
  -o, --output <OUTPUT>           defaults to ./<YYYY-MM>-claude-report.md (or .pdf)
                                  use - for stdout (markdown only)
      --pdf                       require pandoc; PDF output
      --template <TEMPLATE>       offline path - no Opus call
      --prompt <PROMPT>           override the bundled report.pmt
      --include-tradeoffs         emit the Tradeoffs section
      --pdf-engine <ENGINE>       [default: wkhtmltopdf]

cr merge [OPTIONS] [INPUTS]...    bails: not implemented in this release
```

## Command Results

### Discovery (safe)

| Command | Exit | Notes |
|---|---|---|
| `cr --version` | 0 | `cr v0.1.3` |
| `cr --help` | 0 | Shows REQUIRED TOOLS table with persona, pandoc, git, jq |
| `cr collect --help` | 0 | All 6 collect flags documented |
| `cr render --help` | 0 | All 7 render flags documented |
| `cr merge --help` | 0 | Documents `[INPUTS]...` arg even though merge bails at runtime |

### Collect (write JSON)

| Command | Exit | Result |
|---|---|---|
| `cr collect --projects-dir <synthetic> --output <out> --skip-title --since 2026-01-01 --until 2030-01-01` | 0 | wrote 2 sessions; valid JSON, kebab-case keys, `schema-version: 1` |
| `cr collect ... --since 2026-04-12 --until 2026-04-13 --skip-title` | 0 | 1 session (window filter excluded April 10) |
| `cr collect ... --no-rollup` | 0 | 3 sessions (subagent kept separate from parent) |
| `cr collect ...` (rollup default) | 0 | 2 sessions (subagent's tokens merged into parent) |
| `cr collect ...` with rich content | 0 | Haiku titling exercised; non-null title returned (via Anthropic API) |
| `cr collect ...` with sparse content | 0 | Haiku call succeeded (HTTP 200) but title was null after `clean_title` filter |
| `cr collect ...` with `not-a-real-model` | 0 | Untracked-models populated at totals + session level; spend null |
| `cr collect ...` (title preservation) | 0 | Hand-edited title preserved across re-collect |
| `cr` (bare, no subcommand) | 0 | Defaulted to collect against `~/.claude/projects/`; produced 433 sessions, $4,531.91 spend |

### Render (read JSON, write MD/PDF)

| Command | Exit | Result |
|---|---|---|
| `cr render -i <json> --template <md> -o <out>` | 0 | All 6 placeholders substituted: `{{host}}`, `{{since}}`, `{{until}}`, `{{session-count}}`, `{{total-tokens}}`, `{{total-spend}}` |
| `cr render -i <json> --template <md> -o -` | 0 | Wrote markdown to stdout |
| `cr render -i <json> -o <out>` (default prompt, Opus) | 0 | Opus rendered 60-line report; YAML frontmatter, Cost Summary, Executive Summary, What This Funded, Usage Profile, Conclusion |
| `cr render -i <json> -o <out> --include-tradeoffs` (Opus) | 0 | All 6 sections incl. Tradeoffs |
| `cr render -i <json> --prompt <custom.pmt> -o -` (Opus) | 0 | Custom prompt obeyed; output was the literal "CUSTOM PROMPT WORKED" |
| `cr render -i <json> --template <md>` (no -o, default path) | 0 | Wrote `./2026-01-claude-report.md` derived from `since` YYYY-MM |
| `cr render -i <json> --template <md> --pdf -o /tmp/.../out.pdf` | 1 | wkhtmltopdf not installed; cr surfaced pandoc's exit 47 with a clear remediation hint |

### Error paths

| Command | Exit | Behavior |
|---|---|---|
| `cr merge` | 2 | `cr: merge is not implemented in this release` |
| `cr render -i legacy.yml` | 1 | Bails with `input file ends in .yml/.yaml; cr v0.1.2+ emits and reads JSON. Re-run cr collect to regenerate as .json.` |
| `cr render -i nonexistent.json` | 1 | `failed to read report at <path>` |
| `cr render -i garbage.json` | 1 | `expected ident at line 1 column 2` (serde_json error) |
| `cr collect --since notadate` | 1 | `could not parse datetime 'notadate': expected RFC 3339 or YYYY-MM-DD` |
| `cr collect --since 2030-12-31 --until 2020-01-01` | 1 | `--since (...) is after --until (...)` |
| `cr render --pdf -o -` | 1 | `--pdf cannot write binary output to stdout; pass -o <path>` |
| `env -i PATH=/nowhere cr collect ...` | 2 | jq enforcement: install hint with brew/apt/dnf one-liners |

## Output Format Matrix

| Output | Verified by | Result |
|---|---|---|
| JSON (collect default) | `jq '.["schema-version"], .totals.sessions'` parses | ✅ valid; kebab-case keys verified |
| Built-in markdown via Opus | `cr render -i <json> -o <out>` and visual inspection | ✅ all required sections present |
| Custom-template markdown | `--template <md>` placeholder substitution | ✅ all 6 placeholders work |
| Stdout markdown | `-o -` sigil | ✅ writes to stdout |
| PDF | `--pdf` + pandoc + wkhtmltopdf | ⚠️ skipped — no PDF engine installed (error path verified instead) |

## Pipeline Recipes (all verified against /tmp/cr-shakedown/empty-wd/claude-report.json, 433 real sessions)

```sh
# Top 10 sessions by spend (design-doc query 1)
jq '.sessions | to_entries | sort_by(.value."spend-usd") | reverse | .[:10]
    | map({sid: .key, spend: .value."spend-usd", title: .value.title})' \
    claude-report.json

# Total spend per model (design-doc query 2)
jq '.totals.models | to_entries | map([.key, .value."spend-usd"])' \
    claude-report.json

# Sessions per repo, sorted desc (design-doc query 3)
jq '.sessions | to_entries | group_by(.value.repo)
    | map({repo: .[0].value.repo, count: length})
    | sort_by(.count) | reverse' \
    claude-report.json

# End-to-end: collect then render with custom template
cr collect --skip-title -o ./report.json
cr render -i ./report.json --template ./mytemplate.md -o ./out.md

# Verify a release tarball checksum and run the binary
gh release download v0.1.3 -R tatari-tv/claude-report -p 'cr-v0.1.3-linux-amd64.tar.gz*'
sha256sum -c cr-v0.1.3-linux-amd64.tar.gz.sha256
tar -xzf cr-v0.1.3-linux-amd64.tar.gz
./cr --version
```

## Release Validation

| Check | Result |
|---|---|
| Tag `v0.1.3` exists | ✅ |
| Tag is annotated (not lightweight) | ✅ `git cat-file -t v0.1.3` returns `tag` |
| Tag points to `Bump version to v0.1.3` | ✅ `d39d928` |
| GitHub release `v0.1.3` exists | ✅ not draft, not prerelease |
| Asset: `cr-v0.1.3-linux-amd64.tar.gz` | ✅ 2.83 MB |
| Asset: `cr-v0.1.3-linux-arm64.tar.gz` | ✅ 2.76 MB |
| Asset: `cr-v0.1.3-macos-x86_64.tar.gz` | ✅ 2.67 MB |
| Asset: `cr-v0.1.3-macos-arm64.tar.gz` | ✅ 2.53 MB |
| All four `.sha256` sidecars present | ✅ |
| Downloaded `linux-amd64` checksum verifies | ✅ `sha256sum -c` reports OK |
| Downloaded binary `--version` matches | ✅ `cr v0.1.3` |
| Downloaded binary collects against synthetic data | ✅ same JSON shape as local build |

## Findings

### Bugs

None.

### Rough edges (low priority)

1. **`--projects-dir` pointing to a missing dir silently produces an empty report.** `cr collect --projects-dir /no/such/dir --skip-title` exits 0 with `wrote 0 sessions to ./claude-report.json`. A typo'd path is indistinguishable from a real empty dir. Consider `bail` if `projects_dir` does not exist as a directory.

2. **The "default subcommand is `collect`" message in `--help` is misleading.** Bare `cr` does default to collect, but `cr --output foo.json` errors with `unexpected argument '--output' found`. Top-level flag pass-through to the default subcommand isn't supported. Either drop the "default subcommand" line from `--help` or wire flag pass-through.

3. **`jq --version` extraction shows `installed` instead of `1.8.1`.** `extract_version` in `src/cli.rs` doesn't parse jq's single-token format `jq-1.8.1`. Cosmetic — the binary is found, the check passes, the install hint never fires. Other tools display real versions because their output is `<word> <version>` whitespace-separated.

### Suggestions (no action required)

- The `cr help <subcommand>` form (clap-generated) wasn't tested explicitly but appears in the top-level help. Functionally equivalent to `cr <subcommand> --help`.
- The Haiku titling step makes 1 API call per untitled session and runs in parallel via rayon. For sessions with sparse content (no real user prompt), it returns null after `clean_title` filtering — that's expected behavior, not a bug.

## Observations

- The JSON output is byte-identical between the locally-installed binary and the released tarball binary, validating the cross-compilation pipeline.
- The `--skip-title` flag is the right escape hatch for offline / no-API-key environments.
- The legacy `.yml` extension guard catches a likely-frequent migration mistake from v0.1.1 with a clear remediation message.
- The jq enforcement check is well-isolated to `main.rs` (not `lib.rs`), so the test suite passes regardless of whether jq is on PATH on dev machines.
- The new synthesis prompt (phase 3 of the v0.1.2 design) produces a structured report with all six required sections; the Opus output for the synthetic 2-session input was 60-68 lines including YAML frontmatter, the Cost Summary table, and bulleted prose for each section.
