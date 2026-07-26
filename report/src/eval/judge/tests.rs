#![allow(clippy::unwrap_used)]

use super::*;
use crate::summarize::Transport;
use std::cell::RefCell;
use std::collections::BTreeMap;

/// A spec carrying nothing but the floors under test.
fn spec_with(floors: BTreeMap<Dimension, u8>) -> Spec {
    Spec {
        floors,
        ..Spec::default()
    }
}

/// A transport that returns a canned reply and records what it was asked. The judge's own logic
/// (prompt, brief, parse, floors) is fully testable this way; what a real model scores is measured
/// by `otto eval`, not asserted here.
struct Canned {
    reply: String,
    seen: RefCell<Vec<String>>,
}

impl Canned {
    fn new(reply: &str) -> Self {
        Self {
            reply: reply.to_string(),
            seen: RefCell::new(Vec::new()),
        }
    }
}

impl Transport for Canned {
    fn complete(&self, job: Job<'_>, system: &str, prompt: &str, json_body: &str) -> Result<String> {
        assert_eq!(job.kind, Kind::Judge, "the judge must run as its own job kind");
        self.seen.borrow_mut().push(format!("{system}\n{prompt}\n{json_body}"));
        Ok(self.reply.clone())
    }
}

fn verdict_json(citation: u8, coverage: u8, prohibition: u8, readability: u8) -> String {
    format!(
        r#"{{"citation-accuracy":{{"score":{citation},"reason":"c"}},
            "coverage":{{"score":{coverage},"reason":"the top by-repo row is never named"}},
            "prohibition-compliance":{{"score":{prohibition},"reason":"p"}},
            "readability":{{"score":{readability},"reason":"r"}}}}"#
    )
}

const CONTEXT: &str = r#"{
  "period": {"since": "2026-04-01", "until": "2026-04-30", "days": 30, "active-days": 21},
  "totals": {"sessions": 44, "spend": "$1,234.56"},
  "basis": {"note": "modeled at published list rates"},
  "unit-costs": {"per-commit": "$12.00"},
  "attribution": {"covered": "$1,234.56"},
  "reconciliation-status": "no authoritative export was supplied",
  "outcomes": {"totals": {"commits": 20}},
  "aggregates": {"by-day": [{"date": "2026-04-01", "spend": "$44.00", "active": true}], "by-repo": [
    {"repo": "northwind-media/beacon", "spend": "$700.00"},
    {"repo": "northwind-media/tideline", "spend": "$300.00"},
    {"repo": "jrivera/sextant", "spend": "$200.00"},
    {"repo": "openpipe-oss/quill", "spend": "$34.56"}
  ]},
  "efficiency": {"agent-type-costs": [{"name": "(main-session)", "spend": "$900.00"}]}
}"#;

/// The brief carries exactly the coverage targets the rubric grades against: the top THREE by-repo
/// rows and the top agent type. A fourth row would make the coverage floor unmeetable; two would
/// make it weaker than the design specifies.
#[test]
fn the_brief_carries_the_top_three_repos_and_the_top_agent_type() {
    let brief = brief(CONTEXT).unwrap();
    let json = serde_json::to_string(&brief).unwrap();
    assert!(json.contains("northwind-media/beacon"));
    assert!(json.contains("northwind-media/tideline"));
    assert!(json.contains("jrivera/sextant"));
    assert!(json.contains("(main-session)"));
    assert!(json.contains("no authoritative export was supplied"));
}

/// The brief carries the WHOLE context. Two narrower briefs each mis-scored on their first real
/// run: a hand-picked subset made legitimate by-day and reconciliation figures read as unsupported,
/// and dropping `sessions[]` made legitimate citations of sessions below the outlier cut read as
/// fabricated.
///
/// BITES: drop any part of the context from `Brief::context` and one of these assertions fails.
#[test]
fn the_brief_carries_the_whole_context() {
    let context = format!(
        "{{{}, \"sessions\": [{{\"short-id\": \"deadbeef\"}}]}}",
        CONTEXT.trim().trim_start_matches('{').trim_end_matches('}')
    );
    let brief = brief(&context).unwrap();
    let json = serde_json::to_string(&brief).unwrap();
    assert!(
        json.contains("openpipe-oss/quill"),
        "a fourth by-repo row is still in the context"
    );
    assert!(json.contains("by-day"), "by-day is in the context");
    assert!(
        json.contains("deadbeef"),
        "the per-session list is what makes a session citation checkable: {json}"
    );
}

