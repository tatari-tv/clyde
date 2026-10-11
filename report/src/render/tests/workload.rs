#![allow(clippy::unwrap_used)]

//! Tests for `render::workload`: the efficiency view's three attribution maps, and the one of them
//! that is a declared PARTITION of the headline.

use std::collections::BTreeMap;

use efficiency::{McpCalls, WorkloadCost};

use crate::render::tests::sample_report;
use crate::render::{CallRow, workload::*};
use crate::report::SessionEntry;

/// One session entry carrying `spend` and agent-type buckets `(name, cost_usd)`. Tokens are a
/// constant: they ride along and never weigh the split.
fn session(spend: Option<f64>, pairs: &[(&str, f64)]) -> SessionEntry {
    let mut entry = sample_report().sessions.into_values().next().unwrap();
    entry.spend_usd = spend;
    entry.agent_type_costs = pairs
        .iter()
        .map(|(name, cost)| {
            (
                (*name).to_string(),
                WorkloadCost {
                    tokens: 1_000,
                    cost_usd: *cost,
                },
            )
        })
        .collect();
    entry
}

fn cents_of(parts: &[(String, u64, i64)]) -> Vec<(&str, i64)> {
    parts.iter().map(|(n, _, c)| (n.as_str(), *c)).collect()
}

fn spends(rows: &[CallRow]) -> Vec<&str> {
    rows.iter().map(|r| r.spend.as_str()).collect()
}

/// Sum a set of `$X.XX` display strings back into cents, the way a reader adds a table up.
fn displayed_cents(rows: &[CallRow]) -> i64 {
    rows.iter()
        .map(|r| {
            let cleaned = r.spend.replace(['$', ','], "");
            (cleaned.parse::<f64>().unwrap() * 100.0).round() as i64
        })
        .sum()
}

/// THE defect, now fixed at the source. Four buckets whose independently-rounded cents sum to
/// $671.27 under a session of $671.28 (the medium fixture's old shape): the session's own cents are
/// split across its buckets by largest remainder, so the parts ARE the session's cents and the
/// table built from them cannot miss the headline.
#[test]
fn the_agent_type_partition_reconciles_to_the_displayed_headline() {
    let entry = session(
        Some(671.28),
        &[
            ("(main-session)", 479.7449),
            ("phase-implementer", 79.3312),
            ("doc-writer", 61.7751),
            ("code-reviewer", 50.4266),
        ],
    );
    let parts = session_agent_cents(&entry);
    assert_eq!(
        parts.iter().map(|p| p.2).sum::<i64>(),
        67128,
        "the session's cents, exactly"
    );
    // The two residual cents land on the largest discarded fractions (.68 and .65).
    assert_eq!(
        cents_of(&parts),
        vec![
            ("(main-session)", 47975),
            ("code-reviewer", 5043),
            ("doc-writer", 6177),
            ("phase-implementer", 7933),
        ]
    );
}

/// A shortfall is taken back, not handed out: the parts must not exceed the session either.
#[test]
fn the_partition_takes_a_cent_back_from_the_smallest_remainder() {
    let parts = session_agent_cents(&session(Some(15.0), &[("a", 10.009), ("b", 5.001)]));
    assert_eq!(cents_of(&parts), vec![("a", 1000), ("b", 500)]);
}

/// Inverted from `a_gap_larger_than_rounding_is_left_visible_not_smeared`. The rows used to be
/// reallocated against the headline at display time and declined on a gap larger than rounding,
/// leaving a table that did not sum. Now the session's cents are the authority and its buckets only
/// say how to divide them, so even buckets that disagree with the session sum to its cents.
#[test]
fn buckets_that_disagree_with_the_session_still_split_exactly_its_cents() {
    let parts = session_agent_cents(&session(Some(0.60), &[("implementer", 9.00), ("researcher", 4.50)]));
    assert_eq!(cents_of(&parts), vec![("implementer", 40), ("researcher", 20)]);
}

