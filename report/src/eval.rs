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
// crate-internal, so their types can stay `pub(crate)` alongside `render::facts::RenderContext`.
pub(crate) mod fixture;
pub(crate) mod judge;
pub(crate) mod mechanical;
pub mod synth;

use crate::aggregate::DEFAULT_OUTLIERS;
use crate::cli::{Format, Llm};
use crate::config::TransportKind;
use crate::render;
use crate::render::facts::RenderContext;
use crate::report::Report;
use crate::summarize::{ApiTransport, CliTransport};
use crate::{OutputDest, RunResult};
use chrono::{DateTime, Utc};
use claude_pricing::Pricing;
use eyre::{Context, Result, bail};
use fixture::{Dimension, Fixture};
use judge::Verdict;
use log::debug;
use mechanical::{Finding, Ground};
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
    pub model: String,
    pub judge_max_output_tokens: u32,
    /// Output ceiling for one prose slot, from `render.slot-max-output-tokens`.
    pub slot_max_output_tokens: u32,
}

/// One fixture's result.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct FixtureOutcome {
    pub name: String,
    pub summary: String,
    pub sessions: usize,
    pub spend: String,
    /// Whether the fresh render produced an artifact at all, and why not when it did not.
    ///
    /// Post-inversion this can only be an INFRASTRUCTURE failure (an unreadable report, a transport
    /// that could not be resolved), never a rejected artifact: Rust authors the document, so there is
    /// nothing left that can refuse to publish one. Recorded rather than propagated, so one fixture's
    /// failure does not discard the others' paid calls.
    pub markdown_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown_error: Option<String>,
    /// Mechanical findings against the fresh render. Empty is a pass.
    pub markdown_findings: Vec<Finding>,
    /// How many slots were attempted, and how many shipped empty after their retry. A degraded slot
    /// is not a failure -- it is the designed worst case -- but it IS the number to watch.
    pub slots_attempted: usize,
    pub slots_degraded: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    /// Why the judge produced no verdict for a markdown render that DID survive its guards: a
    /// transport failure, an unparseable score block, a rate limit. Recorded like a guard rejection
    /// rather than propagated, for the same reason -- a transient failure on fixture 3 must not
    /// discard fixtures 1 and 2's paid renders (see [`Guards`]). Fails this fixture, never the run's
    /// ability to write its report.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge_error: Option<String>,
    /// Why the fixture could not be evaluated AT ALL: an unreadable `eval.yml` or `report.json`, a
    /// context that would not build, a golden that would not write. Same contract as
    /// [`Self::judge_error`] -- recorded, not propagated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_error: Option<String>,
    /// `(dimension, score, floor)` for every dimension below its floor.
    pub regressions: Vec<Regression>,
    pub passed: bool,
}

impl FixtureOutcome {
    /// The outcome for a fixture that could not be loaded or evaluated. Named from the directory,
    /// because the `eval.yml` that carries the real name is exactly what could not be read.
    fn unloadable(dir: &Path, error: &eyre::Report) -> Self {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| dir.display().to_string());
        Self {
            name,
            summary: format!("fixture at {} could not be evaluated", dir.display()),
            sessions: 0,
            spend: crate::fmt::format_usd(0.0),
            markdown_ok: false,
            markdown_error: None,
            markdown_findings: Vec::new(),
            slots_attempted: 0,
            slots_degraded: 0,
            verdict: None,
            judge_error: None,
            load_error: Some(format!("{error:#}")),
            regressions: Vec::new(),
            passed: false,
        }
    }
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

