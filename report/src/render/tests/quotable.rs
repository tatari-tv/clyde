#![allow(clippy::unwrap_used)]

//! Phase 10: the quotable-facts whitelist measured and exercised against a REAL context block --
//! `build_context_block` output, not a hand-written JSON snippet -- so the narrowing and the
//! false-positive corpus are pinned to the shape the renderer actually produces.

use super::*;
use crate::outcome::{OutcomeTotals, Outcomes, PrRef};
use crate::quotable::{QuotableFacts, all_numeric_tokens};

/// A fixed splitmix64 step: distinct, well-spread hex per session without a rand dependency and
/// without a fixture that changes between runs.
fn hash(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A window shaped like a real one: uuid-keyed sessions, RFC3339 spans, commit shas, PR refs,
/// enrich `summary`/`tags`, and an UNTITLED session every fifth row. Identifier-dense the way the
/// live block is (design: at ~940KB it is mostly ids, timestamps and shas), which is what the
/// narrowing has to be measured against.
fn realistic_report(n: usize) -> Report {
    let mut sessions = BTreeMap::new();
    for i in 0..n {
        let mut models = BTreeMap::new();
        models.insert(
            "claude-opus-4-7".into(),
            ModelTokens {
                input: 1_000 + i as u64,
                output: 500 + i as u64,
                cache_5m_write: 0,
                cache_1h_write: 0,
                cache_read: 40_000 + i as u64,
                total: 41_500 + 3 * i as u64,
                spend_usd: Some(1.0 + i as f64),
            },
        );
        // A distinct uuid per session, spread the way real ones are, so `short-id` and the session
        // key carry the digit soup the pre-change whitelist mistook for quotable figures.
        let h = hash(i as u64);
        let uuid = format!(
            "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
            0xa14bc3d2u32.wrapping_add(i as u32),
            h as u16,
            (h >> 16) as u16 & 0xfff,
            (h >> 32) as u16,
            hash(h) & 0xffff_ffff_ffff
        );
        let day = 1 + (i % 28);
        let mut entry = session_entry(
            (i % 5 != 0).then(|| format!("ship the thing {i}")).as_deref(),
            Some("tatari-tv/clyde"),
            ts(&format!("2026-04-{day:02}T09:14:22Z")),
            ts(&format!("2026-04-{day:02}T11:02:41Z")),
            Some(1.0 + i as f64),
            models,
            Some(Outcomes {
                commits: vec![format!("{:016x}{:016x}{:08x}", hash(h), hash(h ^ 0x5eed), i)],
                prs: vec![PrRef {
                    number: 600 + i as u64,
                    url: format!("https://github.com/tatari-tv/clyde/pull/{}", 600 + i),
                    repository: Some("tatari-tv/clyde".into()),
                }],
                confluence_writes: 0,
                jira_writes: 0,
                slack_messages: 0,
                files_edited: 7 + 5 * i as u64,
                ..Default::default()
            }),
        );
        entry.summary = Some(format!(
            "Reworked the {i}th slice of the guard; 14 whitelisted tokens and 3 shas were involved."
        ));
        entry.tags = vec!["render".into(), "guard".into()];
        sessions.insert(uuid, entry);
    }

    let mut totals_models = BTreeMap::new();
    totals_models.insert("claude-opus-4-7".into(), opus_tokens());
    let mut report = Report {
        schema_version: 2,
        generated: ts("2026-04-29T19:42:08Z"),
        host: "desk".into(),
        since: ts("2026-04-01T00:00:00Z"),
        until: ts("2026-04-30T00:00:00Z"),
        outcomes_enabled: Some(true),
        notes: Vec::new(),
        totals: totals(n, (0..n).map(|i| 1.0 + i as f64).sum(), totals_models),
        sessions,
    };
    report.totals.outcomes = Some(OutcomeTotals {
        sessions_with_commits: n as u64,
        commits: n as u64,
        prs_opened: n as u64,
        confluence_writes: 0,
        jira_writes: 0,
        slack_messages: 0,
        files_edited: 7 * n as u64,
        lines_written: 310 * n as u64,
        lines_replaced: 96 * n as u64,
    });
    report
}

/// Phase 10 success criterion 1: the figure whitelist is under 20% of the pre-change whitelist on
/// the same fixture. The pre-change whitelist was literally `numeric_tokens(context)` -- a `Vec`
/// scanned with `contains` -- so its token count is the raw count, and that is what this asserts.
///
/// The two stricter readings are printed beside it and deliberately NOT asserted, because they are
/// not what the phase can deliver and pretending otherwise would hide it: distinct-set against
/// distinct-set, and the share of previously-accepted tokens the guard still accepts. Both are
/// recorded in the phase's implementation notes with the reason.
///
/// BITES: point the guard back at `all_numeric_tokens` and every one of the three goes to 100%.
#[test]
fn figure_whitelist_is_under_a_fifth_of_the_pre_change_whitelist() {
    let block = ctx(&realistic_report(60), false);
    let facts = QuotableFacts::from_context_json(&block).unwrap();

    let raw = crate::quotable::numeric_token_count(&block);
    let (distinct, retained, retained_share) = retained_share(&block, &facts);
    let share = 100.0 * facts.figure_count() as f64 / raw as f64;
    println!(
        "fixture: pre-change raw={raw} distinct={distinct} figures={} ({share:.1}% of raw, {:.1}% of distinct) \
         still-accepted={retained} ({retained_share:.1}%)",
        facts.figure_count(),
        100.0 * facts.figure_count() as f64 / distinct as f64,
    );
    assert!(
        share < 20.0,
        "figure whitelist must be under 20% of the pre-change whitelist: {} of {raw} ({share:.1}%)",
        facts.figure_count()
    );
}

/// How much of the pre-change whitelist the narrowed guard still accepts: every token the old guard
/// approved, restated as bare prose, and counted through the new guard. Tokenizers differ between
/// the two (the new one keeps `9,450.31` and `2026-07-14` whole), so comparing SET SIZES would
/// compare different units; what the criterion is really about is how many of the numbers that used
/// to sail through still do.
fn retained_share(block: &str, facts: &QuotableFacts) -> (usize, usize, f64) {
    let pre_change: Vec<String> = all_numeric_tokens(block).into_iter().collect();
    let as_prose = pre_change.join(" ");
    let rejected = facts.foreign_figures(&as_prose).len();
    let retained = pre_change.len() - rejected;
    (
        pre_change.len(),
        retained,
        100.0 * retained as f64 / pre_change.len() as f64,
    )
}

/// The same narrowing measured against a REAL collected window instead of a fixture:
/// `CLYDE_REAL_REPORT=/path/to/claude-report.json cargo test -p report -- --ignored measure`.
/// Ignored by default (CI has no collected artifact); this is how the numbers in the phase's
/// implementation notes were produced, and how Phase 13 can re-take them.
#[test]
#[ignore = "needs a real `report collect` artifact, path in CLYDE_REAL_REPORT"]
fn measure_narrowing_on_a_real_window() {
    let Ok(path) = std::env::var("CLYDE_REAL_REPORT") else {
        panic!("set CLYDE_REAL_REPORT to a `report collect` artifact");
    };
    let body = std::fs::read_to_string(&path).unwrap();
    let report: Report = serde_json::from_str(&body).unwrap();
    let block = ctx(&report, false);
    let facts = QuotableFacts::from_context_json(&block).unwrap();

    let raw = crate::quotable::numeric_token_count(&block);
    let (pre_change, retained, share) = retained_share(&block, &facts);
    println!(
        "real window: context_bytes={} sessions={} pre-change raw={raw} distinct={pre_change} \
         still-accepted={retained} ({share:.2}%) figures={}",
        block.len(),
        report.sessions.len(),
        facts.figure_count(),
    );
}

/// Phase 10 success criterion 2, end to end over a real context block: the planted "14 hours of
/// engineering time" is rejected, and the same block's pre-change whitelist contained `14` -- so
/// the sentence used to pass. Both halves asserted, or the test proves nothing.
///
/// KNOWN LIMIT, measured and recorded in the implementation notes: this holds at fixture scale and
/// NOT on a real 1,500-session window, where `14` is a genuine licensed count (a day with 14
/// sessions, a repo with 14 sessions, a session that edited 14 files). No whitelist OF VALUES can
/// separate a real count from a fabricated duration that happens to share its digits; closing that
/// needs a claim-shaped check (a figure followed by a time unit or an `x` multiplier), which is not
/// this phase's mechanism.
#[test]
fn planted_fourteen_hours_is_rejected_where_it_previously_passed() {
    let block = ctx(&realistic_report(20), false);
    let facts = QuotableFacts::from_context_json(&block).unwrap();

    assert!(
        all_numeric_tokens(&block).contains("14"),
        "the pre-change whitelist contained 14 (from ids, shas and summaries), so the old guard passed this"
    );
    let err = reject_foreign_numbers(
        "markdown",
        "The window saved roughly 14 hours of engineering time.",
        &facts,
    )
    .unwrap_err();
    assert!(
        format!("{err}").contains("\"14\""),
        "the rejection must name the fabricated figure: {err}"
    );
}

/// Regression guard for the dedup this phase removed from `foreign_figures` itself (one entry per
/// OCCURRENCE now, not per token, so `excerpt_at` can quote the exact span that was rejected). Left
/// unguarded at the citation layer, that turns one repeated fabrication into a wall of near-identical
/// lines. A real rejection on the `pathological` fixture (`~/.local/share/clyde/logs/report.log`,
/// 2026-07-27T09:21:16Z) named four distinct tokens (`01`, `05`, `09`, `16`) under the OLD deduped
/// code; had any one of those recurred densely in the artifact, per-occurrence citation (with no
/// grouping) would have multiplied that one token into one line per hit. `reject_foreign_numbers`
/// must still cite a repeated token exactly ONCE, name how many more times it recurred rather than
/// dropping the count silently, and keep the message a bounded size no matter how many times the
/// token repeats.
#[test]
fn a_token_repeated_past_the_cap_is_cited_once_with_the_elided_count_named() {
    let facts = QuotableFacts::from_context_json(r#"{"totals":{"spend":"$4.12"}}"#).unwrap();
    // 50 repeats of one fabricated token, well past any sane citation count. This exercises the
    // RECURRENCE collapse specifically (one distinct token), separate from `MAX_CITED`'s cap on how
    // many DISTINCT tokens get a full excerpt.
    let prose = vec!["the count was 77 that day."; 50].join(" ");

    let err = reject_foreign_numbers("markdown", &prose, &facts).unwrap_err();
    let msg = format!("{err}");

    assert_eq!(
        msg.matches("\"77\"").count(),
        1,
        "a token repeated 50 times must be cited exactly once, not once per occurrence: {msg}"
    );
    assert!(
        msg.contains("and 49 more occurrence"),
        "the elided repeat count must be named rather than silently dropped: {msg}"
    );
    assert!(
        msg.len() < 1000,
        "one repeated token must not balloon the message regardless of how many times it repeats: \
         {} bytes",
        msg.len()
    );
}

/// Phase 10 success criterion 3: three known-good artifacts, all figures passing. One markdown
/// narrative, one HTML artifact scanned as the renderer scans it (visible text only), and one
/// citation-heavy fragment. Every one of them cites an UNTITLED session by `short-id` and carries a
/// prose PR reference, because those are the two false positives narrowing is most likely to cause.
///
/// These stand in for Phase 13's committed goldens, which do not exist yet; Phase 13 re-runs this
/// criterion against them.
#[test]
fn all_three_known_good_artifacts_pass() {
    let report = realistic_report(20);
    let block = ctx(&report, false);
    let facts = QuotableFacts::from_context_json(&block).unwrap();

    // Artifact 1: the markdown narrative. Headline figures, a repo row, unit costs, an untitled
    // session cited by short-id, and a PR reference in prose.
    let markdown = "# Claude Code, April 2026\n\n\
        Across 2026-04-01 to 2026-04-30 (30 days, 20 of them active) the window ran 20 sessions \
        for $210.00 at published list rates, all of it attributed (100.0% covered, $0.00 \
        uncovered).\n\n\
        tatari-tv/clyde carried $210.00 and 830,570 tokens, producing 20 commits, 20 PRs and \
        140 files edited across 6,200 lines written.\n\n\
        The costliest untitled session, a14bc3e1, spent $16.00 on its own; its work landed in \
        PR 615 (https://github.com/tatari-tv/clyde/pull/615).\n\n\
        Unit costs: $10.50 per commit, $10.50 per PR, $10.50 per active day, with a $10.00 \
        median session and a $18.00 p90.\n";
    assert!(
        facts.foreign_figures(markdown).is_empty(),
        "known-good artifact 1 (markdown narrative) must pass"
    );

    // Artifact 2: the HTML artifact, scanned the way the renderer scans it -- CSS/JS numbers are
    // authored geometry and never reach the guard, but every visible figure does.
    let html = "<!doctype html><html><head><style>body{padding:24px;font-size:14px}\
        .bar{width:63.5%}</style><script>var t=1755;</script></head><body>\
        <h1>Claude Code, April 2026</h1>\
        <p class=\"lede\">20 sessions, $210.00, 2026-04-01 to 2026-04-30.</p>\
        <table><tr><td>tatari-tv/clyde</td><td>20</td><td>$210.00</td><td>830,570</td></tr></table>\
        <p>Untitled session a14bc3e1 ($16.00) shipped #615.</p>\
        <div class=\"bar\" style=\"width: 100%\"></div></body></html>";
    assert!(
        facts.foreign_figures(&visible_text(html)).is_empty(),
        "known-good artifact 2 (html) must pass"
    );

    // Artifact 3: the citation-dense fragment -- short shas, full shas, session spans, tags and a
    // verbatim title quote, which is where a narrowed whitelist most easily false-positives.
    let citations = "Evidence: untitled session a14bc3d2 (2026-04-01, $1.00, 41,500 tokens, \
        7 files edited) and a14bc3d3 (\"ship the thing 1\", $2.00), commit \
        a706dd2f4d197e6fad9f769ae33abd7b00000000 and its short form a706dd2, PRs #600 and #601, \
        tagged render and guard, spanning 2026-04-01T09:14:22Z to 2026-04-01T11:02:41Z.\n";
    assert!(
        facts.foreign_figures(citations).is_empty(),
        "known-good artifact 3 (citations) must pass"
    );
}
