//! Fold an Anthropic Enterprise Analytics PER-USER cost export into a [`Reconciliation`] block
//! (`clyde report render --reconcile <analytics.json>`, design Phase 12): the operator's billed
//! spend from the authoritative export against clyde's own modeled total. This closes finding 6 of
//! the design ("The dollar figure is a modeled list-price equivalent and nothing says so") -- an
//! authoritative source is reachable, so a report that models a number and never cites the real one
//! is leaving the honest answer on the table (Alternative 5, rejected).
//!
//! **The export must be PER-USER (`--report user-cost`), and the comparison is scoped to the
//! operator this report belongs to.** `clyde report` reads one user's session logs on one machine,
//! so the only billed figure it can honestly set beside its own total is that same user's bill.
//! Measured on a real 30-day window: the org-wide `--report cost` export bills every seat in the
//! organization, which published an `unseen-account-spend` more than an order of magnitude larger
//! than the operator's entire modeled total -- the rest of the company's Claude usage, presented in
//! a per-user report as spend clyde failed to account for. Scoped to the operator, the same window
//! reconciles to partial coverage with a remainder
//! (claude.ai web, Cowork, other clients and hosts) the scope note can actually explain. An
//! org-wide export is therefore REJECTED here, by name, rather than folded.
//!
//! The export is produced OUTSIDE clyde, by the `anthropic-usage-report` skill's
//! `pull-usage-report.py --report user-cost` (`~/.claude/skills/anthropic-usage-report/SKILL.md`).
//! clyde never holds the Analytics key and never calls the API itself; it only reads a file the user
//! already produced (design Non-Goals, "Putting an Analytics API key in clyde").

use crate::fmt::{format_optional_usd, format_usd, format_usd_signed};
use crate::report::Report;
use chrono::{DateTime, NaiveDate, Utc};
use eyre::{Context, Result, bail};
use log::{debug, trace};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

/// The fixed `source` label (design Data Model, `Reconciliation.source`).
const SOURCE: &str = "anthropic enterprise analytics";

/// Bucket key for an export row with no `model` field (e.g. a future export grouped by a dimension
/// other than `model`), matching the house parenthesized-bucket precedent
/// (`report::MAIN_SESSION_BUCKET`, `aggregate::UNATTRIBUTED_ORG`).
const UNGROUPED_MODEL: &str = "(ungrouped)";

/// The command that produces an export this module accepts, quoted in every rejection so a wrong
/// file names its own remedy.
const PULL_COMMAND: &str = "python3 ~/.claude/skills/anthropic-usage-report/pull-usage-report.py --report user-cost \
     --start <since> --end <until>";

/// The interpretation guard, carried verbatim into both templates next to the figure (design "The
/// delta is not an error term, and the report must say so"), rewritten for the operator scope this
/// module now enforces. The gap it describes is the OPERATOR'S OWN usage that clyde's catalog
/// cannot see -- claude.ai web, Cowork, other clients, other hosts -- never other people's usage,
/// which is no longer in the comparison at all. `billed >= modeled` is the EXPECTED relationship,
/// and a positive `unseen-account-spend` means "usage clyde does not see", never "clyde
/// miscounted".
pub fn scope_note(operator: &str) -> String {
    format!(
        "This billed figure is the Claude Enterprise Analytics cost report for {operator} alone, \
         covering everything that account was billed across every Claude product: claude.ai web, \
         Cowork, other clients, and other hosts. clyde report covers only the Claude Code sessions \
         in this catalog on this machine. Billed spend meeting or exceeding modeled spend is the \
         expected relationship here; a positive unseen-account-spend figure is the same person's \
         usage that clyde cannot see, never that clyde miscounted."
    )
}

