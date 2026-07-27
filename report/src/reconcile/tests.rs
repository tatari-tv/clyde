#![allow(clippy::unwrap_used)]

use super::*;
use crate::report::{ModelTokens, Totals};
use std::collections::BTreeMap;
use tempfile::TempDir;

/// The operator every test scopes to, and the one whose rows carry the figures asserted below.
const ME: &str = "jordan.rivera@northwind-media.example";

/// Another seat in the same org. Every export here carries their rows too, because a real
/// `--report user-cost` export carries every seat: a test whose export holds one actor could not
/// tell a working filter from no filter at all.
const SOMEONE_ELSE: &str = "other.person@northwind-media.example";

fn ts(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

fn model_tokens(spend_usd: Option<f64>) -> ModelTokens {
    ModelTokens {
        input: 100,
        output: 50,
        cache_5m_write: 0,
        cache_1h_write: 0,
        cache_read: 0,
        total: 150,
        spend_usd,
    }
}

/// A minimal report carrying just what `fold` reads: `since`/`until` and `totals` (spend + models).
fn report_with(since: &str, until: &str, spend_usd: f64, models: Vec<(&str, Option<f64>)>) -> Report {
    let mut totals_models = BTreeMap::new();
    for (name, spend) in models {
        totals_models.insert(name.to_string(), model_tokens(spend));
    }
    Report {
        schema_version: 2,
        generated: ts("2026-07-26T00:00:00Z"),
        host: "desk".into(),
        since: ts(since),
        until: ts(until),
        outcomes_enabled: None,
        notes: Vec::new(),
        totals: Totals {
            sessions: 0,
            spend_usd,
            untracked_models: Vec::new(),
            models: totals_models,
            outcomes: None,
            cache_read_share: None,
            tool_error_rate: None,
        },
        sessions: BTreeMap::new(),
    }
}

/// One `pull-usage-report.py --report user-cost` row: an `actor` (the discriminator between the
/// per-user and org-wide exports), `model`, and `amount` (decimal-string **CENTS**, matching the
/// real export -- `"7000.00"` is `$70.00`). Timestamps are supplied here so the strongest window
/// check is the one under test; [`user_cost_record`] is the real per-user shape, which leaves them
/// null.
fn record(email: &str, model: &str, amount: &str, starting_at: &str, ending_at: &str) -> serde_json::Value {
    serde_json::json!({
        "product": null,
        "model": model,
        "amount": amount,
        "list_amount": amount,
        "cost_type": null,
        "token_type": null,
        "currency": "USD",
        "requests": 10,
        "actor": {
            "type": "user_actor",
            "user_id": "user_0134J8JE89MVRJka6xDmKSup",
            "name": "A Person",
            "email": email,
            "deleted": false,
        },
        "starting_at": starting_at,
        "ending_at": ending_at,
    })
}

/// The REAL `--report user-cost` shape: one row per member for the whole window, with
/// `starting_at`/`ending_at` NULL (verified against a live pull, 2026-07-26).
fn user_cost_record(email: &str, model: &str, amount: &str) -> serde_json::Value {
    let mut row = record(email, model, amount, "1970-01-01T00:00:00Z", "1970-01-01T00:00:00Z");
    row["starting_at"] = serde_json::Value::Null;
    row["ending_at"] = serde_json::Value::Null;
    row
}

/// An ORG-WIDE `--report cost` row: same columns, no `actor`.
fn org_record(model: &str, amount: &str, starting_at: &str, ending_at: &str) -> serde_json::Value {
    let mut row = record("ignored@example.com", model, amount, starting_at, ending_at);
    row.as_object_mut().unwrap().remove("actor");
    row
}

fn write_export(dir: &TempDir, name: &str, records: Vec<serde_json::Value>) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, serde_json::to_string(&records).unwrap()).unwrap();
    path
}

