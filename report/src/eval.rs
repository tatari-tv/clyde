//! `clyde report eval`: render the frozen fixtures, check them mechanically, and score them with a
//! judge (design Phase 13).
//!
//! Two layers, split by what they cost:
//!
//! - [`mechanical`] is deterministic, offline and free, so it runs in `otto ci` against the
//!   COMMITTED goldens and again here against every FRESH render before the judge is paid for.
//! - [`judge`] costs tokens and needs a network, so it runs only here, from `otto eval`, before a
//!   release (design Non-Goals: "Making the *judged* render eval part of `otto ci`").
//!
//! Three things about this module are load-bearing rather than incidental.
//!
//! **Fixtures are synthesized, never derived.** `tatari-tv/clyde` is a PUBLIC repo, so a committed
//! fixture may not come from real session data; the titles and enrich summaries ARE the sensitive
//! payload and the eval needs them realistic. [`synth`] invents all of it from a fixed seed. The
//! real-data eval stays local: `--fixture <dir>` accepts an uncommitted directory, and
//! `fixtures/report/local/` is gitignored.
//!
//! **The persona never comes from the machine.** A render normally calls `persona::whoami()`; this
//! module passes the fixture's own INVENTED persona instead, because splicing the operator's real
//! name, title, team and email into an artifact committed to a public repo is the same leak by
//! another route.
//!
//! **Pricing is pinned to [`Pricing::embedded`].** A fixture priced against the live feed would
//! score differently on two days because the feed moved, which measures the feed rather than the
//! render, and would silently invalidate every committed golden the next time the feed refreshed.
//! `eval::tests::fixture_models_still_carry_the_rates_the_goldens_were_rendered_against` is the
//! loud version of that failure, with the remedy in its message.

// `synth` is `pub` because the `fixtures` bin (a separate crate) drives it; the rest are
// crate-internal, so their types can stay `pub(crate)` alongside `quotable::RenderContext`.
pub(crate) mod fixture;
pub(crate) mod judge;
pub(crate) mod mechanical;
pub mod synth;

use crate::aggregate::DEFAULT_OUTLIERS;
use crate::cli::{Format, Llm};
use crate::config::TransportKind;
use crate::quotable::RenderContext;
use crate::render;
use crate::report::Report;
use crate::summarize::{ApiTransport, CliTransport};
use crate::{OutputDest, RunResult};
use chrono::{DateTime, Utc};
use claude_pricing::Pricing;
use eyre::{Context, Result, bail};
use fixture::{Dimension, Fixture};
use judge::Verdict;
use log::debug;
use mechanical::{Finding, Ground, Kind};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Default destination for the scored report when `--out` is omitted. Gitignored: it is a run
/// artifact, not a committed one.
pub const DEFAULT_OUT: &str = "./eval-report.json";

/// The resolved `report eval` surface.
#[derive(Debug, Clone)]
pub struct EvalConfig {
    /// Fixture directories to evaluate. Defaults to the three committed ones under
    /// `fixtures/report/`; `--fixture` replaces the set, which is how a local real-data directory
    /// is evaluated without entering git.
    pub fixtures: Vec<PathBuf>,
    /// Model pin for the judge (`--judge`), defaulting to the markdown render pin so the eval needs
    /// no config key of its own.
    pub judge_model: String,
    pub out: PathBuf,
    /// Overwrite each fixture's goldens with this run's fresh render (`--write-goldens`). Only a
    /// render that PASSED its mechanical checks is written: a golden is a known-good artifact by
    /// definition, and committing a failing one would make `otto ci` green against a broken render.
    pub write_goldens: bool,
    pub llm: Llm,
    pub markdown_model: String,
    pub html_model: String,
    pub markdown_max_output_tokens: u32,
    pub html_max_output_tokens: u32,
}

/// One fixture's result.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct FixtureOutcome {
    pub name: String,
    pub summary: String,
    pub sessions: usize,
    pub spend: String,
    /// Whether the fresh markdown render survived the render pipeline's own guards, and why not
    /// when it did not. A rejection is RECORDED rather than aborting the whole run: one fixture's
    /// stochastic guard trip must not throw away the other fixtures' paid calls (see [`Guards`]).
    pub markdown_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown_error: Option<String>,
    /// Mechanical findings against the fresh markdown render. Empty is a pass.
    pub markdown_findings: Vec<Finding>,
    /// The same, for the fresh HTML render and its geometry allowlist.
    pub html_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html_error: Option<String>,
    pub html_findings: Vec<Finding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    /// `(dimension, score, floor)` for every dimension below its floor.
    pub regressions: Vec<Regression>,
    pub passed: bool,
}

