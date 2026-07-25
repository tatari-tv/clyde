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
session; `html`/`marquee-html` need an LLM transport but **not** an API key -- by default they shell
out to the locally installed `claude` CLI and use the Claude Code login you already have (see
[LLM transport](#llm-transport) below). There is no offline path for them, and `--template`
is rejected for these two formats since it produces Markdown, not HTML.

## LLM transport

Both model-authored paths (the Markdown narrative and the HTML dashboard) go through one of two
transports, selected with `--llm`:

| `--llm` | what it uses | credential |
|---|---|---|
| `cli` | shells out to the local `claude` CLI in headless print mode | the Claude Code login you already have |
| `api` | posts to `api.anthropic.com` directly | `ANTHROPIC_API_KEY` |
| `auto` (default) | `cli` when `claude` is on `PATH`, else `api` when a key is set | whichever it picked |

`auto` prefers `cli`, so a render works out of the box for anyone with a working Claude Code and
requires no second credential. clyde never reads, stores, refreshes, or transmits a token: the
`claude` binary owns auth end to end, the same way `marquee` owns its own Okta tokens.

Configure the default and the model pins in `clyde.yml`:

```yaml
# ~/.config/clyde/clyde.yml
render:
  llm: auto                        # auto | api | cli    (default auto, which prefers cli)
  markdown-model: claude-opus-4-8  # model pin for the Markdown narrative
  html-model: claude-opus-4-8      # model pin for the HTML dashboard
```

Precedence is the house convention: flag > config > default.

### Three things to know before you rely on it

**There is no fallback once a transport is chosen.** Selection is a presence check on the `claude`
binary, never a success check. If the CLI is present but fails -- logged out, stale install, rate
limited, plan cap -- the render **fails loudly** naming `--llm api`, rather than silently switching
to your key. That is deliberate: a silent fallback would make one command nondeterministic across
two transports and two billing paths, and would hide a broken login indefinitely. To roll back to
the api path, it is one line:

```yaml
render:
  llm: api        # or --llm api per invocation
```

**Automated callers must pin the transport explicitly.** A CI image or cron host that has a `claude`
binary but no usable login will now fail even with a valid `ANTHROPIC_API_KEY` set, because `auto`
picks `cli` on presence alone. Any non-interactive caller should pass `--llm api` (or set
`render.llm: api`) rather than trusting `auto`.

**The cli transport bills your Claude Code plan, and costs more per render.** Measured on a real
1,310-session month: the CLI bills the ~242K-token payload as a 1-hour cache write at $10/Mtok where
the api path bills it as plain input at $5/Mtok, plus a small internal sub-call -- about **$2.93 via
cli vs ~$1.53 via api** for a Markdown render. Renders also draw on your personal plan's rate limits
rather than a service key's, so several parallel renders can hit a per-user limit. If you hold a key
and render in bulk on one machine, set `render.llm: api`.

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
