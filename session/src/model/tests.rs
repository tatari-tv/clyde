#![allow(clippy::unwrap_used)]

use super::*;

/// A `ParsedSession` carrying only the three fields [`ParsedSession::title`] reads. Everything else is
/// inert filler: the derivation is a pure function of these three, and a builder that pretended
/// otherwise would invite a test to depend on a field the code never touches.
fn titled(ai_title: Option<&str>, first_prompt: Option<&str>, command_name: Option<&str>) -> ParsedSession {
    ParsedSession {
        session_id: "s".into(),
        cwd: None,
        project_dir: PathBuf::from("/p"),
        ai_title: ai_title.map(str::to_string),
        first_prompt: first_prompt.map(str::to_string),
        command_name: command_name.map(str::to_string),
        git_branch: None,
        model: None,
        n_msgs: 0,
        created: None,
        activity_at: None,
        modified: DateTime::from_timestamp(0, 0).unwrap(),
        body: String::new(),
        jsonl_paths: Vec::new(),
    }
}

fn title_of(ai_title: Option<&str>, first_prompt: Option<&str>, command_name: Option<&str>) -> Option<String> {
    titled(ai_title, first_prompt, command_name).title()
}

#[test]
fn the_fallback_chain_is_ai_title_then_prompt_then_command() {
    assert_eq!(
        title_of(Some("real title"), Some("a prompt"), Some("cmd")).as_deref(),
        Some("real title")
    );
    assert_eq!(
        title_of(None, Some("a prompt"), Some("cmd")).as_deref(),
        Some("a prompt")
    );
    assert_eq!(title_of(None, None, Some("cmd")).as_deref(), Some("cmd"));
    assert_eq!(title_of(None, None, None), None);
}

/// A short title is passed through byte for byte: the transforms must be invisible in the normal case,
/// which is ~96% of sessions.
#[test]
fn a_short_single_line_title_is_unchanged() {
    let t = "Terraform Marquee bucket setup";
    assert_eq!(title_of(Some(t), None, None).as_deref(), Some(t));
    assert!(!t.contains(TITLE_ELLIPSIS));
}

/// The reported bug: a session with no `ai-title` whose first prompt is a multi-line agent launch or a
/// context-compaction summary. Before this, the whole 2,000-char blob was the title.
///
/// BITES: drop the `.lines()` step and the title carries the second line's text; drop the
/// `MAX_TITLE_CHARS` cap and it runs to the full length.
#[test]
fn a_multiline_prompt_yields_only_its_first_line() {
    let prompt = "Implement exactly **Phase 2** of the design doc at:\n\
                  `/home/saidler/repos/tatari-tv/slack-cli/main/docs/design/2026-07-01-slack-cli.md`\n\n\
                  Working directory: `/home/saidler/repos/tatari-tv/slack-cli/main`";
    let got = title_of(None, Some(prompt), None).unwrap();
    assert_eq!(got, "Implement exactly **Phase 2** of the design doc at:");
    assert!(!got.contains("Working directory"));
    assert!(!got.contains('\n'), "a title is one line: {got:?}");
}

/// Leading blank lines are skipped rather than yielding an empty title.
#[test]
fn leading_blank_lines_are_skipped() {
    assert_eq!(
        title_of(None, Some("\n\n   \nthe actual first line\nmore"), None).as_deref(),
        Some("the actual first line")
    );
    // Nothing but whitespace has no title to give, and must not become `Some("")`.
    assert_eq!(title_of(None, Some("\n\n   \n"), None), None);
}

