//! Phase 12 (`render --reconcile`): tests split out per the file-size limit.
use super::*;
use crate::persona::PersonaBlock;
use crate::render::reconciliation::{NO_RECONCILE_NOTE, NO_RECONCILE_WARNING, reconciled_note};

/// The operator the exports below are scoped to. A real render reads this off the persona block;
/// these tests supply it both ways (persona and `--reconcile-user`) so both paths are covered.
const ME: &str = "jordan.rivera@northwind-media.example";

/// Write an `anthropic-usage-report --report user-cost` export (a flat JSON array of per-user,
/// per-model cost records) to `<dir>/<name>` and return the path.
fn write_export(dir: &TempDir, name: &str, records: Vec<serde_json::Value>) -> std::path::PathBuf {
    let path = dir.path().join(name);
    fs::write(&path, serde_json::to_string(&records).unwrap()).unwrap();
    path
}

/// `amount` is decimal-string CENTS, matching the real export -- `"110.00"` is `$1.10`. Every row
/// carries an `actor`: that is what makes it a per-user export rather than the org-wide one, which
/// `reconcile::fold` rejects by name.
fn cost_record(email: &str, model: &str, amount: &str, starting_at: &str, ending_at: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "amount": amount,
        "actor": { "type": "user_actor", "email": email, "deleted": false },
        "starting_at": starting_at,
        "ending_at": ending_at,
    })
}

/// A persona block carrying just the work email the operator is resolved from.
fn persona_with_email(email: &str) -> PersonaBlock {
    PersonaBlock {
        email: Some(email.to_string()),
        ..PersonaBlock::default()
    }
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

/// `--reconcile <file>` whose window matches this report's lights up the reconciliation block:
/// billed, modeled, `unseen-account-spend`, `operator` and `scope-note` all present, and the status
/// sentence switches to the reconciled wording. The operator here comes from the PERSONA block, the
/// same identity the report already resolves for its persona section.
#[test]
fn build_context_block_reconciliation_present_when_window_matches() {
    let tmp = TempDir::new().unwrap();
    let report = sample_report(); // since=2026-04-01T00:00:00Z until=2026-04-30T00:00:00Z, spend $0.60
    let export_path = write_export(
        &tmp,
        "export.json",
        vec![
            cost_record(
                ME,
                "claude-opus-4-7",
                "110.00",
                "2026-04-01T00:00:00Z",
                "2026-04-30T00:00:00Z",
            ),
            // Another seat's spend, two orders of magnitude larger: the per-user scoping is what
            // keeps it out of every figure below.
            cost_record(
                "someone.else@northwind-media.example",
                "claude-opus-4-7",
                "99000.00",
                "2026-04-01T00:00:00Z",
                "2026-04-30T00:00:00Z",
            ),
        ],
    );

    let persona = persona_with_email(ME);
    let block = build_context_block(
        &report,
        false,
        Some(&persona),
        &pricing(),
        crate::aggregate::DEFAULT_OUTLIERS,
        None,
        Some(&export_path),
        None,
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
    assert_eq!(recon.get("operator").and_then(|v| v.as_str()), Some(ME));
    assert_eq!(
        recon.get("billed").and_then(|v| v.as_str()),
        Some("$1.10"),
        "billed is THIS operator's, never the other seat's $990.00"
    );
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
        scope_note.contains("never that clyde miscounted") || scope_note.to_lowercase().contains("cannot see"),
        "scope note must deny the miscount framing: {scope_note}"
    );
    assert!(
        scope_note.contains(ME),
        "scope note must name whose bill this is: {scope_note}"
    );

    let status = parsed.get("reconciliation-status").and_then(|v| v.as_str()).unwrap();
    assert_eq!(status, reconciled_note(ME));
}

/// `--reconcile-user` beats the persona's email, so a report rendered on one machine for another
/// person's window can still be scoped correctly.
#[test]
fn reconcile_user_overrides_the_persona_email() {
    let tmp = TempDir::new().unwrap();
    let report = sample_report();
    let export_path = write_export(
        &tmp,
        "export.json",
        vec![cost_record(
            "someone.else@northwind-media.example",
            "claude-opus-4-7",
            "110.00",
            "2026-04-01T00:00:00Z",
            "2026-04-30T00:00:00Z",
        )],
    );

    let persona = persona_with_email(ME);
    let block = build_context_block(
        &report,
        false,
        Some(&persona),
        &pricing(),
        crate::aggregate::DEFAULT_OUTLIERS,
        None,
        Some(&export_path),
        Some("someone.else@northwind-media.example"),
    )
    .unwrap()
    .json;
    let parsed: serde_json::Value = serde_json::from_str(&block).unwrap();
    let recon = parsed.get("reconciliation").unwrap();
    assert_eq!(
        recon.get("operator").and_then(|v| v.as_str()),
        Some("someone.else@northwind-media.example")
    );
    assert_eq!(recon.get("billed").and_then(|v| v.as_str()), Some("$1.10"));
}

/// An ORG-WIDE `--report cost` export (no `actor` on any row) fails the WHOLE render rather than
/// publishing the organization's bill as this one person's unaccounted-for spend.
#[test]
fn build_context_block_bails_on_an_org_wide_export() {
    let tmp = TempDir::new().unwrap();
    let report = sample_report();
    let mut row = cost_record(
        ME,
        "claude-opus-4-7",
        "99999999.00",
        "2026-04-01T00:00:00Z",
        "2026-04-30T00:00:00Z",
    );
    row.as_object_mut().unwrap().remove("actor");
    let export_path = write_export(&tmp, "enterprise-cost-2026-04-01-2026-04-30.json", vec![row]);

    let persona = persona_with_email(ME);
    let err = build_context_block(
        &report,
        false,
        Some(&persona),
        &pricing(),
        crate::aggregate::DEFAULT_OUTLIERS,
        None,
        Some(&export_path),
        None,
    )
    .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("ORG-WIDE"), "must say what the file is: {msg}");
    assert!(
        msg.contains("--report user-cost"),
        "must name the command that produces the right export: {msg}"
    );
}