/// One row of the export's normalized JSON: `pull-usage-report.py --report user-cost`'s flat
/// output, one row per org member per group (model, and any other `--group-by` dimension). Only the
/// fields this module reads are named; every other column the export carries (`product`,
/// `cost_type`, `token_type`, `context_window`, `list_amount`, `requests`, ...) is silently ignored
/// rather than rejected, because this crate does not own that script's schema and a future column
/// it adds must not break this parse.
#[derive(Debug, Deserialize)]
struct CostRecord {
    model: Option<String>,
    /// Decimal-string **CENTS**, e.g. `"41280.000000"` is `$412.80`. The Analytics cost endpoints
    /// report minor units and `pull-usage-report.py` writes them through as-is, so this MUST be
    /// divided by [`CENTS_PER_DOLLAR`] before it is treated as money (see the skill's SKILL.md,
    /// "Amount fields on cost endpoints are decimal-string cents"). Reading it as dollars overstates
    /// the authoritative billed figure by 100x.
    ///
    /// This is the export's ACTUAL billed figure (`amount`), after any negotiated-discount pricing
    /// the org has -- not `list_amount`, the export's separate undiscounted list-rate figure.
    /// "Billed" per this design's own wording means the real account-level bill, so `amount` is what
    /// [`fold`] reports as `billed`.
    amount: String,
    /// Present on the ORG-WIDE (`--report cost`) shape, where the script expands each time bucket's
    /// `results[]` and stamps the bucket's window onto every row. NULL on every `user-cost` row: the
    /// per-user endpoints return one row per member for the WHOLE window and leave both timestamps
    /// unset. See [`window`] for what verifies the period in that case.
    #[serde(default)]
    starting_at: Option<DateTime<Utc>>,
    #[serde(default)]
    ending_at: Option<DateTime<Utc>>,
    /// The org member this row belongs to. Present on every `user-cost` row and absent from every
    /// org-wide `cost` row, which is exactly how [`fold`] tells the two exports apart.
    #[serde(default)]
    actor: Option<Actor>,
}

/// The `actor` object on a per-user export row. The row key is `email` (NOT `email_address`);
/// `user_id`, `name`, `type` and `deleted` are carried by the export and ignored here.
#[derive(Debug, Deserialize)]
struct Actor {
    #[serde(default)]
    email: Option<String>,
}

/// Folded Analytics cost export vs. clyde's own modeled total (design Data Model,
/// "Reconciliation"). Present in the render context only when `--reconcile` was supplied and its
/// window matched this report's; see `render::build_reconciliation_view` for the absent case,
/// which is never silent even though this struct itself is.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Reconciliation {
    pub source: String,
    /// The person both figures are scoped to. Serialized so the artifact can state the scope as a
    /// fact rather than leaving the reader to infer it from `scope_note`'s prose.
    pub operator: String,
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

/// The Analytics cost endpoints report money in minor units (cents). Every `amount` read out of an
/// export is divided by this exactly once, in [`fold`], so nothing downstream has to remember.
const CENTS_PER_DOLLAR: f64 = 100.0;

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