/// A single very long LINE is still bounded, and the cut lands on a word boundary.
///
/// BITES: remove the cap and the assertion on the char count fails at the source's full length.
#[test]
fn a_long_single_line_is_capped_at_a_word_boundary() {
    let long = "we need to reconcile the archived session spend against the settled analytics export \
                because the report is undercounting by roughly thirty percent on every host we checked";
    let got = title_of(None, Some(long), None).unwrap();
    assert!(
        got.chars().count() <= MAX_TITLE_CHARS + TITLE_ELLIPSIS.chars().count(),
        "capped title is {} chars: {got:?}",
        got.chars().count()
    );
    assert!(
        got.ends_with(TITLE_ELLIPSIS),
        "a truncated title must mark itself: {got:?}"
    );
    // The word boundary: the cut must not split a word, so the text before the ellipsis is a prefix of
    // the source ending at a whole word.
    let body = got.strip_suffix(TITLE_ELLIPSIS).unwrap();
    assert!(long.starts_with(body), "the kept text must be a prefix of the source");
    assert!(
        !body.ends_with(' '),
        "trailing space before the ellipsis is sloppy: {got:?}"
    );
    // Char-indexed, not byte-sliced. `long[body.len()..]` would read naturally here and is exactly the
    // panic this whole function guards against, so the test must not commit it either --
    // `clippy::string_slice` is denied crate-wide and caught it.
    assert_eq!(
        long.chars().nth(body.chars().count()),
        Some(' '),
        "the cut must land on a word boundary: {got:?}"
    );
}

/// Truncation counts CHARACTERS, not bytes. A byte-slice cut here panics; this is the regression test
/// for that whole class, so it uses multibyte text that straddles the cap.
///
/// BITES: replace `chars().take(max)` with `&s[..max]` and this panics on a char-boundary split.
#[test]
fn truncation_is_char_safe_on_multibyte_text() {
    // 4-byte chars, so a byte cut at 120 lands mid-character.
    let emoji = "🔥".repeat(400);
    let got = title_of(None, Some(&emoji), None).unwrap();
    assert!(got.chars().count() <= MAX_TITLE_CHARS + TITLE_ELLIPSIS.chars().count());
    assert!(got.ends_with(TITLE_ELLIPSIS));

    // CJK, no spaces at all: no word boundary exists inside the cap, so the hard cut applies.
    let cjk = "日本語のテキストがここにあります".repeat(30);
    let got = title_of(None, Some(&cjk), None).unwrap();
    assert_eq!(got.chars().count(), MAX_TITLE_CHARS + TITLE_ELLIPSIS.chars().count());

    // Mixed, with the cap landing inside a multibyte run.
    let mixed = format!("{}{}", "a".repeat(MAX_TITLE_CHARS - 1), "é".repeat(50));
    let got = title_of(None, Some(&mixed), None).unwrap();
    assert!(got.chars().count() <= MAX_TITLE_CHARS + TITLE_ELLIPSIS.chars().count());
}

/// A word boundary very early in the cap is ignored in favour of the hard cut: one short word followed
/// by an enormous token would otherwise reduce the title to that one word.
#[test]
fn a_too_early_word_boundary_falls_back_to_the_hard_cut() {
    let s = format!("fix {}", "x".repeat(500));
    let got = title_of(None, Some(&s), None).unwrap();
    assert_ne!(
        got,
        format!("fix{TITLE_ELLIPSIS}"),
        "must not collapse to the first word"
    );
    assert_eq!(got.chars().count(), MAX_TITLE_CHARS + TITLE_ELLIPSIS.chars().count());
}

/// Exactly at the cap is NOT truncated; one char over is. The off-by-one both ways.
#[test]
fn the_cap_boundary_is_inclusive() {
    let at = "a".repeat(MAX_TITLE_CHARS);
    assert_eq!(title_of(None, Some(&at), None).as_deref(), Some(at.as_str()));

    let over = "a".repeat(MAX_TITLE_CHARS + 1);
    let got = title_of(None, Some(&over), None).unwrap();
    assert!(got.ends_with(TITLE_ELLIPSIS));
    assert_eq!(got.chars().count(), MAX_TITLE_CHARS + TITLE_ELLIPSIS.chars().count());
}

/// The cap applies to `ai_title` too, not just the fallback. A uniform rule cannot be defeated by a
/// future source, and it is a no-op for every well-formed `ai-title`.
#[test]
fn the_cap_applies_to_every_source() {
    let long = "T".repeat(400);
    for (ai, prompt, cmd) in [
        (Some(long.as_str()), None, None),
        (None, Some(long.as_str()), None),
        (None, None, Some(long.as_str())),
    ] {
        let got = title_of(ai, prompt, cmd).unwrap();
        assert!(got.ends_with(TITLE_ELLIPSIS), "source must still be capped: {got:?}");
    }
}