/// No operator anywhere (no persona email, no `--reconcile-user`) fails the render naming the flag,
/// rather than reconciling against something unscoped.
#[test]
fn build_context_block_bails_when_no_operator_can_be_resolved() {
    let tmp = TempDir::new().unwrap();
    let report = sample_report();
    let export_path = write_export(
        &tmp,
        "export.json",
        vec![cost_record(
            ME,
            "claude-opus-4-7",
            "110.00",
            "2026-04-01T00:00:00Z",
            "2026-04-30T00:00:00Z",
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
        None,
    )
    .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("--reconcile-user"), "must name the remedy flag: {msg}");
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
            ME,
            "claude-opus-4-7",
            "110.00",
            "2026-05-01T00:00:00Z",
            "2026-05-31T00:00:00Z",
        )],
    );

    let persona = persona_with_email(ME);
    let err = build_context_block(
        &report,
        false,
        Some(&persona),
        &pricing(),
        crate::aggregate::DEFAULT_OUTLIERS,
        None,
        Some(&export_path),
        None,
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
/// Month, which is omitted without `--prior`), license `unseen-account-spend`, deny the miscount
/// framing by name, and -- since the figure is now ONE PERSON'S -- name `operator` and forbid
/// describing the billed figure as the organization's.
///
/// BITES: revert either template to treating the reconciliation section as optional, drop the
/// miscount denial, or drop the per-user scoping, and the matching assertion fails.
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
        // Both words must land in ONE sentence ABOUT THIS SECTION. Two independent `contains`
        // checks passed on any template that said `ALWAYS` about some other field and mentioned
        // "reconciliation" anywhere -- including one reverted to calling the section optional, which
        // is precisely the revert the BITES contract above promises to catch.
        //
        // Line-scoping alone is still not enough: both templates ALSO carry
        // "`reconciliation-status`: ALWAYS present", which is about the FIELD. Anchoring on
        // "Reconciliation section" / "Reconciliation card" pins the sentence that governs whether
        // the SECTION is emitted (markdown says "section", the dashboard says "card").
        assert!(
            tpl.lines().any(|line| {
                line.contains("ALWAYS present")
                    && (line.contains("Reconciliation section") || line.contains("Reconciliation card"))
            }),
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
        assert!(
            tpl.contains("`operator`"),
            "{name} must document the operator the billed figure is scoped to"
        );
        assert!(
            tpl.contains("ONE PERSON'S"),
            "{name} must state the billed figure is one person's, not the organization's"
        );
    }
}
