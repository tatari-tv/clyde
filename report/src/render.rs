use crate::aggregate::{self, Aggregates, Attribution, OrgRow, RepoRow, UnitCosts};
use crate::cli::Format;
use crate::config::{RenderConfig, TransportKind};
use crate::fmt::{format_int, format_optional_usd, format_tokens_human, format_usd, short_id};
use crate::outcome::OutcomeTotals;
use crate::persona::{self, PersonaBlock};
use crate::proc::run_bounded;
use crate::quotable::{QuotableFacts, RenderContext};
use crate::report::{Report, SCHEMA_VERSION, SessionEntry};
use crate::summarize;
use crate::{OutputDest, RunResult};
use chrono::{DateTime, Utc};
use claude_pricing::Pricing;
use efficiency::{RawCounters, WorkloadCost, finalize};
use eyre::{Context, Result, bail};
use log::debug;
use serde::Serialize;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};

const STDOUT_SIGIL: &str = "-";
pub const DEFAULT_PROMPT: &str = include_str!("../templates/report.pmt");
const WORKSPACE_PROMPT_PATH: &str = "templates/report.pmt";
pub const DEFAULT_HTML_PROMPT: &str = include_str!("../templates/report-html.pmt");
const WORKSPACE_HTML_PROMPT_PATH: &str = "templates/report-html.pmt";

pub fn run(cfg: &RenderConfig, pricing: &Pricing) -> Result<RunResult> {
    log::info!(
        "render::run: input={} format={:?} space={:?} prompt={:?} outliers={}",
        cfg.input.display(),
        cfg.format,
        cfg.space,
        cfg.prompt,
        cfg.outliers
    );

    if let Some(ext) = cfg.input.extension().and_then(OsStr::to_str)
        && (ext.eq_ignore_ascii_case("yml") || ext.eq_ignore_ascii_case("yaml"))
    {
        bail!(
            "input file ends in .yml/.yaml; report collect emits JSON. Re-run report collect to regenerate as .json."
        );
    }

    let body =
        fs::read_to_string(&cfg.input).with_context(|| format!("failed to read report at {}", cfg.input.display()))?;
    check_schema_version(&body, &cfg.input)?;
    let report: Report =
        serde_json::from_str(&body).with_context(|| format!("failed to parse report at {}", cfg.input.display()))?;

    // Branch once at the source: the html-source family (`Html`, `MarqueeHtml`) never touches
    // pandoc; the markdown-source family is the unchanged template-or-opus pipeline. Generation
    // (live API) and routing (write/publish an already-generated artifact string) are separated so
    // routing is unit-testable with injected strings — see `route_html_artifact` /
    // `route_markdown_artifact` and their tests.
    let dest = if cfg.format.is_html_source() {
        let html = generate_html(cfg, &report, pricing)?;
        route_html_artifact(&html, &report, cfg)?
    } else {
        let markdown = generate_markdown(cfg, &report, pricing)?;
        route_markdown_artifact(&markdown, &report, cfg)?
    };

    Ok(RunResult {
        sessions_emitted: report.totals.sessions,
        output: dest,
    })
}

/// Produce the markdown-source artifact: the offline `--template` path, or the `report.pmt` -> opus
/// path. Unchanged from the pre-HTML pipeline (only extracted out of `run` for the source-family
/// branch and the generation/routing split).
fn generate_markdown(cfg: &RenderConfig, report: &Report, pricing: &Pricing) -> Result<String> {
    if let Some(template_path) = cfg.template.as_deref() {
        let template = load_template(Some(template_path))?;
        Ok(to_markdown(report, &template, pricing))
    } else {
        let prompt = resolve_prompt(cfg.prompt.as_deref(), Path::new("."))?;
        let persona_block = persona::whoami();
        let context = build_context_block(
            report,
            cfg.include_tradeoffs,
            persona_block.as_ref(),
            pricing,
            cfg.outliers,
            cfg.prior.as_deref(),
        )?;
        render_via_opus_markdown(&context, &prompt, cfg)
    }
}

/// Produce the html-source artifact: context block -> `report-html.pmt` -> opus (streaming) -> a
/// validated, self-contained HTML document. Pandoc is never invoked; there is no offline path.
fn generate_html(cfg: &RenderConfig, report: &Report, pricing: &Pricing) -> Result<String> {
    let prompt = resolve_html_prompt(cfg.prompt.as_deref(), Path::new("."))?;
    let persona_block = persona::whoami();
    let context = build_context_block(
        report,
        cfg.include_tradeoffs,
        persona_block.as_ref(),
        pricing,
        cfg.outliers,
        cfg.prior.as_deref(),
    )?;
    render_via_opus_html(&context, &prompt, cfg)
}

/// Route an already-generated markdown artifact to its destination (local file / stdout / PDF /
/// marquee). Takes the artifact string so it is unit-testable without the live API.
fn route_markdown_artifact(markdown: &str, report: &Report, cfg: &RenderConfig) -> Result<OutputDest> {
    debug!(
        "render::route_markdown_artifact: format={:?} bytes={}",
        cfg.format,
        markdown.len()
    );
    match cfg.format {
        Format::Markdown => write_local_markdown(markdown, report, cfg),
        Format::Pdf => write_local_pdf(markdown, report, cfg),
        Format::MarqueeMarkdown => publish_marquee_markdown(markdown, report, cfg),
        other => bail!("route_markdown_artifact called with a non-markdown-source format: {other:?}"),
    }
}

/// Route an already-generated, validated HTML artifact to its destination (local file / stdout, or
/// marquee publish). Takes the artifact string so it is unit-testable without the live API.
fn route_html_artifact(html: &str, report: &Report, cfg: &RenderConfig) -> Result<OutputDest> {
    debug!(
        "render::route_html_artifact: format={:?} bytes={}",
        cfg.format,
        html.len()
    );
    match cfg.format {
        Format::Html => write_local_html(html, report, cfg),
        Format::MarqueeHtml => publish_marquee_html(html, report, cfg),
        other => bail!("route_html_artifact called with a non-html-source format: {other:?}"),
    }
}

/// Write the rendered markdown to `-o <path>`, to stdout (`-o -`), or to the default
/// `./<YYYY-MM>-claude-report.md` beside the input when `-o` is omitted.
fn write_local_markdown(markdown: &str, report: &Report, cfg: &RenderConfig) -> Result<OutputDest> {
    let output = match cfg.output.as_deref() {
        Some(p) => p.to_path_buf(),
        None => default_output_path(report, Format::Markdown),
    };
    debug!("render::write_local_markdown: output={}", output.display());

    if output.as_os_str() == STDOUT_SIGIL {
        std::io::stdout()
            .write_all(markdown.as_bytes())
            .context("failed to write markdown to stdout")?;
        return Ok(OutputDest::Stdout);
    }
    let dir = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(dir).with_context(|| format!("failed to create output dir {}", dir.display()))?;
    fs::write(&output, markdown).with_context(|| format!("failed to write markdown to {}", output.display()))?;
    Ok(OutputDest::File(output))
}

/// Write the validated HTML document to `-o <path>`, to stdout (`-o -`), or to the default
/// `./<YYYY-MM>-claude-report.html` when `-o` is omitted. Mirrors [`write_local_markdown`]
/// (including the `-o -` stdout sigil); the html artifact is text, so stdout is legal here (unlike
/// the binary PDF path).
fn write_local_html(html: &str, report: &Report, cfg: &RenderConfig) -> Result<OutputDest> {
    let output = match cfg.output.as_deref() {
        Some(p) => p.to_path_buf(),
        None => default_output_path(report, Format::Html),
    };
    debug!(
        "render::write_local_html: output={} bytes={}",
        output.display(),
        html.len()
    );

    if output.as_os_str() == STDOUT_SIGIL {
        std::io::stdout()
            .write_all(html.as_bytes())
            .context("failed to write HTML to stdout")?;
        return Ok(OutputDest::Stdout);
    }
    let dir = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(dir).with_context(|| format!("failed to create output dir {}", dir.display()))?;
    fs::write(&output, html).with_context(|| format!("failed to write HTML to {}", output.display()))?;
    Ok(OutputDest::File(output))
}