/// Inverted from the first-bucket-by-name fallback: when every bucket weighs zero there is no
/// proportion to honor, so the session's cents land on `(main-session)` (appended when absent,
/// reused when present), never on whichever bucket sorts first.
#[test]
fn zero_weight_buckets_put_the_session_cents_on_the_main_session_row() {
    let parts = session_agent_cents(&session(Some(7.83), &[("alpha", 0.0), ("beta", 0.0)]));
    assert_eq!(
        cents_of(&parts),
        vec![("alpha", 0), ("beta", 0), (crate::report::MAIN_SESSION_BUCKET, 783)]
    );
    let parts = session_agent_cents(&session(Some(7.83), &[("(main-session)", 0.0), ("alpha", 0.0)]));
    assert_eq!(cents_of(&parts), vec![("(main-session)", 783), ("alpha", 0)]);
    assert_eq!(parts.iter().map(|p| p.2).sum::<i64>(), 783);
}

/// A session with spend and no buckets puts its cents on `(main-session)`; an unpriced session
/// with no buckets contributes no row.
#[test]
fn spend_with_no_bucket_lands_on_the_main_session_row() {
    assert_eq!(
        cents_of(&session_agent_cents(&session(Some(1.23), &[]))),
        vec![(crate::report::MAIN_SESSION_BUCKET, 123)]
    );
    assert!(session_agent_cents(&session(None, &[])).is_empty());
    assert_eq!(
        cents_of(&session_agent_cents(&session(None, &[("a", 2.0)]))),
        vec![("a", 0)],
        "an unpriced session keeps its bucket at zero cents"
    );
}

/// Rows are sums of per-session cents, in cents-descending order with a name tiebreak, and
/// deterministic.
#[test]
fn the_partition_keeps_display_order_and_is_deterministic() {
    let map = |pairs: &[(&str, i64)]| -> BTreeMap<String, AgentCents> {
        pairs
            .iter()
            .map(|(n, c)| {
                (
                    (*n).to_string(),
                    AgentCents {
                        tokens: 1_000,
                        cents: *c,
                        ..AgentCents::default()
                    },
                )
            })
            .collect()
    };
    let input = [("zulu", 101), ("alpha", 101), ("big", 9000)];
    let first = partition_rows(map(&input));
    let second = partition_rows(map(&input));
    assert_eq!(
        first.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        vec!["big", "alpha", "zulu"],
        "spend descending, then name ascending"
    );
    assert_eq!(spends(&first), spends(&second), "same input, same cents, every run");
    assert_eq!(spends(&first), vec!["$90.00", "$1.01", "$1.01"]);
    assert_eq!(displayed_cents(&first), 9202);
}

#[test]
fn an_empty_partition_renders_no_rows() {
    assert!(partition_rows(BTreeMap::new()).is_empty());
}

/// A zero-spend window renders `0.0%`, never a `NaN` percent -- matching `compute_attribution`'s
/// precedent for the same divide-by-zero.
#[test]
fn coverage_note_on_a_zero_total_is_zero_percent_not_nan() {
    assert_eq!(coverage_note(0.0, 0.0), "$0.00 of $0.00 (0.0%), embedded-price basis");
}

fn cost(tokens: u64, usd: f64) -> WorkloadCost {
    WorkloadCost { tokens, cost_usd: usd }
}

/// Two sessions: `counted` carries call maps, `uncounted` is the shape a pre-call-counting blob
/// parses to (`None`), and both hold spend under the same keys.
fn two_sessions(uncounted_calls: bool) -> crate::report::Report {
    let mut report = sample_report();
    let mut entries = std::mem::take(&mut report.sessions).into_iter();
    let (id_a, mut a) = entries.next().unwrap();
    let (id_b, mut b) = entries.next().unwrap();
    a.by_skill = BTreeMap::from([("a".to_string(), cost(1_000, 1.0)), ("b".to_string(), cost(10, 0.5))]);
    a.skill_calls = Some(BTreeMap::from([("a".to_string(), 4), ("b".to_string(), 0)]));
    a.by_mcp = BTreeMap::from([
        ("srv__t".to_string(), cost(500, 2.0)),
        ("srv__u".to_string(), cost(500, 1.0)),
    ]);
    a.mcp_calls = Some(BTreeMap::from([
        (
            "srv__t".to_string(),
            McpCalls {
                calls: 2,
                timed: 2,
                total_ms: 5_000,
            },
        ),
        (
            "srv__u".to_string(),
            McpCalls {
                calls: 3,
                timed: 0,
                total_ms: 0,
            },
        ),
    ]));
    b.by_skill = BTreeMap::from([("a".to_string(), cost(3_000, 3.0)), ("c".to_string(), cost(20, 2.0))]);
    b.by_mcp = BTreeMap::from([("bare".to_string(), cost(100, 0.7))]);
    if uncounted_calls {
        b.skill_calls = None;
        b.mcp_calls = None;
    } else {
        a.skill_calls = None;
        a.mcp_calls = None;
    }
    report.sessions.insert(id_a, a);
    report.sessions.insert(id_b, b);
    report
}

