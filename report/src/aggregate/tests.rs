#![allow(clippy::unwrap_used)]

use super::*;
use crate::report::{ModelTokens, SessionEntry, Totals};
use chrono::{DateTime, Utc};
use claude_pricing::Pricing;

fn ts(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

fn pricing() -> Pricing {
    Pricing::embedded()
}

fn tokens(total: u64, spend_usd: Option<f64>) -> ModelTokens {
    ModelTokens {
        input: total,
        output: 0,
        cache_5m_write: 0,
        cache_1h_write: 0,
        cache_read: 0,
        total,
        spend_usd,
    }
}

fn entry(
    title: Option<&str>,
    repo: Option<&str>,
    begin: DateTime<Utc>,
    model: &str,
    total_tokens: u64,
    spend_usd: Option<f64>,
) -> SessionEntry {
    let mut models = BTreeMap::new();
    models.insert(model.to_string(), tokens(total_tokens, spend_usd));
    SessionEntry {
        title: title.map(str::to_string),
        repo: repo.map(str::to_string),
        begin,
        end: begin,
        spend_usd,
        untracked_models: if spend_usd.is_none() {
            vec![model.to_string()]
        } else {
            Vec::new()
        },
        jsonl_paths: Vec::new(),
        models,
    }
}

fn report_with(since: &str, until: &str, sessions: Vec<(&str, SessionEntry)>) -> Report {
    let mut map = BTreeMap::new();
    let mut totals_spend = 0.0;
    for (sid, e) in sessions {
        totals_spend += e.spend_usd.unwrap_or(0.0);
        map.insert(sid.to_string(), e);
    }
    Report {
        schema_version: 1,
        generated: ts("2026-07-01T00:00:00Z"),
        host: "desk".into(),
        since: ts(since),
        until: ts(until),
        totals: Totals {
            sessions: map.len(),
            spend_usd: totals_spend,
            untracked_models: Vec::new(),
            models: BTreeMap::new(),
        },
        sessions: map,
    }
}

#[test]
fn by_org_sums_equal_totals_and_buckets_unattributed() {
    let report = report_with(
        "2026-06-01T00:00:00Z",
        "2026-07-01T00:00:00Z",
        vec![
            (
                "s1",
                entry(
                    Some("a"),
                    Some("tatari-tv/clyde"),
                    ts("2026-06-05T10:00:00Z"),
                    "claude-opus-4-7",
                    1_000,
                    Some(1.0),
                ),
            ),
            (
                "s2",
                entry(
                    Some("b"),
                    Some("tatari-tv/philo"),
                    ts("2026-06-06T10:00:00Z"),
                    "claude-opus-4-7",
                    2_000,
                    Some(2.0),
                ),
            ),
            (
                "s3",
                entry(
                    Some("c"),
                    Some("scottidler/loopr"),
                    ts("2026-06-07T10:00:00Z"),
                    "claude-sonnet-4-6",
                    500,
                    Some(0.5),
                ),
            ),
            (
                "s4",
                entry(
                    None,
                    None,
                    ts("2026-06-08T10:00:00Z"),
                    "claude-sonnet-4-6",
                    100,
                    Some(0.1),
                ),
            ),
        ],
    );

    let aggregates = compute(&report, DEFAULT_OUTLIERS, &pricing());

    let sum_sessions: usize = aggregates.by_org.iter().map(|r| r.sessions).sum();
    let sum_tokens: u64 = aggregates.by_org.iter().map(|r| r.tokens).sum();
    let sum_spend: f64 = aggregates.by_org.iter().map(|r| r.spend_raw).sum();
    assert_eq!(sum_sessions, report.totals.sessions);
    assert_eq!(sum_tokens, 3_600);
    assert!((sum_spend - report.totals.spend_usd).abs() < 1e-9);

    let tatari = aggregates.by_org.iter().find(|r| r.org == "tatari-tv").unwrap();
    assert_eq!(tatari.repos, 2);
    assert_eq!(tatari.sessions, 2);
    assert_eq!(tatari.tokens, 3_000);
    assert_eq!(tatari.spend, "$3.00");

    let unattributed = aggregates
        .by_org
        .iter()
        .find(|r| r.org == UNATTRIBUTED_ORG)
        .expect("(unattributed) bucket must exist for repo:None sessions");
    assert_eq!(unattributed.sessions, 1);
    assert_eq!(unattributed.repos, 0);
}

#[test]
fn by_repo_excludes_none_repo_and_sorts_by_spend_descending() {
    let report = report_with(
        "2026-06-01T00:00:00Z",
        "2026-07-01T00:00:00Z",
        vec![
            (
                "s1",
                entry(
                    Some("cheap"),
                    Some("tatari-tv/clyde"),
                    ts("2026-06-05T10:00:00Z"),
                    "claude-opus-4-7",
                    100,
                    Some(1.0),
                ),
            ),
            (
                "s2",
                entry(
                    Some("expensive"),
                    Some("tatari-tv/philo"),
                    ts("2026-06-06T10:00:00Z"),
                    "claude-opus-4-7",
                    100,
                    Some(9.0),
                ),
            ),
            (
                "s3",
                entry(
                    None,
                    None,
                    ts("2026-06-07T10:00:00Z"),
                    "claude-opus-4-7",
                    100,
                    Some(0.1),
                ),
            ),
        ],
    );

    let aggregates = compute(&report, DEFAULT_OUTLIERS, &pricing());
    assert_eq!(
        aggregates.by_repo.len(),
        2,
        "repo:None session must not appear in by-repo"
    );
    assert_eq!(aggregates.by_repo[0].repo, "tatari-tv/philo");
    assert_eq!(aggregates.by_repo[1].repo, "tatari-tv/clyde");
    assert_eq!(aggregates.by_repo[0].org, "tatari-tv");
    assert_eq!(aggregates.by_repo[0].models, vec!["claude-opus-4-7".to_string()]);
}

#[test]
fn by_day_clamps_boundary_session_into_period_and_preserves_spend_sum() {
    // Session begun BEFORE `since` (May 31) but whose kept entries made it an in-period session:
    // must attribute to `since`'s date (2026-06-01), never to the out-of-period 2026-05-31.
    let report = report_with(
        "2026-06-01T00:00:00Z",
        "2026-07-01T00:00:00Z",
        vec![
            (
                "boundary",
                entry(
                    Some("straddles the boundary"),
                    Some("tatari-tv/clyde"),
                    ts("2026-05-31T23:00:00Z"),
                    "claude-opus-4-7",
                    1_000,
                    Some(3.0),
                ),
            ),
            (
                "mid",
                entry(
                    Some("mid month"),
                    Some("tatari-tv/clyde"),
                    ts("2026-06-15T10:00:00Z"),
                    "claude-sonnet-4-6",
                    500,
                    Some(1.5),
                ),
            ),
        ],
    );

    let aggregates = compute(&report, DEFAULT_OUTLIERS, &pricing());

    let since_date = report.since.date_naive();
    let until_date = report.until.date_naive();
    for row in &aggregates.by_day {
        let d = chrono::NaiveDate::parse_from_str(&row.date, "%Y-%m-%d").unwrap();
        assert!(
            d >= since_date && d <= until_date,
            "by-day date {} must lie within [{}, {}]",
            row.date,
            since_date,
            until_date
        );
    }
    assert!(
        aggregates.by_day.iter().any(|r| r.date == "2026-06-01"),
        "boundary session must clamp to the period start date, not 2026-05-31"
    );
    assert!(!aggregates.by_day.iter().any(|r| r.date == "2026-05-31"));

    let sum: f64 = aggregates.by_day.iter().map(|r| r.spend_raw).sum();
    assert!(
        (sum - report.totals.spend_usd).abs() < 1e-9,
        "sum(by-day spend) must equal totals.spend: {} vs {}",
        sum,
        report.totals.spend_usd
    );
}

#[test]
fn outliers_are_sorted_by_spend_and_truncated_to_n() {
    let report = report_with(
        "2026-06-01T00:00:00Z",
        "2026-07-01T00:00:00Z",
        vec![
            (
                "aaaaaaaa-0000-0000-0000-000000000000",
                entry(
                    Some("low"),
                    Some("tatari-tv/clyde"),
                    ts("2026-06-05T10:00:00Z"),
                    "claude-opus-4-7",
                    100,
                    Some(1.0),
                ),
            ),
            (
                "bbbbbbbb-0000-0000-0000-000000000000",
                entry(
                    Some("high"),
                    Some("tatari-tv/clyde"),
                    ts("2026-06-06T10:00:00Z"),
                    "claude-opus-4-7",
                    100,
                    Some(9.0),
                ),
            ),
            (
                "cccccccc-0000-0000-0000-000000000000",
                entry(
                    None,
                    Some("tatari-tv/clyde"),
                    ts("2026-06-07T10:00:00Z"),
                    "claude-opus-4-7",
                    100,
                    None,
                ),
            ),
        ],
    );

    let all = compute(&report, DEFAULT_OUTLIERS, &pricing());
    assert_eq!(all.outliers.len(), 3);
    assert_eq!(all.outliers[0].short_id, "bbbbbbbb");
    assert_eq!(all.outliers[0].spend, "$9.00");
    assert_eq!(all.outliers[1].short_id, "aaaaaaaa");
    assert_eq!(all.outliers[2].short_id, "cccccccc");
    assert_eq!(all.outliers[2].spend, "(untracked)");
    assert_eq!(all.outliers[2].title, None);

    let capped = compute(&report, 1, &pricing());
    assert_eq!(capped.outliers.len(), 1);
    assert_eq!(capped.outliers[0].short_id, "bbbbbbbb");
}

#[test]
fn compute_on_empty_report_yields_empty_aggregates() {
    let report = report_with("2026-06-01T00:00:00Z", "2026-07-01T00:00:00Z", vec![]);
    let aggregates = compute(&report, DEFAULT_OUTLIERS, &pricing());
    assert!(aggregates.by_org.is_empty());
    assert!(aggregates.by_repo.is_empty());
    assert!(aggregates.by_day.is_empty());
    assert!(aggregates.outliers.is_empty());
    // No models -> zero-denominator share is "0.0%", and with no cache-bearing unpriced model the
    // counterfactual is still defined (a $0.00 baseline), not absent.
    assert_eq!(aggregates.cache.cache_read_share, "0.0%");
    assert_eq!(aggregates.cache.list_price_equivalent.as_deref(), Some("$0.00"));
}

/// A single model whose tokens fold cleanly: build a totals rollup with cache tokens, price it
/// against the embedded feed, and read the rates back via the public `lookup` so the hand-computed
/// value tracks the feed (never a hardcoded rate) yet still exercises the folding formula
/// independently of `compute`.
fn model_tokens(
    input: u64,
    output: u64,
    cache_read: u64,
    cache_5m: u64,
    cache_1h: u64,
    spend: Option<f64>,
) -> ModelTokens {
    ModelTokens {
        input,
        output,
        cache_5m_write: cache_5m,
        cache_1h_write: cache_1h,
        cache_read,
        total: input + output + cache_read + cache_5m + cache_1h,
        spend_usd: spend,
    }
}

fn report_with_totals(models: Vec<(&str, ModelTokens)>, actual_spend: f64) -> Report {
    let mut totals_models = BTreeMap::new();
    for (name, mt) in models {
        totals_models.insert(name.to_string(), mt);
    }
    Report {
        schema_version: 1,
        generated: ts("2026-07-01T00:00:00Z"),
        host: "desk".into(),
        since: ts("2026-06-01T00:00:00Z"),
        until: ts("2026-07-01T00:00:00Z"),
        totals: Totals {
            sessions: 0,
            spend_usd: actual_spend,
            untracked_models: Vec::new(),
            models: totals_models,
        },
        sessions: BTreeMap::new(),
    }
}

#[test]
fn cache_counterfactual_equals_hand_computed_value() {
    let p = pricing();
    // Token counts chosen small so folded input stays under the 200k long-context tier: the
    // standard per-mtok rates apply and the hand computation is a plain linear formula.
    let input = 1_000u64;
    let output = 500u64;
    let cache_read = 9_000u64;
    let cache_5m = 0u64;
    let cache_1h = 0u64;
    let actual_spend = 0.12_f64;
    let report = report_with_totals(
        vec![(
            "claude-opus-4-7",
            model_tokens(input, output, cache_read, cache_5m, cache_1h, Some(actual_spend)),
        )],
        actual_spend,
    );

    let aggregates = compute(&report, DEFAULT_OUTLIERS, &p);

    // cache-read-share = cache_read / (input + cache_read + cache_5m + cache_1h)
    assert_eq!(aggregates.cache.cache_read_share, "90.0%");
    assert_eq!(aggregates.cache.input_tokens_human, "1,000");
    assert_eq!(aggregates.cache.cache_read_tokens_human, "9,000");

    // Hand-compute the counterfactual from the feed's own published rates: ALL cache tokens folded
    // into input, cache fields zeroed. Rates read via the public API so this tracks the feed.
    let rates = p
        .lookup("claude-opus-4-7")
        .expect("opus-4-7 is priced in the embedded feed");
    let folded_input = input + cache_read + cache_5m + cache_1h;
    let expected_list_price =
        folded_input as f64 * rates.input_per_mtok / 1_000_000.0 + output as f64 * rates.output_per_mtok / 1_000_000.0;

    assert_eq!(
        aggregates.cache.list_price_equivalent.as_deref(),
        Some(format_usd(expected_list_price).as_str())
    );
    assert_eq!(
        aggregates.cache.cache_savings.as_deref(),
        Some(format_usd(expected_list_price - actual_spend).as_str())
    );
}

#[test]
fn cache_counterfactual_absent_when_a_cache_bearing_model_is_unpriced() {
    let report = report_with_totals(
        vec![(
            "claude-does-not-exist-9",
            // Nonzero cache reads on an unpriced model: the whole counterfactual is unknowable.
            model_tokens(1_000, 500, 4_000, 0, 0, None),
        )],
        0.0,
    );

    let aggregates = compute(&report, DEFAULT_OUTLIERS, &pricing());

    assert_eq!(aggregates.cache.list_price_equivalent, None);
    assert_eq!(aggregates.cache.cache_savings, None);

    // The Option::is_none skip means the keys are ABSENT from the serialized context, never "$0".
    let json = serde_json::to_value(&aggregates.cache).unwrap();
    assert!(json.get("list-price-equivalent").is_none());
    assert!(json.get("cache-savings").is_none());
    // The non-pricing fields are still present regardless.
    assert!(json.get("cache-read-share").is_some());
}

#[test]
fn cache_counterfactual_present_when_unpriced_model_has_no_cache_tokens() {
    let p = pricing();
    // A priced cache-bearing model plus an unpriced model with ZERO cache tokens: the latter does
    // not nullify the counterfactual (only cache-bearing unpriced models do).
    let report = report_with_totals(
        vec![
            ("claude-opus-4-7", model_tokens(1_000, 500, 9_000, 0, 0, Some(0.12))),
            ("claude-does-not-exist-9", model_tokens(1_000, 0, 0, 0, 0, None)),
        ],
        0.12,
    );

    let aggregates = compute(&report, DEFAULT_OUTLIERS, &p);
    assert!(aggregates.cache.list_price_equivalent.is_some());
    assert!(aggregates.cache.cache_savings.is_some());
}