#[test]
fn fold_computes_billed_modeled_and_unseen_account_spend_on_matching_window() {
    let report = report_with(
        "2026-06-26T00:00:00Z",
        "2026-07-25T00:00:00Z",
        100.0,
        vec![("claude-opus-5", Some(60.0)), ("claude-sonnet-5", Some(40.0))],
    );
    let dir = TempDir::new().unwrap();
    let export = write_export(
        &dir,
        "export.json",
        vec![
            record(
                ME,
                "claude-opus-5",
                "7000.00",
                "2026-06-26T00:00:00Z",
                "2026-06-27T00:00:00Z",
            ),
            record(
                ME,
                "claude-sonnet-5",
                "4000.00",
                "2026-06-26T00:00:00Z",
                "2026-06-27T00:00:00Z",
            ),
            // billed on claude.ai web / Cowork / another host, invisible to `clyde report` -- the
            // whole point of "billed >= modeled" being expected rather than an error.
            record(
                ME,
                "claude-haiku-4-5",
                "2000.00",
                "2026-07-24T00:00:00Z",
                "2026-07-25T00:00:00Z",
            ),
            // Another seat's spend. It must not touch one single figure below.
            record(
                SOMEONE_ELSE,
                "claude-opus-5",
                "9999999.00",
                "2026-06-26T00:00:00Z",
                "2026-06-27T00:00:00Z",
            ),
        ],
    );

    let recon = fold(&export, Some(ME), &report).unwrap();

    assert_eq!(recon.source, "anthropic enterprise analytics");
    assert_eq!(recon.operator, ME);
    assert_eq!(recon.window, "2026-06-26 to 2026-07-25");
    assert_eq!(recon.billed, "$130.00", "the other seat's $99,999.99 must not be in it");
    assert_eq!(recon.modeled, "$100.00");
    assert_eq!(
        recon.delta, "+$30.00",
        "reader-facing key renamed to unseen-account-spend on serialize"
    );
    assert_eq!(recon.scope_note, scope_note(ME));
    assert!(
        recon.scope_note.contains(ME),
        "the scope note names whose bill this is: {}",
        recon.scope_note
    );
    // Pre-sorted billed-descending, like every other list in the context block.
    let names: Vec<&str> = recon.by_model.iter().map(|r| r.model.as_str()).collect();
    assert_eq!(names, vec!["claude-opus-5", "claude-sonnet-5", "claude-haiku-4-5"]);
    assert_eq!(recon.by_model[0].billed, "$70.00");
    assert_eq!(recon.by_model[0].modeled, "$60.00");
    assert_eq!(recon.by_model[0].delta, "+$10.00");

    // The serialized key really is `unseen-account-spend`, not `delta` -- the field rename must
    // survive `serde_json::to_value`, not just compile.
    let value = serde_json::to_value(&recon).unwrap();
    assert!(value.get("unseen-account-spend").is_some());
    assert!(value.get("delta").is_none());
    assert_eq!(value.get("operator").and_then(|v| v.as_str()), Some(ME));
    assert!(
        !recon.scope_note.to_lowercase().contains("miscount")
            || recon.scope_note.contains("never that clyde miscounted"),
        "scope note must deny the miscount framing, not assert it"
    );
}

#[test]
fn fold_rejects_an_org_wide_cost_export_naming_the_right_command() {
    // The defect this whole scoping change exists to kill: an org-wide export folded into a
    // per-user report published everyone else in the organization's spend as clyde's unaccounted-for gap.
    let report = report_with("2026-06-26T00:00:00Z", "2026-07-25T00:00:00Z", 100.0, vec![]);
    let dir = TempDir::new().unwrap();
    let export = write_export(
        &dir,
        "enterprise-cost-2026-06-26-2026-07-25.json",
        vec![
            org_record(
                "claude-opus-5",
                "99999999.00",
                "2026-06-26T00:00:00Z",
                "2026-06-27T00:00:00Z",
            ),
            org_record(
                "claude-sonnet-5",
                "100000.00",
                "2026-06-26T00:00:00Z",
                "2026-06-27T00:00:00Z",
            ),
        ],
    );

    let err = fold(&export, Some(ME), &report).unwrap_err().to_string();
    assert!(err.contains("ORG-WIDE"), "must say what the file is: {err}");
    assert!(
        err.contains("--report user-cost"),
        "must name the command that produces the right export: {err}"
    );
}