/// Parse `export_path` as an `anthropic-usage-report --report user-cost` export, verify its window
/// matches `report`'s, keep only `operator`'s rows, and fold those into a [`Reconciliation`].
///
/// Every unhappy path is a LOUD error, never a quietly wrong figure (design "fail loudly, fail
/// closed"): an org-wide export, a window that does not match, an export whose period cannot be
/// established at all, an unparseable amount, and -- the one this module exists for -- an export
/// with no row for the operator, which must never degrade to `$0.00 billed` or fall back to the
/// org total.
pub fn fold(export_path: &Path, operator: Option<&str>, report: &Report) -> Result<Reconciliation> {
    debug!(
        "reconcile::fold: export_path={} operator={:?} report_since={} report_until={}",
        export_path.display(),
        operator,
        report.since,
        report.until
    );
    let Some(operator) = operator.map(str::trim).filter(|o| !o.is_empty()) else {
        bail!(
            "--reconcile needs the operator this report belongs to, and none is known: `persona \
             whoami` reported no work email for this machine. Pass --reconcile-user <email> naming \
             the person whose sessions this report covers. clyde report is a per-user tool, so an \
             unscoped comparison against the whole organization's bill would be meaningless."
        );
    };
    let body = fs::read_to_string(export_path)
        .with_context(|| format!("failed to read --reconcile export at {}", export_path.display()))?;
    let records: Vec<CostRecord> = serde_json::from_str(&body).with_context(|| {
        format!(
            "failed to parse --reconcile export at {} as the anthropic-usage-report `--report \
             user-cost` JSON array",
            export_path.display()
        )
    })?;
    if records.is_empty() {
        bail!(
            "--reconcile export at {} contains no cost records; nothing to reconcile",
            export_path.display()
        );
    }
    require_per_user_shape(&records, export_path)?;

    let window = window(&records, export_path, report)?;
    let mine = operator_rows(&records, operator, export_path)?;

    let mut billed_by_model: BTreeMap<String, f64> = BTreeMap::new();
    for record in mine {
        let model = record.model.clone().unwrap_or_else(|| UNGROUPED_MODEL.to_string());
        let cents: f64 = record.amount.trim().parse().with_context(|| {
            format!(
                "unparseable cost amount {:?} for model {model} in --reconcile export at {}",
                record.amount,
                export_path.display()
            )
        })?;
        // `f64::from_str` ACCEPTS "NaN", "inf" and "-inf", so a malformed export slips past the
        // parse above and poisons every figure downstream: `billed_total` becomes NaN,
        // `format_usd` renders garbage, and the `partial_cmp` fallback in the row sort silently
        // degrades to `Ordering::Equal`. This module is fail-closed everywhere else; a
        // non-finite amount is a loud refusal, named by model and export path like its siblings.
        if !cents.is_finite() {
            bail!(
                "non-finite cost amount {:?} for model {model} in --reconcile export at {}; \
                 refusing to publish a billed figure derived from it",
                record.amount,
                export_path.display()
            );
        }
        // The export reports minor units; see `CostRecord::amount`. Convert once, here, so every
        // figure downstream of this loop is already dollars.
        let dollars = cents / CENTS_PER_DOLLAR;
        trace!("fold: model={model} cents={cents} dollars={dollars}");
        *billed_by_model.entry(model).or_insert(0.0) += dollars;
    }

    // Round each model FIRST, then total THOSE. The rows below round each model independently, so
    // totalling the raw values instead let the table disagree with its own headline by a cent: a
    // real export carries fractional cents (`amount` is a decimal string with six decimals, e.g.
    // `"41280.000000"`), and several models' fractions accumulate. This is the one block whose job
    // is quoting an authoritative BILLED figure to a finance reader, and a table that does not add
    // up to its own total invites exactly the "clyde miscounted" reading `scope_note` exists to
    // prevent. Rounding in this order makes the two agree by construction rather than by luck.
    //
    // Note this is the OPPOSITE of the report's own `totals.spend-usd`, which prices the union once
    // (ratio-of-sums) because it is deriving a figure. Here the figures are GIVEN by the export and
    // the only question is what the table shows, so the displayed rows are the source of truth for
    // the displayed total.
    for value in billed_by_model.values_mut() {
        *value = round_cents(*value);
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
        "reconcile::fold: operator={operator} operator-rows-models={} billed-total={} \
         modeled-total={} unseen-account-spend={}",
        billed_by_model.len(),
        billed_total,
        modeled_total,
        delta_total
    );

    Ok(Reconciliation {
        source: SOURCE.to_string(),
        operator: operator.to_string(),
        window,
        billed: format_usd(billed_total),
        modeled: format_usd(modeled_total),
        delta: format_usd_signed(delta_total),
        by_model,
        scope_note: scope_note(operator),
    })
}

/// Reject an ORG-WIDE (`--report cost`) export by name. Every `user-cost` row carries an `actor`;
/// no `cost` row does, so the missing field is a reliable, mechanical discriminator -- and shipping
/// the org-wide comparison by accident is the exact failure this function exists to prevent.
fn require_per_user_shape(records: &[CostRecord], export_path: &Path) -> Result<()> {
    let without = records.iter().filter(|r| r.actor.is_none()).count();
    debug!(
        "reconcile::require_per_user_shape: records={} without-actor={}",
        records.len(),
        without
    );
    if without == records.len() {
        bail!(
            "--reconcile export at {} carries no `actor` on any of its {} rows, so it is an \
             ORG-WIDE `--report cost` export covering every user in the organization. `clyde \
             report` reads one user's sessions, so comparing its total against the whole org's \
             bill would publish other people's spend as spend clyde failed to account for. Pull a \
             per-user export instead: {PULL_COMMAND}",
            export_path.display(),
            records.len()
        );
    }
    if without > 0 {
        bail!(
            "--reconcile export at {} carries an `actor` on some rows and not others ({} of {} \
             rows have none), so it is neither a well-formed per-user export nor an org-wide one \
             and cannot be scoped to an operator. Re-pull it: {PULL_COMMAND}",
            export_path.display(),
            without,
            records.len()
        );
    }
    Ok(())
}

/// The operator's own rows, or a loud error. An export with no row for this person is NEVER a
/// silent `$0.00 billed` and never falls back to the org total: it means the export was pulled for
/// a different account, or `--reconcile-user` names someone who has no seat usage this window, and
/// either way the reader must be told rather than shown a zero.
fn operator_rows<'a>(records: &'a [CostRecord], operator: &str, export_path: &Path) -> Result<Vec<&'a CostRecord>> {
    let mine: Vec<&CostRecord> = records
        .iter()
        .filter(|r| {
            r.actor
                .as_ref()
                .and_then(|a| a.email.as_deref())
                .is_some_and(|email| email.trim().eq_ignore_ascii_case(operator))
        })
        .collect();
    let actors = records
        .iter()
        .filter_map(|r| r.actor.as_ref().and_then(|a| a.email.as_deref()))
        .collect::<BTreeSet<&str>>()
        .len();
    debug!(
        "reconcile::operator_rows: operator={operator} matched={} of {} rows across {actors} actors",
        mine.len(),
        records.len()
    );
    if mine.is_empty() {
        bail!(
            "--reconcile export at {} has no row for {operator} (it carries {actors} other \
             accounts). Pull the export for the account whose sessions this report covers, or \
             correct --reconcile-user; a per-user reconciliation is never emitted with a $0.00 \
             billed figure and never falls back to the organization total. {PULL_COMMAND}",
            export_path.display()
        );
    }
    Ok(mine)
}

