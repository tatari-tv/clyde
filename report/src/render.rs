use crate::aggregate::{self, Aggregates, Attribution, UnitCosts};
use crate::cli::Format;
use crate::config::{RenderConfig, TransportKind};
use crate::fmt::{format_optional_usd, format_tokens_human, format_usd, short_id};
use crate::outcome::OutcomeTotals;
use crate::persona::{self, PersonaBlock};
use crate::proc::run_bounded;
use crate::reconcile::Reconciliation;
use crate::report::{Report, SCHEMA_VERSION, SessionEntry};
use crate::summarize;
use crate::{OutputDest, RunResult};
use chrono::{DateTime, Utc};
use claude_pricing::Pricing;
use eyre::{Context, Result, bail};
use log::debug;
use serde::Serialize;
use std::ffi::OsStr;
use std::fs;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};

mod document;
pub(crate) mod facts;
mod reconciliation;
mod slots;
use reconciliation::{build_reconciliation_view, no_reconcile_warning};
mod workload;
use document::{Artifact, ChartMode, PriorView};
use facts::RenderContext;
use workload::build_efficiency_view;

/// The view-assembly inputs that are neither the report, the persona, nor the pricing feed: the
/// three optional file/identity arguments plus the tradeoffs flag. A struct so
/// [`document::build_views`] and its callers cannot transpose four same-shaped optionals.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ViewOpts<'a> {
    pub(crate) include_tradeoffs: bool,
    pub(crate) prior: Option<&'a Path>,
    pub(crate) reconcile: Option<&'a Path>,
    pub(crate) reconcile_user: Option<&'a str>,
}

const STDOUT_SIGIL: &str = "-";

pub fn run(cfg: &RenderConfig, pricing: &Pricing) -> Result<RunResult> {
    log::info!(
        "render::run: input={} format={:?} space={:?} outliers={} reconcile={:?} prior={:?} llm={:?}",
        cfg.input.display(),
        cfg.format,
        cfg.space,
        cfg.outliers,
        cfg.reconcile,
        cfg.prior,
        cfg.llm
    );

    // Design Phase 12, "Absence is never silent" -- stderr half; the artifact's half is
    // `reconciliation::NO_RECONCILE_NOTE`. Advisory only, mirroring the `--min-enrichment` warning.
    if let Some(warning) = no_reconcile_warning(cfg.reconcile.as_deref()) {
        log::warn!("render::run: {warning}");
        eprintln!("{warning}");
    }

    if let Some(ext) = cfg.input.extension().and_then(OsStr::to_str)
        && (ext.eq_ignore_ascii_case("yml") || ext.eq_ignore_ascii_case("yaml"))
    {
        bail!(
            "input file ends in .yml/.yaml; report collect emits JSON. Re-run report collect to regenerate as .json."
        );
    }

    let report = load_report(&cfg.input, "report")?;

    let artifact = generate_markdown(cfg, &report, pricing)?;
    let dest = route_document_artifact(&artifact, &report, cfg)?;

    Ok(RunResult {
        sessions_emitted: report.totals.sessions,
        output: dest,
    })
}

/// Produce the artifact: the deterministic document, plus whatever prose the slots returned.
///
/// There is exactly one renderer now. Rust authors every table, number, and chart
/// (`document::render`); the slots contribute digit-free prose (`slots::generate`) that cannot fail
/// the render. Nothing here can reject an artifact, because nothing here is authored by a model.
fn generate_markdown(cfg: &RenderConfig, report: &Report, pricing: &Pricing) -> Result<Artifact> {
    let default_persona = PersonaBlock::default();
    let resolved = persona::whoami();
    let aggregates = aggregate::compute(report, cfg.outliers, pricing);
    let block = document::build_views(
        report,
        &aggregates,
        resolved.as_ref().unwrap_or(&default_persona),
        pricing,
        ViewOpts {
            include_tradeoffs: cfg.include_tradeoffs,
            prior: cfg.prior.as_deref(),
            reconcile: cfg.reconcile.as_deref(),
            reconcile_user: cfg.reconcile_user.as_deref(),
        },
    )?;
    let prose = slot_prose(cfg, &document::registry(&block));
    Ok(document::render(&block, &prose, chart_mode(cfg)))
}