/// A judged dimension that fell below the fixture's committed floor.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Regression {
    pub dimension: String,
    pub score: u8,
    pub floor: u8,
    pub reason: String,
}

/// How often a FRESH render was rejected by its own guards across this run, per path.
///
/// This is a MEASUREMENT, not a gate, and it is the instrument the design asked for. Both guards
/// are stochastic against a live model and both were observed failing on one invocation and passing
/// on an identical retry:
///
/// - the HTML path's geometry allowlist (Phase 11), which Phase 12 saw reject a chart `<svg>`
///   carrying `preserveaspectratio`, an attribute the allowlist does not permit;
/// - the prose path's quotable-facts guard (Phase 10), whose own notes record that a narrowed
///   whitelist trades silent acceptance for loud rejection.
///
/// Whether to widen either list is a pending decision, so this sizes the problem and reports it
/// rather than quietly loosening the guard that found it. A MARKDOWN rejection fails its fixture
/// (there is no artifact left to judge); an HTML rejection does not, because gating on it would
/// make `otto eval` flake for exactly the reason it exists to measure. Either way the RATE is what
/// this report is for.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Guards {
    pub markdown_renders: usize,
    pub markdown_rejections: usize,
    /// Percent of markdown renders the quotable-facts guard rejected, as a display string.
    pub markdown_rejection_rate: String,
    pub html_renders: usize,
    pub html_rejections: usize,
    /// Percent of html renders the geometry allowlist (or the prose guard) rejected.
    pub html_rejection_rate: String,
    /// One line per rejection, naming the fixture, the path, and the value that caused it.
    pub reasons: Vec<String>,
}

/// The scored report `--out` receives.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct EvalReport {
    pub generated: DateTime<Utc>,
    pub judge_model: String,
    pub pricing_basis: String,
    pub fixtures: Vec<FixtureOutcome>,
    pub guards: Guards,
    pub passed: bool,
}

/// Run the eval over every configured fixture.
///
/// Returns `Err` when any fixture fails, so the process exits non-zero (design Phase 13: "non-zero
/// exit when any fixture regresses below its floor"). The scored report is written FIRST, so the
/// evidence for the failure is on disk before the failure is raised.
pub(crate) fn run(cfg: &EvalConfig, pricing: &Pricing) -> Result<RunResult> {
    log::info!(
        "eval::run: fixtures={} judge={} out={} llm={:?}",
        cfg.fixtures.len(),
        cfg.judge_model,
        cfg.out.display(),
        cfg.llm
    );
    if cfg.fixtures.is_empty() {
        bail!("no fixtures to evaluate; pass --fixture <dir> or run from the clyde workspace root");
    }

    let mut outcomes = Vec::with_capacity(cfg.fixtures.len());
    let mut guards = Guards::default();
    let mut sessions_total = 0usize;
    for dir in &cfg.fixtures {
        let outcome = evaluate(dir, cfg, pricing, &mut guards)?;
        sessions_total += outcome.sessions;
        outcomes.push(outcome);
    }
    guards.markdown_rejection_rate = rate(guards.markdown_rejections, guards.markdown_renders);
    guards.html_rejection_rate = rate(guards.html_rejections, guards.html_renders);

    let passed = outcomes.iter().all(|o| o.passed);
    let report = EvalReport {
        generated: Utc::now(),
        judge_model: cfg.judge_model.clone(),
        pricing_basis: format!("embedded pricing feed, data-version {}", feed_version(pricing)),
        fixtures: outcomes,
        guards,
        passed,
    };
    write_report(&report, &cfg.out)?;
    print_summary(&report);

    if !report.passed {
        let failed: Vec<&str> = report
            .fixtures
            .iter()
            .filter(|f| !f.passed)
            .map(|f| f.name.as_str())
            .collect();
        bail!(
            "render eval FAILED for {}; the scored report is at {}",
            failed.join(", "),
            cfg.out.display()
        );
    }
    Ok(RunResult {
        // The count of sessions the eval actually rendered, across every fixture. The caller's
        // status line reads "wrote N sessions to <out>"; N is what was graded.
        sessions_emitted: sessions_total,
        output: OutputDest::File(cfg.out.clone()),
    })
}