/// The coverage targets are `null` / empty rather than missing when the context has none, so the
/// rubric's two named fields always exist.
#[test]
fn absent_coverage_targets_are_null_rather_than_omitted() {
    let brief = brief(r#"{"period": {"days": 7}}"#).unwrap();
    let json = serde_json::to_string(&brief).unwrap();
    assert!(json.contains(r#""top-agent-type":null"#), "{json}");
    assert!(json.contains(r#""top-by-repo":[]"#), "{json}");
}

/// Success criterion 4, at the harness level: a verdict whose coverage misses the top `by-repo` row
/// lands below the fixture's floor, and `regressions` reports it. What a real judge scores on a real
/// artifact is measured by `otto eval`; what is asserted here is that a below-floor score is caught
/// rather than rounded up.
///
/// BITES: change `regressions`' comparison to `<=` or drop the floor lookup and this passes an
/// artifact that missed the top row.
#[test]
fn a_coverage_score_below_the_floor_is_a_regression() {
    let transport = Canned::new(&verdict_json(3, 1, 3, 3));
    let brief = brief(CONTEXT).unwrap();
    let verdict = score(&transport, "test-model", 1_024, "# artifact", &brief).unwrap();

    let spec = spec_with(BTreeMap::from([
        (Dimension::Coverage, 2),
        (Dimension::CitationAccuracy, 3),
    ]));
    let regressions = verdict.regressions(&spec);
    assert_eq!(regressions.len(), 1, "{regressions:?}");
    assert_eq!(regressions[0], (Dimension::Coverage, 1, 2));
    assert!(regressions[0].0.as_str() == "coverage");
}

/// A verdict that clears every floor reports no regression, including on dimensions the spec sets
/// no floor for (an unset floor is zero, so nothing can fall below it).
#[test]
fn a_verdict_at_or_above_every_floor_is_clean() {
    let transport = Canned::new(&verdict_json(3, 2, 3, 2));
    let brief = brief(CONTEXT).unwrap();
    let verdict = score(&transport, "test-model", 1_024, "# artifact", &brief).unwrap();
    let spec = spec_with(BTreeMap::from([(Dimension::Coverage, 2), (Dimension::Readability, 2)]));
    assert!(verdict.regressions(&spec).is_empty());
}

/// The judge is handed the artifact AND the brief, and the rubric names all four dimensions. A
/// rubric that drifted from `Dimension::ALL` would score something the floors do not gate.
#[test]
fn the_prompt_and_body_carry_the_rubric_and_the_artifact() {
    let transport = Canned::new(&verdict_json(3, 3, 3, 3));
    let brief = brief(CONTEXT).unwrap();
    score(&transport, "test-model", 1_024, "# the artifact body", &brief).unwrap();
    let seen = transport.seen.borrow();
    let sent = seen.first().expect("one call");
    for dimension in Dimension::ALL {
        assert!(sent.contains(dimension.as_str()), "the rubric must name {dimension:?}");
    }
    assert!(sent.contains("the artifact body"));
    assert!(sent.contains("top-by-repo"));
}

/// A reply wrapped in prose or a fence still parses: the model is told to emit bare JSON, and a
/// run that failed on a stray "Here is the verdict:" would burn a paid call for nothing.
#[test]
fn a_fenced_or_prefixed_reply_still_parses() {
    let wrapped = format!("Here is the verdict:\n```json\n{}\n```\n", verdict_json(3, 2, 3, 2));
    let verdict = parse(&wrapped).unwrap();
    assert_eq!(verdict.coverage.score, 2);
}

/// An unparseable verdict is a LOUD error. Defaulting it to a passing score would report quality
/// the eval never measured, which is the failure this whole phase exists to remove.
#[test]
fn an_unparseable_reply_is_an_error() {
    for reply in ["", "no json here", r#"{"coverage": {"score": 2}}"#] {
        assert!(parse(reply).is_err(), "{reply:?} must not parse");
    }
}

/// A score outside the 0..=3 scale is rejected rather than compared against a floor it cannot
/// meaningfully clear.
#[test]
fn a_score_above_the_scale_is_rejected() {
    let err = parse(&verdict_json(9, 3, 3, 3)).unwrap_err().to_string();
    assert!(err.contains("citation-accuracy"), "{err}");
}

/// A missing dimension is a parse error, never a silent zero: the floors gate all four.
#[test]
fn a_missing_dimension_is_rejected() {
    let partial = r#"{"citation-accuracy":{"score":3,"reason":"c"},"coverage":{"score":3,"reason":"c"}}"#;
    assert!(parse(partial).is_err());
}
