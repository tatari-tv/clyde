//! Render-side wiring for `--reconcile <analytics.json>` (design Phase 12): the stderr warning and
//! the always-present context-block status sentence. The actual export parse/fold lives in
//! [`crate::reconcile`] (a sibling top-level module, not this one -- kept separate so `render.rs`
//! stays under the house 1500-line file cap, per Phase 11's precedent of splitting `chart`/
//! `geometry` out of this same file).

use crate::reconcile::{self, Reconciliation};
use crate::report::Report;
use eyre::Result;
use log::debug;
use std::path::Path;

/// Printed to stderr when `--reconcile` is absent (design Phase 12, "Absence is never silent"),
/// mirroring `run_collect`'s `--min-enrichment` warning (Phase 3): advisory, never fails the
/// render. The artifact's own half of this rule is [`NO_RECONCILE_NOTE`], carried verbatim into
/// the context block so BOTH the terminal and the artifact state the gap.
pub(super) const NO_RECONCILE_WARNING: &str = "warning: no --reconcile <analytics.json> was supplied; this \
     render's total spend is a modeled figure only, not checked against your own billed spend in \
     the Claude Enterprise Analytics cost report. Pull a per-user export with the \
     anthropic-usage-report skill (--report user-cost) and pass --reconcile to close the gap.";

/// The stderr warning for a render with no `--reconcile` supplied, or `None` when one was (design
/// Phase 12, "Absence is never silent"). Returns the message rather than printing it (house rule:
/// return data, not side effects), so this is unit-testable without capturing stderr -- mirrors
/// `lib.rs`'s `enrichment_warning`. `render::run` is the one call site that actually prints it.
pub(super) fn no_reconcile_warning(reconcile: Option<&Path>) -> Option<&'static str> {
    if reconcile.is_some() {
        return None;
    }
    Some(NO_RECONCILE_WARNING)
}

/// Quoted verbatim by both templates when `--reconcile` was NOT supplied (design Phase 12,
/// "Absence is never silent"): the total above is modeled only, and this render never checked it
/// against the authoritative export. The old absence indicator ("not an invoice") no longer exists
/// under the citing wording (`BASIS_NOTE`, in `render.rs`) -- so this is the one place that states
/// the gap.
pub(super) const NO_RECONCILE_NOTE: &str = "No Claude Enterprise Analytics cost export was supplied for this \
     render (--reconcile <file>); the total spend above is a modeled figure only and has not been \
     reconciled against this operator's authoritative billed spend.";

/// Quoted verbatim by both templates when reconciliation succeeded -- the counterpart to
/// [`NO_RECONCILE_NOTE`]; the artifact states exactly one of the two, never neither. Names the
/// operator, because the whole point of the per-user scoping is that the billed figure beside a
/// per-user modeled total belongs to that same person and to nobody else.
pub(super) fn reconciled_note(operator: &str) -> String {
    format!(
        "This render's modeled total was reconciled against the Claude Enterprise Analytics cost \
         export for {operator} over this exact window; see the Reconciliation section for the \
         billed figure and the scope note."
    )
}

/// Fold `--reconcile <analytics.json>` into the context block, or state the gap when it was not
/// supplied (design Phase 12). The SECOND return value is ALWAYS present in the context block
/// (`reconciliation-status`), so the artifact states the reconciliation state in prose even in a
/// render where [`Reconciliation`] itself is entirely absent. A window mismatch (`reconcile::fold`)
/// is the only hard failure here; absence of `--reconcile` is a stated gap, not a render failure --
/// see `render::run`'s stderr warning for the other half of "never silent".
pub(super) fn build_reconciliation_view(
    reconcile_path: Option<&Path>,
    operator: Option<&str>,
    report: &Report,
) -> Result<(Option<Reconciliation>, String)> {
    let Some(path) = reconcile_path else {
        debug!("render::reconciliation::build_reconciliation_view: no --reconcile supplied");
        return Ok((None, NO_RECONCILE_NOTE.to_string()));
    };
    debug!(
        "render::reconciliation::build_reconciliation_view: path={} operator={:?}",
        path.display(),
        operator
    );
    let reconciliation = reconcile::fold(path, operator, report)?;
    debug!(
        "render::reconciliation::build_reconciliation_view: operator={} billed={} modeled={} \
         unseen-account-spend={}",
        reconciliation.operator, reconciliation.billed, reconciliation.modeled, reconciliation.delta
    );
    let note = reconciled_note(&reconciliation.operator);
    Ok((Some(reconciliation), note))
}
