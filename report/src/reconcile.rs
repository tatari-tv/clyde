//! Fold an Anthropic Enterprise Analytics cost export into a [`Reconciliation`] block
//! (`clyde report render --reconcile <analytics.json>`, design Phase 12): billed spend from the
//! authoritative export against clyde's own modeled total. This closes finding 6 of the design
//! ("The dollar figure is a modeled list-price equivalent and nothing says so") -- an authoritative
//! source is reachable, so a report that models a number and never cites the real one is leaving
//! the honest answer on the table (Alternative 5, rejected).
//!
//! The export is produced OUTSIDE clyde, by the `anthropic-usage-report` skill's
//! `pull-usage-report.py --report cost` (`~/.claude/skills/anthropic-usage-report/SKILL.md`). clyde
//! never holds the Analytics key and never calls the API itself; it only reads a file the user
//! already produced (design Non-Goals, "Putting an Analytics API key in clyde").

use crate::fmt::{format_optional_usd, format_usd, format_usd_signed};
use crate::report::Report;
use chrono::{DateTime, Utc};
use eyre::{Context, Result, bail};
use log::debug;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// The fixed `source` label (design Data Model, `Reconciliation.source`).
const SOURCE: &str = "anthropic enterprise analytics";

/// Bucket key for an export row with no `model` field (e.g. a future export grouped by a dimension
/// other than `model`), matching the house parenthesized-bucket precedent
/// (`report::MAIN_SESSION_BUCKET`, `aggregate::UNATTRIBUTED_ORG`).
const UNGROUPED_MODEL: &str = "(ungrouped)";

/// The interpretation guard, carried verbatim into both templates next to the figure (design "The
/// delta is not an error term, and the report must say so"). An Analytics export covers everything
/// the account billed; `clyde report` covers Claude Code sessions in one catalog. `billed >=
/// modeled` is therefore the EXPECTED relationship, and a positive `unseen-account-spend` means
/// "usage clyde does not see", never "clyde miscounted".
pub const SCOPE_NOTE: &str = "An Analytics export covers everything the account billed: claude.ai \
     web, other clients, and other hosts. clyde report covers only the Claude Code sessions in one \
     catalog. Billed spend meeting or exceeding modeled spend is the expected relationship here; a \
     positive unseen-account-spend figure means usage clyde does not see, never that clyde \
     miscounted.";

/// One row of the export's normalized JSON: `pull-usage-report.py --report cost`'s flat output,
/// after the SKILL's OWN `results[]`-per-bucket expansion (see the skill's `references/api-notes.md`,
/// "Response shape: bucketed vs. flat") -- a per-model, per-day cost bucket. Only the fields this
/// module reads are named; every other column the export carries (`product`, `inference_geo`,
/// `speed`, `requests`, ...) is silently ignored rather than rejected, because this crate does not
/// own that script's schema and a future column it adds must not break this parse.
#[derive(Debug, Deserialize)]
struct CostRecord {
    model: Option<String>,
    /// Decimal-string dollars, e.g. `"3640.325000"`. This is the export's ACTUAL billed figure
    /// (`amount`), after any negotiated-discount pricing the org has -- not `list_amount`, the
    /// export's separate undiscounted list-rate figure. "Billed" per this design's own wording
    /// means the real account-level bill, so `amount` is what [`fold`] reports as `billed`.
    amount: String,
    starting_at: DateTime<Utc>,
    ending_at: DateTime<Utc>,
}

/// Folded Analytics cost export vs. clyde's own modeled total (design Data Model,
/// "Reconciliation"). Present in the render context only when `--reconcile` was supplied and its
/// window matched this report's; see `render::build_reconciliation_view` for the absent case,
/// which is never silent even though this struct itself is.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Reconciliation {
    pub source: String,
    pub window: String,
    pub billed: String,
    pub modeled: String,
    /// Reader-facing key is `unseen-account-spend`, not "delta" (design Phase 12): this is a
    /// part-to-whole difference, not a variance, and "delta" invites reading it as clyde's error.
    /// The Rust field name states what it computes; only the serialized key is renamed.
    #[serde(rename = "unseen-account-spend")]
    pub delta: String,
    pub by_model: Vec<ReconRow>,
    pub scope_note: String,
}

/// One per-model row of the reconciliation (design Data Model: "model, billed, modeled, delta").
/// The same reader-facing rename applies here as on the top-level figure, for the same reason.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ReconRow {
    pub model: String,
    pub billed: String,
    pub modeled: String,
    #[serde(rename = "unseen-account-spend")]
    pub delta: String,
}

/// Round a dollar figure to cents, normalizing negative zero to `+0.0` (same convention as
/// `report::round_cents` / `merge::round_cents` -- every dollar choke point in this crate
/// re-normalizes independently rather than sharing a public helper across modules).
fn round_cents(x: f64) -> f64 {
    let cents = (x * 100.0).round() / 100.0;
    if cents == 0.0 { 0.0 } else { cents }
}

/// A by-model row's modeled figure, distinguishing three states a bare `Option<f64>` cannot: the
/// model is entirely absent from `totals.models` (clyde's catalog never used it this window, a
/// real priced zero), present but unpriced (Phase 6's untracked gate), or present and priced.
/// Collapsing the first two to the same `None` would render a genuinely zero-usage model as
/// `"(untracked)"`, which overstates how much of its billed spend is "clyde saw it but couldn't
/// price it" versus "clyde never saw it at all".
enum ModeledFigure {
    Zero,
    Untracked,
    Priced(f64),
}