fn cell<'a>(rows: &'a [crate::render::CallRow], name: &str) -> &'a crate::render::CallRow {
    rows.iter().find(|r| r.name == name).unwrap()
}

/// A row fed by a session that did not count calls carries `\u{2020}`, divides only counted-session
/// spend, and shows `n/a` rather than `0` when no counted call backs it. A counted row that really
/// made no calls keeps its `0`. BITES: divide total spend, or default a missing call count to 0.
#[test]
fn partial_row_marks_and_never_zero() {
    let eff = build_efficiency_view(&two_sessions(true));

    let a = cell(&eff.by_skill, "a");
    assert_eq!(a.calls.as_deref(), Some("4\u{2020}"));
    assert_eq!(a.sessions.as_deref(), Some("2"));
    assert_eq!(a.spend, "$4.00", "spend is every session's");
    assert_eq!(
        a.per_call.as_deref(),
        Some("$0.25"),
        "$1.00 counted spend over 4 calls, not $4.00"
    );

    let c = cell(&eff.by_skill, "c");
    assert_eq!(c.calls.as_deref(), Some("n/a"));
    assert_eq!(c.per_call.as_deref(), Some("n/a"));

    let b = cell(&eff.by_skill, "b");
    assert_eq!(
        b.calls.as_deref(),
        Some("0"),
        "a counted session's zero is a measurement"
    );
    assert_eq!(b.per_call.as_deref(), Some("n/a"));

    let t = cell(&eff.by_mcp, "t");
    assert_eq!(t.server.as_deref(), Some("srv"));
    assert_eq!(t.calls.as_deref(), Some("2"));
    assert_eq!(t.time.as_deref(), Some("5.0s"));
    assert_eq!(t.avg.as_deref(), Some("2.5s"));
    assert_eq!(t.per_call.as_deref(), Some("$1.00"));
    let u = cell(&eff.by_mcp, "u");
    assert_eq!(u.avg.as_deref(), Some("n/a"), "3 calls, none timed");
    assert_eq!(u.time.as_deref(), Some("0.0s"));

    let bare = cell(&eff.by_mcp, "bare");
    assert_eq!(
        bare.server.as_deref(),
        Some("_unknown"),
        "a key with no `__` has no server"
    );
    assert_eq!(bare.calls.as_deref(), Some("n/a"));
    assert_eq!(bare.avg.as_deref(), Some("n/a"));
    assert_eq!(bare.time.as_deref(), Some("n/a"));

    assert_eq!(
        eff.call_captions,
        [
            "\u{2020} includes sessions recorded before call counting",
            "Calls counted on 1 of 2 sessions"
        ]
    );
}

/// The row set is the union of the cost and calls maps: a key with calls but no attributed turns
/// shows `$0.00` and `0` tokens.
#[test]
fn calls_without_attributed_spend_still_get_a_row() {
    let mut report = two_sessions(true);
    let entry = report.sessions.values_mut().next().unwrap();
    entry.skill_calls.as_mut().unwrap().insert("ghost".to_string(), 7);
    let eff = build_efficiency_view(&report);
    let ghost = cell(&eff.by_skill, "ghost");
    assert_eq!(ghost.calls.as_deref(), Some("7"));
    assert_eq!(ghost.spend, "$0.00");
    assert_eq!(ghost.tokens_human, "0");
    assert_eq!(ghost.per_call.as_deref(), Some("$0.00"));
    assert_eq!(ghost.sessions.as_deref(), Some("1"));
}