#[test]
fn fold_rejects_an_export_with_actors_on_only_some_rows() {
    let report = report_with("2026-06-26T00:00:00Z", "2026-07-25T00:00:00Z", 100.0, vec![]);
    let dir = TempDir::new().unwrap();
    let export = write_export(
        &dir,
        "mixed.json",
        vec![
            record(
                ME,
                "claude-opus-5",
                "7000.00",
                "2026-06-26T00:00:00Z",
                "2026-07-25T00:00:00Z",
            ),
            org_record(
                "claude-sonnet-5",
                "4000.00",
                "2026-06-26T00:00:00Z",
                "2026-07-25T00:00:00Z",
            ),
        ],
    );

    let err = fold(&export, Some(ME), &report).unwrap_err().to_string();
    assert!(err.contains("some rows and not others"), "got: {err}");
}

#[test]
fn fold_fails_loudly_when_the_export_has_no_row_for_the_operator() {
    // Never a silent $0.00 billed, never a fallback to the org total.
    let report = report_with("2026-06-26T00:00:00Z", "2026-07-25T00:00:00Z", 100.0, vec![]);
    let dir = TempDir::new().unwrap();
    let export = write_export(
        &dir,
        "export.json",
        vec![record(
            SOMEONE_ELSE,
            "claude-opus-5",
            "7000.00",
            "2026-06-26T00:00:00Z",
            "2026-07-25T00:00:00Z",
        )],
    );

    let err = fold(&export, Some(ME), &report).unwrap_err().to_string();
    assert!(err.contains(ME), "must name the operator it looked for: {err}");
    assert!(err.contains("export.json"), "must name the export it looked in: {err}");
    assert!(
        err.contains("$0.00") && err.contains("organization total"),
        "must state what it refuses to do instead: {err}"
    );
}

#[test]
fn fold_fails_loudly_when_no_operator_is_known() {
    let report = report_with("2026-06-26T00:00:00Z", "2026-07-25T00:00:00Z", 100.0, vec![]);
    let dir = TempDir::new().unwrap();
    let export = write_export(
        &dir,
        "export.json",
        vec![record(
            ME,
            "claude-opus-5",
            "7000.00",
            "2026-06-26T00:00:00Z",
            "2026-07-25T00:00:00Z",
        )],
    );

    for operator in [None, Some(""), Some("   ")] {
        let err = fold(&export, operator, &report).unwrap_err().to_string();
        assert!(
            err.contains("--reconcile-user"),
            "must name the flag that fixes it ({operator:?}): {err}"
        );
    }
}

#[test]
fn fold_matches_the_operator_case_insensitively() {
    let report = report_with(
        "2026-06-26T00:00:00Z",
        "2026-06-27T00:00:00Z",
        60.0,
        vec![("claude-opus-5", Some(60.0))],
    );
    let dir = TempDir::new().unwrap();
    let export = write_export(
        &dir,
        "export.json",
        vec![record(
            "Jordan.Rivera@Northwind-Media.Example",
            "claude-opus-5",
            "7000.00",
            "2026-06-26T00:00:00Z",
            "2026-06-27T00:00:00Z",
        )],
    );

    let recon = fold(&export, Some(ME), &report).unwrap();
    assert_eq!(recon.billed, "$70.00");
}

#[test]
fn fold_reads_the_window_from_the_filename_when_the_rows_carry_none() {
    // The REAL `--report user-cost` shape: every row's starting_at/ending_at is null, so the
    // period comes from the name `pull-usage-report.py` wrote it under.
    let report = report_with(
        "2026-06-26T00:00:00Z",
        "2026-07-25T00:00:00Z",
        100.0,
        vec![("claude-opus-5", Some(100.0))],
    );
    let dir = TempDir::new().unwrap();
    let export = write_export(
        &dir,
        "enterprise-user-cost-2026-06-26-2026-07-25.json",
        vec![
            user_cost_record(ME, "claude-opus-5", "11000.00"),
            user_cost_record(SOMEONE_ELSE, "claude-opus-5", "500000.00"),
        ],
    );

    let recon = fold(&export, Some(ME), &report).unwrap();
    assert_eq!(recon.window, "2026-06-26 to 2026-07-25");
    assert_eq!(recon.billed, "$110.00");
}

