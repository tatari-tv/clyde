# report

Scans Claude Code session JSONL files and emits a per-host JSON report, plus a synthesized
markdown writeup. A member crate of the [`clyde`](../README.md) umbrella workspace.

- Umbrella: `clyde report <collect|render|merge>`

Library API: `report::{ReportArgs, ReportCli, run}`. See the top-level README and
`docs/design/2026-06-24-clyde-umbrella-cli.md` for the umbrella architecture.

## Render output formats

`report render` turns a collected JSON report into one of five output formats, selected with
`--format` (case-insensitive). The formats split into two source families: `markdown`/`pdf`/
`marquee-markdown` render the report as Markdown first (template or LLM); `html`/`marquee-html`
skip Markdown entirely and have the model author a complete, self-contained HTML dashboard
directly from the same report data. Pandoc is only ever invoked for `pdf`.

| `--format`         | source   | what it does                                                             | `-o`     | pandoc |
|--------------------|----------|---------------------------------------------------------------------------|----------|--------|
| `markdown` (default) | markdown | writes Markdown to `-o <path>`, to stdout (`-o -`), or to `./<YYYY-MM>-claude-report.md` | yes | no |
| `pdf`              | markdown | converts the Markdown to PDF via pandoc (`--pdf-engine`, default `wkhtmltopdf`) | yes | **yes** |
| `marquee-markdown` | markdown | publishes the Markdown as `index.md` to [marquee](https://github.com/tatari-tv/marquee); marquee applies its house style | rejected | no |
| `html`             | html     | writes a self-contained, model-authored HTML dashboard to `-o <path>`, to stdout (`-o -`), or to `./<YYYY-MM>-claude-report.html` | yes | no |
| `marquee-html`     | html     | publishes the same model-authored HTML dashboard as `index.html` to marquee | rejected | no |

`pdf` requires `pandoc` on `PATH`; `marquee-*` require the `marquee` CLI with an authenticated
session; `html`/`marquee-html` require `ANTHROPIC_API_KEY` (no offline path; `--template`
is rejected for these two formats since it produces Markdown, not HTML).

```bash
clyde report render                              # Markdown (default)
clyde report render --format pdf -o report.pdf
clyde report render --format html                # writes ./<YYYY-MM>-claude-report.html
clyde report render --format marquee-markdown    # prints the published URL to stdout
url=$(clyde report render --format marquee-html --space eng)
```

- The `marquee-*` variants print the published **URL to stdout** (the status line goes to stderr),
  so `url=$(clyde report render --format marquee-html)` captures it. Use `--space <space>` to
  target a marquee space other than your personal one.
- `-o`/`--output` is rejected with a `marquee-*` format — the output is a URL, not a file.
- **marquee auth:** render probes `marquee whoami`. If you are not logged in *and* you are on an
  interactive terminal, it runs `marquee login` once, then retries. In a non-TTY context (SSH
  without a tty, CI, an agent) it does **not** launch the interactive flow — it errors and tells
  you to run `marquee login` yourself, so a headless render can never hang on a login prompt.

## Default format via `clyde.yml`

The default `--format` (used when the flag is omitted) can be set in
`$XDG_CONFIG_HOME/clyde/clyde.yml`. Precedence is the usual **CLI flag > config > built-in**
(`markdown`):

```yaml
# ~/.config/clyde/clyde.yml
render:
  format: marquee-markdown   # markdown | pdf | html | marquee-html | marquee-markdown
```

With the above, a bare `clyde report render` publishes to marquee, while `--format markdown` still
overrides back to a local Markdown file for a single run. An absent file (or absent `render:`
section) leaves the default at `markdown`.