/// With no counted session the call columns do not exist: no cell is `n/a`, no `\u{2020}`, and the
/// caption says how few counted.
#[test]
fn no_counted_sessions_omits_call_columns() {
    let mut report = two_sessions(true);
    for entry in report.sessions.values_mut() {
        entry.skill_calls = None;
        entry.mcp_calls = None;
    }
    let eff = build_efficiency_view(&report);
    assert!(!eff.by_skill.is_empty() && !eff.by_mcp.is_empty());
    for row in eff.by_skill.iter().chain(&eff.by_mcp) {
        assert!(row.calls.is_none() && row.sessions.is_none() && row.per_call.is_none());
        assert!(row.time.is_none() && row.avg.is_none() && row.counts.is_none());
    }
    assert_eq!(eff.call_captions, ["Calls counted on 0 of 2 sessions"]);
    let json = serde_json::to_string(&eff.by_skill).unwrap();
    assert!(!json.contains("calls") && !json.contains("per-call"), "{json}");
}

/// Every session counted: no `\u{2020}`, no counted-on caption.
#[test]
fn fully_counted_report_has_no_captions() {
    let mut report = two_sessions(true);
    for entry in report.sessions.values_mut() {
        entry.skill_calls.get_or_insert_with(BTreeMap::new);
        entry.mcp_calls.get_or_insert_with(BTreeMap::new);
    }
    let eff = build_efficiency_view(&report);
    assert!(eff.call_captions.is_empty());
    assert_eq!(cell(&eff.by_skill, "c").calls.as_deref(), Some("0"));
}

const NO_ROLLUP_CAPTION: &str =
    "Under --no-rollup, a subagent that inherited its parent's skill tag shows that skill's tokens with 0 calls";

fn has_caption(report: &crate::report::Report) -> bool {
    build_efficiency_view(report)
        .call_captions
        .iter()
        .any(|c| c == NO_ROLLUP_CAPTION)
}

fn subagent(agent_id: &str) -> efficiency::SubagentEfficiency {
    efficiency::SubagentEfficiency {
        agent_id: agent_id.to_string(),
        agent_type: Some("general-purpose".to_string()),
        signals: efficiency::EfficiencySignals::default(),
    }
}

/// Every session counted, so the only possible caption is the `--no-rollup` one.
fn counted_rollup_report() -> crate::report::Report {
    let mut report = two_sessions(true);
    for entry in report.sessions.values_mut() {
        entry.skill_calls.get_or_insert_with(BTreeMap::new);
        entry.mcp_calls.get_or_insert_with(BTreeMap::new);
    }
    report
}

/// The `--no-rollup` shape: each session re-keyed `<sid>/<agent-id>` holding that one subagent.
fn exploded(report: crate::report::Report) -> crate::report::Report {
    let mut out = report;
    out.sessions = std::mem::take(&mut out.sessions)
        .into_iter()
        .enumerate()
        .map(|(i, (sid, mut entry))| {
            let agent_id = format!("agent-a{i}");
            entry.efficiency.subagents = vec![subagent(&agent_id)];
            (format!("{sid}/{agent_id}"), entry)
        })
        .collect();
    out
}

/// Merge two hosts' copies of the same report, through the real `merge_reports` re-keying.
fn merged(build: impl Fn() -> crate::report::Report) -> crate::report::Report {
    let a = build();
    let mut b = build();
    b.host = "other".to_string();
    crate::merge::merge_reports(vec![a, b]).unwrap()
}

/// A merged ROLLUP report re-keys every session `<host>/<sid>`; that `/` is not a `--no-rollup`
/// row. Inverts the old key-shape test. BITES: go back to `keys().any(contains('/'))`.
#[test]
fn merged_rollup_report_has_no_no_rollup_caption() {
    let report = merged(|| {
        let mut report = counted_rollup_report();
        for entry in report.sessions.values_mut() {
            entry.efficiency.subagents = vec![subagent("agent-a0")];
        }
        report
    });
    assert!(report.sessions.keys().all(|k| k.contains('/')));
    assert!(!has_caption(&report));
}