#[test]
fn fold_rejects_a_filename_window_that_does_not_match_the_report() {
    let report = report_with("2026-06-26T00:00:00Z", "2026-07-25T00:00:00Z", 100.0, vec![]);
    let dir = TempDir::new().unwrap();
    let export = write_export(
        &dir,
        "enterprise-user-cost-2026-05-01-2026-05-31.json",
        vec![user_cost_record(ME, "claude-opus-5", "11000.00")],
    );

    let err = fold(&export, Some(ME), &report).unwrap_err().to_string();
    assert!(err.contains("2026-05-01"), "must name the export's window: {err}");
    assert!(err.contains("2026-06-26"), "must name the report's window: {err}");
    assert!(err.contains("does not match"), "must state the mismatch plainly: {err}");
}

#[test]
fn fold_rejects_an_undatable_export_rather_than_comparing_unknown_periods() {
    let report = report_with("2026-06-26T00:00:00Z", "2026-07-25T00:00:00Z", 100.0, vec![]);
    let dir = TempDir::new().unwrap();
    let export = write_export(
        &dir,
        "user-cost.json",
        vec![user_cost_record(ME, "claude-opus-5", "11000.00")],
    );

    let err = fold(&export, Some(ME), &report).unwrap_err().to_string();
    assert!(err.contains("states no window"), "got: {err}");
    assert!(
        err.contains("enterprise-user-cost-<start>-<end>.json"),
        "must name the filename that fixes it: {err}"
    );
}

#[test]
fn fold_rejects_window_mismatch_naming_both_windows() {
    let report = report_with("2026-06-26T00:00:00Z", "2026-07-25T00:00:00Z", 10.0, vec![]);
    let dir = TempDir::new().unwrap();
    let export = write_export(
        &dir,
        "export.json",
        vec![record(
            ME,
            "claude-opus-5",
            "5.00",
            "2026-06-01T00:00:00Z",
            "2026-06-02T00:00:00Z",
        )],
    );

    let err = fold(&export, Some(ME), &report).unwrap_err().to_string();
    assert!(
        err.contains("2026-06-26T00:00:00+00:00"),
        "must name the report's window: {err}"
    );
    assert!(
        err.contains("2026-06-01T00:00:00+00:00"),
        "must name the export's window: {err}"
    );
    assert!(err.contains("does not match"), "must state the mismatch plainly: {err}");
}

#[test]
fn fold_rejects_empty_export() {
    let report = report_with("2026-06-26T00:00:00Z", "2026-07-25T00:00:00Z", 10.0, vec![]);
    let dir = TempDir::new().unwrap();
    let export = write_export(&dir, "empty.json", vec![]);

    let err = fold(&export, Some(ME), &report).unwrap_err().to_string();
    assert!(err.contains("no cost records"), "got: {err}");
}

#[test]
fn fold_rejects_unparseable_amount_naming_the_model() {
    let report = report_with("2026-06-26T00:00:00Z", "2026-07-25T00:00:00Z", 10.0, vec![]);
    let dir = TempDir::new().unwrap();
    let export = write_export(
        &dir,
        "bad.json",
        vec![record(
            ME,
            "claude-opus-5",
            "not-a-number",
            "2026-06-26T00:00:00Z",
            "2026-07-25T00:00:00Z",
        )],
    );

    let err = fold(&export, Some(ME), &report).unwrap_err();
    let full = format!("{err:#}");
    assert!(full.contains("claude-opus-5"), "must name the offending model: {full}");
}

#[test]
fn fold_rejects_a_non_finite_amount_naming_the_model() {
    // `f64::from_str` ACCEPTS these, so they sail past the parse and poison every figure downstream:
    // `billed_total` becomes NaN, `format_usd` renders garbage, and the row sort's `partial_cmp`
    // fallback silently degrades to `Ordering::Equal`. Each must be a loud refusal instead.
    for amount in ["NaN", "nan", "inf", "-inf", "infinity"] {
        let report = report_with("2026-06-26T00:00:00Z", "2026-07-25T00:00:00Z", 10.0, vec![]);
        let dir = TempDir::new().unwrap();
        let export = write_export(
            &dir,
            "nonfinite.json",
            vec![record(
                ME,
                "claude-opus-5",
                amount,
                "2026-06-26T00:00:00Z",
                "2026-07-25T00:00:00Z",
            )],
        );

        let err = fold(&export, Some(ME), &report)
            .expect_err("a non-finite amount must be rejected, not folded into a billed figure");
        let full = format!("{err:#}");
        assert!(
            full.contains("non-finite"),
            "{amount:?} must be refused as non-finite: {full}"
        );
        assert!(full.contains("claude-opus-5"), "must name the offending model: {full}");
    }
}