/// The export's window, verified against the report's, as a display string.
///
/// Two shapes, in this order, because a `user-cost` export does not state its own period:
///
/// 1. **Rows carry `starting_at`/`ending_at`** (the org-wide bucketed shape, and any future per-user
///    export that stamps them): exact-instant equality against `report.since`/`report.until`, the
///    strongest check available and the one the design specifies.
/// 2. **No row carries either** (every `user-cost` export today): the period is read from the
///    FILENAME, which `pull-usage-report.py` writes from the very window it requested
///    (`enterprise-user-cost-<start>-<end>.json`), and compared at DATE granularity. This is
///    provenance from the same tool, not a guess -- but it does mean a renamed export cannot be
///    verified, so an unnameable window is a hard error rather than an unchecked comparison.
fn window(records: &[CostRecord], export_path: &Path, report: &Report) -> Result<String> {
    let stamped: Vec<(DateTime<Utc>, DateTime<Utc>)> = records
        .iter()
        .filter_map(|r| Some((r.starting_at?, r.ending_at?)))
        .collect();
    debug!(
        "reconcile::window: records={} stamped={} path={}",
        records.len(),
        stamped.len(),
        export_path.display()
    );
    if let (Some(start), Some(end)) = (
        stamped.iter().map(|(s, _)| *s).min(),
        stamped.iter().map(|(_, e)| *e).max(),
    ) {
        if start != report.since || end != report.until {
            bail!(
                "--reconcile export window [{} .. {}] does not match this report's window [{} .. \
                 {}]; pull the Analytics cost export for the exact --since/--until used by `report \
                 collect` and re-run render",
                start.to_rfc3339(),
                end.to_rfc3339(),
                report.since.to_rfc3339(),
                report.until.to_rfc3339()
            );
        }
        return Ok(display_window(start.date_naive(), end.date_naive()));
    }

    let (start, end) = filename_window(export_path).ok_or_else(|| {
        eyre::eyre!(
            "--reconcile export at {} states no window: its rows carry no starting_at/ending_at \
             (the `--report user-cost` shape) and its filename names no [start, end] date pair, so \
             the period cannot be verified and this render will not compare two possibly different \
             periods. Keep the name pull-usage-report.py writes \
             (enterprise-user-cost-<start>-<end>.json), or re-pull: {PULL_COMMAND}",
            export_path.display()
        )
    })?;
    let (want_start, want_end) = (report.since.date_naive(), report.until.date_naive());
    if start != want_start || end != want_end {
        bail!(
            "--reconcile export window [{start} .. {end}], read from the filename {}, does not \
             match this report's window [{want_start} .. {want_end}]; pull the export for the \
             exact --since/--until used by `report collect` and re-run render",
            export_path.display()
        );
    }
    Ok(display_window(start, end))
}

/// The last two `YYYY-MM-DD` dates in the file's name, which is where
/// `pull-usage-report.py --output`'s default puts the window it requested. The LAST two, so a
/// directory or a prefix carrying a date of its own cannot displace the real pair.
fn filename_window(export_path: &Path) -> Option<(NaiveDate, NaiveDate)> {
    let name = export_path.file_name()?.to_str()?;
    let dates: Vec<NaiveDate> = date_pattern()
        .find_iter(name)
        .filter_map(|m| NaiveDate::parse_from_str(m.as_str(), "%Y-%m-%d").ok())
        .collect();
    let end = *dates.last()?;
    let start = *dates.get(dates.len().checked_sub(2)?)?;
    debug!("reconcile::filename_window: name={name} start={start} end={end}");
    Some((start, end))
}

fn date_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\d{4}-\d{2}-\d{2}").expect("the export-filename date pattern is a valid regex"))
}

fn display_window(start: NaiveDate, end: NaiveDate) -> String {
    format!("{start} to {end}")
}

#[cfg(test)]
mod tests;
