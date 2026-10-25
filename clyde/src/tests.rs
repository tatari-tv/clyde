#![allow(clippy::unwrap_used)]

use super::*;
use chrono::Utc;

#[test]
fn short_id_takes_first_eight_chars() {
    assert_eq!(short_id("9d4c1f28-7a3b-4a9c"), "9d4c1f28");
    assert_eq!(short_id("abc"), "abc");
}

#[test]
fn truncate_title_collapses_multiline_and_caps_width() {
    let multiline = "line one\n  line two\n\n   line three";
    assert_eq!(truncate_title(multiline), "line one line two line three");

    let long = "word ".repeat(50);
    let out = truncate_title(&long);
    assert_eq!(out.chars().count(), TITLE_DISPLAY_WIDTH);
    assert!(out.ends_with('…'));
}

/// The terminal cap composes with the STORED title's own marker instead of mangling it.
///
/// `session::ParsedSession::title` caps at 120 chars and marks the cut with an ASCII `...`. A stored
/// title of 78 content chars plus that marker is 81 chars, so this function's 79-char cut lands one
/// character into the marker and used to emit a dangling `.…`. Found by CodeRabbit on PR #82.
///
/// BITES: drop the `trim_end_matches` and the first assertion sees `.…`.
#[test]
fn truncate_title_does_not_stack_markers_on_an_already_marked_title() {
    // Exactly the boundary case: 78 content chars + the stored `...` marker.
    let stored = format!("{}...", "w".repeat(78));
    assert_eq!(stored.chars().count(), 81, "the fixture must exceed the terminal cap");
    let out = truncate_title(&stored);
    assert!(!out.contains(".…"), "a dangling dot before the ellipsis: {out:?}");
    assert!(out.ends_with('…'));
    assert!(
        !out.ends_with("..…") && !out.ends_with(".…"),
        "exactly one marker must survive: {out:?}"
    );

    // A stored title whose whole tail is the marker collapses to one marker, not four dots.
    let all_marker = format!("{}...", "w".repeat(TITLE_DISPLAY_WIDTH));
    let out = truncate_title(&all_marker);
    assert!(out.ends_with('…'));
    assert!(!out.contains(".…"), "{out:?}");
}

#[test]
fn truncate_title_is_char_boundary_safe() {
    let s = "héllo wörld ".repeat(20);
    // Must not panic on multibyte boundaries.
    let out = truncate_title(&s);
    assert!(out.chars().count() <= TITLE_DISPLAY_WIDTH);
}

// ---------------------------------------------------------------------------
// dormant_after_duration - arg -> ExportContext mapping
// ---------------------------------------------------------------------------

/// A `--dormant-after` span (e.g. `7d`) recovers as (approximately) that span's `chrono::Duration`,
/// via the shared `parse_since` span parser, not a second one.
#[test]
fn dormant_after_duration_recovers_the_span() {
    let now = Utc::now();
    let dur = dormant_after_duration("7d", common::DateTz::Utc, now).expect("valid span");
    // parse_since resolves "7d" against its OWN internal `Utc::now()` call (a few micros after
    // ours), so the recovered duration is 7 days minus that small epsilon - never over 7 days,
    // and no more than a second short of it in a healthy test run.
    let seven_days = chrono::Duration::days(7);
    assert!(
        dur <= seven_days,
        "recovered duration must not exceed the requested span: {dur}"
    );
    assert!(
        dur > seven_days - chrono::Duration::seconds(5),
        "recovered duration should be within a few seconds of the requested span: {dur}"
    );
}

/// An unparseable span is a loud error, not a silently wrong duration.
#[test]
fn dormant_after_duration_rejects_garbage() {
    let now = Utc::now();
    let result = dormant_after_duration("not-a-span", common::DateTz::Utc, now);
    assert!(result.is_err(), "garbage span must error, not silently default");
}

