//! Phase 12 (`render --reconcile`): tests split out per the file-size limit.
use super::*;
use crate::render::reconciliation::{NO_RECONCILE_NOTE, NO_RECONCILE_WARNING, RECONCILED_NOTE};

/// Write an `anthropic-usage-report --report cost` export (a flat JSON array of per-model,
/// per-day cost records) to `<dir>/<name>` and return the path.
fn write_export(dir: &TempDir, name: &str, records: Vec<serde_json::Value>) -> std::path::PathBuf {
    let path = dir.path().join(name);
    fs::write(&path, serde_json::to_string(&records).unwrap()).unwrap();
    path
}

fn cost_record(model: &str, amount: &str, starting_at: &str, ending_at: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "amount": amount,
        "starting_at": starting_at,
        "ending_at": ending_at,
    })
}

/// Without `--reconcile`, the render still succeeds, but the artifact states the gap: `reconciliation`
/// is entirely absent (never an empty object) and `reconciliation-status` carries the
/// no-export-supplied sentence verbatim -- design Phase 12, "Absence is never silent".
#[test]
fn build_context_block_reconciliation_absent_by_default_states_no_export_supplied() {
    let report = sample_report(); // since=2026-04-01 until=2026-04-30
    let block = build_context_block(
        &report,
        false,
        None,
        &pricing(),
        crate::aggregate::DEFAULT_OUTLIERS,
        None,
        None,
    )
    .unwrap()
    .json;
    let parsed: serde_json::Value = serde_json::from_str(&block).unwrap();

    assert!(
        parsed.get("reconciliation").is_none(),
        "no --reconcile -> no reconciliation key at all, not an empty object"
    );
    let status = parsed
        .get("reconciliation-status")
        .and_then(|v| v.as_str())
        .expect("reconciliation-status must ALWAYS be present, flag or no flag");
    assert_eq!(status, NO_RECONCILE_NOTE);
    assert!(
        status.to_lowercase().contains("no") && status.to_lowercase().contains("export"),
        "must state plainly that no export was supplied: {status}"
    );
}

/// `--reconcile <file>` whose window matches this report's exactly lights up the reconciliation
/// block: billed, modeled, `unseen-account-spend`, and `scope-note` all present, and the status
/// sentence switches to the reconciled wording.
#[test]
fn build_context_block_reconciliation_present_when_window_matches() {
    let tmp = TempDir::new().unwrap();
    let report = sample_report(); // since=2026-04-01T00:00:00Z until=2026-04-30T00:00:00Z, spend $0.60
    let export_path = write_export(
        &tmp,
        "export.json",
        vec![cost_record(
            "claude-opus-4-7",
            "1.10",
            "2026-04-01T00:00:00Z",
            "2026-04-30T00:00:00Z",
        )],
    );

    let block = build_context_block(
        &report,
        false,
        None,
        &pricing(),
        crate::aggregate::DEFAULT_OUTLIERS,
        None,
        Some(&export_path),
    )
    .unwrap()
    .json;
    let parsed: serde_json::Value = serde_json::from_str(&block).unwrap();

    let recon = parsed
        .get("reconciliation")
        .expect("reconciliation key must be present when --reconcile matches the window");
    assert_eq!(
        recon.get("source").and_then(|v| v.as_str()),
        Some("anthropic enterprise analytics")
    );
    assert_eq!(recon.get("billed").and_then(|v| v.as_str()), Some("$1.10"));
    assert_eq!(recon.get("modeled").and_then(|v| v.as_str()), Some("$0.60"));
    assert_eq!(
        recon.get("unseen-account-spend").and_then(|v| v.as_str()),
        Some("+$0.50")
    );
    assert!(
        recon.get("delta").is_none(),
        "the serialized key is renamed, never `delta`"
    );
    let scope_note = recon
        .get("scope-note")
        .and_then(|v| v.as_str())
        .expect("scope-note must be present");
    assert!(
        scope_note.contains("never that clyde miscounted") || scope_note.to_lowercase().contains("does not see"),
        "scope note must deny the miscount framing: {scope_note}"
    );

    let status = parsed.get("reconciliation-status").and_then(|v| v.as_str()).unwrap();
    assert_eq!(status, RECONCILED_NOTE);
}

/// An export whose window does not match the report's fails the WHOLE render (not a silent
/// comparison of different periods), naming both windows so the operator can tell what to re-pull.
#[test]
fn build_context_block_bails_on_reconcile_window_mismatch_naming_both_windows() {
    let tmp = TempDir::new().unwrap();
    let report = sample_report(); // since=2026-04-01T00:00:00Z until=2026-04-30T00:00:00Z
    let export_path = write_export(
        &tmp,
        "export.json",
        vec![cost_record(
            "claude-opus-4-7",
            "1.10",
            "2026-05-01T00:00:00Z",
            "2026-05-31T00:00:00Z",
        )],
    );

    let err = build_context_block(
        &report,
        false,
        None,
        &pricing(),
        crate::aggregate::DEFAULT_OUTLIERS,
        None,
        Some(&export_path),
    )
    .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("2026-04-01"), "must name the report's window: {msg}");
    assert!(msg.contains("2026-05-01"), "must name the export's window: {msg}");
}

/// The stderr warning fires exactly when `--reconcile` is absent, and never when it is supplied
/// (whether or not the fold itself later succeeds -- the warning is about the FLAG, not the
/// outcome). A pure function (house rule: return data, not side effects), so this asserts the
/// content without capturing a subprocess's stderr.
#[test]
fn no_reconcile_warning_fires_only_when_the_flag_is_absent() {
    assert_eq!(no_reconcile_warning(None), Some(NO_RECONCILE_WARNING));
    let path = std::path::PathBuf::from("/tmp/whatever.json");
    assert_eq!(no_reconcile_warning(Some(&path)), None);
}

/// Phase 12's prompt-edit ledger: BOTH templates document `reconciliation-status` (always present)
/// and `reconciliation` (optional), state the section itself is ALWAYS emitted (unlike Month over
/// Month, which is omitted without `--prior`), license `unseen-account-spend`, and deny the
/// miscount framing by name so the rendered prose cannot land there by omission.
///
/// BITES: revert either template to treating the reconciliation section as optional, or drop the
/// miscount denial, and the matching assertion fails.
#[test]
fn both_templates_document_reconciliation_as_always_present_and_deny_miscount_framing() {
    for (name, tpl) in [("report.pmt", DEFAULT_PROMPT), ("report-html.pmt", DEFAULT_HTML_PROMPT)] {
        assert!(
            tpl.contains("reconciliation-status"),
            "{name} must document the always-present reconciliation-status field"
        );
        assert!(
            tpl.contains("`reconciliation`"),
            "{name} must document the optional reconciliation block"
        );
        assert!(
            tpl.contains("ALWAYS") && tpl.to_lowercase().contains("reconciliation"),
            "{name} must state the reconciliation section is ALWAYS present, flag or no flag"
        );
        assert!(
            tpl.contains("unseen-account-spend"),
            "{name} must license the reader-facing unseen-account-spend figure, never bare \"delta\""
        );
        assert!(
            tpl.contains("undercounted"),
            "{name} must deny the miscount framing by name, not just omit it"
        );
        assert!(
            tpl.contains("(untracked)"),
            "{name} must document the untracked-model row case for by-model"
        );
    }
}
