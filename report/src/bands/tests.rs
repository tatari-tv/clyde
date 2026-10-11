#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use super::*;
use crate::outcome::PrRef;

fn fixture_report(name: &str) -> Report {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/report")
        .join(name)
        .join("report.json");
    crate::render::load_report(&path, "fixture report").unwrap()
}

/// A session built from a committed fixture's first entry, so every field a real one carries is
/// present, then set to the case under test.
fn entry(spend: Option<f64>, outcomes: Option<Outcomes>, models: &[&str], turns: u64) -> SessionEntry {
    let report = fixture_report("small");
    let template = report.sessions.values().next().unwrap();
    let mut value = serde_json::to_value(template).unwrap();
    value["models"] = serde_json::Value::Object(
        models
            .iter()
            .map(|m| {
                (
                    (*m).to_string(),
                    serde_json::to_value(template.models.values().next().unwrap()).unwrap(),
                )
            })
            .collect(),
    );
    let mut built: SessionEntry = serde_json::from_value(value).unwrap();
    built.spend_usd = spend;
    built.outcomes = outcomes;
    built.efficiency.aggregate.raw.turns = turns;
    built.efficiency.flags.clear();
    built
}

fn pr() -> PrRef {
    PrRef {
        number: 1,
        url: "https://example.invalid/pull/1".to_string(),
        repository: None,
    }
}

fn sha() -> Vec<String> {
    vec!["abc".to_string()]
}

#[test]
fn bands_classify_a_commit_as_committed() {
    let o = Outcomes {
        commits: sha(),
        ..Outcomes::default()
    };
    assert_eq!(classify(Some(&o)), Band::Committed);
}

#[test]
fn bands_classify_an_opened_pr_alone_as_committed() {
    let o = Outcomes {
        prs: vec![pr()],
        ..Outcomes::default()
    };
    assert_eq!(classify(Some(&o)), Band::Committed);
}

#[test]
fn bands_classify_edited_files_without_a_commit_as_edited() {
    let o = Outcomes {
        files_edited: 1,
        ..Outcomes::default()
    };
    assert_eq!(classify(Some(&o)), Band::Edited);
}

#[test]
fn bands_classify_each_record_write_as_wrote_records() {
    for o in [
        Outcomes {
            confluence_writes: 1,
            ..Outcomes::default()
        },
        Outcomes {
            jira_writes: 1,
            ..Outcomes::default()
        },
        Outcomes {
            slack_messages: 1,
            ..Outcomes::default()
        },
    ] {
        assert_eq!(classify(Some(&o)), Band::WroteRecords, "{o:?}");
    }
}

#[test]
fn bands_first_match_wins() {
    let everything = Outcomes {
        commits: sha(),
        prs: vec![pr()],
        files_edited: 3,
        jira_writes: 2,
        ..Outcomes::default()
    };
    assert_eq!(classify(Some(&everything)), Band::Committed);
    let edit_and_write = Outcomes {
        files_edited: 3,
        slack_messages: 2,
        ..Outcomes::default()
    };
    assert_eq!(classify(Some(&edit_and_write)), Band::Edited);
}

#[test]
fn bands_classify_nothing_observed_as_no_observed_outcome() {
    assert_eq!(classify(None), Band::NoObservedOutcome);
    assert_eq!(classify(Some(&Outcomes::default())), Band::NoObservedOutcome);
    // Written lines alone are not a file edit: the file count is what the band reads.
    let lines_only = Outcomes {
        lines_written: 40,
        lines_replaced: 4,
        ..Outcomes::default()
    };
    assert_eq!(classify(Some(&lines_only)), Band::NoObservedOutcome);
}

#[test]
fn bands_names_and_rules_are_distinct_and_in_order() {
    let names: Vec<&str> = BANDS.iter().map(|b| b.name()).collect();
    assert_eq!(names, ["committed", "edited", "wrote-records", "no-observed-outcome"]);
    let mut rules: Vec<&str> = BANDS.iter().map(|b| b.rule()).collect();
    rules.sort_unstable();
    rules.dedup();
    assert_eq!(rules.len(), 4);
}