/// Evaluate one fixture: fresh markdown render -> mechanical -> fresh html render -> mechanical ->
/// judge -> floors.
fn evaluate(dir: &Path, cfg: &EvalConfig, pricing: &Pricing, guards: &mut Guards) -> Result<FixtureOutcome> {
    let fixture = Fixture::load(dir)?;
    let report = render::load_report(&fixture.report, "fixture report")?;
    let context = build_context(&fixture, &report, pricing)?;
    let ground = Ground::from_context_json(&context.json)?;
    debug!(
        "eval::evaluate: fixture={} sessions={} context_bytes={}",
        fixture.name,
        report.totals.sessions,
        context.json.len()
    );

    eprintln!("eval: {} -- rendering markdown", fixture.name);
    guards.markdown_renders += 1;
    let markdown = render::markdown_from_context(
        &context,
        &render::resolve_prompt(None, Path::new("."))?,
        render::Pins {
            llm: cfg.llm,
            format: Format::Markdown,
            model: &cfg.markdown_model,
            ceiling: cfg.markdown_max_output_tokens,
        },
    );
    let (markdown_ok, markdown_error, markdown_findings) = match &markdown {
        Ok(prose) => {
            let findings = mechanical::check(Kind::Markdown, prose, &context, &ground, &fixture.spec);
            if cfg.write_goldens && findings.is_empty() {
                write_golden(&fixture.golden_path(false), prose)?;
            }
            (true, None, findings)
        }
        Err(e) => {
            let reason = format!("{e}");
            log::warn!(
                "eval::evaluate: fixture={} markdown render rejected: {reason}",
                fixture.name
            );
            guards.markdown_rejections += 1;
            guards.reasons.push(format!("{} (markdown): {reason}", fixture.name));
            (false, Some(reason), Vec::new())
        }
    };

    eprintln!("eval: {} -- rendering html", fixture.name);
    guards.html_renders += 1;
    let html = render::html_from_context(
        &context,
        &render::resolve_html_prompt(None, Path::new("."))?,
        render::Pins {
            llm: cfg.llm,
            format: Format::Html,
            model: &cfg.html_model,
            ceiling: cfg.html_max_output_tokens,
        },
    );
    let (html_ok, html_error, html_findings) = match &html {
        Ok(doc) => {
            let findings = mechanical::check(Kind::Html, doc, &context, &ground, &fixture.spec);
            if cfg.write_goldens && findings.is_empty() {
                write_golden(&fixture.golden_path(true), doc)?;
            }
            (true, None, findings)
        }
        Err(e) => {
            let reason = format!("{e}");
            log::warn!(
                "eval::evaluate: fixture={} html render rejected: {reason}",
                fixture.name
            );
            guards.html_rejections += 1;
            guards.reasons.push(format!("{} (html): {reason}", fixture.name));
            (false, Some(reason), Vec::new())
        }
    };

    // No artifact, nothing to grade. Judging is skipped rather than faked, and the fixture fails on
    // the rejection itself.
    let verdict = match &markdown {
        Ok(prose) => {
            eprintln!("eval: {} -- judging", fixture.name);
            Some(judge_artifact(cfg, &context, prose)?)
        }
        Err(_) => {
            eprintln!("eval: {} -- markdown render rejected, skipping the judge", fixture.name);
            None
        }
    };
    let regressions: Vec<Regression> = verdict
        .as_ref()
        .map(|v| {
            v.regressions(&fixture.spec)
                .into_iter()
                .map(|(dimension, score, floor)| Regression {
                    dimension: dimension.as_str().to_string(),
                    score,
                    floor,
                    reason: v.get(dimension).reason.clone(),
                })
                .collect()
        })
        .unwrap_or_default();

    // The HTML render's own REJECTION is deliberately not a fixture failure, while a markdown
    // rejection is. The asymmetry is the point: the markdown artifact is the eval's subject (it is
    // what the judge scores and what the goldens are), so losing it means nothing was measured;
    // the HTML render exists to exercise the geometry allowlist, whose stochastic pass rate is the
    // pending decision this eval was asked to SIZE. Gating on it would make `otto eval` flake for
    // exactly the reason it is measuring. Its mechanical findings, when it did render, are a
    // failure like any other.
    let passed = markdown_ok && markdown_findings.is_empty() && html_findings.is_empty() && regressions.is_empty();
    Ok(FixtureOutcome {
        name: fixture.name,
        summary: fixture.spec.summary,
        sessions: report.totals.sessions,
        spend: crate::fmt::format_usd(report.totals.spend_usd),
        markdown_ok,
        markdown_error,
        markdown_findings,
        html_ok,
        html_error,
        html_findings,
        verdict,
        regressions,
        passed,
    })
}