/// Convert the rendered markdown to PDF via pandoc and write it to `-o <path>` or the default
/// `./<YYYY-MM>-claude-report.pdf`. Binary output cannot stream to stdout.
fn write_local_pdf(markdown: &str, report: &Report, cfg: &RenderConfig) -> Result<OutputDest> {
    let output = match cfg.output.as_deref() {
        Some(p) => p.to_path_buf(),
        None => default_output_path(report, Format::Pdf),
    };
    debug!(
        "render::write_local_pdf: output={} engine={}",
        output.display(),
        cfg.pdf_engine
    );

    if output.as_os_str() == STDOUT_SIGIL {
        bail!("--format pdf cannot write binary output to stdout; pass -o <path>");
    }
    write_pdf(markdown, &output, &cfg.pdf_engine)?;
    Ok(OutputDest::File(output))
}

pub(crate) fn default_output_path(report: &Report, format: Format) -> std::path::PathBuf {
    let prefix = report.since.format("%Y-%m");
    let ext = match format {
        Format::Pdf => "pdf",
        Format::Html => "html",
        _ => "md",
    };
    std::path::PathBuf::from(format!("./{}-claude-report.{}", prefix, ext))
}

#[derive(Debug, Clone)]
pub enum Template {
    BuiltIn,
    Custom(String),
}

/// Resolve the configured transport selection against this host: is `claude` on PATH, and is a key
/// set? The impure half of the decision, kept to one line each so `config::resolve_transport` stays
/// pure and its whole precedence matrix is unit-testable.
///
/// `which::which` mirrors `clyde::resolve_claude`, which already canonicalizes a relative PATH hit.
fn resolve_transport_for(cfg: &RenderConfig) -> Result<TransportKind> {
    let resolved = crate::config::resolve_transport(
        cfg.llm,
        which::which("claude").is_ok(),
        summarize::api_key_from_env().is_some(),
        cfg.format,
    )?;
    // Log the SELECTION for both transports, not just cli: an operator reading a log must be able to
    // tell what paid for an artifact without rerunning it. The cli path adds the resolved binary path
    // and version in `CliTransport::resolve`.
    log::info!(
        "render: llm transport selected={resolved:?} (requested={:?}) format={:?}",
        cfg.llm,
        cfg.format
    );
    Ok(resolved)
}

fn render_via_opus_markdown(context: &RenderContext, prompt: &str, cfg: &RenderConfig) -> Result<String> {
    let json_body = &context.json;
    debug!(
        "render::render_via_opus_markdown: context bytes={} prompt bytes={} model={} max_output_tokens={}",
        json_body.len(),
        prompt.len(),
        cfg.markdown_model,
        cfg.markdown_max_output_tokens
    );
    let model = &cfg.markdown_model;
    let ceiling = cfg.markdown_max_output_tokens;
    // Monomorphized per transport; no Box<dyn Transport>, per the house generics-for-DI rule.
    let prose = match resolve_transport_for(cfg)? {
        TransportKind::Api => {
            summarize::markdown(&summarize::ApiTransport::from_env()?, model, ceiling, prompt, json_body)?
        }
        TransportKind::Cli => {
            summarize::markdown(&summarize::CliTransport::resolve()?, model, ceiling, prompt, json_body)?
        }
    };
    // Render invents nothing: the whole markdown document is prose over the string-only facts, so
    // every figure in it must be licensed by a QUOTABLE FACT -- a display figure the binary
    // formatted, or the digits inside an identifier the prose cites verbatim. Not "any numeric token
    // anywhere in the serialized block", which pre-approved every small integer that happened to
    // fall inside a session id or a sha (design "Guard weakness (10)").
    reject_foreign_numbers("markdown", &prose, &context.facts)?;
    Ok(prose)
}

/// The html-source counterpart to [`render_via_opus_markdown`]. There is NO offline HTML path, so
/// the missing-key error deliberately does NOT recommend `--template` (which produces markdown and
/// is rejected for html-source formats).
fn render_via_opus_html(context: &RenderContext, prompt: &str, cfg: &RenderConfig) -> Result<String> {
    let json_body = &context.json;
    debug!(
        "render::render_via_opus_html: context bytes={} prompt bytes={} model={} max_output_tokens={}",
        json_body.len(),
        prompt.len(),
        cfg.html_model,
        cfg.html_max_output_tokens
    );
    let model = &cfg.html_model;
    let ceiling = cfg.html_max_output_tokens;
    let html = match resolve_transport_for(cfg)? {
        TransportKind::Api => {
            summarize::html(&summarize::ApiTransport::from_env()?, model, ceiling, prompt, json_body)?
        }
        TransportKind::Cli => summarize::html(&summarize::CliTransport::resolve()?, model, ceiling, prompt, json_body)?,
    };
    // Render invents nothing (html): CSS/JS geometry is legitimate authored markup full of numbers
    // that are NOT data (px, breakpoints, colors), so the guard runs over the VISIBLE TEXT only
    // (style/script blocks and tag markup stripped). Every data figure a reader sees must be
    // licensed by a quotable fact; a fabricated figure is rejected.
    reject_foreign_numbers("html", &visible_text(&html), &context.facts)?;
    Ok(html)
}

/// Gate on the artifact's `schema-version` BEFORE the full parse, so a wrong-shaped report is named
/// as such instead of surfacing as a serde error about an internal field.
///
/// Without this, rendering a leftover v1 `claude-report.json` (from the pre-v2 `cr`, or any older
/// clyde) failed with `missing field "efficiency" at line 91 column 5`, which reads as a crash in
/// clyde rather than as the DOCUMENTED v1 -> v2 break. The design's Rollout Plan is explicit that
/// this break must "read as expected, not a bug", and it deliberately ships no compat shim: the
/// remedy is to re-collect. So say exactly that, and name both versions.
///
/// Probes ONLY `schema-version` so none of a v1 file's other differences ever reach serde.
fn check_schema_version(body: &str, path: &Path) -> Result<()> {
    #[derive(serde::Deserialize)]
    struct SchemaProbe {
        #[serde(rename = "schema-version")]
        schema_version: u32,
    }

    let probe: SchemaProbe = serde_json::from_str(body).with_context(|| {
        format!(
            "could not read a `schema-version` from {}; it does not look like a `clyde report collect` artifact",
            path.display()
        )
    })?;
    debug!(
        "render::check_schema_version: path={} found=v{} expected=v{}",
        path.display(),
        probe.schema_version,
        SCHEMA_VERSION
    );
    if probe.schema_version != SCHEMA_VERSION {
        bail!(
            "the report at {} is schema v{}, but this clyde reads schema v{}. There is no \
             backward-compatible render path for an older artifact: re-run `clyde report collect` \
             to regenerate it, then render again.",
            path.display(),
            probe.schema_version,
            SCHEMA_VERSION
        );
    }
    Ok(())
}