/// Generate the prose slots for this render, or an empty set.
///
/// This is the top of the degradation ladder and it CANNOT fail. A host with no transport at all
/// (no `claude` on PATH, no API key) is the offline story, not an error: the deterministic document
/// is already complete, so an absent transport costs prose and nothing else. Monomorphized per
/// transport arm, per the house generics-for-DI rule.
fn slot_prose(cfg: &RenderConfig, reg: &facts::FactRegistry) -> document::SlotProse {
    let model = &cfg.markdown_model;
    let ceiling = cfg.slot_max_output_tokens;
    match resolve_selected_transport(cfg.llm, cfg.format) {
        Ok(TransportKind::Api) => match summarize::ApiTransport::from_env() {
            Ok(t) => slots::generate(&t, reg, model, ceiling, cfg.include_tradeoffs),
            Err(e) => no_transport(e),
        },
        Ok(TransportKind::Cli) => match summarize::CliTransport::resolve() {
            Ok(t) => slots::generate(&t, reg, model, ceiling, cfg.include_tradeoffs),
            Err(e) => no_transport(e),
        },
        Err(e) => no_transport(e),
    }
}

/// Degrade to a prose-free artifact, loudly. Never silent: an operator reading stderr must be able
/// to tell a thin report from a broken one.
fn no_transport(e: eyre::Report) -> document::SlotProse {
    let warning = format!(
        "no LLM transport available, so the report's prose sections will be empty; the data \
         sections are unaffected: {e:#}"
    );
    log::warn!("render::slot_prose: {warning}");
    eprintln!("{warning}");
    document::SlotProse::new()
}

/// Where the eval's prose comes from.
///
/// `Stubbed` renders with NO transport, which makes the artifact fully deterministic -- that is what
/// a golden is, and it is why the golden layer runs offline and free in `otto ci`. `Live` generates
/// real slots, which is what `otto eval` pays for and what the judge scores.
#[derive(Debug, Clone, Copy)]
pub(crate) enum SlotSource<'a> {
    Stubbed,
    Live {
        llm: crate::cli::Llm,
        model: &'a str,
        ceiling: u32,
    },
}

/// The prose slots one render produced, keyed by slot name.
pub(crate) type SlotProse = document::SlotProse;

/// One eval render: the artifact and the raw slot prose behind it.
///
/// The serialized context block is NOT returned: the eval builds it through
/// [`build_context_block`], which routes through the same `document::build_views` this does, so a
/// second copy here would be two sources for one value.
pub(crate) struct EvalRender {
    pub(crate) markdown: String,
    /// The slots' prose BEFORE interpolation, which is the form the digit contract applies to.
    pub(crate) prose: document::SlotProse,
    /// How many slots were attempted, so a caller can report how many degraded.
    pub(crate) attempted: usize,
}

/// Render an artifact for the eval, through the SAME document layer users get.
///
/// The eval must not have a second pipeline: a fresh eval render is the user's render or it measures
/// nothing. This shares `document::build_views`, `slots::generate`, and `document::render` with
/// [`run`]; only the prose SOURCE differs, and only so goldens can be deterministic.
pub(crate) fn for_eval(
    report: &Report,
    pricing: &Pricing,
    persona: Option<&PersonaBlock>,
    opts: ViewOpts<'_>,
    outliers: usize,
    slots_from: SlotSource<'_>,
) -> Result<EvalRender> {
    let default_persona = PersonaBlock::default();
    let aggregates = aggregate::compute(report, outliers, pricing);
    let block = document::build_views(report, &aggregates, persona.unwrap_or(&default_persona), pricing, opts)?;
    let reg = document::registry(&block);
    let (prose, attempted) = match slots_from {
        SlotSource::Stubbed => (document::SlotProse::new(), 0),
        SlotSource::Live { llm, model, ceiling } => {
            let prose = match resolve_selected_transport(llm, Format::Markdown)? {
                TransportKind::Api => slots::generate(
                    &summarize::ApiTransport::from_env()?,
                    &reg,
                    model,
                    ceiling,
                    opts.include_tradeoffs,
                ),
                TransportKind::Cli => slots::generate(
                    &summarize::CliTransport::resolve()?,
                    &reg,
                    model,
                    ceiling,
                    opts.include_tradeoffs,
                ),
            };
            (prose, slots::count(opts.include_tradeoffs))
        }
    };
    // `ChartMode::Table` so a golden is one self-contained file: a golden that referenced sibling
    // SVGs would need those committed and diffed too, and the chart DATA is identical either way.
    let artifact = document::render(&block, &prose, ChartMode::Table);
    debug!(
        "render::for_eval: artifact bytes={} slots filled={}/{attempted}",
        artifact.markdown.len(),
        prose.len()
    );
    Ok(EvalRender {
        markdown: artifact.markdown,
        prose,
        attempted,
    })
}

