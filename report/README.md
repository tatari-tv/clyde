# report

Scans Claude Code session JSONL files and emits a per-host JSON report, plus a synthesized
markdown writeup. A member crate of the [`clyde`](../README.md) umbrella workspace.

- Umbrella: `clyde report <collect|render|merge>`

Library API: `report::{ReportArgs, ReportCli, run}`. See the top-level README and
`docs/design/2026-06-24-clyde-umbrella-cli.md` for the umbrella architecture.

## Render output formats

`report render` turns a collected JSON report into one of four output formats, selected with
`--format` (case-insensitive):

| `--format`         | what it does                                                             | requires            |
|--------------------|--------------------------------------------------------------------------|---------------------|
| `markdown` (default) | writes Markdown to `-o <path>`, to stdout (`-o -`), or to `./<YYYY-MM>-claude-report.md` | —                   |
| `pdf`              | converts the Markdown to PDF via pandoc (`--pdf-engine`, default `wkhtmltopdf`) | `pandoc` + a PDF engine |
| `marquee-markdown` | publishes the Markdown as `index.md` to [marquee](https://github.com/tatari-tv/marquee); marquee applies its house style | `marquee` CLI (authenticated) |
| `marquee-html`     | converts the Markdown to self-contained HTML (`pandoc -s --embed-resources`) and publishes it as `index.html` | `pandoc` + `marquee` CLI (authenticated) |

```bash
clyde report render                              # Markdown (default)
clyde report render --format pdf -o report.pdf
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
  format: marquee-markdown   # markdown | pdf | marquee-html | marquee-markdown
```

With the above, a bare `clyde report render` publishes to marquee, while `--format markdown` still
overrides back to a local Markdown file for a single run. An absent file (or absent `render:`
section) leaves the default at `markdown`.