#[test]
fn bands_roll_up_sums_sessions_and_cents_per_band() {
    let committed = || Outcomes {
        commits: sha(),
        ..Outcomes::default()
    };
    let edited = Outcomes {
        files_edited: 2,
        ..Outcomes::default()
    };
    let sessions = [
        entry(Some(1.234), Some(committed()), &["m"], 9),
        entry(Some(2.0), Some(committed()), &["m"], 9),
        entry(Some(0.5), Some(edited), &["m"], 9),
        entry(None, None, &["m"], 9),
        entry(Some(0.004), None, &["m"], 9),
    ];
    let t = roll_up(sessions.iter());
    assert_eq!(
        t[0],
        Tally {
            sessions: 2,
            cents: 123 + 200
        }
    );
    assert_eq!(t[1], Tally { sessions: 1, cents: 50 });
    assert_eq!(t[2], Tally { sessions: 0, cents: 0 });
    assert_eq!(t[3], Tally { sessions: 2, cents: 0 });
}

#[test]
fn bands_session_cents_rounds_and_treats_unpriced_as_zero() {
    assert_eq!(session_cents(&entry(Some(1.006), None, &["m"], 1)), 101);
    assert_eq!(session_cents(&entry(Some(1.004), None, &["m"], 1)), 100);
    assert_eq!(session_cents(&entry(None, None, &["m"], 1)), 0);
}

#[test]
fn bands_percent_rounds_half_up_and_guards_the_denominator() {
    assert_eq!(percent(1, 0), "n/a");
    assert_eq!(percent(0, 0), "n/a");
    assert_eq!(percent(5, -1), "n/a");
    assert_eq!(percent(0, 100), "0%");
    assert_eq!(percent(1, 201), "<1%");
    assert_eq!(percent(1, 200), "1%");
    assert_eq!(percent(1, 3), "33%");
    assert_eq!(percent(2, 3), "67%");
    assert_eq!(percent(100, 100), "100%");
    assert_eq!(percent(3, 8), "38%");
}

#[test]
fn bands_heavy_on_short_counts_a_heavy_model_at_or_under_the_turn_line() {
    let sessions = [
        entry(Some(1.0), None, &["claude-opus-4-8"], 3),
        entry(Some(2.0), None, &["claude-opus-4-8"], 4),
        entry(Some(4.0), None, &["claude-sonnet-5"], 1),
        entry(Some(8.0), None, &["claude-haiku", "claude-FABLE-1"], 0),
    ];
    let t = heavy_on_short(sessions.iter(), &["opus", "fable"], 3);
    assert_eq!(
        t,
        Tally {
            sessions: 2,
            cents: 100 + 800
        }
    );
}

#[test]
fn bands_heavy_on_short_follows_both_thresholds() {
    let sessions = [entry(Some(1.0), None, &["claude-opus-4-8"], 5)];
    assert_eq!(heavy_on_short(sessions.iter(), &["opus"], 4).sessions, 0);
    assert_eq!(heavy_on_short(sessions.iter(), &["opus"], 5).sessions, 1);
    assert_eq!(heavy_on_short(sessions.iter(), &["sonnet"], 5).sessions, 0);
    assert_eq!(heavy_on_short(sessions.iter(), &[], 5).sessions, 0);
}

#[test]
fn bands_flag_rollup_counts_each_kind_a_session_carries() {
    let mut both = entry(Some(1.0), None, &["m"], 9);
    both.efficiency.flags = vec![
        EfficiencyFlag::AutoCompaction { count: 2 },
        EfficiencyFlag::HighToolErrorRate {
            observed: 0.2,
            ceiling: 0.1,
        },
    ];
    let mut one = entry(Some(2.0), None, &["m"], 9);
    one.efficiency.flags = vec![EfficiencyFlag::AutoCompaction { count: 1 }];
    let clean = entry(Some(4.0), None, &["m"], 9);
    let t = flag_rollup([&both, &one, &clean].into_iter());
    assert_eq!(t[0], Tally::default());
    assert_eq!(
        t[1],
        Tally {
            sessions: 1,
            cents: 100
        }
    );
    assert_eq!(
        t[2],
        Tally {
            sessions: 2,
            cents: 300
        }
    );
}