#[test]
fn is_debug_level_selects_debug_form_only_for_debug_and_trace() {
    // debug/trace -> Debug rendering (with Location), case-insensitively.
    assert!(is_debug_level("debug"));
    assert!(is_debug_level("trace"));
    assert!(is_debug_level("DEBUG"));
    assert!(is_debug_level("Trace"));

    // Default and quieter levels -> clean cause-chain rendering.
    assert!(!is_debug_level("info"));
    assert!(!is_debug_level("warn"));
    assert!(!is_debug_level("error"));
    assert!(!is_debug_level(DEFAULT_LOG_LEVEL));

    // Unparseable levels fall back to the non-debug (clean) form.
    assert!(!is_debug_level("nonsense"));
}

// ---------------------------------------------------------------------------
// format_error / render_error: one rendering for the whole binary
// ---------------------------------------------------------------------------

/// A `.context()`-chained report, the shape every real clyde failure has by the time it reaches
/// `main`: an inner cause plus the outer context the caller attached.
fn chained_report() -> eyre::Report {
    eyre::eyre!("no session matches \"deadbeef-0000\"").wrap_err("failed to set scope override")
}

#[test]
fn format_error_at_default_verbosity_is_the_cause_chain_with_no_location() {
    let rendered = format_error(&chained_report(), false);

    // The whole chain reaches the user -- Display alone would drop the inner cause.
    assert!(
        rendered.contains("failed to set scope override"),
        "outer context missing from {rendered:?}"
    );
    assert!(
        rendered.contains("no session matches \"deadbeef-0000\""),
        "inner cause missing from {rendered:?}"
    );

    // This is the defect Phase 1 removes: a CLI user must never see a source location, and the
    // line number must not move with unrelated edits to main.rs.
    assert!(
        !rendered.contains("Location:"),
        "default rendering leaked a source location: {rendered:?}"
    );
    // eyre's own `Error: ` prefix is dropped for parity with report/cost/permit/efficiency.
    assert!(
        !rendered.starts_with("Error:"),
        "default rendering kept eyre's Error: prefix: {rendered:?}"
    );
}

#[test]
fn format_error_under_debug_keeps_the_location_escape_hatch() {
    let rendered = format_error(&chained_report(), true);

    // `--log-level debug|trace` is the diagnosis path; the location capture is the point of it.
    assert!(
        rendered.contains("Location:"),
        "debug rendering lost the location capture: {rendered:?}"
    );
    assert!(
        rendered.contains("no session matches \"deadbeef-0000\""),
        "debug rendering dropped the inner cause: {rendered:?}"
    );

    // The two forms must actually differ, or the escape hatch is a dead branch that looks alive.
    assert_ne!(
        rendered,
        format_error(&chained_report(), false),
        "debug and default renderings are identical"
    );
}

#[test]
fn cost_statusline_parses_without_a_short_flag_collision() {
    // `cost statusline --list` used to declare `short`, colliding with this crate's fleet-wide
    // global `-l/--log-level`. The result was profile-dependent: every DEBUG build panicked before
    // parsing on any `clyde cost statusline` invocation, while RELEASE builds compiled the
    // assertion out and silently resolved `-l` to `--list` instead of the global.
    //
    // This must PARSE rather than call `Command::debug_assert`: clap propagates a `global = true`
    // arg into subcommands during parse, not during build, so the duplicate does not exist yet at
    // build time and `debug_assert` walks right past it (verified -- that version of this test did
    // not fail when the short flag was reintroduced).
    use clap::Parser;
    let cli = Cli::try_parse_from(["clyde", "cost", "statusline", "--list"]).expect("parses");
    assert!(matches!(cli.command, Command::Cost(_)));

    // `-l` reaches the global log level here, exactly as it does for every other subcommand.
    let cli = Cli::try_parse_from(["clyde", "-l", "debug", "cost", "statusline", "--list"]).expect("parses");
    assert_eq!(cli.log_level.as_deref(), Some("debug"));
}