/// What DEGRADED across this run.
///
/// This is a MEASUREMENT, not a gate. It used to report how often a fresh render was REJECTED by its
/// own guards, per path -- the metric the render inversion was built to drive to zero. It reached
/// zero structurally: Rust authors the artifact, so no guard can refuse to publish one, and a rate
/// that can only ever read `0.0%` is a lying field.
///
/// What remains worth watching is slot degradation: a slot that violated its contract twice ships
/// empty, which costs a paragraph rather than an artifact. That is the designed worst case, so it is
/// counted and reported rather than failed on. `markdown_failures` covers the only way a render can
/// now produce nothing at all -- an unreadable fixture, an unresolvable transport -- which is
/// infrastructure, not a verdict on the artifact.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Guards {
    pub markdown_renders: usize,
    /// Renders that were ATTEMPTED and produced no artifact. Structurally this can only be
    /// infrastructure (an unresolvable transport, a view-assembly failure): the artifact is
    /// Rust-authored, so no guard can discard one. The old whole-artifact rejection rate has no
    /// referent and is gone.
    ///
    /// A fixture that never got as far as a render attempt is NOT counted here. `Fixture::load`,
    /// `load_report`, and `build_context` all fail before `markdown_renders` is incremented, so an
    /// unreadable fixture propagates out of `evaluate` and is recorded by `run` as
    /// [`FixtureOutcome::unloadable`] instead. Both counters are therefore honest about their own
    /// scope, and "N attempted, 0 failed" cannot hide a fixture that produced nothing -- that
    /// fixture is in the unloadable list.
    pub markdown_failures: usize,
    /// Slots attempted across the run, and how many shipped empty after their retry. This replaces
    /// the rejection rate as the number an operator watches: it is the only degradation left, and
    /// it costs a paragraph rather than an artifact.
    pub slots_attempted: usize,
    pub slots_degraded: usize,
    /// Percent of attempted slots that shipped empty, as a display string.
    pub slot_degradation_rate: String,
    /// One line per failure or degradation, naming the fixture and the reason.
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
        // A fixture that will not load is RECORDED as a failed fixture, never propagated. Every
        // render in this loop is a paid model call, so letting fixture 3's unreadable `eval.yml`
        // abort the run would throw away fixtures 1 and 2's calls AND skip `write_report`, leaving
        // no evidence on disk -- the outcome the `Guards` contract above exists to prevent.
        let outcome = match evaluate(dir, cfg, pricing, &mut guards) {
            Ok(outcome) => outcome,
            Err(e) => {
                log::error!("eval::run: fixture at {} could not be evaluated: {e:#}", dir.display());
                eprintln!("eval: {} -- FAILED to evaluate: {e:#}", dir.display());
                FixtureOutcome::unloadable(dir, &e)
            }
        };
        sessions_total += outcome.sessions;
        outcomes.push(outcome);
    }
    guards.slot_degradation_rate = rate(guards.slots_degraded, guards.slots_attempted);

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

    eprintln!("eval: {} -- rendering", fixture.name);
    guards.markdown_renders += 1;
    let rendered = render::for_eval(
        &report,
        pricing,
        fixture.spec.persona.as_ref(),
        eval_opts(&fixture),
        DEFAULT_OUTLIERS,
        // `--write-goldens` renders STUBBED, and that makes regenerating a golden a free, offline
        // operation: a golden IS the deterministic half of the document, so paying for slot prose
        // that is then thrown away would be waste. A grading run renders live.
        if cfg.write_goldens {
            render::SlotSource::Stubbed
        } else {
            render::SlotSource::Live {
                llm: cfg.llm,
                model: &cfg.model,
                ceiling: cfg.slot_max_output_tokens,
            }
        },
    );
    let (markdown_ok, markdown_error, markdown_findings, slots_attempted, slots_degraded, artifact, prose) =
        match rendered {
            Ok(r) => {
                let degraded = r.attempted.saturating_sub(r.prose.len());
                guards.slots_attempted += r.attempted;
                guards.slots_degraded += degraded;
                if degraded > 0 {
                    guards
                        .reasons
                        .push(format!("{}: {degraded} slot(s) shipped empty", fixture.name));
                }
                let mut findings = mechanical::check(&r.markdown, &context, &ground, &fixture.spec);
                findings.extend(mechanical::slot_prose(&r.prose));
                // Only a CLEAN render becomes a golden: a golden is a known-good artifact by
                // definition, and committing a failing one would make `otto ci` green against a
                // broken renderer.
                if cfg.write_goldens {
                    if findings.is_empty() {
                        write_golden(&fixture, &r.markdown)?;
                    } else {
                        eprintln!("eval: {} FAILED its checks; its golden is left untouched", fixture.name);
                    }
                }
                (
                    true,
                    None,
                    findings,
                    r.attempted,
                    degraded,
                    Some(r.markdown),
                    Some(r.prose),
                )
            }
            Err(e) => {
                let reason = format!("{e:#}");
                log::warn!("eval::evaluate: fixture={} render failed: {reason}", fixture.name);
                guards.markdown_failures += 1;
                guards.reasons.push(format!("{}: {reason}", fixture.name));
                (false, Some(reason), Vec::new(), 0, 0, None, None)
            }
        };

    // No artifact, nothing to grade. A judge FAILURE is recorded rather than propagated: the judge
    // is a live model call, so a transport blip on this fixture would otherwise discard every
    // earlier fixture's paid render and skip `write_report` entirely.
    //
    // The judge now scores the SLOT PROSE, not the whole artifact. That follows the inversion: every
    // figure in the document is Rust's and needs no scoring, so what is left to judge is the only
    // thing a model wrote.
    let mut judge_error = None;
    let verdict = match (&artifact, &prose) {
        (Some(_), Some(prose)) if !prose.is_empty() => {
            eprintln!("eval: {} -- judging slot prose", fixture.name);
            match judge_artifact(cfg, &context, &joined(prose)) {
                Ok(verdict) => Some(verdict),
                Err(e) => {
                    let reason = format!("{e:#}");
                    log::warn!("eval::evaluate: fixture={} judge failed: {reason}", fixture.name);
                    eprintln!("eval: {} -- judge FAILED: {reason}", fixture.name);
                    judge_error = Some(reason);
                    None
                }
            }
        }
        // No prose to judge, and the two reasons for that are NOT the same event. A
        // `--write-goldens` run renders `SlotSource::Stubbed`, so empty prose is the design rather
        // than a failure; reporting "every slot degraded" there names a degradation that never
        // happened and trains the operator to ignore the line that matters.
        (Some(_), _) if slots_attempted == 0 => {
            eprintln!(
                "eval: {} -- no slots attempted (stubbed render), nothing to judge",
                fixture.name
            );
            None
        }
        (Some(_), _) => {
            eprintln!(
                "eval: {} -- every slot degraded ({slots_attempted} attempted), nothing to judge",
                fixture.name
            );
            None
        }
        _ => {
            eprintln!("eval: {} -- render failed, skipping the judge", fixture.name);
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

    // A judge failure fails the fixture: an unscored artifact is an unmeasured one. A DEGRADED slot
    // does not, because degradation is the designed worst case rather than a defect -- it is counted
    // in `slots_degraded` and reported, not failed on.
    let passed = markdown_ok && judge_error.is_none() && markdown_findings.is_empty() && regressions.is_empty();
    Ok(FixtureOutcome {
        name: fixture.name,
        summary: fixture.spec.summary,
        sessions: report.totals.sessions,
        spend: crate::fmt::format_usd(report.totals.spend_usd),
        markdown_ok,
        markdown_error,
        markdown_findings,
        slots_attempted,
        slots_degraded,
        verdict,
        judge_error,
        load_error: None,
        regressions,
        passed,
    })
}

/// Score one artifact over the transport `--llm` selected. Split out so `evaluate` reads as the
/// sequence it is; monomorphized per transport, no `Box<dyn Transport>`.
fn judge_artifact(cfg: &EvalConfig, context: &RenderContext, artifact: &str) -> Result<Verdict> {
    let brief = judge::brief(&context.json)?;
    let model = &cfg.judge_model;
    let ceiling = cfg.judge_max_output_tokens;
    match render::resolve_selected_transport(cfg.llm, Format::Markdown)? {
        TransportKind::Api => judge::score(&ApiTransport::from_env()?, model, ceiling, artifact, &brief),
        TransportKind::Cli => judge::score(&CliTransport::resolve()?, model, ceiling, artifact, &brief),
    }
}

/// The view options a fixture renders under: its optional `--prior` and `--reconcile` inputs.
///
/// No `--reconcile-user` override: the fixture's own invented persona carries the email its
/// synthesized export is scoped to, so the eval exercises the SAME operator-resolution path a real
/// render takes (persona -> reconcile) rather than a bypass of it. Tradeoffs stay off, so the graded
/// artifact is the shape a default render produces.
fn eval_opts(fixture: &Fixture) -> crate::render::ViewOpts<'_> {
    crate::render::ViewOpts {
        include_tradeoffs: false,
        prior: fixture.prior.as_deref(),
        reconcile: fixture.analytics.as_deref(),
        reconcile_user: None,
    }
}