/// Whether charts ship as sibling SVG assets or as inline markdown tables.
///
/// PDF and stdout can only ever be `Table`: pandoc runs on a tempfile and stdout has no directory,
/// so a sibling file cannot exist on either path. The data is identical in both forms.
fn chart_mode(cfg: &RenderConfig) -> ChartMode {
    let to_stdout = cfg.output.as_deref().is_some_and(|p| p.as_os_str() == STDOUT_SIGIL);
    if matches!(cfg.format, Format::Pdf) || to_stdout {
        ChartMode::Table
    } else {
        ChartMode::Svg
    }
}

/// Route an already-rendered document artifact, sibling assets included.
fn route_document_artifact(artifact: &Artifact, report: &Report, cfg: &RenderConfig) -> Result<OutputDest> {
    debug!(
        "render::route_document_artifact: format={:?} bytes={} assets={}",
        cfg.format,
        artifact.markdown.len(),
        artifact.assets.len()
    );
    match cfg.format {
        Format::Markdown => write_local_markdown(artifact, report, cfg),
        Format::Pdf => write_local_pdf(&artifact.markdown, report, cfg),
        Format::MarqueeMarkdown => publish_marquee_markdown(artifact, report, cfg),
    }
}

/// Write a document artifact's sibling assets into `dir`. Called for every destination that HAS a
/// directory; the two that do not (stdout, pandoc's tempfile) render charts as tables and arrive
/// here with an empty asset list.
fn write_assets(artifact: &Artifact, dir: &Path) -> Result<()> {
    for asset in &artifact.assets {
        let path = dir.join(&asset.filename);
        debug!(
            "render::write_assets: path={} bytes={}",
            path.display(),
            asset.body.len()
        );
        fs::write(&path, &asset.body).with_context(|| format!("failed to write asset {}", path.display()))?;
    }
    Ok(())
}

/// Write the rendered markdown to `-o <path>`, to stdout (`-o -`), or to the default
/// `./<YYYY-MM>-claude-report.md` beside the input when `-o` is omitted.
fn write_local_markdown(artifact: &Artifact, report: &Report, cfg: &RenderConfig) -> Result<OutputDest> {
    let output = match cfg.output.as_deref() {
        Some(p) => p.to_path_buf(),
        None => default_output_path(report, Format::Markdown),
    };
    debug!("render::write_local_markdown: output={}", output.display());

    if output.as_os_str() == STDOUT_SIGIL {
        std::io::stdout()
            .write_all(artifact.markdown.as_bytes())
            .context("failed to write markdown to stdout")?;
        return Ok(OutputDest::Stdout);
    }
    let dir = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(dir).with_context(|| format!("failed to create output dir {}", dir.display()))?;
    fs::write(&output, &artifact.markdown)
        .with_context(|| format!("failed to write markdown to {}", output.display()))?;
    write_assets(artifact, dir)?;
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
    let ext = if matches!(format, Format::Pdf) { "pdf" } else { "md" };
    std::path::PathBuf::from(format!("./{}-claude-report.{}", prefix, ext))
}

/// Resolve the configured transport selection against this host: is `claude` on PATH, and is a key
/// set? The impure half of the decision, kept to one line each so `config::resolve_transport` stays
/// pure and its whole precedence matrix is unit-testable.
///
/// `which::which` mirrors `clyde::resolve_claude`, which already canonicalizes a relative PATH hit.
///
/// Takes the two values it needs rather than a whole [`RenderConfig`], so a caller that has a
/// selection and a format but no config (the eval) resolves through the SAME precedence matrix
/// rather than a second copy of it.
pub(crate) fn resolve_selected_transport(llm: crate::cli::Llm, format: Format) -> Result<TransportKind> {
    let resolved = crate::config::resolve_transport(
        llm,
        which::which("claude").is_ok(),
        summarize::api_key_from_env().is_some(),
        format,
    )?;
    // Log the SELECTION for both transports, not just cli: an operator reading a log must be able to
    // tell what paid for an artifact without rerunning it. The cli path adds the resolved binary path
    // and version in `CliTransport::resolve`.
    log::info!("render: llm transport selected={resolved:?} (requested={llm:?}) format={format:?}");
    Ok(resolved)
}

