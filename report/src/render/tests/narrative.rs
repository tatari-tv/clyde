//! Phase 9 (narrative evidence: `summary`/`tags` passthrough, `enrichment-coverage`): tests split
//! out per the file-size limit.
use super::*;

/// Phase 9's prompt-edit ledger: BOTH templates flip the narrative evidence source from `title`
/// (Claude Code's `ai-title`, resolved from a session's OPENING exchange) to `summary` (the enrich
/// pass's digest of the FULL transcript), document the `enrichment-coverage` context field, and
/// state the fallback to `title` for an unenriched session.
///
/// BITES: revert either template to citing "session titles" as theme evidence, or drop the
/// `enrichment-coverage` documentation, and the matching assertion fails.
#[test]
fn both_templates_cite_summary_over_title_and_document_enrichment_coverage() {
    for (name, tpl) in [("report.pmt", DEFAULT_PROMPT), ("report-html.pmt", DEFAULT_HTML_PROMPT)] {
        assert!(
            tpl.contains("`ai-title`"),
            "{name} must name the defect: title is Claude Code's own ai-title"
        );
        assert!(
            tpl.contains("OPENING exchange"),
            "{name} must state that title is resolved from the opening exchange only"
        );
        assert!(
            tpl.contains("is the evidence a theme claim should cite") || tpl.contains("is the evidence a theme"),
            "{name} must state that summary, not title, is the evidence for a theme"
        );
        assert!(
            tpl.contains("`title` only as the session's LABEL"),
            "{name} must demote title to a label, never evidence"
        );
        assert!(
            tpl.contains("enrichment-coverage"),
            "{name} must document the enrichment-coverage context field"
        );
        assert!(
            tpl.contains("fall back to") && tpl.contains("`title`"),
            "{name} must state the fallback to title for an unenriched session"
        );
    }
}

/// `summary`/`tags` reach the context per session, and the unenriched-session case (`summary`
/// `None`) OMITS both keys rather than emitting `null`/`[]` -- the contrast the prompt's "fall back
/// to title" instruction depends on being visible in the JSON, not just implied.
#[test]
fn build_context_block_carries_summary_and_tags_when_enriched() {
    let mut report = sample_report();
    {
        let entry = report
            .sessions
            .get_mut("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042")
            .expect("the titled fixture session");
        entry.summary = Some("shipped the collect-once render-from-data migration".into());
        entry.tags = vec!["report".into(), "migration".into()];
    }
    let block = ctx(&report, false);
    let parsed: serde_json::Value = serde_json::from_str(&block).expect("must be valid JSON");
    let sessions = parsed
        .get("sessions")
        .and_then(|v| v.as_array())
        .expect("sessions list");

    let enriched = sessions
        .iter()
        .find(|s| s.get("title").and_then(|v| v.as_str()) == Some("ship the report tool"))
        .expect("the enriched session");
    assert_eq!(
        enriched.get("summary").and_then(|v| v.as_str()),
        Some("shipped the collect-once render-from-data migration")
    );
    assert_eq!(enriched.get("tags").and_then(|v| v.as_array()).map(Vec::len), Some(2));

    let unenriched = sessions
        .iter()
        .find(|s| s.get("title").and_then(|v| v.as_str()).is_none())
        .expect("the untitled/unenriched fixture session");
    assert!(
        unenriched.get("summary").is_none(),
        "an unenriched session must omit summary, not emit null: {unenriched}"
    );
    assert!(
        unenriched.get("tags").is_none(),
        "an unenriched session must omit tags, not emit an empty array: {unenriched}"
    );
}

/// `enrichment-coverage` is a single quotable string counting exactly the sessions carried into
/// THIS context (not a separate collect-time sample), so the model's "N of M carry a summary" claim
/// can never drift from what it can actually see.
#[test]
fn build_context_block_carries_enrichment_coverage() {
    let mut report = sample_report();
    {
        let entry = report
            .sessions
            .get_mut("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042")
            .expect("the titled fixture session");
        entry.summary = Some("did the actual work".into());
    }
    let block = ctx(&report, false);
    let parsed: serde_json::Value = serde_json::from_str(&block).expect("must be valid JSON");
    let coverage = parsed
        .get("enrichment-coverage")
        .and_then(|v| v.as_str())
        .expect("enrichment-coverage key");
    assert!(coverage.contains("1 of 2"), "got: {coverage}");
    assert!(coverage.contains("50.0%"), "got: {coverage}");
    assert!(coverage.contains("cited by title only"), "got: {coverage}");
}

/// An empty window has no enrichment coverage to be short of; the message still names "0 of 0"
/// rather than dividing by zero or omitting the field.
#[test]
fn build_enrichment_coverage_on_an_empty_report_names_zero_of_zero() {
    let report = Report {
        schema_version: 2,
        generated: ts("2026-04-27T19:42:08Z"),
        host: "desk".into(),
        since: ts("2026-04-01T00:00:00Z"),
        until: ts("2026-04-30T00:00:00Z"),
        outcomes_enabled: None,
        notes: Vec::new(),
        totals: totals(0, 0.0, BTreeMap::new()),
        sessions: BTreeMap::new(),
    };
    assert_eq!(
        build_enrichment_coverage(&report),
        "0 of 0 sessions carry an enrich summary"
    );
}
