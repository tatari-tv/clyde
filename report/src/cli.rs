use crate::aggregate::DEFAULT_OUTLIERS;
use clap::{Args, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Output format for `report render`, selected via `--format` (case-insensitive, kebab-case).
/// `markdown`, `pdf`, and `html` write locally (see `-o`); the two `marquee-*` variants publish
/// to marquee and print the resulting URL instead of writing a file.
#[derive(ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[clap(rename_all = "kebab-case")]
pub enum Format {
    #[default]
    Markdown,
    Pdf,
    Html,
    MarqueeHtml,
    MarqueeMarkdown,
}

impl Format {
    /// The two publishing variants, whose output is a marquee URL rather than a local path.
    pub fn is_marquee(self) -> bool {
        matches!(self, Format::MarqueeHtml | Format::MarqueeMarkdown)
    }

    /// The two model-authored-HTML variants, which share the html-source render pipeline (context
    /// block -> `report-html.pmt` -> opus -> a complete HTML document; no pandoc) rather than the
    /// markdown-source pipeline every other format uses.
    pub fn is_html_source(self) -> bool {
        matches!(self, Format::Html | Format::MarqueeHtml)
    }
}

/// Which transport performs `report render`'s two model calls, selected via `--llm`.
///
/// `auto` (the default) prefers the `claude` CLI, so the keyless path is what everyone gets and an
/// api key is opt-in rather than the entry fee. This is the SELECTION, not the answer: `auto` still
/// has to be resolved against the environment (see `config::resolve_transport`).
#[derive(ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[clap(rename_all = "kebab-case")]
pub enum Llm {
    #[default]
    Auto,
    Api,
    Cli,
}

/// Map the `clyde.yml` `render.llm` config value onto the CLI [`Llm`]. Lives here (not in `common`)
/// because the mapping's target type is owned by this crate.
impl From<common::config::LlmConfig> for Llm {
    fn from(value: common::config::LlmConfig) -> Self {
        match value {
            common::config::LlmConfig::Auto => Llm::Auto,
            common::config::LlmConfig::Api => Llm::Api,
            common::config::LlmConfig::Cli => Llm::Cli,
        }
    }
}

/// Map the `clyde.yml` `render.format` config value onto the CLI [`Format`]. Lives here (not in
/// `common`) because the mapping's target type is owned by this crate.
impl From<common::config::FormatConfig> for Format {
    fn from(value: common::config::FormatConfig) -> Self {
        match value {
            common::config::FormatConfig::Markdown => Format::Markdown,
            common::config::FormatConfig::Pdf => Format::Pdf,
            common::config::FormatConfig::Html => Format::Html,
            common::config::FormatConfig::MarqueeHtml => Format::MarqueeHtml,
            common::config::FormatConfig::MarqueeMarkdown => Format::MarqueeMarkdown,
        }
    }
}

/// The report command surface, nested under `clyde report ...`. Derives `Args` (not `Parser`)
/// so it can be a `Subcommand` payload in the clyde umbrella; carries no common globals (clyde
/// owns `--log-level`).
#[derive(Args, Debug)]
pub struct ReportArgs {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Read the session catalog (`sessions.db`) and emit a per-host JSON usage report.
    ///
    /// Reads whole sessions whose catalog row falls in the `--since`/`--until` window (session-level,
    /// on `modified`) from `sessions.db` (or `--db`) — tokens, cost, cache/tool/agent-type efficiency
    /// signals, and outcomes, all catalog-sourced (no JSONL scan). With `-o <path>`, writes the JSON
    /// report to that file; without `-o`, streams the JSON to stdout so `report collect | jq` works.
    /// Fails (writing nothing) if any session in the window has not been indexed — run
    /// `clyde session reindex` first.
    Collect(CollectArgs),
    /// Render a collected JSON report into Markdown, PDF, HTML, or a published marquee post
    /// (`--format`).
    ///
    /// Reads the JSON produced by `collect` (default: `./claude-report.json`) and writes a
    /// human-readable Markdown summary. `--format pdf` converts it with the configured
    /// `--pdf-engine`; `--format html` writes a self-contained HTML file locally; `--format
    /// marquee-markdown` / `marquee-html` publish it to marquee and print the resulting URL.
    Render(RenderArgs),
    /// Merge two or more collected JSON reports into one.
    ///
    /// Unions sessions from all inputs, recomputes totals, widens the
    /// `since`/`until` window to the min/max across inputs, and tags the output
    /// with a multi-host marker.
    Merge(MergeArgs),
    /// Render every frozen fixture, run the mechanical checks, score with a judge, and write a
    /// scored report.
    ///
    /// Costs model calls: this is `otto eval`, not `otto ci`. The free, deterministic half of the
    /// same checks runs in `otto ci` against the committed golden artifacts, offline.
    ///
    /// With no `--fixture`, evaluates the three synthesized fixtures under `fixtures/report/`
    /// (run from the workspace root). `--fixture <dir>` replaces that set, which is how a real
    /// month is evaluated locally without the data entering git -- `fixtures/report/local/` is
    /// gitignored for exactly that. Exits non-zero when any fixture fails a mechanical check or
    /// scores below a floor its `eval.yml` commits to.
    Eval(EvalArgs),
}

#[derive(clap::Args, Debug)]
pub struct CollectArgs {
    /// Start of the collection window: RFC 3339 timestamp or `YYYY-MM-DD` date.
    /// When omitted, defaults to midnight on the first day of the current month.
    #[arg(long)]
    pub since: Option<String>,

    /// End of the collection window: RFC 3339 timestamp or `YYYY-MM-DD` date.
    /// When omitted, defaults to now.
    #[arg(long)]
    pub until: Option<String>,

    /// Write the JSON report to this path. When omitted, streams JSON to stdout so
    /// `report collect | jq` works.
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Override the session catalog path (default: `<xdg-data>/clyde/sessions.db`).
    #[arg(long)]
    pub db: Option<PathBuf>,

    /// Present the per-subagent breakdown as its own rows rather than the per-session rollup: the
    /// catalog already holds the canonical rollup, so this is a VIEW over each session's subagents
    /// (a parent-residual row plus one row per subagent), never a re-fold of JSONL.
    #[arg(long)]
    pub no_rollup: bool,

    /// Omit outcomes (commits, PRs opened, Confluence/Jira writes, Slack messages, files edited)
    /// from the report. Outcomes are read from the catalog (not rescanned); with this flag the
    /// produced report carries `outcomes-enabled: false` and no `outcomes` fields anywhere.
    /// Default: outcomes on.
    #[arg(long)]
    pub no_outcomes: bool,

    /// Warn when fewer than this FRACTION of the window's sessions carry an enrich summary
    /// (`0.5` = 50%). The report's themes cite session summaries, so a low-coverage window produces
    /// a narrative resting on titles instead; the warning states the gap and the artifact is still
    /// written. When omitted, falls back to `min-enrichment` in `clyde.yml`, and to `0.5` if that
    /// too is unset.
    #[arg(long)]
    pub min_enrichment: Option<f64>,
}

#[derive(clap::Args, Debug)]
pub struct RenderArgs {
    /// Path to the collected JSON report to render (default: `./claude-report.json`).
    #[arg(short, long)]
    pub input: Option<PathBuf>,

    /// Write rendered Markdown, PDF, or HTML to this path. When omitted, the output defaults to
    /// `./<YYYY-MM>-claude-report.{md,pdf,html}` in the current directory (month derived from the
    /// report's `since`). Not valid with the `marquee-*` formats, whose output is a published URL.
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Output format: `markdown`, `pdf`, `html`, `marquee-markdown`, or `marquee-html`. When
    /// omitted, falls back to the `render.format` value in `clyde.yml`, and to `markdown` if that
    /// too is unset. `markdown`/`pdf`/`html` write locally (see `-o`); the `marquee-*` variants
    /// publish to marquee and print the URL. `pdf` requires `pandoc`; the `marquee-*` variants
    /// require the `marquee` CLI with an authenticated session. `html`/`marquee-html` are
    /// model-authored (no pandoc involved), so they need an LLM transport but NOT an API key: by
    /// default they use the locally installed `claude` CLI and the Claude Code login you already
    /// have (see `--llm`). There is no offline path for them, and `--template` is not valid with
    /// `html`/`marquee-html` (the offline template produces markdown).
    #[arg(long, value_enum, ignore_case = true)]
    pub format: Option<Format>,

    /// Which transport performs the model calls: `auto` (the default) picks `cli` when `claude` is on
    /// PATH, else `api` when a key is set; `cli` shells out to the locally installed `claude` CLI and
    /// uses the Claude Code login you already have -- no API key needed; `api` uses
    /// `ANTHROPIC_API_KEY`.
    ///
    /// When omitted, falls back to `render.llm` in `clyde.yml`, and to `auto` if that too is unset.
    /// There is NO fallback once a transport is chosen: if the `claude` CLI fails (logged out, stale
    /// install, rate limited), the render fails loudly naming `--llm api` rather than silently
    /// switching credentials. Automated callers (CI, cron) should pin `--llm api` explicitly.
    #[arg(long, value_enum, ignore_case = true)]
    pub llm: Option<Llm>,

    /// Target marquee space for the `marquee-*` formats (defaults to your personal ~space).
    /// Ignored by `markdown`/`pdf`.
    #[arg(long)]
    pub space: Option<String>,

    /// Path to a template that overrides the built-in Markdown template. Rendering is
    /// plain `{{token}}` string replacement over exactly six placeholders: `{{host}}`,
    /// `{{since}}`, `{{until}}`, `{{session-count}}`, `{{total-tokens}}`,
    /// `{{total-spend}}`. No other tokens, loops, or conditionals are supported.
    #[arg(long)]
    pub template: Option<PathBuf>,

    /// Path to a file overriding the built-in LLM prompt. Dispatched by the resolved format's
    /// source family: `markdown`/`pdf`/`marquee-markdown` get the markdown report prompt;
    /// `html`/`marquee-html` get the HTML dashboard prompt.
    #[arg(long)]
    pub prompt: Option<PathBuf>,

    /// Include the "Tradeoffs" section in each session summary (omitted by default to
    /// keep reports concise).
    #[arg(long)]
    pub include_tradeoffs: bool,

    /// PDF engine to use when `--format pdf` is set (default: `wkhtmltopdf`), passed to pandoc
    /// as `--pdf-engine`; `pandoc` is the required binary that must be on `PATH`.
    #[arg(long, default_value = "wkhtmltopdf")]
    pub pdf_engine: String,

    /// Number of top-spend sessions to include in the outlier table.
    #[arg(long, default_value_t = DEFAULT_OUTLIERS)]
    pub outliers: usize,

    /// Prior-period report JSON (schema-gated, same requirement as `-i`), lighting up the Month
    /// over Month section in both templates. Aggregated through the SAME `aggregate::compute` as
    /// the current period, so the two sides of the comparison are computed identically rather than
    /// by two code paths that could drift. Omitted -> the Month over Month section is entirely
    /// absent from the rendered artifact, not an empty header.
    #[arg(long)]
    pub prior: Option<PathBuf>,

    /// Anthropic Enterprise Analytics cost export (produced OUTSIDE clyde by the
    /// `anthropic-usage-report` skill's `pull-usage-report.py --report cost`; clyde never holds the
    /// Analytics key). Lights up the Reconciliation section in both templates: billed spend from
    /// the export against clyde's own modeled total, plus the reader-facing `unseen-account-spend`
    /// figure. The export's window must match this report's `since`/`until` EXACTLY, or the render
    /// fails naming both windows. Omitted -> the render still succeeds, but warns on stderr and the
    /// artifact states that no authoritative export was supplied -- this figure is never silently
    /// missing.
    #[arg(long)]
    pub reconcile: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
pub struct EvalArgs {
    /// Fixture directories to evaluate, space separated. Each must hold a `report.json`; a
    /// committed fixture also holds `eval.yml` and its goldens. When omitted, the three committed
    /// fixtures under `fixtures/report/` are used.
    #[arg(long, num_args = 1..)]
    pub fixture: Option<Vec<PathBuf>>,

    /// Model pin for the judge. When omitted, falls back to `render.markdown-model` in `clyde.yml`,
    /// so the eval needs no config key of its own.
    #[arg(long)]
    pub judge: Option<String>,

    /// Write the scored JSON report here (default: `./eval-report.json`).
    #[arg(long)]
    pub out: Option<PathBuf>,

    /// Overwrite each fixture's committed `golden.md` / `golden.html` with this run's fresh render.
    /// The goldens are model-authored, so this is how they are regenerated; a hand-run `report
    /// render` would splice in the machine's real persona and price against the live feed instead
    /// of the fixture's invented persona and the eval's pinned embedded pricing. A render that
    /// failed its own mechanical checks is NOT written -- a golden is a known-good artifact by
    /// definition, and committing a failing one would make `otto ci` green against a broken render.
    #[arg(long)]
    pub write_goldens: bool,

    /// Which transport performs the renders and the judge call: `auto`, `cli`, or `api`. Same
    /// semantics as `render --llm`, and the judge inherits it -- there is no second credential.
    /// When omitted, falls back to `render.llm` in `clyde.yml`, and to `auto` if that too is unset.
    #[arg(long, value_enum, ignore_case = true)]
    pub llm: Option<Llm>,
}

#[derive(clap::Args, Debug)]
pub struct MergeArgs {
    /// Two or more collected JSON report files to merge. Each must share the same
    /// schema version. Providing a single file is accepted (identity operation).
    pub inputs: Vec<PathBuf>,

    /// Write the merged JSON report to this path. With `-o <path>`, writes that file;
    /// without `-o`, streams JSON to stdout so `report merge a.json b.json | jq` works.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

#[cfg(test)]
mod tests;