#[test]
fn fold_accepts_a_finite_amount_at_the_edges() {
    // The guard rejects only NON-finite values: a plain zero and a large finite amount still fold.
    let report = report_with("2026-06-26T00:00:00Z", "2026-06-27T00:00:00Z", 0.0, vec![]);
    let dir = TempDir::new().unwrap();
    let export = write_export(
        &dir,
        "finite.json",
        vec![
            record(
                ME,
                "claude-opus-5",
                "0.00",
                "2026-06-26T00:00:00Z",
                "2026-06-27T00:00:00Z",
            ),
            record(
                ME,
                "claude-sonnet-5",
                "123456789.99",
                "2026-06-26T00:00:00Z",
                "2026-06-27T00:00:00Z",
            ),
        ],
    );

    let out = fold(&export, Some(ME), &report).unwrap();
    assert!(
        !out.billed.contains("NaN") && !out.billed.contains("inf"),
        "a finite export folds to a real dollar figure: {}",
        out.billed
    );
    assert_eq!(out.billed, "$1,234,567.90", "0.00c + 123456789.99c in dollars");
}

#[test]
fn fold_marks_a_modeled_untracked_model_as_untracked_not_zero() {
    // `claude-opus-5` was priced by clyde ($60); `claude-fable-5` was seen but never priced
    // (Phase 6 untracked gate) -- its `modeled` figure must read "(untracked)", never a
    // fabricated "$0.00" that would overstate its unseen-account-spend contribution.
    let report = report_with(
        "2026-06-26T00:00:00Z",
        "2026-06-27T00:00:00Z",
        60.0,
        vec![("claude-opus-5", Some(60.0)), ("claude-fable-5", None)],
    );
    let dir = TempDir::new().unwrap();
    let export = write_export(
        &dir,
        "export.json",
        vec![
            record(
                ME,
                "claude-opus-5",
                "6000.00",
                "2026-06-26T00:00:00Z",
                "2026-06-27T00:00:00Z",
            ),
            record(
                ME,
                "claude-fable-5",
                "500.00",
                "2026-06-26T00:00:00Z",
                "2026-06-27T00:00:00Z",
            ),
        ],
    );

    let recon = fold(&export, Some(ME), &report).unwrap();
    let fable_row = recon
        .by_model
        .iter()
        .find(|r| r.model == "claude-fable-5")
        .expect("claude-fable-5 row present");
    assert_eq!(fable_row.modeled, "(untracked)");
    assert_eq!(fable_row.billed, "$5.00");
}

#[test]
fn fold_includes_a_model_present_only_in_export_with_zero_modeled() {
    let report = report_with(
        "2026-06-26T00:00:00Z",
        "2026-06-27T00:00:00Z",
        60.0,
        vec![("claude-opus-5", Some(60.0))],
    );
    let dir = TempDir::new().unwrap();
    let export = write_export(
        &dir,
        "export.json",
        vec![
            record(
                ME,
                "claude-opus-5",
                "6000.00",
                "2026-06-26T00:00:00Z",
                "2026-06-27T00:00:00Z",
            ),
            // a model clyde's catalog never saw at all this window: this same person's claude.ai chat.
            record(
                ME,
                "claude-sonnet-5",
                "1500.00",
                "2026-06-26T00:00:00Z",
                "2026-06-27T00:00:00Z",
            ),
        ],
    );

    let recon = fold(&export, Some(ME), &report).unwrap();
    let row = recon
        .by_model
        .iter()
        .find(|r| r.model == "claude-sonnet-5")
        .expect("claude-sonnet-5 row present");
    assert_eq!(row.modeled, "$0.00");
    assert_eq!(row.billed, "$15.00");
    assert_eq!(row.delta, "+$15.00");
}