/// Reject generated prose that stated a figure no quotable fact licenses. This is the RUNTIME half
/// of the render-invents-nothing guard: the prompt-level "no arithmetic" rule is advisory, so a
/// poisoned narrative is caught here and never escapes. `kind` names the path (markdown / html) for
/// the operator-facing error and WARN. Fail closed.
///
/// `facts` is the [`QuotableFacts`] set built beside the context block, NOT the serialized block
/// itself: the block's session ids, timestamps and shas used to pre-approve effectively every
/// small integer (design "Guard weakness (10)"), which is why a fabricated "14 hours of engineering
/// time" passed. A false positive here is a hard render failure, so the identifier half of the fact
/// set exists to keep legitimate citations (an untitled session by `short-id`, a prose PR reference)
/// passing.
fn reject_foreign_numbers(kind: &str, prose: &str, facts: &QuotableFacts) -> Result<()> {
    debug!(
        "render::reject_foreign_numbers: kind={kind} prose_chars={} quotable_figures={}",
        prose.chars().count(),
        facts.figure_count()
    );
    let foreign = facts.foreign_figures(prose);
    if !foreign.is_empty() {
        log::warn!(
            "render::reject_foreign_numbers: {kind} path REJECTED -- generated prose stated \
             figure(s) no quotable fact licenses: {foreign:?}"
        );
        bail!(
            "{kind} rendering introduced number(s) absent from the computed facts: {foreign:?} -- the \
             render-invents-nothing contract was violated; refusing to emit the artifact"
        );
    }
    debug!("render::reject_foreign_numbers: kind={kind} clean");
    Ok(())
}