impl ModeledFigure {
    fn display(&self) -> String {
        match self {
            ModeledFigure::Zero => format_usd(0.0),
            ModeledFigure::Untracked => format_optional_usd(None),
            ModeledFigure::Priced(v) => format_usd(*v),
        }
    }

    /// The value used for the per-row `unseen-account-spend` math. `Untracked` contributes `0.0`
    /// here (not a fabricated number) -- the row's `modeled` display string, `"(untracked)"`,
    /// already carries the caveat that this delta is not a clean like-for-like comparison.
    fn value(&self) -> f64 {
        match self {
            ModeledFigure::Zero | ModeledFigure::Untracked => 0.0,
            ModeledFigure::Priced(v) => *v,
        }
    }
}

/// Parse `export_path` as an `anthropic-usage-report --report cost` export, verify its window
/// matches `report`'s exactly, and fold it into a [`Reconciliation`]. Window mismatch is a LOUD
/// error naming BOTH windows (design Phase 12), never a silent comparison of different periods;
/// an empty or unparseable export is likewise a loud error, never a report emitted with a bogus
/// zero reconciliation.
pub fn fold(export_path: &Path, report: &Report) -> Result<Reconciliation> {
    debug!(
        "reconcile::fold: export_path={} report_since={} report_until={}",
        export_path.display(),
        report.since,
        report.until
    );
    let body = fs::read_to_string(export_path)
        .with_context(|| format!("failed to read --reconcile export at {}", export_path.display()))?;
    let records: Vec<CostRecord> = serde_json::from_str(&body).with_context(|| {
        format!(
            "failed to parse --reconcile export at {} as the anthropic-usage-report `--report cost` \
             JSON array",
            export_path.display()
        )
    })?;
    if records.is_empty() {
        bail!(
            "--reconcile export at {} contains no cost records; nothing to reconcile",
            export_path.display()
        );
    }

    // `expect` is safe: the empty-check above guarantees at least one record, so `min`/`max` over
    // a non-empty iterator of `DateTime<Utc>` (a `Copy` total order) always returns `Some`.
    let export_start = records
        .iter()
        .map(|r| r.starting_at)
        .min()
        .expect("records is non-empty (checked above)");
    let export_end = records
        .iter()
        .map(|r| r.ending_at)
        .max()
        .expect("records is non-empty (checked above)");
    if export_start != report.since || export_end != report.until {
        bail!(
            "--reconcile export window [{} .. {}] does not match this report's window [{} .. {}]; \
             pull the Analytics cost export for the exact --since/--until used by `report collect` \
             and re-run render",
            export_start.to_rfc3339(),
            export_end.to_rfc3339(),
            report.since.to_rfc3339(),
            report.until.to_rfc3339()
        );
    }

    let mut billed_by_model: BTreeMap<String, f64> = BTreeMap::new();
    for record in &records {
        let model = record.model.clone().unwrap_or_else(|| UNGROUPED_MODEL.to_string());
        let dollars: f64 = record.amount.trim().parse().with_context(|| {
            format!(
                "unparseable cost amount {:?} for model {model} in --reconcile export at {}",
                record.amount,
                export_path.display()
            )
        })?;
        *billed_by_model.entry(model).or_insert(0.0) += dollars;
    }

    let billed_total = round_cents(billed_by_model.values().sum());
    let modeled_total = round_cents(report.totals.spend_usd);
    let delta_total = round_cents(billed_total - modeled_total);

    let mut models: BTreeSet<String> = billed_by_model.keys().cloned().collect();
    models.extend(report.totals.models.keys().cloned());
    let mut rows: Vec<(String, f64, ModeledFigure)> = models
        .into_iter()
        .map(|model| {
            let billed = round_cents(*billed_by_model.get(&model).unwrap_or(&0.0));
            let modeled = match report.totals.models.get(&model) {
                // The model key is entirely absent from `totals.models`: clyde's catalog never
                // used it this window at all, which is a real, priced ZERO -- distinct from
                // "used but unpriced" below, and never `format_optional_usd`'s `"(untracked)"`.
                None => ModeledFigure::Zero,
                Some(mt) => match mt.spend_usd {
                    Some(v) => ModeledFigure::Priced(round_cents(v)),
                    // Phase 6's untracked gate: clyde saw the model but has no price for it.
                    None => ModeledFigure::Untracked,
                },
            };
            (model, billed, modeled)
        })
        .collect();
    // Billed-descending, so the by-model table reads the same "biggest first" convention as every
    // other pre-sorted list in the context block (`totals.models`, `aggregates.by-repo`).
    rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let by_model = rows
        .into_iter()
        .map(|(model, billed, modeled)| ReconRow {
            model,
            billed: format_usd(billed),
            modeled: modeled.display(),
            delta: format_usd_signed(round_cents(billed - modeled.value())),
        })
        .collect();

    debug!(
        "reconcile::fold: records={} models-in-export={} billed-total={} modeled-total={} \
         unseen-account-spend={}",
        records.len(),
        billed_by_model.len(),
        billed_total,
        modeled_total,
        delta_total
    );

    Ok(Reconciliation {
        source: SOURCE.to_string(),
        window: format!(
            "{} to {}",
            export_start.format("%Y-%m-%d"),
            export_end.format("%Y-%m-%d")
        ),
        billed: format_usd(billed_total),
        modeled: format_usd(modeled_total),
        delta: format_usd_signed(delta_total),
        by_model,
        scope_note: SCOPE_NOTE.to_string(),
    })
}

#[cfg(test)]
mod tests;