/// A merged `--no-rollup` report (`<host>/<sid>/<agent-id>`) still prints the caption.
#[test]
fn merged_no_rollup_report_keeps_caption() {
    let report = merged(|| exploded(counted_rollup_report()));
    assert!(report.sessions.keys().all(|k| k.matches('/').count() == 2));
    assert!(has_caption(&report));
}

/// An unmerged `--no-rollup` report prints the caption; the rolled-up one does not. BITES: drop the
/// caption, or show it on every report.
#[test]
fn no_rollup_report_keeps_caption() {
    assert!(!has_caption(&counted_rollup_report()));
    assert!(has_caption(&exploded(counted_rollup_report())));
}

/// A rollup session with exactly one subagent is keyed by its bare session id, so it is not a
/// subagent row; neither is a slashed key whose suffix is not the subagent's id.
#[test]
fn single_subagent_rollup_session_is_not_a_subagent_row() {
    let mut entry = sample_report().sessions.into_values().next().unwrap();
    entry.efficiency.subagents = vec![subagent("agent-a0")];
    assert!(!is_subagent_row("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042", &entry));
    assert!(!is_subagent_row("desk/9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042", &entry));
    assert!(is_subagent_row("9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042/agent-a0", &entry));
    entry.efficiency.subagents.push(subagent("agent-a1"));
    assert!(!is_subagent_row(
        "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042/agent-a0",
        &entry
    ));
}

/// A subagent of `agent_type` (`None` = untyped) that spent `tokens` output tokens on opus.
fn spender(agent_id: &str, agent_type: Option<&str>, tokens: u64) -> efficiency::SubagentEfficiency {
    let mut signals = efficiency::EfficiencySignals::default();
    if tokens > 0 {
        signals.raw.by_model.insert(
            "claude-opus-4-7".to_string(),
            common::metrics::TokenTotals {
                output: tokens,
                total: tokens,
                ..Default::default()
            },
        );
    }
    efficiency::SubagentEfficiency {
        agent_id: agent_id.to_string(),
        agent_type: agent_type.map(str::to_string),
        signals,
    }
}

/// The Calls cell of every agent row, by name.
fn agent_calls(report: &crate::report::Report) -> BTreeMap<String, String> {
    build_efficiency_view(report)
        .agent_type_costs
        .into_iter()
        .map(|r| (r.name, r.calls.unwrap()))
        .collect()
}

fn medium(no_rollup: bool) -> crate::report::Report {
    let pricing = claude_pricing::Pricing::embedded();
    let kind = crate::eval::synth::Kind::Medium;
    if no_rollup {
        crate::eval::synth::build_no_rollup(kind, &pricing).unwrap()
    } else {
        crate::eval::synth::build(kind, &pricing).unwrap()
    }
}

/// On synth medium every type's Calls is its count of tokened subagents (counted here from the raw
/// token split, independent of the render predicate), `(main-session)` is `n/a`, and the rows still
/// partition `totals.spend` in cents. BITES: count every subagent, or key `None` elsewhere.
#[test]
fn agent_calls_equal_subagent_entries() {
    let report = medium(false);
    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for sub in report.sessions.values().flat_map(|e| &e.efficiency.subagents) {
        if sub.signals.raw.by_model.values().map(|t| t.total).sum::<u64>() > 0 {
            let key = sub
                .agent_type
                .clone()
                .unwrap_or_else(|| crate::report::UNKNOWN_AGENT_TYPE.to_string());
            *counts.entry(key).or_default() += 1;
        }
    }
    assert!(counts.len() > 1, "medium must carry several subagent types: {counts:?}");
    for (name, n) in &counts {
        expected.insert(name.clone(), crate::fmt::format_int(*n));
    }
    expected.insert(crate::report::MAIN_SESSION_BUCKET.to_string(), "n/a".to_string());
    assert_eq!(agent_calls(&report), expected);

    let rows = build_efficiency_view(&report).agent_type_costs;
    assert_eq!(
        displayed_cents(&rows),
        crate::cents::to_cents(report.totals.spend_usd),
        "the agent rows still sum to the headline"
    );
}