/// The reader-visible text of an HTML document: `<style>`/`<script>` block CONTENTS and all tag
/// markup (attributes included) removed, leaving only text nodes. The foreign-number guard runs
/// over THIS, not the raw HTML, because CSS/JS are legitimately full of authored numbers (px,
/// breakpoints, hex colors, bar-width percentages in `style=`) that are geometry, not data. A
/// fabricated DATA figure always surfaces in visible text (a headline, a label, a table cell); the
/// pre-formatted display strings the model may quote also live there. Byte-slice-free (char-based)
/// per the crate lint.
fn visible_text(html: &str) -> String {
    let stripped = strip_blocks(&strip_blocks(html, "script"), "style");
    let mut out = String::with_capacity(stripped.len());
    let mut in_tag = false;
    for ch in stripped.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

/// Remove every `<tag>...</tag>` block (contents included), case-insensitively, for a single tag
/// name (`style` / `script`). A missing closing tag drops the rest of the document from that opener
/// (fail closed: unmatched markup never leaks unchecked into the visible-text scan). Char-based, no
/// byte slicing.
fn strip_blocks(html: &str, tag: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let chars: Vec<char> = html.chars().collect();
    let lower_chars: Vec<char> = lower.chars().collect();
    let open_pat: Vec<char> = open.chars().collect();
    let close_pat: Vec<char> = close.chars().collect();
    let mut out = String::with_capacity(html.len());
    let mut i = 0usize;
    while i < chars.len() {
        if matches_at(&lower_chars, i, &open_pat) {
            match find_from(&lower_chars, i + open_pat.len(), &close_pat) {
                Some(end) => i = end + close_pat.len(),
                None => break, // no closing tag: drop the remainder
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// True when `pat` occurs in `hay` starting exactly at index `i`.
fn matches_at(hay: &[char], i: usize, pat: &[char]) -> bool {
    i + pat.len() <= hay.len() && hay[i..i + pat.len()] == *pat
}

/// The index in `hay` at or after `start` where `pat` begins, if any.
fn find_from(hay: &[char], start: usize, pat: &[char]) -> Option<usize> {
    if pat.is_empty() || start > hay.len() {
        return None;
    }
    (start..=hay.len().saturating_sub(pat.len())).find(|&i| hay[i..i + pat.len()] == *pat)
}

pub(crate) fn resolve_prompt(explicit: Option<&Path>, workspace_dir: &Path) -> Result<String> {
    if let Some(path) = explicit {
        return fs::read_to_string(path)
            .with_context(|| format!("failed to read prompt template at {}", path.display()));
    }
    let workspace_pmt = workspace_dir.join(WORKSPACE_PROMPT_PATH);
    if workspace_pmt.exists() {
        return fs::read_to_string(&workspace_pmt)
            .with_context(|| format!("failed to read workspace prompt at {}", workspace_pmt.display()));
    }
    Ok(DEFAULT_PROMPT.to_string())
}

/// Resolve the html-source prompt with the identical 3-tier precedence as [`resolve_prompt`]:
/// `--prompt` path > workspace `templates/report-html.pmt` > baked-in [`DEFAULT_HTML_PROMPT`].
/// `--prompt` is one flag dispatched by the resolved format's source family.
pub(crate) fn resolve_html_prompt(explicit: Option<&Path>, workspace_dir: &Path) -> Result<String> {
    if let Some(path) = explicit {
        return fs::read_to_string(path)
            .with_context(|| format!("failed to read prompt template at {}", path.display()));
    }
    let workspace_pmt = workspace_dir.join(WORKSPACE_HTML_PROMPT_PATH);
    if workspace_pmt.exists() {
        return fs::read_to_string(&workspace_pmt)
            .with_context(|| format!("failed to read workspace prompt at {}", workspace_pmt.display()));
    }
    Ok(DEFAULT_HTML_PROMPT.to_string())
}

/// Slim render context sent to Opus: `{persona, options, period, totals, aggregates, outcomes,
/// sessions}`. Deliberately NOT the whole [`Report`] (that leaked `jsonl-paths`, 44.8% of context
/// bytes with zero model signal, plus full per-model token detail per session). `Report` itself
/// is unchanged; these are render-only view structs (design "API Design" section).
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ContextBlock<'a> {
    persona: &'a PersonaBlock,
    options: ContextOptions,
    /// The pricing basis for every dollar figure below (design "Pricing basis, always present",
    /// Phase 6): what it is priced against, whether it is an invoice (never), and which feed
    /// resolved it. `basis.note` is the required, verbatim header disclosure both templates carry.
    basis: Basis,
    period: PeriodView,
    totals: TotalsView,
    /// How much of `totals.spend` carries a repo and on what evidence, one row per `repo-source`
    /// plus `(unattributed)`. Top-level rather than under `aggregates` because it is a statement
    /// ABOUT the whole figure, not another rollup of it, and because the prose cites it next to the
    /// headline. The rows sum to `totals.spend` by construction.
    attribution: Attribution,
    /// How much of the window's sessions carry an enrich `summary` (design Phase 9, "Systemic
    /// property"): the evidence base the narrative should cite. `run_collect` already warns on
    /// stderr when this falls below `--min-enrichment` (Phase 3); this is the SAME fact carried into
    /// the prompt so the narrative can state the gap in prose and correctly fall back to `title`
    /// for the sessions it does not cover, rather than silently treating every session as enriched.
    enrichment_coverage: String,
    /// The period's spend set against its own output and calendar (design Phase 7, finding 9):
    /// `per-commit`, `per-pr`, `per-active-day`, `per-session`, and the session-spend percentiles,
    /// each a display string the binary divided. Top-level for the same reason as `attribution`: it
    /// is a statement ABOUT the headline figure, not another rollup of it. Fields are ABSENT on a
    /// zero denominator, and both templates carry the exact wording that keeps a ratio from being
    /// read as a price tag (see [`UnitCosts`]).
    unit_costs: UnitCosts,
    aggregates: &'a Aggregates,
    /// The v2 efficiency signal set, all pre-formatted display strings (design Phase 5): the
    /// agent-type cost headline plus the report-wide cache/tool/interrupt/compaction signals and the
    /// by-skill / by-mcp attribution. Carries no raw numeric operand, so the model can only quote it.
    efficiency: EfficiencyView,
    /// Absent (never `null`, never zeroed) when the report carries no outcome rollup
    /// (`--no-outcomes`, pre-outcomes JSONs, mixed-capability merges). The prompt omits the
    /// Quantified Output section when this key is missing.
    #[serde(skip_serializing_if = "Option::is_none")]
    outcomes: Option<OutcomesView>,
    sessions: Vec<SessionView<'a>>,
    /// The prior period's aggregates (design Phase 8, gap 7): lights up the Month over Month
    /// section both templates already document but had no backing field for. Absent entirely
    /// (never an empty object) when `--prior` was not supplied, so the prompt's "omit the section"
    /// rule needs no empty-vs-absent special case.
    #[serde(skip_serializing_if = "Option::is_none")]
    prior: Option<PriorView>,
}

/// The report-wide efficiency headline, string-only (design Phase 5). `agent-type-costs` is the
/// HEADLINE (cost + tokens attributed to each subagent TYPE, pre-sorted by spend descending); the
/// ratios are pre-formatted percents (`"96.0%"` / `"n/a"`); `interrupts` and `compactions` are the
/// report-wide observed counts (sanctioned context counts, never operands the model recombines).
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct EfficiencyView {
    /// Report-wide cache-read share (ratio-of-sums from `totals`), e.g. `"96.0%"` or `"n/a"`.
    cache_read_share: String,
    /// Report-wide tool-error rate (ratio-of-sums from `totals`), e.g. `"2.4%"` or `"n/a"`.
    tool_error_rate: String,
    /// Report-wide share of cache writes that paid the 1h premium, e.g. `"18.0%"` or `"n/a"`.
    cache_1h_write_fraction: String,
    /// Total interrupts observed across the window (structured + text markers).
    interrupts: u64,
    /// Total context compactions observed across the window.
    compactions: u64,
    /// HEADLINE: tokens + `$` attributed to each subagent TYPE, pre-sorted by spend descending, plus
    /// the `(main-session)` residual. A true PARTITION of `totals.spend` (Phase 5): same pricing
    /// basis, every dollar in exactly one row.
    agent_type_costs: Vec<WorkloadRow>,
    /// Tokens + `$` grouped by skill (`attributionSkill`), pre-sorted by spend descending. An
    /// attribution TAG set, not a partition -- see [`Self::by_skill_coverage`].
    by_skill: Vec<WorkloadRow>,
    /// Tokens + `$` grouped by MCP tool (`attributionMcpTool`), pre-sorted by spend descending. An
    /// attribution TAG set, not a partition -- see [`Self::by_mcp_coverage`].
    by_mcp: Vec<WorkloadRow>,
    /// How much of `totals.spend` the `by-skill` rows cover, and on what pricing basis, e.g.
    /// `"$412.19 of $9,450.31 (4.4%), embedded-price basis"`. Binary-computed so the prose can state
    /// the coverage as a fact rather than reconciling a tag set that cannot sum to anything.
    by_skill_coverage: String,
    /// The same statement for the `by-mcp` rows.
    by_mcp_coverage: String,
}

/// One agent-type / skill / mcp-tool attribution row, string-only. `spend` is a display string. On
/// `agent-type-costs` it is priced from report's fetched feed (the same basis as `totals.spend`); on
/// `by-skill` / `by-mcp` it is the catalog's embedded-priced figure, which is why those two carry a
/// coverage string instead of summing to the total.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct WorkloadRow {
    name: String,
    tokens_human: String,
    spend: String,
}

/// `outcomes.totals` per the prompt's context-block schema: the persisted [`OutcomeTotals`]
/// rollup re-exposed with fields present-if-nonzero, so "only fields present were observed"
/// holds and a zero can never be mistaken for an observation.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct OutcomesView {
    totals: OutcomeTotalsView,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct OutcomeTotalsView {
    #[serde(skip_serializing_if = "Option::is_none")]
    sessions_with_commits: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    commits: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prs_opened: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    confluence_writes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    jira_writes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    slack_messages: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    files_edited: Option<u64>,
    /// Volume of file content produced (Phase 7), the evidence `files-edited`'s bare path count
    /// lacks. Absent until a session is reindexed under Phase 7, so a pre-Phase-7 catalog omits it
    /// rather than reporting zero lines against thousands of edited files.
    #[serde(skip_serializing_if = "Option::is_none")]
    lines_written: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lines_replaced: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ContextOptions {
    include_tradeoffs: bool,
}

/// The pricing basis, always present (design Phase 6, "Pricing basis, always present"). Every dollar
/// figure downstream is `tokens x published per-token rate`, never a billed/invoiced total, and
/// `note` states that scope in the same sentence as the citation so a finance reader does not expect
/// this figure to reconcile against the authoritative Analytics cost report (design Resolved
/// Decisions, "Tatari pays for Claude Enterprise..."; supersedes the earlier "not an amount
/// invoiced" wording).
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct Basis {
    /// Always "published list rates" -- `is_invoice` is the machine-checkable half of the same fact.
    pricing: String,
    /// Always `false`: this artifact never carries a billed/invoiced total (see `note`).
    is_invoice: bool,
    /// Which resolution path produced the [`Pricing`] this render used: `embedded` | `fetched` |
    /// `override`. NOTE: `claude-pricing` does not distinguish a live network fetch from an on-disk
    /// cache hit at the type level (both resolve to `Source::Fetched`), so both surface as
    /// `fetched` here; `override` covers `Source::UserOverride`, a fourth case the doc's
    /// `embedded | cached | fetched` vocabulary does not name.
    feed_source: String,
    /// The resolved feed's own `data-version` (an ISO-8601 timestamp), or `"unknown"` when the
    /// feed carried none.
    feed_version: String,
    /// One sentence, carried verbatim into the required header line by both templates.
    note: String,
}

/// The disclosure sentence, verbatim (design Phase 6 / Resolved Decisions "Tatari pays for Claude
/// Enterprise, and the Analytics cost report is the authoritative spend number"). The scope caveat
/// rides in the SAME sentence as the citation -- naming only the authoritative source, with no scope
/// statement, is what let a reader expect the two figures to match and read a mismatch as "clyde
/// miscounted".
const BASIS_NOTE: &str = "Total spend is modeled Claude Code catalog spend at published list rates; \
     account-level billed spend comes from Claude Enterprise Analytics.";

fn build_basis(pricing: &Pricing) -> Basis {
    let feed_source = match pricing.source() {
        claude_pricing::Source::Embedded => "embedded",
        claude_pricing::Source::Fetched { .. } => "fetched",
        claude_pricing::Source::UserOverride(_) => "override",
    };
    debug!(
        "render::build_basis: feed-source={} feed-version={:?}",
        feed_source,
        pricing.data_version()
    );
    Basis {
        pricing: "published list rates".to_string(),
        is_invoice: false,
        feed_source: feed_source.to_string(),
        feed_version: pricing.data_version().unwrap_or("unknown").to_string(),
        note: BASIS_NOTE.to_string(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PeriodView {
    since: String,
    until: String,
    /// INCLUSIVE calendar date-span count (design "by-day, corrected"): `num_days() + 1` between
    /// `since` and `until`'s dates, e.g. June 26 -> July 25 = 30. This is the same count `by-day`
    /// zero-fills, so `by-day.len() == period.days` always holds by construction. Previously
    /// exclusive (`num_days()` alone), which could make `active-days` exceed `days` and print the
    /// nonsense "Active Days: 30 of 29".
    days: i64,
    /// Count of `aggregates.by-day` rows with `active: true`, NOT the row count -- every calendar
    /// date in the window now has a row, active or not.
    active_days: usize,
    generated: String,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct TotalsView {
    sessions: usize,
    repo_count: usize,
    spend: String,
    tokens_human: String,
    untracked_models: Vec<String>,
    /// Sorted by spend descending by this builder; `Report.totals.models` is a name-keyed
    /// `BTreeMap` (alphabetical iteration) and cannot itself back the "pre-sorted, never
    /// re-sort" promise the prompt makes.
    models: Vec<ModelRow>,
    total_row: TotalRow,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ModelRow {
    model: String,
    sessions_using: usize,
    tokens_human: String,
    /// Raw per-model spend, kept ONLY to compute this row's `spend-percent-of-max` and pre-sort the
    /// list; NOT serialized (`skip`) so no raw numeric operand reaches the model (design Phase 5,
    /// string-only context). The display string `spend` / `(untracked)` is what the model sees.
    #[serde(skip)]
    spend_usd: Option<f64>,
    spend: String,
    /// Bar-chart geometry (design "Chart truthfulness"): see [`aggregate::percent_of_max`],
    /// scaled against the max `spend-usd` across `totals.models`. Absent when every model is
    /// unpriced/$0 - render-only view, so this is computed here rather than in `aggregate.rs`.
    #[serde(skip_serializing_if = "Option::is_none")]
    spend_percent_of_max: Option<f64>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct TotalRow {
    /// `totals.sessions` (distinct sessions), NOT the column sum: a session using several
    /// models appears in each model's `sessions-using`, so the column overlaps by design.
    sessions_using: usize,
    tokens_human: String,
    spend: String,
}

/// Slim per-session view: `short-id`, `title`, `summary`, `tags`, `repo`, `begin`/`end`,
/// `tokens-human`, `spend-display`, and model NAMES only (no per-model token detail). No
/// `jsonl-paths`, and NO raw `spend` operand (design Phase 5, string-only context): sessions feed
/// THEMES and CITATIONS only (never counting or summing), so the display string is all the model
/// needs; `short-id` backs the untitled-session fallback.
///
/// `title` is Claude Code's own `ai-title`, resolved from the session's OPENING exchange alone
/// (design "Systemic property"): a label for identifying a session in a table or citation, never
/// evidence of a theme. `summary` is the enrich pass's own digest of the FULL transcript (head +
/// tail up to 500K chars) and is the evidence a theme should cite; `None` for a session the enrich
/// pass never reached, in which case the prompt falls back to `title` (see the top-level
/// `enrichment-coverage` field for how much of the window that affects). `tags` is the enrich
/// pass's topic labels, empty when absent.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct SessionView<'a> {
    short_id: String,
    title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tags: Vec<&'a str>,
    repo: Option<&'a str>,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    tokens_human: String,
    spend_display: String,
    models: Vec<&'a str>,
    /// The session's observed outcomes (commit shas, PR refs, write counts), absent when
    /// extraction ran and found nothing or never ran; theme/citation material only, per the
    /// prompt's "never for counting or summing" rule.
    #[serde(skip_serializing_if = "Option::is_none")]
    outcomes: Option<&'a crate::outcome::Outcomes>,
}

/// Build the model's context block AND the quotable-facts sets that bound what the artifact may say
/// about it. The two are returned together ([`RenderContext`]) so the guard can never be run against
/// a different block than the one the model was handed.
pub(crate) fn build_context_block(
    report: &Report,
    include_tradeoffs: bool,
    persona: Option<&PersonaBlock>,
    pricing: &Pricing,
    outliers_n: usize,
    prior_path: Option<&Path>,
) -> Result<RenderContext> {
    debug!(
        "render::build_context_block: sessions={} include_tradeoffs={} outliers-n={} prior={:?}",
        report.sessions.len(),
        include_tradeoffs,
        outliers_n,
        prior_path
    );
    let default_persona = PersonaBlock::default();
    let aggregates = aggregate::compute(report, outliers_n, pricing);
    let period = build_period_view(report, &aggregates);
    let prior = build_prior_view(prior_path, period.days, pricing)?;
    let block = ContextBlock {
        persona: persona.unwrap_or(&default_persona),
        options: ContextOptions { include_tradeoffs },
        basis: build_basis(pricing),
        period,
        totals: build_totals_view(report),
        attribution: aggregate::compute_attribution(report),
        enrichment_coverage: build_enrichment_coverage(report),
        unit_costs: aggregate::compute_unit_costs(report, &aggregates.by_day),
        aggregates: &aggregates,
        efficiency: build_efficiency_view(report),
        outcomes: build_outcomes_view(report),
        sessions: report
            .sessions
            .iter()
            .map(|(sid, entry)| build_session_view(sid, entry))
            .collect(),
        prior,
    };
    let json = serde_json::to_string(&block).context("failed to serialize context block to JSON")?;
    let facts = QuotableFacts::from_context_json(&json)?;
    debug!(
        "render::build_context_block: context_bytes={} quotable_figures={}",
        json.len(),
        facts.figure_count()
    );
    Ok(RenderContext { json, facts })
}

fn build_period_view(report: &Report, aggregates: &Aggregates) -> PeriodView {
    let days = (report.until.date_naive() - report.since.date_naive()).num_days() + 1;
    let active_days = aggregates.by_day.iter().filter(|r| r.active).count();
    debug!(
        "render::build_period_view: days={} active-days={} by-day-rows={}",
        days,
        active_days,
        aggregates.by_day.len()
    );
    PeriodView {
        since: report.since.format("%Y-%m-%d").to_string(),
        until: report.until.format("%Y-%m-%d").to_string(),
        days,
        active_days,
        generated: report.generated.format("%Y-%m-%d").to_string(),
    }
}

fn build_totals_view(report: &Report) -> TotalsView {
    debug!("render::build_totals_view: models={}", report.totals.models.len());
    let repo_count = report
        .sessions
        .values()
        .filter_map(|e| e.repo.as_deref())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let total_tokens: u64 = report.totals.models.values().map(|m| m.total).sum();

    let mut models: Vec<ModelRow> = report
        .totals
        .models
        .iter()
        .map(|(model, mt)| ModelRow {
            model: model.clone(),
            sessions_using: report
                .sessions
                .values()
                .filter(|e| e.models.contains_key(model))
                .count(),
            tokens_human: format_tokens_human(mt.total),
            spend_usd: mt.spend_usd,
            spend: format_optional_usd(mt.spend_usd),
            spend_percent_of_max: None,
        })
        .collect();
    let max_spend = models.iter().filter_map(|r| r.spend_usd).fold(0.0_f64, f64::max);
    for row in &mut models {
        row.spend_percent_of_max = aggregate::percent_of_max(row.spend_usd.unwrap_or(0.0), max_spend);
    }
    models.sort_by(|a, b| {
        b.spend_usd
            .unwrap_or(0.0)
            .partial_cmp(&a.spend_usd.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    debug!(
        "render::build_totals_view: rows={} max-spend={}",
        models.len(),
        max_spend
    );

    TotalsView {
        sessions: report.totals.sessions,
        repo_count,
        spend: format_usd(report.totals.spend_usd),
        tokens_human: format_tokens_human(total_tokens),
        untracked_models: report.totals.untracked_models.clone(),
        models,
        total_row: TotalRow {
            sessions_using: report.totals.sessions,
            tokens_human: format_tokens_human(total_tokens),
            spend: format_usd(report.totals.spend_usd),
        },
    }
}

/// The `enrichment-coverage` context field (design Phase 9): how many of the window's sessions
/// carry an enrich `summary`, as a single factual sentence the model may quote verbatim rather than
/// recompute. Counted over `report.sessions` -- the SAME collection `sessions` in the context is
/// built from -- so the figure matches exactly what the model can see, not a separate collect-time
/// sample. `run_collect`'s `--min-enrichment` warning (Phase 3) fires on the same underlying fact at
/// collect time; this is that fact's render-time counterpart, carried where the prompt can cite it.
fn build_enrichment_coverage(report: &Report) -> String {
    let total = report.sessions.len();
    let enriched = report.sessions.values().filter(|e| e.summary.is_some()).count();
    debug!("render::build_enrichment_coverage: enriched={enriched} total={total}");
    if total == 0 {
        return "0 of 0 sessions carry an enrich summary".to_string();
    }
    let share = enriched as f64 / total as f64 * 100.0;
    format!(
        "{enriched} of {total} sessions in the window ({share:.1}%) carry an enrich summary; the \
         rest are cited by title only"
    )
}

/// Re-expose the persisted `Totals.outcomes` rollup as the context's `outcomes.totals`, fields
/// present-if-nonzero (design API section). `None` when the report carries no rollup, which
/// keeps the `outcomes` key out of the context entirely.
fn build_outcomes_view(report: &Report) -> Option<OutcomesView> {
    let totals = report.totals.outcomes.as_ref()?;
    Some(OutcomesView {
        totals: outcome_totals_view(totals),
    })
}

/// Shared by [`build_outcomes_view`] (the current period) and [`build_prior_view`] (Phase 8): the
/// same present-if-nonzero conversion, so the two periods' outcome figures are built by one code
/// path rather than two that could drift.
fn outcome_totals_view(totals: &OutcomeTotals) -> OutcomeTotalsView {
    let nonzero = |v: u64| if v == 0 { None } else { Some(v) };
    OutcomeTotalsView {
        sessions_with_commits: nonzero(totals.sessions_with_commits),
        commits: nonzero(totals.commits),
        prs_opened: nonzero(totals.prs_opened),
        confluence_writes: nonzero(totals.confluence_writes),
        jira_writes: nonzero(totals.jira_writes),
        slack_messages: nonzero(totals.slack_messages),
        files_edited: nonzero(totals.files_edited),
        lines_written: nonzero(totals.lines_written),
        lines_replaced: nonzero(totals.lines_replaced),
    }
}

/// The prior period's aggregates (design Phase 8, `--prior`): lights up the Month over Month
/// section both templates already document but had no backing field for. Aggregated through the
/// SAME [`aggregate::compute`] as the current period, from a schema-gated report file, so the two
/// sides of the comparison are computed identically rather than by two code paths that could
/// drift. Absent entirely (never emitted with empty/zeroed fields) when `--prior` was not supplied.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PriorView {
    since: String,
    until: String,
    days: i64,
    /// `false` when `days` differs from the current period's `period.days`, so the prompt states
    /// the length mismatch rather than comparing e.g. a 30-day window against a 14-day one as if
    /// they covered equal ground.
    comparable: bool,
    /// Present only when this prior artifact predates repo-source provenance and the outcome
    /// counters added by this design (see [`predates_fidelity_fields`]). When present, `outcomes`
    /// below is deliberately omitted: a `0` from a build that never measured the field is not the
    /// same fact as an observed zero, and both templates must quote this sentence instead of citing
    /// `outcomes` as if it were a real measurement.
    #[serde(skip_serializing_if = "Option::is_none")]
    predates_fields: Option<String>,
    totals: TotalsView,
    by_repo: Vec<RepoRow>,
    by_org: Vec<OrgRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcomes: Option<OutcomeTotalsView>,
}

/// Verbatim caveat both templates quote in place of `prior.outcomes` when [`predates_fidelity_fields`]
/// fires. Stated once here so the two templates and any future caller never restate it differently.
const PRIOR_PREDATES_NOTE: &str = "the prior period was collected before this clyde build tracked \
     repo-source provenance and several outcome counters (lines written, lines replaced); its \
     per-session outcome figures are not comparable and are omitted here.";

/// `true` when `report` predates repo-source provenance (design Phase 1-3 of this doc): at least
/// one session carries a `repo` but none carries a `repo_source`. Phase 3 is the first phase that
/// persists `repo_source` alongside `repo`, so this is a reliable signal already present in the
/// artifact that `report` was collected before every fidelity fix in this design landed --
/// including the Phase 7 `lines-written`/`lines-replaced` counters, which default to `0` under
/// `#[serde(default)]` and would otherwise read as a real zero measurement rather than "not
/// measured yet" for a session this old.
fn predates_fidelity_fields(report: &Report) -> bool {
    let has_repo = report.sessions.values().any(|s| s.repo.is_some());
    let has_repo_source = report.sessions.values().any(|s| s.repo_source.is_some());
    has_repo && !has_repo_source
}

/// Load, schema-gate, and aggregate a `--prior <report.json>` file into a [`PriorView`]. `None`
/// when `--prior` was not supplied. `current_days` is the CURRENT period's already-computed
/// `period.days`, used only to set [`PriorView::comparable`].
fn build_prior_view(prior_path: Option<&Path>, current_days: i64, pricing: &Pricing) -> Result<Option<PriorView>> {
    let Some(path) = prior_path else {
        debug!("render::build_prior_view: no --prior supplied");
        return Ok(None);
    };
    debug!("render::build_prior_view: path={}", path.display());
    let body =
        fs::read_to_string(path).with_context(|| format!("failed to read --prior report at {}", path.display()))?;
    check_schema_version(&body, path)?;
    let report: Report =
        serde_json::from_str(&body).with_context(|| format!("failed to parse --prior report at {}", path.display()))?;

    let days = (report.until.date_naive() - report.since.date_naive()).num_days() + 1;
    let comparable = days == current_days;
    let predates_fields = predates_fidelity_fields(&report).then(|| PRIOR_PREDATES_NOTE.to_string());
    // Aggregated through the SAME `aggregate::compute` as the current period (design Phase 8), so
    // both sides of the comparison are computed identically rather than by two drifting code paths.
    // `outliers_n` is 0: the prior period's outlier table is not part of this design's scope.
    let aggregates = aggregate::compute(&report, 0, pricing);
    let outcomes = if predates_fields.is_none() {
        report.totals.outcomes.as_ref().map(outcome_totals_view)
    } else {
        None
    };
    debug!(
        "render::build_prior_view: sessions={} days={} comparable={} predates-fields={} by-repo={} by-org={}",
        report.sessions.len(),
        days,
        comparable,
        predates_fields.is_some(),
        aggregates.by_repo.len(),
        aggregates.by_org.len()
    );
    Ok(Some(PriorView {
        since: report.since.format("%Y-%m-%d").to_string(),
        until: report.until.format("%Y-%m-%d").to_string(),
        days,
        comparable,
        predates_fields,
        totals: build_totals_view(&report),
        by_repo: aggregates.by_repo,
        by_org: aggregates.by_org,
        outcomes,
    }))
}

fn build_session_view<'a>(sid: &str, entry: &'a SessionEntry) -> SessionView<'a> {
    SessionView {
        short_id: short_id(sid).to_string(),
        title: entry.title.as_deref(),
        summary: entry.summary.as_deref(),
        tags: entry.tags.iter().map(String::as_str).collect(),
        repo: entry.repo.as_deref(),
        begin: entry.begin,
        end: entry.end,
        tokens_human: format_tokens_human(entry.total_tokens()),
        spend_display: format_optional_usd(entry.spend_usd),
        models: entry.models.keys().map(String::as_str).collect(),
        outcomes: entry.outcomes.as_ref(),
    }
}

/// Build the report-wide [`EfficiencyView`] from the collected sessions' curated signals + raw
/// passthrough (design Phase 5). The two ratios `totals` already carries authoritatively
/// (`cache-read-share`, `tool-error-rate`, both ratio-of-sums from Phase 4) are formatted straight
/// from `totals`; `cache-1h-write-fraction` is recomputed via the SAME `finalize` path over the
/// union of every session's raw counters (so it stays consistent with those two). Interrupts,
/// compactions, and the agent-type / by-skill / by-mcp buckets are additive, so they sum across the
/// report's rows. In the default (rollup) view each session is one row and these sums are exact; in
/// `--no-rollup` they sum over the displayed decomposition (documented tradeoff).
fn build_efficiency_view(report: &Report) -> EfficiencyView {
    debug!(
        "render::build_efficiency_view: sessions={} cache-read-share={:?} tool-error-rate={:?}",
        report.sessions.len(),
        report.totals.cache_read_share,
        report.totals.tool_error_rate
    );
    let mut grand = RawCounters::default();
    let mut agent: BTreeMap<String, WorkloadCost> = BTreeMap::new();
    let mut by_skill: BTreeMap<String, WorkloadCost> = BTreeMap::new();
    let mut by_mcp: BTreeMap<String, WorkloadCost> = BTreeMap::new();
    let mut interrupts: u64 = 0;
    let mut compactions: u64 = 0;
    for entry in report.sessions.values() {
        grand.merge(&entry.efficiency.aggregate.raw);
        interrupts += entry.interrupts;
        compactions += entry.compactions;
        merge_workload(&mut agent, &entry.agent_type_costs);
        merge_workload(&mut by_skill, &entry.by_skill);
        merge_workload(&mut by_mcp, &entry.by_mcp);
    }
    let grand_signals = finalize(grand);
    let skill_covered: f64 = by_skill.values().map(|w| w.cost_usd).sum();
    let mcp_covered: f64 = by_mcp.values().map(|w| w.cost_usd).sum();
    let view = EfficiencyView {
        cache_read_share: fmt_ratio_pct(report.totals.cache_read_share),
        tool_error_rate: fmt_ratio_pct(report.totals.tool_error_rate),
        cache_1h_write_fraction: fmt_ratio_pct(grand_signals.cache_1h_write_fraction),
        interrupts,
        compactions,
        agent_type_costs: workload_rows(agent),
        by_skill: workload_rows(by_skill),
        by_mcp: workload_rows(by_mcp),
        by_skill_coverage: coverage_note(skill_covered, report.totals.spend_usd),
        by_mcp_coverage: coverage_note(mcp_covered, report.totals.spend_usd),
    };
    debug!(
        "render::build_efficiency_view: agent-types={} skills={} mcp-tools={} interrupts={} compactions={} skill-coverage={} mcp-coverage={}",
        view.agent_type_costs.len(),
        view.by_skill.len(),
        view.by_mcp.len(),
        view.interrupts,
        view.compactions,
        view.by_skill_coverage,
        view.by_mcp_coverage
    );
    view
}

/// The `by-skill` / `by-mcp` coverage statement (design Phase 5): how much of `totals.spend` a TAG
/// set accounts for, and on what pricing basis. These buckets are not a partition -- a dollar can
/// carry no skill tag or several, and they keep the catalog's embedded prices -- so the honest move
/// is to state the coverage as a computed fact rather than let the reader reconcile them.
///
/// A zero total renders `0.0%` rather than a `NaN`, matching `compute_attribution`'s precedent.
fn coverage_note(covered: f64, total: f64) -> String {
    let share = if total == 0.0 {
        "0.0%".to_string()
    } else {
        format!("{:.1}%", covered / total * 100.0)
    };
    format!(
        "{} of {} ({}), embedded-price basis",
        format_usd(covered),
        format_usd(total),
        share
    )
}

/// A ratio in `[0, 1]` as a one-decimal percent string; `None` -> `"n/a"` (never `NaN`). One decimal
/// matches the existing `aggregates.cache.cache-read-share` display convention.
fn fmt_ratio_pct(x: Option<f64>) -> String {
    match x {
        Some(v) => format!("{:.1}%", v * 100.0),
        None => "n/a".to_string(),
    }
}

/// Accumulate a per-key `WorkloadCost` map into `base` (tokens + `$` both additive).
fn merge_workload(base: &mut BTreeMap<String, WorkloadCost>, add: &BTreeMap<String, WorkloadCost>) {
    for (k, v) in add {
        let bucket = base.entry(k.clone()).or_default();
        bucket.tokens += v.tokens;
        bucket.cost_usd += v.cost_usd;
    }
}

/// Convert an accumulated `WorkloadCost` map into pre-formatted display rows, pre-sorted by spend
/// descending (ties broken by name for determinism) so the model copies the order, never re-sorts.
fn workload_rows(map: BTreeMap<String, WorkloadCost>) -> Vec<WorkloadRow> {
    let mut rows: Vec<(String, WorkloadCost)> = map.into_iter().collect();
    rows.sort_by(|a, b| {
        b.1.cost_usd
            .partial_cmp(&a.1.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    rows.into_iter()
        .map(|(name, wc)| WorkloadRow {
            name,
            tokens_human: format_tokens_human(wc.tokens),
            spend: format_usd(wc.cost_usd),
        })
        .collect()
}

fn load_template(custom: Option<&Path>) -> Result<Template> {
    match custom {
        Some(path) => {
            let body =
                fs::read_to_string(path).with_context(|| format!("failed to read template at {}", path.display()))?;
            Ok(Template::Custom(body))
        }
        None => Ok(Template::BuiltIn),
    }
}

pub fn to_markdown(report: &Report, template: &Template, pricing: &Pricing) -> String {
    match template {
        Template::BuiltIn => render_built_in(report, pricing),
        Template::Custom(body) => render_custom(report, body, pricing),
    }
}

fn render_built_in(report: &Report, pricing: &Pricing) -> String {
    let mut out = String::new();
    out.push_str("# Claude Code session report\n\n");
    out.push_str(&format!("- **host:** {}\n", report.host));
    out.push_str(&format!(
        "- **period:** {} -> {}\n",
        report.since.format("%Y-%m-%d"),
        report.until.format("%Y-%m-%d")
    ));
    out.push_str(&format!("- **sessions:** {}\n", report.totals.sessions));

    let total_tokens: u64 = report.totals.models.values().map(|m| m.total).sum();
    out.push_str(&format!("- **total tokens:** {}\n", format_int(total_tokens)));
    out.push_str(&format!("- **total spend:** {}\n", format_usd(report.totals.spend_usd)));
    out.push_str(&format!("- **pricing basis:** {}\n", build_basis(pricing).note));
    if !report.totals.untracked_models.is_empty() {
        out.push_str(&format!(
            "- **untracked models:** {}\n",
            report.totals.untracked_models.join(", ")
        ));
    }
    out.push('\n');

    out.push_str("## Totals by model\n\n");
    if report.totals.models.is_empty() {
        out.push_str("_no model usage_\n\n");
    } else {
        out.push_str("| model | input | output | cache 5m write | cache 1h write | cache read | total | spend |\n");
        out.push_str("|-------|------:|-------:|---------------:|---------------:|-----------:|------:|------:|\n");
        for (model, m) in &report.totals.models {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
                model,
                format_int(m.input),
                format_int(m.output),
                format_int(m.cache_5m_write),
                format_int(m.cache_1h_write),
                format_int(m.cache_read),
                format_int(m.total),
                format_optional_usd(m.spend_usd),
            ));
        }
        out.push('\n');
    }

    // Sourced from `aggregate::compute` (design: "aggregate.rs subsumes and replaces
    // render::group_by_repo"). Outliers are unused by this table, so 0 is passed rather than
    // computing a table this renderer never shows.
    let by_repo = aggregate::compute(report, 0, pricing).by_repo;
    out.push_str("## By repo\n\n");
    if by_repo.is_empty() {
        out.push_str("_no sessions with a detected repo_\n\n");
    } else {
        out.push_str("| repo | sessions | total tokens | spend | models |\n");
        out.push_str("|------|---------:|-------------:|------:|--------|\n");
        for row in &by_repo {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                row.repo,
                row.sessions,
                row.tokens_human,
                row.spend,
                row.models.join(", "),
            ));
        }
        out.push('\n');
    }

    out.push_str("## Sessions\n\n");
    let mut by_repo_with_none: BTreeMap<String, Vec<(String, &SessionEntry)>> = BTreeMap::new();
    for (sid, entry) in &report.sessions {
        let key = entry.repo.clone().unwrap_or_else(|| "(no repo)".into());
        by_repo_with_none.entry(key).or_default().push((sid.clone(), entry));
    }
    for (key, mut entries) in by_repo_with_none {
        entries.sort_by_key(|a| a.1.begin);
        out.push_str(&format!("### {}\n\n", key));
        for (sid, entry) in entries {
            let title = entry.title.as_deref().unwrap_or("<untitled>");
            let short = short_id(&sid);
            let models_str: Vec<&str> = entry.models.keys().map(|s| s.as_str()).collect();
            let untracked_suffix = if entry.untracked_models.is_empty() {
                String::new()
            } else {
                format!(" | untracked: {}", entry.untracked_models.join(", "))
            };
            out.push_str(&format!(
                "- **{}** ({}) {} -> {} | {} | {} tokens | {}{}\n",
                title,
                short,
                entry.begin.format("%Y-%m-%d %H:%M"),
                entry.end.format("%Y-%m-%d %H:%M"),
                models_str.join(", "),
                format_int(entry.total_tokens()),
                format_optional_usd(entry.spend_usd),
                untracked_suffix,
            ));
        }
        out.push('\n');
    }

    out
}

fn render_custom(report: &Report, body: &str, pricing: &Pricing) -> String {
    let total_tokens: u64 = report.totals.models.values().map(|m| m.total).sum();
    body.replace("{{host}}", &report.host)
        .replace("{{since}}", &report.since.format("%Y-%m-%d").to_string())
        .replace("{{until}}", &report.until.format("%Y-%m-%d").to_string())
        .replace("{{session-count}}", &report.totals.sessions.to_string())
        .replace("{{total-tokens}}", &format_int(total_tokens))
        .replace("{{total-spend}}", &format_usd(report.totals.spend_usd))
        .replace("{{basis-note}}", &build_basis(pricing).note)
}

fn write_pdf(markdown: &str, output: &Path, pdf_engine: &str) -> Result<()> {
    debug!("render::write_pdf: output={} engine={}", output.display(), pdf_engine);
    let mut tmp = tempfile::NamedTempFile::new().context("failed to create temp markdown for pandoc")?;
    tmp.write_all(markdown.as_bytes())
        .context("failed to write temp markdown for pandoc")?;
    tmp.flush().context("failed to flush temp markdown")?;

    let mut cmd = Command::new("pandoc");
    cmd.arg(tmp.path())
        .arg(format!("--pdf-engine={}", pdf_engine))
        .arg("-o")
        .arg(output);
    let result = run_bounded("pandoc (--format pdf)", &mut cmd, |e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            eyre::eyre!(
                "pandoc is required for --format pdf output but was not found on PATH; install pandoc and try again"
            )
        } else {
            eyre::eyre!("failed to invoke pandoc: {}", e)
        }
    })?;

    if !result.status.success() {
        bail!(
            "pandoc exited with {} (engine '{}'); the output was not written. If the engine is missing, install it or pass --pdf-engine=<other>. {}",
            result.status,
            pdf_engine,
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    Ok(())
}

/// Marquee post title / slug seed, derived from the report's period so a temp-dir name never
/// leaks into the published slug (e.g. `claude-report-2026-07`).
fn marquee_title(report: &Report) -> String {
    format!("claude-report-{}", report.since.format("%Y-%m"))
}

/// Write the rendered markdown as `index.md` in a temp dir and publish it to marquee, letting the
/// marquee server apply its house style. Returns the published URL.
fn publish_marquee_markdown(markdown: &str, report: &Report, cfg: &RenderConfig) -> Result<OutputDest> {
    debug!("render::publish_marquee_markdown: space={:?}", cfg.space);
    let dir = tempfile::tempdir().context("failed to create temp dir for marquee publish")?;
    let index = dir.path().join("index.md");
    fs::write(&index, markdown).with_context(|| format!("failed to write {}", index.display()))?;
    let url = marquee_publish(dir.path(), report, cfg)?;
    Ok(OutputDest::Marquee(url))
}

/// Write the model-authored, validated HTML document as `index.html` in a temp dir and publish it
/// to marquee (which hosts our HTML as-is under its Okta-gated HTML lane). Pandoc is NOT involved:
/// the artifact arrives already complete and self-contained from `summarize::html`. Returns the URL.
fn publish_marquee_html(html: &str, report: &Report, cfg: &RenderConfig) -> Result<OutputDest> {
    debug!(
        "render::publish_marquee_html: space={:?} bytes={}",
        cfg.space,
        html.len()
    );
    let dir = tempfile::tempdir().context("failed to create temp dir for marquee publish")?;
    let index = dir.path().join("index.html");
    fs::write(&index, html).with_context(|| format!("failed to write {}", index.display()))?;
    let url = marquee_publish(dir.path(), report, cfg)?;
    Ok(OutputDest::Marquee(url))
}

/// Publish a prepared directory (containing `index.md` or `index.html`) to marquee, ensuring an
/// authenticated session first. Returns the published URL parsed from marquee's stdout.
fn marquee_publish(dir: &Path, report: &Report, cfg: &RenderConfig) -> Result<String> {
    debug!("render::marquee_publish: dir={} space={:?}", dir.display(), cfg.space);
    ensure_marquee_auth()?;
    let title = marquee_title(report);
    let mut cmd = Command::new("marquee");
    cmd.arg("publish")
        .arg(dir)
        .arg("--title")
        .arg(&title)
        .arg("--output")
        .arg("url");
    if let Some(space) = &cfg.space {
        cmd.arg("--space").arg(space);
    }
    let output = run_bounded("marquee publish", &mut cmd, marquee_spawn_err)?;
    if !output.status.success() {
        bail!(
            "marquee publish failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if url.is_empty() {
        bail!("marquee publish reported success but returned no URL");
    }
    Ok(url)
}

/// Ensure a usable marquee session: probe `marquee whoami`, and on failure attempt an interactive
/// `marquee login` ONCE before re-probing. The login is attempted ONLY when both stdin and stdout
/// are TTYs — `marquee login` is an interactive browser/device OAuth flow, so auto-launching it
/// over SSH-without-a-tty, in CI, or under an agent would block `report render` forever. Outside a
/// TTY (or if login/re-probe still fails) we error with the captured `whoami` detail and the
/// manual remediation.
fn ensure_marquee_auth() -> Result<()> {
    debug!("render::ensure_marquee_auth");
    let whoami = marquee_whoami()?;
    if whoami.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&whoami.stderr).trim().to_string();
    let detail = if detail.is_empty() {
        "no detail".to_string()
    } else {
        detail
    };

    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        bail!("not authenticated with marquee (whoami: {detail}); run `marquee login` and retry");
    }

    log::warn!("marquee: not authenticated ({detail}); attempting interactive `marquee login`");
    // Interactive: inherit the terminal for the browser/device flow. NOT time-bounded — a human is
    // driving it — which is exactly why it is gated behind the TTY check above.
    let status = Command::new("marquee")
        .arg("login")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(marquee_spawn_err)?;
    if !status.success() {
        bail!("`marquee login` failed ({status}); run `marquee login` manually and retry");
    }
    if marquee_whoami()?.status.success() {
        return Ok(());
    }
    bail!("still not authenticated with marquee after login; run `marquee login` and retry");
}

/// Run `marquee whoami` with a wall-clock timeout, returning its captured output (exit 0 = a valid
/// cached token). Stderr is preserved so a non-auth failure (e.g. a malformed marquee config) can
/// be surfaced rather than silently read as "logged out".
fn marquee_whoami() -> Result<Output> {
    let mut cmd = Command::new("marquee");
    cmd.arg("whoami");
    let output = run_bounded("marquee whoami", &mut cmd, marquee_spawn_err)?;
    debug!("render::marquee_whoami: success={}", output.status.success());
    Ok(output)
}

/// Map a `marquee` spawn error to a helpful message, distinguishing "not installed" from other
/// invocation failures.
fn marquee_spawn_err(e: std::io::Error) -> eyre::Report {
    if e.kind() == std::io::ErrorKind::NotFound {
        eyre::eyre!(
            "the `marquee` CLI is required for --format marquee-html / marquee-markdown but was not found on PATH; install it and try again"
        )
    } else {
        eyre::eyre!("failed to invoke marquee: {}", e)
    }
}

#[cfg(test)]
mod tests;
