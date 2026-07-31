# report

Scans Claude Code session JSONL files and emits a per-host JSON report, plus a synthesized
markdown writeup. A member crate of the [`clyde`](../README.md) umbrella workspace.

- Umbrella: `clyde report <collect|render|merge>`

Library API: `report::{ReportArgs, ReportCli, run}`. See the top-level README and
`docs/design/2026-06-24-clyde-umbrella-cli.md` for the umbrella architecture.

## What the dollar figures mean

Every rendered artifact carries this disclosure verbatim, next to the total-spend figure it
qualifies:

> Total spend is modeled Claude Code catalog spend at published list rates; account-level billed
> spend comes from Claude Enterprise Analytics.

Every dollar amount this crate emits is **modeled**, not billed. It is token counts multiplied by
Anthropic's published per-token list rates, sourced from clyde's own pricing feed at
<https://tatari-tv.github.io/clyde/> (embedded baseline as the fallback; see `pricing/CLAUDE.md`).
Rates and the cache multipliers it applies (1.25x for a 5m cache write, 2x for 1h, 0.1x for a read)
come from <https://platform.claude.com/docs/en/about-claude/pricing.md>.

**Tatari is on Claude Enterprise, and the authoritative spend figure is the Claude Enterprise
Analytics cost report**, not this crate. Pull the PER-USER one with the `anthropic-usage-report`
skill (`pull-usage-report.py --report user-cost --start <since> --end <until>`); it needs an
owner-created Analytics key with `read:analytics` scope, which clyde itself never holds.

So when a report says `$9,450.31`, read it as "this much token consumption, priced at list." Two
figures can legitimately differ from it:

- **The Analytics cost report for the same person** is the real number. It covers everything that
  account was billed across every Claude product, including claude.ai web, Cowork and other clients
  and hosts, so `billed >= modeled` is the expected relationship and a positive difference means
  usage clyde cannot see, not a miscount.
- **Per-model spend inside a report** is re-priced from the fetched feed, while the catalog's
  `efficiency_json` stores costs at the **embedded** baseline on purpose, so a persisted value stays
  reproducible on a later reindex regardless of network state (`efficiency/src/metrics.rs`).

`report render --reconcile <analytics-export.json>` does the comparison for you and prints billed,
modeled and `unseen-account-spend` in the artifact:

```bash
python3 ~/.claude/skills/anthropic-usage-report/pull-usage-report.py \
  --report user-cost --start 2026-06-26T00:00:00Z --end 2026-07-25T00:00:00Z
clyde report render --reconcile enterprise-user-cost-2026-06-26-2026-07-25.json
```

Three things that surprise people:

- **The export must be `--report user-cost`, not `--report cost`.** `clyde report` reads one user's
  sessions on one machine, so the only billed figure it can honestly set beside its own total is
  that same user's. The org-wide export is rejected by name: it bills every seat in the
  organization, so setting it beside one operator's modeled total publishes the rest of the
  company's Claude usage as spend clyde failed to account for -- on a real window that dwarfed the
  operator's own figure by more than an order of magnitude. Scoped to the operator, the same window
  reconciles to partial coverage with an explainable remainder.
- **The operator is the persona's work email** (`persona whoami`), the same identity the report's
  persona block carries. Override it with `--reconcile-user <email>`. An export with no row for that
  person is a hard error: never a silent `$0.00` billed, never a fallback to the org total.
- **The window is checked, and a `user-cost` export does not state its own.** The per-user endpoints
  return `starting_at`/`ending_at` as null, so the period is read from the filename
  `pull-usage-report.py` writes (`enterprise-user-cost-<start>-<end>.json`) and compared against the
  report's `--since`/`--until`. Rename the file and the render fails rather than compare two
  possibly different periods.

Amounts on the Analytics cost endpoints are decimal-string **cents** (`"41280.000000"` is `$412.80`);
`reconcile` divides by 100 exactly once. `billed` reads the export's `amount` (what the account was
actually billed), not `list_amount`.

## Render output formats

`report render` turns a collected JSON report into one of three output formats, selected with
`--format` (case-insensitive). All three share ONE source: Rust deterministically authors the
Markdown document -- every table, every figure, every chart -- and the LLM fills a small fixed set of
prose slots inside it. Pandoc is only ever invoked for `pdf`.