/// `--no-rollup` explodes each subagent into its own row, holding that one subagent; the per-type
/// Calls do not move. BITES: count the residual row's (empty) list, or count per row not per entry.
#[test]
fn agent_calls_same_under_no_rollup() {
    let rollup = agent_calls(&medium(false));
    let exploded = agent_calls(&medium(true));
    assert!(rollup.len() > 2, "{rollup:?}");
    assert_eq!(rollup, exploded);
}

/// A 0-token untyped subagent spent nothing, so it has no cost bucket and no call: no `unknown`
/// row. `(main-session)` is not a spawn and shows `n/a`, never `0`. BITES: drop the token filter.
#[test]
fn zero_token_subagent_adds_no_call() {
    let mut report = sample_report();
    for entry in report.sessions.values_mut() {
        entry.efficiency.subagents = vec![
            spender("agent-z", None, 0),
            spender("agent-g", Some("general-purpose"), 500),
        ];
        entry.agent_type_costs = BTreeMap::from([
            (crate::report::MAIN_SESSION_BUCKET.to_string(), cost(1_000, 0.3)),
            ("general-purpose".to_string(), cost(500, 0.1)),
        ]);
    }
    let eff = build_efficiency_view(&report);
    assert!(
        eff.agent_type_costs
            .iter()
            .all(|r| r.name != crate::report::UNKNOWN_AGENT_TYPE),
        "a 0-token untyped subagent makes no row"
    );
    let general = cell(&eff.agent_type_costs, "general-purpose");
    assert_eq!(general.calls.as_deref(), Some("2"), "one tokened spawn per session");
    let main = cell(&eff.agent_type_costs, crate::report::MAIN_SESSION_BUCKET);
    assert_eq!(main.calls.as_deref(), Some("n/a"));
    assert_eq!(main.per_call.as_deref(), Some("n/a"));
    assert!(main.counts.is_none(), "no chart draws the residual");
}

/// Hand-built: two sessions both hold `(main-session)`, one holds `implementer` with two spawns and
/// exactly $3.00 of cents. Sessions is one per bucket per entry; $/call is DOLLARS over calls, so
/// `$1.50`, never `$150.00`. BITES: divide raw cents, or key main-session on zero spawns.
#[test]
fn agent_sessions_and_per_call_values() {
    let mut report = sample_report();
    let mut entries = std::mem::take(&mut report.sessions).into_iter();
    let (id_a, mut a) = entries.next().unwrap();
    let (id_b, mut b) = entries.next().unwrap();
    a.spend_usd = Some(4.0);
    a.agent_type_costs = BTreeMap::from([
        (crate::report::MAIN_SESSION_BUCKET.to_string(), cost(1_000, 1.0)),
        ("implementer".to_string(), cost(2_000, 3.0)),
    ]);
    a.efficiency.subagents = vec![
        spender("agent-1", Some("implementer"), 1_000),
        spender("agent-2", Some("implementer"), 1_000),
    ];
    b.spend_usd = Some(2.0);
    b.agent_type_costs = BTreeMap::from([(crate::report::MAIN_SESSION_BUCKET.to_string(), cost(500, 2.0))]);
    b.efficiency.subagents = Vec::new();
    report.sessions.insert(id_a, a);
    report.sessions.insert(id_b, b);

    let eff = build_efficiency_view(&report);
    let implementer = cell(&eff.agent_type_costs, "implementer");
    assert_eq!(implementer.calls.as_deref(), Some("2"));
    assert_eq!(implementer.sessions.as_deref(), Some("1"));
    assert_eq!(implementer.spend, "$3.00");
    assert_eq!(implementer.per_call.as_deref(), Some("$1.50"));
    let counts = implementer.counts.unwrap();
    assert_eq!(
        (counts.calls, counts.spend, counts.counted_spend),
        (2, 3.0, 3.0),
        "dollars, not cents"
    );
    assert!(!counts.partial);
    assert!(implementer.server.is_none() && implementer.time.is_none() && implementer.avg.is_none());

    let main = cell(&eff.agent_type_costs, crate::report::MAIN_SESSION_BUCKET);
    assert_eq!(main.sessions.as_deref(), Some("2"));
    assert_eq!(main.calls.as_deref(), Some("n/a"));
    assert_eq!(main.per_call.as_deref(), Some("n/a"));
    assert_eq!(main.spend, "$3.00");
}