#[test]
fn fold_reads_the_export_amount_as_cents_not_dollars() {
    // Regression: the Analytics cost endpoints report MINOR UNITS. `fold` originally parsed
    // `amount` straight into dollars, which overstated the authoritative billed figure by 100x --
    // publishing an eight-figure bill for a five-figure one, in the one block whose entire job is
    // citing the real number to a finance reader. The skill documents the unit explicitly: "Amount fields on cost endpoints are
    // decimal-string cents (e.g. `\"41280.000000\"` = $412.80)".
    let report = report_with(
        "2026-06-26T00:00:00Z",
        "2026-06-27T00:00:00Z",
        100.0,
        vec![("claude-opus-5", Some(100.0))],
    );
    let dir = TempDir::new().unwrap();
    let export = write_export(
        &dir,
        "export.json",
        vec![record(
            ME,
            "claude-opus-5",
            // the skill's own worked example
            "41280.000000",
            "2026-06-26T00:00:00Z",
            "2026-06-27T00:00:00Z",
        )],
    );

    let recon = fold(&export, Some(ME), &report).unwrap();
    assert_eq!(recon.billed, "$412.80", "41280 cents is $412.80, not $41,280.00");
    assert_eq!(recon.by_model[0].billed, "$412.80");
    // and the derived figure inherits the correction rather than being fixed up separately
    assert_eq!(recon.delta, "+$312.80");
}

#[test]
fn the_scope_note_describes_the_operators_own_unseen_usage_not_other_users() {
    // The rewritten guard sentence has one job: explain the remainder as the SAME PERSON'S usage
    // clyde cannot see. Wording that implies other users would leave the dominant term of the old
    // org-wide figure uncovered, which is how a per-user report published the whole company's
    // spend as one person's unaccounted-for usage.
    let note = scope_note(ME);
    assert!(note.contains("claude.ai web"));
    assert!(note.contains("Cowork"));
    assert!(note.contains("the same person's usage that clyde cannot see"));
    assert!(note.contains("never that clyde miscounted"));
    for org_wording in ["organization", "other users", "every user", "account-wide"] {
        assert!(
            !note.to_lowercase().contains(org_wording),
            "the scope note must not imply org scope ({org_wording}): {note}"
        );
    }
}

#[test]
fn the_by_model_rows_sum_to_the_billed_total_across_fractional_cents() {
    // A real export carries fractional cents (`amount` is a decimal string with six decimals), and
    // three models each a third of a cent over will round UP individually while their raw sum
    // rounds DOWN. Totalling the raw values let the displayed table disagree with its own displayed
    // headline by a cent -- in the one block quoting an authoritative billed figure to a finance
    // reader, which is exactly the "clyde miscounted" reading `scope_note` works to prevent.
    let report = report_with("2026-06-26T00:00:00Z", "2026-06-27T00:00:00Z", 0.0, vec![]);
    let dir = TempDir::new().unwrap();
    let export = write_export(
        &dir,
        "fractional.json",
        vec![
            record(
                ME,
                "claude-opus-5",
                "100.400000",
                "2026-06-26T00:00:00Z",
                "2026-06-27T00:00:00Z",
            ),
            record(
                ME,
                "claude-sonnet-5",
                "200.400000",
                "2026-06-26T00:00:00Z",
                "2026-06-27T00:00:00Z",
            ),
            record(
                ME,
                "claude-haiku-4-5",
                "300.400000",
                "2026-06-26T00:00:00Z",
                "2026-06-27T00:00:00Z",
            ),
        ],
    );

    let out = fold(&export, Some(ME), &report).unwrap();
    let rows: i64 = out
        .by_model
        .iter()
        .map(|r| (r.billed.replace(['$', ','], "").parse::<f64>().unwrap() * 100.0).round() as i64)
        .sum();
    let total = (out.billed.replace(['$', ','], "").parse::<f64>().unwrap() * 100.0).round() as i64;
    assert_eq!(
        rows,
        total,
        "the by-model rows must add up to the billed total they sit under: rows={:?} total={}",
        out.by_model.iter().map(|r| &r.billed).collect::<Vec<_>>(),
        out.billed
    );
}