| `--format`         | what it does                                                             | `-o`     | pandoc |
|--------------------|---------------------------------------------------------------------------|----------|--------|
| `markdown` (default) | writes Markdown to `-o <path>`, to stdout (`-o -`), or to `./<YYYY-MM>-claude-report.md` | yes | no |
| `pdf`              | converts the Markdown to PDF via pandoc (`--pdf-engine`, default `wkhtmltopdf`) | yes | **yes** |
| `marquee-markdown` | publishes the Markdown as `index.md` to [marquee](https://github.com/tatari-tv/marquee); marquee applies its house style | rejected | no |

There is no HTML format and no HTML authorship anywhere in clyde: markdown to HTML is marquee's job.
`pdf` requires `pandoc` on `PATH`; `marquee-markdown` requires the `marquee` CLI with an
authenticated session.

An LLM transport is **optional**. With none available the data sections render in full and the prose
slots come out empty, with a WARN on stderr -- a render cannot fail whole-artifact. When it IS
present it needs no credential at all, shelling out to the locally installed `claude` CLI and using
the Claude Code login you already have (see [LLM transport](#llm-transport) below).

## LLM transport

The prose slots and the `report eval` judge go through the one LLM transport clyde has: it shells
out to the local `claude` CLI in headless print mode, using the Claude Code login you already have.
clyde never reads, stores, refreshes, or transmits a credential: the `claude` binary owns auth end to
end, the same way `marquee` owns its own Okta tokens.

Configure the model pins and the output ceilings in `clyde.yml`:

```yaml
# ~/.config/clyde/clyde.yml
render:
  model: claude-opus-4-8           # model pin for the prose slots and the eval judge
  slot-max-output-tokens: 1500     # output ceiling for ONE prose slot
  judge-max-output-tokens: 32000   # output ceiling for the eval judge
```

Precedence is the house convention: flag > config > default.

One `model` pin covers both jobs. The key is deliberately not named `markdown-model`: that name
belonged to the whole-document authoring job, which no longer exists.

The slot ceiling is small on purpose. A slot is a few sentences of digit-free prose, so 1500 leaves
generous headroom while still catching a model that starts writing a whole document instead.

The ceiling cannot be put on the wire as a hard cap: the transport instead compares the reported
`usage.output_tokens` against it after the fact. The artifact is therefore complete when it is
refused, and the tokens are already billed -- so raise the key the error names rather than
re-running. `0` is rejected at config load.

### Two things to know before you rely on it

**There is no fallback if `claude` is missing or broken.** Resolving `claude` on PATH is a presence
check, never a success check. If the CLI is present but fails -- logged out, stale install, rate
limited, plan cap -- the render **fails loudly** naming the install-and-login remedy. There is no
second transport to fall back to and none is silently picked, so a broken login surfaces immediately
instead of hiding behind a billing switch.

**The transport bills your Claude Code plan.** Renders draw on your personal plan's rate limits, so
several parallel renders can hit a per-user limit. There is no service-credential path to route
around that.

```bash
clyde report render                              # Markdown (default)
clyde report render --format pdf -o report.pdf
url=$(clyde report render --format marquee-markdown --space eng)   # prints the published URL
```

- `marquee-markdown` prints the published **URL to stdout** (the status line goes to stderr), so
  `url=$(clyde report render --format marquee-markdown)` captures it. Use `--space <space>` to
  target a marquee space other than your personal one.
- `-o`/`--output` is rejected with `marquee-markdown`: the output is a URL, not a file.
- **marquee auth:** render probes `marquee whoami`. If you are not logged in *and* you are on an
  interactive terminal, it runs `marquee login` once, then retries. In a non-TTY context (SSH
  without a tty, CI, an agent) it does **not** launch the interactive flow: it errors and tells
  you to run `marquee login` yourself, so a headless render can never hang on a login prompt.

## Default format via `clyde.yml`

The default `--format` (used when the flag is omitted) can be set in
`$XDG_CONFIG_HOME/clyde/clyde.yml`. Precedence is the usual **CLI flag > config > built-in**
(`markdown`):

```yaml
# ~/.config/clyde/clyde.yml
render:
  format: marquee-markdown   # markdown | pdf | marquee-markdown
```

With the above, a bare `clyde report render` publishes to marquee, while `--format markdown` still
overrides back to a local Markdown file for a single run. An absent file (or absent `render:`
section) leaves the default at `markdown`.

## Measuring render quality: `clyde report eval`

The same report JSON renders differently every month by design, so narrative quality is measured on
frozen fixtures rather than eyeballed. There are two layers, split by what they cost.

**Mechanical** checks are deterministic, offline and free, so they run in `otto ci` against the
committed golden artifacts under `fixtures/report/`: every cited repo, date and quoted phrase must
exist in the context, the required sections must be present and the forbidden ones absent, Hard
prohibition 2's phrase list and the em-dash must be absent, the foreign-number guard must be clean,
and every digit-bearing chart attribute must be one the binary computed.

**Judged** scoring costs tokens and needs a network, so it runs from `otto eval` before a release,
never in CI. A model scores a FRESH render 0 to 3 on citation accuracy, coverage of the top three
`by-repo` rows and the top agent type, prohibition compliance, and readability; a score below the
floor committed in that fixture's `eval.yml` exits non-zero.

```bash
otto eval                                              # the three committed fixtures, judged
clyde report eval --fixture fixtures/report/local      # a real month, locally, never committed
clyde report eval --write-goldens                      # regenerate the committed goldens
```

Every committed fixture is **synthesized** by a seeded generator (`cargo run -p report --bin
fixtures`), never derived from real session data: this repo is public, and session titles and enrich
summaries are the sensitive payload. Real-data evaluation reads `fixtures/report/local/`, which is
gitignored. See `fixtures/report/README.md` for the layout and the regeneration flow.