#[test]
fn bands_flag_rollup_names_every_variant() {
    let flags = [
        (
            EfficiencyFlag::LowCacheReadShare {
                observed: 0.1,
                floor: 0.6,
            },
            0,
        ),
        (
            EfficiencyFlag::HighToolErrorRate {
                observed: 0.3,
                ceiling: 0.1,
            },
            1,
        ),
        (EfficiencyFlag::AutoCompaction { count: 1 }, 2),
    ];
    for (flag, index) in flags {
        let mut e = entry(Some(1.0), None, &["m"], 9);
        e.efficiency.flags = vec![flag];
        let t = flag_rollup([&e].into_iter());
        for (i, tally) in t.iter().enumerate() {
            assert_eq!(
                tally.sessions,
                usize::from(i == index),
                "flag in slot {index}, slot {i}"
            );
        }
    }
}

/// Design Phase 4 criterion: on every committed fixture, band session counts sum to
/// `totals.sessions` and band cents sum to the sum of the sessions' own cents.
#[test]
fn bands_sum_to_the_report_totals_on_every_fixture() {
    for name in ["small", "medium", "pathological"] {
        let report = fixture_report(name);
        let tallies = roll_up(report.sessions.values());
        let sessions: usize = tallies.iter().map(|t| t.sessions).sum();
        let cents: i64 = tallies.iter().map(|t| t.cents).sum();
        let expected: i64 = report
            .sessions
            .values()
            .map(|e| ((e.spend_usd.unwrap_or(0.0)) * 100.0).round() as i64)
            .sum();
        assert_eq!(sessions, report.totals.sessions, "{name}: sessions");
        assert_eq!(cents, expected, "{name}: cents");
    }
}

#[test]
fn bands_all_four_appear_in_the_medium_fixture() {
    let tallies = roll_up(fixture_report("medium").sessions.values());
    for (band, tally) in BANDS.iter().zip(tallies) {
        assert!(tally.sessions > 0, "medium has no {} session", band.name());
    }
}

#[test]
fn bands_the_medium_fixture_has_heavy_short_sessions_and_flags() {
    let report = fixture_report("medium");
    let view = build_view(&report, &["opus", "fable"], 3);
    assert_ne!(view.heavy_on_short.sessions, "0");
    assert_eq!(view.flags.len(), 2);
}

#[test]
fn bands_view_total_row_equals_the_sum_of_the_rows() {
    let view = build_view(&fixture_report("medium"), &["opus"], 3);
    let table = view.bands.unwrap();
    let sessions: usize = table.rows.iter().map(|r| r.tally.sessions).sum();
    let cents: i64 = table.rows.iter().map(|r| r.tally.cents).sum();
    assert_eq!(table.total_sessions, format_int(sessions as u64));
    assert_eq!(table.total_spend, usd(cents));
    assert_eq!(table.rows.len(), 4);
    assert_eq!(table.rows[0].share, percent(table.rows[0].tally.cents, cents));
}

#[test]
fn bands_view_is_absent_unless_outcomes_were_collected() {
    for enabled in [None, Some(false)] {
        let mut report = fixture_report("medium");
        report.outcomes_enabled = enabled;
        let view = build_view(&report, &["opus"], 3);
        assert!(view.bands.is_none(), "outcomes_enabled={enabled:?}");
    }
    let mut report = fixture_report("medium");
    report.outcomes_enabled = Some(true);
    assert!(build_view(&report, &["opus"], 3).bands.is_some());
}

#[test]
fn bands_view_with_zero_spend_shares_render_not_available() {
    let mut report = fixture_report("small");
    for e in report.sessions.values_mut() {
        e.spend_usd = None;
    }
    let table = build_view(&report, &["opus"], 3).bands.unwrap();
    assert!(table.rows.iter().all(|r| r.share == "n/a"));
    assert_eq!(table.total_spend, "$0.00");
}

#[test]
fn bands_view_echoes_the_configured_thresholds() {
    let view = build_view(&fixture_report("small"), &["opus", "fable"], 3);
    assert_eq!(view.heavy_on_short.models, ["opus", "fable"]);
    assert_eq!(view.heavy_on_short.max_turns, "3");
    let view = build_view(&fixture_report("small"), &[], 1200);
    assert!(view.heavy_on_short.models.is_empty());
    assert_eq!(view.heavy_on_short.max_turns, "1,200");
}
