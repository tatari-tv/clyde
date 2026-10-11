#![allow(clippy::unwrap_used)]

use super::*;
use crate::report::{ModelTokens, SessionEntry, Totals};
use chrono::{DateTime, Utc};
use claude_pricing::Pricing;
use efficiency::{RawCounters, SessionEfficiency, finalize};

fn ts(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

fn pricing() -> Pricing {
    Pricing::embedded()
}

/// An empty v2 efficiency passthrough -- aggregate rollups read tokens/spend from `models`, never the
/// efficiency object, so an all-zero scope is enough for these fixtures.
fn empty_efficiency() -> SessionEfficiency {
    SessionEfficiency {
        session_id: "x".into(),
        aggregate: finalize(RawCounters::default()),
        subagents: Vec::new(),
        flags: Vec::new(),
    }
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
        summary: None,
        tags: Vec::new(),
        repo: repo.map(str::to_string),
        repo_source: repo.map(|_| RepoSource::GitOrigin.as_str().to_string()),
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
        by_day: None,
        outcomes: None,
        agent_type_costs: BTreeMap::new(),
        cache_read_share: None,
        tool_error_rate: None,
        cache_1h_write_fraction: None,
        interrupts: 0,
        compactions: 0,
        by_skill: BTreeMap::new(),
        by_mcp: BTreeMap::new(),
        efficiency: empty_efficiency(),
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
        schema_version: crate::report::SCHEMA_VERSION,
        generated: ts("2026-07-01T00:00:00Z"),
        host: "desk".into(),
        since: ts(since),
        until: ts(until),
        outcomes_enabled: None,
        notes: Vec::new(),
        totals: Totals {
            sessions: map.len(),
            spend_usd: totals_spend,
            untracked_models: Vec::new(),
            models: BTreeMap::new(),
            outcomes: None,
            cache_read_share: None,
            tool_error_rate: None,
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

/// Inverts `by_day_excludes_pre_since_session_into_carried_in_row`: the carried-in row is gone.
/// A session with no per-day buckets still sits on its begin day, and one that began before
/// `since` has its spend in the OUT-OF-WINDOW remainder (never clamped onto `since`, never on a
/// row), so the rows plus the remainder equal the headline to the cent.
#[test]
fn by_day_puts_an_unbucketed_pre_since_session_out_of_window() {
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
    assert!(
        !aggregates.by_day.iter().any(|r| r.date == "2026-05-31"),
        "the out-of-period date must never appear as a by-day row"
    );
    let since_row = aggregates.by_day.iter().find(|r| r.date == "2026-06-01").unwrap();
    assert_eq!(since_row.sessions, 0, "never clamped onto the since row");
    assert!(!since_row.active);

    let c = &aggregates.day_coverage;
    assert_eq!((c.out_of_window_sessions, c.out_of_window.as_str()), (1, "$3.00"));
    assert_eq!((c.unbucketed_sessions, c.unbucketed_spend.as_str()), (2, "$4.50"));
    assert_eq!(c.in_window, "$1.50");
    let rows: i64 = aggregates.by_day.iter().map(|r| r.spend_cents).sum();
    assert_eq!(rows, 150);
    assert_eq!(
        rows + c.out_of_window_cents,
        crate::cents::to_cents(report.totals.spend_usd)
    );
}

/// A day-bucketed entry: tokens and cents per (day, model).
fn bucketed(begin: &str, days: &[(&str, &str, u64, i64)]) -> SessionEntry {
    let total_cents: i64 = days.iter().map(|d| d.3).sum();
    let mut e = entry(
        Some("bucketed"),
        Some("tatari-tv/clyde"),
        ts(begin),
        days[0].1,
        0,
        Some(total_cents as f64 / 100.0),
    );
    let mut by_day: BTreeMap<chrono::NaiveDate, BTreeMap<String, crate::report::DayModel>> = BTreeMap::new();
    e.models.clear();
    for (day, model, cache_read, cents) in days {
        by_day.entry(day.parse().unwrap()).or_default().insert(
            (*model).to_string(),
            crate::report::DayModel {
                input: 10,
                cache_read: *cache_read,
                spend_cents: *cents,
                ..Default::default()
            },
        );
        let m = e.models.entry((*model).to_string()).or_default();
        m.input += 10;
        m.cache_read += cache_read;
        m.total += 10 + cache_read;
        m.spend_usd = Some(m.spend_usd.unwrap_or(0.0) + *cents as f64 / 100.0);
    }
    e.by_day = Some(by_day);
    e
}

/// THE fix: a multi-day session's spend lands on the days its turns ran, per model, a turn after
/// `until` lands in the remainder, and the day's cache-read share is computed from its own tokens.
/// BITES: put a bucketed session back on its begin day and the June 3 row reads $0.00.
#[test]
fn by_day_puts_each_turn_on_its_day_and_the_rest_out_of_window() {
    let report = report_with(
        "2026-06-01T00:00:00Z",
        "2026-06-05T00:00:00Z",
        vec![(
            "multi",
            bucketed(
                "2026-06-02T22:00:00Z",
                &[
                    ("2026-06-02", "claude-opus-4-7", 90, 125),
                    ("2026-06-03", "claude-opus-4-7", 30, 300),
                    ("2026-06-03", "claude-sonnet-4-6", 0, 75),
                    ("2026-06-06", "claude-opus-4-7", 0, 40),
                ],
            ),
        )],
    );
    let aggregates = compute(&report, DEFAULT_OUTLIERS, &pricing());
    let day = |d: &str| aggregates.by_day.iter().find(|r| r.date == d).unwrap();
    assert_eq!(day("2026-06-02").spend, "$1.25");
    assert_eq!(day("2026-06-03").spend, "$3.75");
    assert_eq!(day("2026-06-03").model_cents.get("claude-sonnet-4-6"), Some(&75));
    assert_eq!(day("2026-06-03").model_cents.get("claude-opus-4-7"), Some(&300));
    assert_eq!(
        (day("2026-06-02").sessions, day("2026-06-03").sessions),
        (1, 1),
        "active on both days"
    );
    assert_eq!(day("2026-06-04").sessions, 0);
    assert_eq!(day("2026-06-02").cache_read_share, Some(0.9));
    assert_eq!(day("2026-06-03").cache_read_share, Some(0.6));
    assert_eq!(
        day("2026-06-04").cache_read_share,
        None,
        "no tokens is no share, not zero"
    );

    let c = &aggregates.day_coverage;
    assert_eq!(
        (c.out_of_window_cents, c.out_of_window_sessions, c.unbucketed_sessions),
        (40, 1, 0)
    );
    assert_eq!(c.in_window, "$5.00");
    assert_eq!(
        aggregates.by_day.iter().map(|r| r.spend_cents).sum::<i64>() + c.out_of_window_cents,
        crate::cents::to_cents(report.totals.spend_usd)
    );
}

/// Success criterion (design Phase 4): the `since` row's session count equals only the sessions
/// that actually BEGAN that day, never the sessions carried in from before the window (the real
/// window measured 14 real day-1 sessions against 16 carried-in ones, previously merged into a
/// reported 30).
#[test]
fn since_row_counts_only_sessions_that_began_on_since_date() {
    let carried = |sid: &'static str| {
        (
            sid,
            entry(
                Some("began before since"),
                Some("tatari-tv/clyde"),
                ts("2026-05-20T10:00:00Z"),
                "claude-opus-4-7",
                50,
                Some(0.5),
            ),
        )
    };
    let sessions = vec![
        (
            "since-1",
            entry(
                Some("began on since"),
                Some("tatari-tv/clyde"),
                ts("2026-06-01T01:00:00Z"),
                "claude-opus-4-7",
                100,
                Some(1.0),
            ),
        ),
        (
            "since-2",
            entry(
                Some("also began on since"),
                Some("tatari-tv/clyde"),
                ts("2026-06-01T20:00:00Z"),
                "claude-opus-4-7",
                100,
                Some(1.0),
            ),
        ),
        carried("carried-1"),
        carried("carried-2"),
        carried("carried-3"),
    ];

    let report = report_with("2026-06-01T00:00:00Z", "2026-07-01T00:00:00Z", sessions);
    let aggregates = compute(&report, DEFAULT_OUTLIERS, &pricing());

    let since_row = aggregates
        .by_day
        .iter()
        .find(|r| r.date == "2026-06-01")
        .expect("since row must exist");
    assert_eq!(since_row.sessions, 2, "only sessions that began ON since count here");
    assert_eq!(
        aggregates.day_coverage.out_of_window_sessions, 3,
        "the pre-since sessions land in the out-of-window remainder"
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

/// Inverts the pre-Phase-4 pin: `by-day` used to hold only ACTIVE days, so an empty report gave
/// zero rows. It now zero-fills one row per calendar date in the window regardless of activity
/// (design finding 3, gaps visible), so an empty report still gets a full, all-inactive series.
#[test]
fn compute_on_empty_report_zero_fills_by_day_and_yields_empty_coverage() {
    let report = report_with("2026-06-01T00:00:00Z", "2026-07-01T00:00:00Z", vec![]);
    let aggregates = compute(&report, DEFAULT_OUTLIERS, &pricing());
    assert!(aggregates.by_org.is_empty());
    assert!(aggregates.by_repo.is_empty());
    assert!(aggregates.outliers.is_empty());

    // June 1 through July 1 inclusive: 31 calendar dates, every one a zero-fill row.
    assert_eq!(aggregates.by_day.len(), 31);
    assert!(aggregates.by_day.iter().all(|r| !r.active && r.sessions == 0));
    assert!(aggregates.by_day.iter().any(|r| r.date == "2026-06-01"));
    assert!(aggregates.by_day.iter().any(|r| r.date == "2026-07-01"));

    assert_eq!(aggregates.day_coverage.out_of_window_sessions, 0);
    assert_eq!(aggregates.day_coverage.out_of_window, "$0.00");
    assert_eq!(aggregates.day_coverage.in_window, "$0.00");

    // No models -> zero-denominator share is "0.0%", and with no cache-bearing unpriced model the
    // counterfactual is still defined (a $0.00 baseline), not absent.
    assert_eq!(aggregates.cache.cache_read_share, "0.0%");
    assert_eq!(aggregates.cache.list_price_equivalent.as_deref(), Some("$0.00"));
}

/// Design "Chart truthfulness" success criterion: a `[200, 100, 50]` spend series must yield
/// `spend-percent-of-max` `[100.0, 50.0, 25.0]`, one decimal, and the kebab-case JSON key.
/// One repo per org keeps `by-org` and `by-repo` numerically identical so both series are checked
/// from a single fixture.
#[test]
fn spend_percent_of_max_known_values_fixture() {
    let report = report_with(
        "2026-06-01T00:00:00Z",
        "2026-07-01T00:00:00Z",
        vec![
            (
                "s1",
                entry(
                    Some("high"),
                    Some("org-a/x"),
                    ts("2026-06-05T10:00:00Z"),
                    "claude-opus-4-7",
                    100,
                    Some(200.0),
                ),
            ),
            (
                "s2",
                entry(
                    Some("mid"),
                    Some("org-b/y"),
                    ts("2026-06-06T10:00:00Z"),
                    "claude-opus-4-7",
                    100,
                    Some(100.0),
                ),
            ),
            (
                "s3",
                entry(
                    Some("low"),
                    Some("org-c/z"),
                    ts("2026-06-07T10:00:00Z"),
                    "claude-opus-4-7",
                    100,
                    Some(50.0),
                ),
            ),
        ],
    );

    let aggregates = compute(&report, DEFAULT_OUTLIERS, &pricing());

    let by_org: Vec<Option<f64>> = aggregates.by_org.iter().map(|r| r.spend_percent_of_max).collect();
    assert_eq!(by_org, vec![Some(100.0), Some(50.0), Some(25.0)]);

    let by_repo: Vec<Option<f64>> = aggregates.by_repo.iter().map(|r| r.spend_percent_of_max).collect();
    assert_eq!(by_repo, vec![Some(100.0), Some(50.0), Some(25.0)]);

    // Serialized JSON uses the kebab-case key.
    let json = serde_json::to_value(&aggregates.by_org[0]).unwrap();
    assert_eq!(json.get("spend-percent-of-max").and_then(|v| v.as_f64()), Some(100.0));
}

/// A zero-spend series (every session untracked) has no meaningful proportion: the field must be
/// `None` in Rust and ABSENT from the serialized JSON, never a fabricated `0.0`.
#[test]
fn spend_percent_of_max_absent_when_series_max_is_zero() {
    let report = report_with(
        "2026-06-01T00:00:00Z",
        "2026-07-01T00:00:00Z",
        vec![(
            "s1",
            entry(
                Some("untracked"),
                Some("org-a/x"),
                ts("2026-06-05T10:00:00Z"),
                "claude-does-not-exist-9",
                100,
                None,
            ),
        )],
    );

    let aggregates = compute(&report, DEFAULT_OUTLIERS, &pricing());
    assert_eq!(aggregates.by_org[0].spend_percent_of_max, None);
    assert_eq!(aggregates.by_repo[0].spend_percent_of_max, None);
    assert_eq!(aggregates.by_day[0].spend_percent_of_max, None);

    let json = serde_json::to_value(&aggregates.by_org[0]).unwrap();
    assert!(
        json.get("spend-percent-of-max").is_none(),
        "zero-max series must omit spend-percent-of-max entirely, got: {json}"
    );
}

/// `sessions-percent-of-max` on `DayRow` scales against the max daily session COUNT, independent
/// of spend: a heavier-spend day with fewer sessions must not out-rank a cheaper, busier day.
#[test]
fn sessions_percent_of_max_scales_against_max_daily_session_count() {
    let report = report_with(
        "2026-06-01T00:00:00Z",
        "2026-07-01T00:00:00Z",
        vec![
            (
                "s1",
                entry(
                    Some("busy day, one session"),
                    Some("org-a/x"),
                    ts("2026-06-01T10:00:00Z"),
                    "claude-opus-4-7",
                    100,
                    Some(1.0),
                ),
            ),
            (
                "s2",
                entry(
                    Some("quiet day, two sessions a"),
                    Some("org-a/x"),
                    ts("2026-06-02T10:00:00Z"),
                    "claude-opus-4-7",
                    100,
                    Some(0.1),
                ),
            ),
            (
                "s3",
                entry(
                    Some("quiet day, two sessions b"),
                    Some("org-a/x"),
                    ts("2026-06-02T14:00:00Z"),
                    "claude-opus-4-7",
                    100,
                    Some(0.1),
                ),
            ),
        ],
    );

    let aggregates = compute(&report, DEFAULT_OUTLIERS, &pricing());
    let day1 = aggregates.by_day.iter().find(|r| r.date == "2026-06-01").unwrap();
    let day2 = aggregates.by_day.iter().find(|r| r.date == "2026-06-02").unwrap();
    // day1: 1 session vs max 2 -> 50.0%. day2: 2 sessions == max -> 100.0%.
    assert_eq!(day1.sessions_percent_of_max, Some(50.0));
    assert_eq!(day2.sessions_percent_of_max, Some(100.0));
    // day1 has the higher spend but fewer sessions: spend-percent-of-max must not track sessions.
    assert_eq!(day1.spend_percent_of_max, Some(100.0));
    assert_eq!(day2.spend_percent_of_max, Some(20.0));
}

/// Build an entry with an explicit `repo_source`, for the provenance rollups.
fn entry_sourced(repo: Option<&str>, source: Option<RepoSource>, spend_usd: f64) -> SessionEntry {
    let mut e = entry(
        None,
        repo,
        ts("2026-06-10T10:00:00Z"),
        "claude-opus-4-8",
        100,
        Some(spend_usd),
    );
    e.repo_source = source.map(|s| s.as_str().to_string());
    e
}

/// `attribution` partitions `totals.spend-usd`: one row per `repo-source` plus `(unattributed)`,
/// and the rows sum to the headline figure exactly. BITES: drop a bucket or double-count one and
/// the sum assertion fails.
#[test]
fn attribution_rows_sum_to_totals_spend() {
    let report = report_with(
        "2026-06-01T00:00:00Z",
        "2026-06-30T00:00:00Z",
        vec![
            (
                "a",
                entry_sourced(Some("tatari-tv/clyde"), Some(RepoSource::GitOrigin), 100.0),
            ),
            (
                "b",
                entry_sourced(Some("tatari-tv/clyde"), Some(RepoSource::KnownPath), 50.0),
            ),
            (
                "c",
                entry_sourced(Some("scottidler/sb"), Some(RepoSource::FilesTouched), 25.0),
            ),
            (
                "d",
                entry_sourced(Some("tatari-tv/clyde-ft"), Some(RepoSource::PathGuess), 20.0),
            ),
            ("e", entry_sourced(None, None, 5.0)),
        ],
    );
    let attribution = compute_attribution(&report);

    let summed: f64 = attribution.rows.iter().map(|r| r.spend_raw).sum();
    assert!(
        (summed - report.totals.spend_usd).abs() < 0.005,
        "rows must sum to totals.spend-usd: {summed} vs {}",
        report.totals.spend_usd
    );
    assert_eq!(attribution.covered, "$195.00", "everything but the repo-less session");
    assert_eq!(attribution.uncovered, "$5.00");
    assert_eq!(attribution.covered_share, "97.5%");

    let by_source: BTreeMap<&str, (usize, &str)> = attribution
        .rows
        .iter()
        .map(|r| (r.source.as_str(), (r.sessions, r.confidence.as_str())))
        .collect();
    assert_eq!(by_source["git-origin"], (1, "observed"));
    assert_eq!(by_source["known-path"], (1, "observed"));
    assert_eq!(by_source["files-touched"], (1, "inferred"));
    assert_eq!(by_source["path-guess"], (1, "guessed"));
    assert_eq!(by_source["(unattributed)"], (1, "unattributed"));
}

/// The `(unattributed)` row absorbs the difference between `totals.spend-usd` and the sum of the
/// per-session spends, so the partition holds even when the two pricing bases disagree (a model's
/// long-context tier crossed by the unioned token counts but not by any one session).
#[test]
fn attribution_carries_the_pricing_residual_in_the_unattributed_bucket() {
    let mut report = report_with(
        "2026-06-01T00:00:00Z",
        "2026-06-30T00:00:00Z",
        vec![(
            "a",
            entry_sourced(Some("tatari-tv/clyde"), Some(RepoSource::GitOrigin), 100.0),
        )],
    );
    // The union priced $10 higher than the sessions did on their own.
    report.totals.spend_usd = 110.0;

    let attribution = compute_attribution(&report);
    let summed: f64 = attribution.rows.iter().map(|r| r.spend_raw).sum();
    assert!(
        (summed - 110.0).abs() < 0.005,
        "rows still sum to the headline: {summed}"
    );
    let unattributed = attribution
        .rows
        .iter()
        .find(|r| r.source == UNATTRIBUTED_ORG)
        .expect("the residual creates the bucket");
    assert_eq!(unattributed.spend, "$10.00");
    assert_eq!(unattributed.sessions, 0, "no session is invented to carry it");
}

/// The residual's sign is NOT constrained by the pricing story that justifies it. A merged artifact
/// (or `round_cents` on the headline against unrounded per-session spends) can put the per-session
/// sum ABOVE `totals.spend-usd`, and folding that in creates an `(unattributed)` row holding a
/// NEGATIVE amount: `format_usd` renders `-$X.XX` in the artifact, the sort drops it below every
/// real bucket, and the prose is licensed to quote it. Drop it instead; the rows falling short of
/// the headline is the honest failure.
#[test]
fn attribution_refuses_to_publish_a_negative_residual() {
    let mut report = report_with(
        "2026-06-01T00:00:00Z",
        "2026-06-30T00:00:00Z",
        vec![(
            "a",
            entry_sourced(Some("tatari-tv/clyde"), Some(RepoSource::GitOrigin), 100.0),
        )],
    );
    // The sessions priced $10 HIGHER than the headline.
    report.totals.spend_usd = 90.0;

    let attribution = compute_attribution(&report);
    assert!(
        attribution.rows.iter().all(|r| r.spend_raw >= 0.0),
        "no row may carry a negative amount: {:?}",
        attribution
            .rows
            .iter()
            .map(|r| (&r.source, r.spend_raw))
            .collect::<Vec<_>>()
    );
    assert!(
        !attribution.rows.iter().any(|r| r.spend.starts_with('-')),
        "and none may RENDER as a negative dollar figure the prose could quote"
    );
    assert!(
        !attribution.rows.iter().any(|r| r.source == UNATTRIBUTED_ORG),
        "the negative residual creates no bucket at all"
    );
    // The single real row is untouched -- the drop costs the partition its exactness, not its rows.
    let summed: f64 = attribution.rows.iter().map(|r| r.spend_raw).sum();
    assert!(
        (summed - 100.0).abs() < 0.005,
        "rows keep their measured spend: {summed}"
    );
}

/// The other side of the same guard: an EXISTING `(unattributed)` bucket must not be silently
/// reduced by a negative residual either.
#[test]
fn a_negative_residual_does_not_shrink_an_existing_unattributed_bucket() {
    let mut report = report_with(
        "2026-06-01T00:00:00Z",
        "2026-06-30T00:00:00Z",
        vec![
            (
                "a",
                entry_sourced(Some("tatari-tv/clyde"), Some(RepoSource::GitOrigin), 60.0),
            ),
            ("b", entry_sourced(None, None, 40.0)),
        ],
    );
    report.totals.spend_usd = 90.0;

    let attribution = compute_attribution(&report);
    let unattributed = attribution
        .rows
        .iter()
        .find(|r| r.source == UNATTRIBUTED_ORG)
        .expect("session b has no repo, so the bucket exists on its own merits");
    assert_eq!(
        unattributed.spend, "$40.00",
        "the bucket keeps the spend its own sessions measured"
    );
    assert_eq!(unattributed.sessions, 1);
}

/// A pre-v10 artifact folded in by `report merge` has repos with no provenance. Those are bucketed
/// as `(unknown-source)`, never silently counted as observed.
#[test]
fn attribution_buckets_a_repo_with_no_provenance_as_unknown() {
    let report = report_with(
        "2026-06-01T00:00:00Z",
        "2026-06-30T00:00:00Z",
        vec![("a", entry_sourced(Some("tatari-tv/clyde"), None, 40.0))],
    );
    let attribution = compute_attribution(&report);
    assert_eq!(attribution.rows.len(), 1);
    assert_eq!(attribution.rows[0].source, UNKNOWN_SOURCE);
    assert_eq!(attribution.rows[0].confidence, "unknown");
    assert_eq!(
        attribution.covered, "$40.00",
        "it still counts as covered; it has a repo"
    );
}

/// `by-repo` marks a row with the STRONGEST evidence any of its sessions has for the slug. A repo
/// only ever guessed at is marked `path-guess` (the fabricated-sibling case); a repo with even one
/// observed session is not, however many guesses it also carries.
#[test]
fn by_repo_marks_a_row_with_its_strongest_provenance() {
    let report = report_with(
        "2026-06-01T00:00:00Z",
        "2026-06-30T00:00:00Z",
        vec![
            (
                "a",
                entry_sourced(Some("tatari-tv/clyde"), Some(RepoSource::PathGuess), 10.0),
            ),
            (
                "b",
                entry_sourced(Some("tatari-tv/clyde"), Some(RepoSource::GitOrigin), 90.0),
            ),
            (
                "c",
                entry_sourced(Some("tatari-tv/clyde-ft"), Some(RepoSource::PathGuess), 30.0),
            ),
        ],
    );
    let rows = compute_by_repo(&report);
    let by_repo: BTreeMap<&str, Option<&str>> = rows
        .iter()
        .map(|r| (r.repo.as_str(), r.repo_source.as_deref()))
        .collect();
    assert_eq!(
        by_repo["tatari-tv/clyde"],
        Some("git-origin"),
        "one observed session proves the repo is real"
    );
    assert_eq!(
        by_repo["tatari-tv/clyde-ft"],
        Some("path-guess"),
        "a row nobody ever observed is marked as the guess it is"
    );
}

// ---- Phase 7: by-repo outcomes, output geometry, unit costs ----

/// Attach a session's observed outcomes to a fixture entry.
fn with_outcomes(
    mut e: SessionEntry,
    commits: &[&str],
    pr_urls: &[&str],
    files: u64,
    lines: (u64, u64),
) -> SessionEntry {
    use crate::outcome::{Outcomes, PrRef};
    e.outcomes = Some(Outcomes {
        commits: commits.iter().map(|s| s.to_string()).collect(),
        prs: pr_urls
            .iter()
            .enumerate()
            .map(|(i, url)| PrRef {
                number: i as u64 + 1,
                url: url.to_string(),
                repository: None,
            })
            .collect(),
        confluence_writes: 0,
        jira_writes: 0,
        slack_messages: 0,
        files_edited: files,
        lines_written: lines.0,
        lines_replaced: lines.1,
        ..Default::default()
    });
    e
}

fn outcome_report() -> Report {
    let clyde_a = with_outcomes(
        entry_sourced(Some("tatari-tv/clyde"), Some(RepoSource::GitOrigin), 100.0),
        // `sha-shared` also appears in clyde-b: deduped WITHIN the repo row.
        &["sha-a", "sha-shared"],
        &["https://github.com/tatari-tv/clyde/pull/1"],
        3,
        (120, 40),
    );
    let clyde_b = with_outcomes(
        entry_sourced(Some("tatari-tv/clyde"), Some(RepoSource::GitOrigin), 60.0),
        &["sha-shared", "sha-b"],
        &["https://github.com/tatari-tv/clyde/pull/1"],
        2,
        (30, 10),
    );
    let philo = with_outcomes(
        entry_sourced(Some("tatari-tv/philo"), Some(RepoSource::GitOrigin), 40.0),
        &["sha-c"],
        &[],
        1,
        (10, 0),
    );
    let mut report = report_with(
        "2026-06-01T00:00:00Z",
        "2026-06-10T00:00:00Z",
        vec![("s1", clyde_a), ("s2", clyde_b), ("s3", philo)],
    );
    report.totals.outcomes = Some(crate::outcome::rollup(
        report.sessions.values().map(|e| e.outcomes.as_ref()),
    ));
    report
}

#[test]
fn by_repo_carries_outcomes_deduped_within_each_row() {
    let rows = compute_by_repo(&outcome_report());
    let clyde = rows.iter().find(|r| r.repo == "tatari-tv/clyde").unwrap();
    let outcomes = clyde.outcomes.as_ref().expect("an observed repo carries outcomes");
    assert_eq!(
        outcomes.commits, 3,
        "sha-shared appears in both of the repo's sessions and counts once in the row"
    );
    assert_eq!(outcomes.prs_opened, 1, "the same PR url from two sessions counts once");
    assert_eq!(outcomes.files_edited, 5, "per-session distinct counts sum");
    assert_eq!((outcomes.lines_written, outcomes.lines_replaced), (150, 50));
}

/// The phase's success criterion: the GLOBAL dedupe over the by-repo rows matches
/// `totals.outcomes` for commits and PRs, so a commit touching two repos does not double-count
/// into the total. Asserted by rebuilding the global rollup from the same attributed sessions and
/// comparing it against the deduped totals, then showing the naive ROW SUM overshoots -- which is
/// exactly why the rows are documented as never-summed.
#[test]
fn by_repo_outcomes_globally_dedupe_to_totals() {
    let mut report = outcome_report();
    // Move one session to a second repo so `sha-shared` now spans two REPOS, not just two sessions.
    report.sessions.get_mut("s2").unwrap().repo = Some("tatari-tv/philo".to_string());
    report.totals.outcomes = Some(crate::outcome::rollup(
        report.sessions.values().map(|e| e.outcomes.as_ref()),
    ));
    let totals = report.totals.outcomes.clone().unwrap();
    let rows = compute_by_repo(&report);

    let global = crate::outcome::rollup(report.sessions.values().map(|e| e.outcomes.as_ref()));
    assert_eq!(
        (global.commits, global.prs_opened),
        (totals.commits, totals.prs_opened),
        "every session in this fixture has a repo, so the global dedupe over the by-repo rows IS \
         totals.outcomes: 4 distinct shas, 1 distinct PR url"
    );
    assert_eq!((totals.commits, totals.prs_opened), (4, 1));

    let row_sum: u64 = rows.iter().filter_map(|r| r.outcomes.as_ref()).map(|o| o.commits).sum();
    assert_eq!(
        row_sum, 5,
        "the row sum double-counts the cross-repo sha (2 + 3); totals.outcomes is the deduped \
         figure and the rows are never summed"
    );
}

/// The other half of the same criterion, and the case the real 30-day window actually hits: an
/// outcome observed in an UNATTRIBUTED session reaches `totals.outcomes` and can reach no by-repo
/// row, because `by-repo` has no row to put it in. So the equality above is scoped to the
/// attributed sessions, and the residual is a coverage fact rather than a dedupe bug (measured
/// 2026-07-26: 490 total commits, 484 across attributed sessions, 6 in sessions with no repo).
#[test]
fn by_repo_outcomes_cannot_carry_an_unattributed_session() {
    let mut report = outcome_report();
    let orphan = with_outcomes(
        entry_sourced(None, None, 25.0),
        &["sha-orphan"],
        &["https://github.com/tatari-tv/clyde/pull/9"],
        1,
        (5, 1),
    );
    report.sessions.insert("s4".to_string(), orphan);
    report.totals.sessions = report.sessions.len();
    report.totals.outcomes = Some(crate::outcome::rollup(
        report.sessions.values().map(|e| e.outcomes.as_ref()),
    ));
    let totals = report.totals.outcomes.clone().unwrap();
    let rows = compute_by_repo(&report);

    let attributed = crate::outcome::rollup(
        report
            .sessions
            .values()
            .filter(|e| e.repo.is_some())
            .map(|e| e.outcomes.as_ref()),
    );
    assert_eq!(
        (totals.commits, attributed.commits),
        (5, 4),
        "the orphan sha counts in the period total and in no repo row"
    );
    assert!(
        rows.iter().all(|r| r.repo != UNATTRIBUTED_ORG),
        "by-repo never invents a row for unattributed spend; `attribution` is where that lives"
    );
}

#[test]
fn by_repo_output_geometry_is_absent_for_a_repo_with_no_outcomes() {
    let mut report = outcome_report();
    report.sessions.get_mut("s3").unwrap().outcomes = None;
    let rows = compute_by_repo(&report);
    let philo = rows.iter().find(|r| r.repo == "tatari-tv/philo").unwrap();
    assert!(philo.outcomes.is_none(), "no observation -> no outcomes block");
    assert!(
        philo.commits_percent_of_max.is_none() && philo.prs_percent_of_max.is_none(),
        "an unobserved repo gets no bar at all, never a zero-length one"
    );

    let clyde = rows.iter().find(|r| r.repo == "tatari-tv/clyde").unwrap();
    assert_eq!(
        clyde.commits_percent_of_max,
        Some(100.0),
        "the series max copies to 100%, geometry the model never computes"
    );
}

#[test]
fn by_repo_pr_geometry_is_absent_when_no_repo_opened_a_pr() {
    let mut report = outcome_report();
    for sid in ["s1", "s2"] {
        report
            .sessions
            .get_mut(sid)
            .unwrap()
            .outcomes
            .as_mut()
            .unwrap()
            .prs
            .clear();
    }
    let rows = compute_by_repo(&report);
    assert!(
        rows.iter().all(|r| r.prs_percent_of_max.is_none()),
        "an all-zero series has no meaningful proportion, so the field is absent everywhere"
    );
    assert!(
        rows.iter().any(|r| r.commits_percent_of_max.is_some()),
        "the commit series is unaffected"
    );
}

#[test]
fn unit_costs_divide_the_period_spend_by_each_denominator() {
    let report = outcome_report();
    let (by_day, _) = compute_by_day(&report);
    let costs = compute_unit_costs(&report, &by_day);
    // $200.00 over 4 distinct commits, 1 distinct PR url, 3 sessions, 1 active day.
    assert_eq!(costs.per_commit.as_deref(), Some("$50.00"), "200 / 4 distinct commits");
    assert_eq!(costs.per_pr.as_deref(), Some("$200.00"), "200 / 1 distinct PR url");
    assert_eq!(costs.per_session.as_deref(), Some("$66.67"), "200 / 3 sessions");
    let active = by_day.iter().filter(|d| d.active).count();
    assert_eq!(active, 1, "every fixture session begins on the same day");
    assert_eq!(costs.per_active_day.as_deref(), Some("$200.00"), "200 / 1 active day");
    assert_eq!(
        (costs.session_spend_p50.as_deref(), costs.session_spend_p90.as_deref()),
        (Some("$60.00"), Some("$100.00")),
        "nearest-rank over [40, 60, 100]: p50 is a real session's spend, never an interpolation"
    );
}

#[test]
fn unit_costs_are_absent_on_every_zero_denominator() {
    let mut report = outcome_report();
    // Zero-commit, zero-PR window: the outcome rollup is present but empty.
    for sid in ["s1", "s2", "s3"] {
        let outcomes = report.sessions.get_mut(sid).unwrap().outcomes.as_mut().unwrap();
        outcomes.commits.clear();
        outcomes.prs.clear();
    }
    report.totals.outcomes = Some(crate::outcome::rollup(
        report.sessions.values().map(|e| e.outcomes.as_ref()),
    ));
    let (by_day, _) = compute_by_day(&report);
    let costs = compute_unit_costs(&report, &by_day);
    assert!(
        costs.per_commit.is_none() && costs.per_pr.is_none(),
        "no $Inf, no dollars-per-zero-commits: the field is absent"
    );
    assert!(
        costs.per_session.is_some(),
        "the denominators that are nonzero still divide"
    );

    let json = serde_json::to_string(&costs).unwrap();
    assert!(
        !json.contains("per-commit"),
        "an absent field is ABSENT from the context, never null: {json}"
    );
}

#[test]
fn unit_costs_are_absent_when_the_report_carries_no_outcome_rollup() {
    let mut report = outcome_report();
    report.totals.outcomes = None;
    let (by_day, _) = compute_by_day(&report);
    let costs = compute_unit_costs(&report, &by_day);
    assert!(
        costs.per_commit.is_none() && costs.per_pr.is_none(),
        "`--no-outcomes` leaves no output denominator to divide by"
    );
}

#[test]
fn unit_cost_percentiles_ignore_unpriced_sessions() {
    let mut report = outcome_report();
    // An untracked-model session has an UNKNOWN spend, not a $0 one.
    let s3 = report.sessions.get_mut("s3").unwrap();
    s3.spend_usd = None;
    let (by_day, _) = compute_by_day(&report);
    let costs = compute_unit_costs(&report, &by_day);
    assert_eq!(
        costs.session_spend_p50.as_deref(),
        Some("$60.00"),
        "the distribution is [60, 100]; folding the unpriced session in as $0 would drag it to $40"
    );
}

#[test]
fn unit_costs_are_all_absent_on_an_empty_window() {
    let report = report_with("2026-06-01T00:00:00Z", "2026-06-10T00:00:00Z", vec![]);
    let (by_day, _) = compute_by_day(&report);
    let costs = compute_unit_costs(&report, &by_day);
    assert_eq!(
        serde_json::to_string(&costs).unwrap(),
        "{}",
        "a window with no sessions, no active days, and no outcomes emits no unit costs at all"
    );
}

/// Parse a `$1,234.56` display string back to cents, the way a reader adds a column up.
fn displayed_cents(spends: impl IntoIterator<Item = String>) -> i64 {
    spends
        .into_iter()
        .map(|s| (s.replace(['$', ','], "").parse::<f64>().unwrap() * 100.0).round() as i64)
        .sum()
}

/// A report whose headline DISAGREES with its sessions by a cent: the shape every fixture had while
/// `totals.spend-usd` priced the unioned token counts. Since the headline became the sum of session
/// cents this cannot be collected, so the rows must follow the sessions, never the headline.
fn report_a_cent_over(sessions: Vec<(&str, SessionEntry)>, headline: f64) -> Report {
    let mut report = report_with("2026-06-01T00:00:00Z", "2026-06-30T00:00:00Z", sessions);
    report.totals.spend_usd = headline;
    report
}

/// `by-repo` is a complete partition when every session carries a repo, and the artifact presents it
/// that way, so the DISPLAYED column must sum to the DISPLAYED headline. Every row is a sum of the
/// same per-session cents the headline sums, so it does with no reallocation.
#[test]
fn by_repo_reconciles_to_the_headline_when_every_session_is_attributed() {
    let report = report_with(
        "2026-06-01T00:00:00Z",
        "2026-06-30T00:00:00Z",
        vec![
            (
                "a",
                entry_sourced(Some("northwind/beacon"), Some(RepoSource::GitOrigin), 40.44),
            ),
            (
                "b",
                entry_sourced(Some("northwind/halyard"), Some(RepoSource::KnownPath), 24.42),
            ),
        ],
    );
    let aggregates = compute(&report, DEFAULT_OUTLIERS, &pricing());
    let rows: i64 = aggregates.by_repo.iter().map(|r| r.spend_cents).sum();
    assert_eq!(rows, crate::report::session_cents_total(report.sessions.values()));
    assert_eq!(
        displayed_cents(aggregates.by_repo.iter().map(|r| r.spend.clone())),
        6486,
        "rows: {:?}",
        aggregates.by_repo.iter().map(|r| &r.spend).collect::<Vec<_>>()
    );
}

/// Inverted from `a_lone_repo_row_equals_the_headline_exactly`, which forced a lone row onto a
/// headline its one session did not support. The row is its session's cents: `$49.47`, whatever a
/// stale headline says.
#[test]
fn a_lone_repo_row_is_its_sessions_cents_not_a_disagreeing_headline() {
    let report = report_a_cent_over(
        vec![(
            "a",
            entry_sourced(Some("jrivera/driftwood"), Some(RepoSource::GitOrigin), 49.47),
        )],
        49.48,
    );
    let aggregates = compute(&report, DEFAULT_OUTLIERS, &pricing());
    assert_eq!(aggregates.by_repo.len(), 1);
    assert_eq!(aggregates.by_repo[0].spend, "$49.47");
    assert_eq!(aggregates.by_repo[0].spend_cents, 4947);
}

/// With even ONE unattributed session the table is a genuine SUBSET, not a partition: the
/// attributed repo keeps exactly its own sessions' cents.
#[test]
fn by_repo_is_left_alone_when_a_session_is_unattributed() {
    let report = report_with(
        "2026-06-01T00:00:00Z",
        "2026-06-30T00:00:00Z",
        vec![
            (
                "a",
                entry_sourced(Some("northwind/beacon"), Some(RepoSource::GitOrigin), 40.00),
            ),
            ("b", entry_sourced(None, None, 24.86)),
        ],
    );
    let aggregates = compute(&report, DEFAULT_OUTLIERS, &pricing());
    assert_eq!(aggregates.by_repo.len(), 1, "the unattributed session has no row");
    assert_eq!(
        aggregates.by_repo[0].spend, "$40.00",
        "the attributed repo keeps the spend its own sessions measured"
    );
}

/// `by-org` is ALWAYS a partition, unlike `by-repo`: an unattributed session lands in this table's
/// own `(unattributed)` org bucket rather than being dropped, so the rows sum to the session cents.
#[test]
fn by_org_reconciles_to_the_headline_even_with_unattributed_sessions() {
    let report = report_with(
        "2026-06-01T00:00:00Z",
        "2026-06-30T00:00:00Z",
        vec![
            (
                "a",
                entry_sourced(Some("northwind/beacon"), Some(RepoSource::GitOrigin), 40.00),
            ),
            ("b", entry_sourced(None, None, 24.86)),
        ],
    );
    let aggregates = compute(&report, DEFAULT_OUTLIERS, &pricing());
    assert!(
        aggregates.by_org.iter().any(|r| r.org == UNATTRIBUTED_ORG),
        "the unattributed session still gets an org bucket"
    );
    assert_eq!(
        displayed_cents(aggregates.by_org.iter().map(|r| r.spend.clone())),
        6486,
        "rows: {:?}",
        aggregates.by_org.iter().map(|r| &r.spend).collect::<Vec<_>>()
    );
}

/// Inverted from `reconciliation_moves_the_display_string_and_not_the_operand`: there is no
/// display-time reallocation left, so the display string, the cents twin and the geometry operand
/// are one figure.
#[test]
fn repo_display_cents_and_operand_are_one_figure() {
    let report = report_a_cent_over(
        vec![(
            "a",
            entry_sourced(Some("jrivera/driftwood"), Some(RepoSource::GitOrigin), 49.47),
        )],
        49.48,
    );
    let aggregates = compute(&report, DEFAULT_OUTLIERS, &pricing());
    let row = &aggregates.by_repo[0];
    assert_eq!(row.spend, "$49.47");
    assert_eq!(row.spend_cents, 4947);
    assert!((row.spend_raw - 49.47).abs() < 1e-9, "operand: {}", row.spend_raw);
}

mod cache;