/// Read, schema-gate, and parse a collected artifact. ONE path for every reader of one: the primary
/// `-i` input, `--prior`, and the eval's fixtures. `label` names which input failed, so an error on
/// a prior artifact does not read as an error on the report being rendered.
pub(crate) fn load_report(path: &Path, label: &str) -> Result<Report> {
    let body = fs::read_to_string(path).with_context(|| format!("failed to read {label} at {}", path.display()))?;
    check_schema_version(&body, path)?;
    serde_json::from_str(&body).with_context(|| format!("failed to parse {label} at {}", path.display()))
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

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ContextBlock<'a> {
    persona: &'a PersonaBlock,
    options: ContextOptions,
    /// The pricing basis for every dollar figure below (design "Pricing basis, always present",
    /// Phase 6): what it is priced against, whether it is an invoice (never), and which feed
    /// resolved it. `basis.note` is the required, verbatim header disclosure both templates carry.
    basis: Basis,
    /// How this artifact was produced, one pre-formatted sentence per note, straight from
    /// [`Report::notes`]: always the M2 window statement, plus one line per field a MERGE could not
    /// carry. Absent entirely (never an empty list) when the report recorded none, so the prompt's
    /// "omit it" rule needs no empty-vs-absent special case. Without this the reader never learns
    /// the window is session-level or that a merged field was omitted (design "API Design", `notes`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notes: Vec<&'a str>,
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
    /// Present only when `--reconcile` matched this report's window (design Phase 12, finding 6
    /// closure): billed spend from the authoritative Analytics export against the modeled total.
    /// See [`Self::reconciliation_status`] (ALWAYS present) for the never-silent absence case.
    #[serde(skip_serializing_if = "Option::is_none")]
    reconciliation: Option<Reconciliation>,
    /// Always present (design Phase 12, "Absence is never silent"): quoted verbatim, states
    /// whether [`Self::reconciliation`] is this render's authoritative figure or absent because no
    /// export was supplied -- see [`reconciliation::NO_RECONCILE_NOTE`] / [`reconciliation::reconciled_note`].
    reconciliation_status: String,
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
/// about it, serialized. The eval is the only consumer left: it reads the block to know what the
/// artifact was built FROM, for the judge brief and the mechanical citation checks.
pub(crate) fn build_context_block(
    report: &Report,
    include_tradeoffs: bool,
    persona: Option<&PersonaBlock>,
    pricing: &Pricing,
    outliers_n: usize,
    prior_path: Option<&Path>,
    reconcile_path: Option<&Path>,
    reconcile_user: Option<&str>,
) -> Result<RenderContext> {
    debug!(
        "render::build_context_block: sessions={} include_tradeoffs={} outliers-n={} prior={:?} \
         reconcile={:?} reconcile-user={:?}",
        report.sessions.len(),
        include_tradeoffs,
        outliers_n,
        prior_path,
        reconcile_path,
        reconcile_user
    );
    let default_persona = PersonaBlock::default();
    let aggregates = aggregate::compute(report, outliers_n, pricing);
    let opts = ViewOpts {
        include_tradeoffs,
        prior: prior_path,
        reconcile: reconcile_path,
        reconcile_user,
    };
    let block = document::build_views(report, &aggregates, persona.unwrap_or(&default_persona), pricing, opts)?;
    let json = serde_json::to_string(&block).context("failed to serialize context block to JSON")?;
    debug!("render::build_context_block: context_bytes={}", json.len());
    Ok(RenderContext { json })
}

/// The artifact's production notes, borrowed as-is: [`Report::notes`] is already a list of
/// display sentences (the M2 window statement, and one line per field a merge omitted), so there is
/// nothing to format. An empty list serializes to no key at all.
fn build_notes(report: &Report) -> Vec<&str> {
    let notes: Vec<&str> = report.notes.iter().map(String::as_str).collect();
    debug!("render::build_notes: notes={}", notes.len());
    notes
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
fn publish_marquee_markdown(artifact: &Artifact, report: &Report, cfg: &RenderConfig) -> Result<OutputDest> {
    debug!("render::publish_marquee_markdown: space={:?}", cfg.space);
    let dir = tempfile::tempdir().context("failed to create temp dir for marquee publish")?;
    let index = dir.path().join("index.md");
    fs::write(&index, &artifact.markdown).with_context(|| format!("failed to write {}", index.display()))?;
    // The chart SVGs ride the SAME bundle as `index.md`, so marquee picks them up as post assets
    // and `![](chart-N.svg)` has something to resolve to.
    write_assets(artifact, dir.path())?;
    let url = marquee_publish(dir.path(), report, cfg)?;
    Ok(OutputDest::Marquee(url))
}

/// Publish a prepared directory (containing `index.md` and any sibling assets) to marquee,
/// ensuring an
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
            "the `marquee` CLI is required for --format marquee-markdown but was not found on PATH; install it and try again"
        )
    } else {
        eyre::eyre!("failed to invoke marquee: {}", e)
    }
}

#[cfg(test)]
mod tests;