/// Score one artifact over the transport `--llm` selected. Split out so `evaluate` reads as the
/// sequence it is; monomorphized per transport, no `Box<dyn Transport>`.
fn judge_artifact(cfg: &EvalConfig, context: &RenderContext, artifact: &str) -> Result<Verdict> {
    let brief = judge::brief(&context.json)?;
    let model = &cfg.judge_model;
    let ceiling = cfg.markdown_max_output_tokens;
    match render::resolve_selected_transport(cfg.llm, Format::Markdown)? {
        TransportKind::Api => judge::score(&ApiTransport::from_env()?, model, ceiling, artifact, &brief),
        TransportKind::Cli => judge::score(&CliTransport::resolve()?, model, ceiling, artifact, &brief),
    }
}

/// Build the render context for a fixture: the fixture's OWN invented persona, its optional
/// `--prior` and `--reconcile` inputs, and the default outlier count.
pub(crate) fn build_context(fixture: &Fixture, report: &Report, pricing: &Pricing) -> Result<RenderContext> {
    render::build_context_block(
        report,
        false,
        fixture.spec.persona.as_ref(),
        pricing,
        DEFAULT_OUTLIERS,
        fixture.prior.as_deref(),
        fixture.analytics.as_deref(),
    )
}

/// The resolved feed's `data-version`, for the scored report's provenance line.
fn feed_version(pricing: &Pricing) -> &str {
    pricing.data_version().unwrap_or("unknown")
}

fn rate(part: usize, whole: usize) -> String {
    if whole == 0 {
        return "n/a".to_string();
    }
    format!("{:.1}%", 100.0 * part as f64 / whole as f64)
}

/// Overwrite one golden with a fresh, mechanically-clean render.
fn write_golden(path: &Path, artifact: &str) -> Result<()> {
    debug!("eval::write_golden: path={} bytes={}", path.display(), artifact.len());
    std::fs::write(path, artifact).with_context(|| format!("failed to write the golden at {}", path.display()))?;
    eprintln!("eval: wrote {} ({} bytes)", path.display(), artifact.len());
    Ok(())
}

fn write_report(report: &EvalReport, out: &Path) -> Result<()> {
    let json = serde_json::to_string_pretty(report).context("failed to serialize the eval report")?;
    let dir = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    std::fs::write(out, json + "\n").with_context(|| format!("failed to write the eval report to {}", out.display()))
}

/// The operator-facing summary, on stderr so a piped `--out` path stays the only thing on stdout.
fn print_summary(report: &EvalReport) {
    eprintln!();
    eprintln!("render eval -- judge {}", report.judge_model);
    eprintln!("{}", report.pricing_basis);
    for f in &report.fixtures {
        let verdict = f
            .verdict
            .as_ref()
            .map(|v| {
                Dimension::ALL
                    .iter()
                    .map(|d| format!("{}={}", d.as_str(), v.get(*d).score))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_else(|| "not judged".to_string());
        eprintln!(
            "  [{}] {:<14} {} sessions, {} -- {}",
            if f.passed { "PASS" } else { "FAIL" },
            f.name,
            f.sessions,
            f.spend,
            verdict
        );
        for finding in f.markdown_findings.iter().chain(&f.html_findings) {
            eprintln!("        {} -- {}", finding.check, finding.detail);
        }
        for r in &f.regressions {
            eprintln!(
                "        {} scored {} against a floor of {} -- {}",
                r.dimension, r.score, r.floor, r.reason
            );
        }
        for (path, e) in [("markdown", &f.markdown_error), ("html", &f.html_error)] {
            if let Some(e) = e {
                eprintln!("        {path} render rejected -- {e}");
            }
        }
    }
    let g = &report.guards;
    eprintln!(
        "  guard rejections: markdown {} of {} ({}), html {} of {} ({})",
        g.markdown_rejections,
        g.markdown_renders,
        g.markdown_rejection_rate,
        g.html_rejections,
        g.html_renders,
        g.html_rejection_rate
    );
    eprintln!();
}

#[cfg(test)]
mod tests;