/// Overwrite a fixture's `golden.md`.
///
/// The artifact handed in is always a STUBBED render (see the `SlotSource` choice in `evaluate`),
/// because a golden whose bytes move between runs is not a regression net. Live slot prose is not
/// reproducible; the deterministic document is, and that is exactly what `otto ci` compares offline
/// and for free.
fn write_golden(fixture: &Fixture, artifact: &str) -> Result<()> {
    let path = fixture.golden_path();
    std::fs::write(&path, artifact).with_context(|| format!("failed to write the golden at {}", path.display()))?;
    eprintln!("eval: wrote {} ({} bytes)", path.display(), artifact.len());
    Ok(())
}

/// Every slot's prose, joined for the judge. One string, because the judge scores prose quality
/// rather than per-slot structure, and its rubric is about the writing.
fn joined(prose: &render::SlotProse) -> String {
    prose.values().map(String::as_str).collect::<Vec<_>>().join("\n\n")
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
        // No `--reconcile-user` override: the fixture's own invented persona carries the email its
        // synthesized export is scoped to, so the eval exercises the SAME operator-resolution path
        // a real render takes (persona -> reconcile), not a bypass of it.
        None,
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
        for finding in &f.markdown_findings {
            eprintln!("        {} -- {}", finding.check, finding.detail);
        }
        for r in &f.regressions {
            eprintln!(
                "        {} scored {} against a floor of {} -- {}",
                r.dimension, r.score, r.floor, r.reason
            );
        }
        for (path, e) in [("render", &f.markdown_error)] {
            if let Some(e) = e {
                eprintln!("        {path} render rejected -- {e}");
            }
        }
        if let Some(e) = &f.judge_error {
            eprintln!("        judge failed -- {e}");
        }
        if let Some(e) = &f.load_error {
            eprintln!("        fixture could not be evaluated -- {e}");
        }
    }
    let g = &report.guards;
    eprintln!(
        "  renders: {} attempted, {} failed to produce an artifact",
        g.markdown_renders, g.markdown_failures
    );
    eprintln!(
        "  slot degradation: {} of {} shipped empty ({})",
        g.slots_degraded, g.slots_attempted, g.slot_degradation_rate
    );
    eprintln!();
}

#[cfg(test)]
mod tests;
