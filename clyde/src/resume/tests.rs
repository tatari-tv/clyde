#![allow(clippy::unwrap_used)]

use super::*;
use chrono::Utc;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// SessionRecord fixture helpers
// ---------------------------------------------------------------------------

/// Minimum-valid `SessionRecord` with all optional fields set to `None` / empty / sentinel
/// defaults. The caller overrides only the fields relevant to the branch under test.
fn base_record(transcript_path: PathBuf) -> SessionRecord {
    SessionRecord {
        id: 1,
        session_id: "aaaa1111-0000-0000-0000-000000000000".to_string(),
        cwd: None,
        project_dir: "-home-user-project".to_string(),
        transcript_path,
        title: None,
        first_prompt: None,
        summary: None,
        tags: vec![],
        tags_source: None,
        git_branch: None,
        repo: None,
        repo_source: None,
        model: None,
        n_msgs: 0,
        created: None,
        activity_at: None,
        modified: Utc::now(),
        cost: None,
        host: "testhost".to_string(),
        archived: false,
        staged_path: None,
    }
}

// ---------------------------------------------------------------------------
// plan_resume - branch coverage
// ---------------------------------------------------------------------------

/// Branch 1: no recorded `cwd` -> `NoCwd`.
#[test]
fn plan_resume_no_cwd_returns_no_cwd() {
    // transcript_path existence does not matter here because cwd is checked first.
    let rec = base_record(PathBuf::from("/nonexistent/path"));
    // cwd is already None in base_record.
    let action = plan_resume(&rec, vec![]);
    assert_eq!(
        action,
        ResumeAction::NoCwd {
            id: rec.session_id.clone()
        }
    );
}

/// Branch 2: `cwd` recorded but the directory does not exist -> `MissingDir`.
#[test]
fn plan_resume_missing_dir_returns_missing_dir() {
    let missing = PathBuf::from("/this/path/does/not/exist/ever");
    let mut rec = base_record(PathBuf::from("/nonexistent/transcript"));
    rec.cwd = Some(missing.to_string_lossy().to_string());

    let action = plan_resume(&rec, vec![]);
    assert_eq!(action, ResumeAction::MissingDir { dir: missing });
}

/// Branch 2b: `cwd` exists but is a file (not a directory) -> `MissingDir`.
#[test]
fn plan_resume_cwd_is_file_returns_missing_dir() {
    let tmp = TempDir::new().unwrap();
    // Create a regular file at the recorded cwd path.
    let file_path = tmp.path().join("not-a-dir");
    std::fs::write(&file_path, b"data").unwrap();

    let mut rec = base_record(PathBuf::from("/nonexistent/transcript"));
    rec.cwd = Some(file_path.to_string_lossy().to_string());

    let action = plan_resume(&rec, vec![]);
    assert_eq!(action, ResumeAction::MissingDir { dir: file_path });
}

/// Branch 3: cwd is a real directory AND transcript exists -> `Launch`.
/// Also verifies that `extra` is threaded into the `Launch` variant.
#[test]
fn plan_resume_live_transcript_returns_launch_with_extra() {
    let tmp = TempDir::new().unwrap();
    let cwd_dir = tmp.path().join("project");
    std::fs::create_dir(&cwd_dir).unwrap();

    let transcript = tmp.path().join("transcript.jsonl");
    std::fs::write(&transcript, b"{}").unwrap();

    let mut rec = base_record(transcript.clone());
    rec.cwd = Some(cwd_dir.to_string_lossy().to_string());

    let extra = vec!["--model".to_string(), "opus".to_string()];
    let action = plan_resume(&rec, extra.clone());
    assert_eq!(
        action,
        ResumeAction::Launch {
            dir: cwd_dir,
            id: rec.session_id.clone(),
            extra,
        }
    );
}

/// Branch 4: cwd is a real directory, transcript is gone, but staged copy exists -> `StagedOnly`.
#[test]
fn plan_resume_staged_only_returns_staged_only() {
    let tmp = TempDir::new().unwrap();
    let cwd_dir = tmp.path().join("project");
    std::fs::create_dir(&cwd_dir).unwrap();

    // transcript_path does NOT exist.
    let transcript = tmp.path().join("transcript.jsonl");

    // staged copy DOES exist.
    let staged = tmp.path().join("staged").join("transcript.jsonl");
    std::fs::create_dir(tmp.path().join("staged")).unwrap();
    std::fs::write(&staged, b"{}").unwrap();

    let mut rec = base_record(transcript);
    rec.cwd = Some(cwd_dir.to_string_lossy().to_string());
    rec.staged_path = Some(staged.clone());

    let action = plan_resume(&rec, vec![]);
    assert_eq!(action, ResumeAction::StagedOnly { staged });
}

/// Branch 5: cwd exists, transcript gone, staged path is also gone -> `Reaped`.
#[test]
fn plan_resume_reaped_when_both_paths_gone() {
    let tmp = TempDir::new().unwrap();
    let cwd_dir = tmp.path().join("project");
    std::fs::create_dir(&cwd_dir).unwrap();

    // Neither transcript nor staged path exists on disk.
    let transcript = tmp.path().join("transcript.jsonl");
    let staged = tmp.path().join("staged").join("transcript.jsonl");

    let mut rec = base_record(transcript);
    rec.cwd = Some(cwd_dir.to_string_lossy().to_string());
    rec.staged_path = Some(staged); // path set but file absent

    let action = plan_resume(&rec, vec![]);
    assert_eq!(action, ResumeAction::Reaped);
}

/// Branch 5b: cwd exists, transcript gone, no staged path at all -> `Reaped`.
#[test]
fn plan_resume_reaped_when_no_staged_path() {
    let tmp = TempDir::new().unwrap();
    let cwd_dir = tmp.path().join("project");
    std::fs::create_dir(&cwd_dir).unwrap();

    let transcript = tmp.path().join("transcript.jsonl");

    let mut rec = base_record(transcript);
    rec.cwd = Some(cwd_dir.to_string_lossy().to_string());
    // staged_path is None.

    let action = plan_resume(&rec, vec![]);
    assert_eq!(action, ResumeAction::Reaped);
}

/// `extra` is empty when no extra args are forwarded (happy path).
#[test]
fn plan_resume_launch_with_no_extra() {
    let tmp = TempDir::new().unwrap();
    let cwd_dir = tmp.path().join("project");
    std::fs::create_dir(&cwd_dir).unwrap();

    let transcript = tmp.path().join("transcript.jsonl");
    std::fs::write(&transcript, b"{}").unwrap();

    let mut rec = base_record(transcript.clone());
    rec.cwd = Some(cwd_dir.to_string_lossy().to_string());

    let action = plan_resume(&rec, vec![]);
    assert_eq!(
        action,
        ResumeAction::Launch {
            dir: cwd_dir,
            id: rec.session_id.clone(),
            extra: vec![],
        }
    );
}

// ---------------------------------------------------------------------------
// Query resolution: id detection and the Resolution decision
// ---------------------------------------------------------------------------

fn words(ws: &[&str]) -> Vec<String> {
    ws.iter().map(|w| w.to_string()).collect()
}

fn hit(session_id: &str) -> sessions::SearchHit {
    let mut record = base_record(PathBuf::from("/nonexistent/transcript"));
    record.session_id = session_id.to_string();
    sessions::SearchHit {
        record,
        matched: sessions::MatchSource::Body,
        score: -1.0,
        snippet: String::new(),
        terms_matched: None,
        terms_total: None,
    }
}

fn results(ids: &[&str], fallback: Option<Fallback>, truncated: bool) -> SearchResults {
    SearchResults {
        count: ids.len(),
        results: ids.iter().map(|id| hit(id)).collect(),
        fallback,
        unenriched: sessions::model::Unenriched::default(),
        truncated,
    }
}

const FULL_ID: &str = "b8024af2-61ac-4746-98c2-6e0f106459e7";

#[test]
fn looks_like_id_accepts_full_uuid_prefix_and_hex_word() {
    assert_eq!(looks_like_id(&words(&[FULL_ID])), Some(FULL_ID));
    assert_eq!(looks_like_id(&words(&["b8024af2"])), Some("b8024af2"));
    assert_eq!(looks_like_id(&words(&["beef"])), Some("beef"));
}

#[test]
fn looks_like_id_rejects_words_urls_multiple_args_and_non_hex() {
    assert_eq!(looks_like_id(&words(&["marquee"])), None);
    assert_eq!(
        looks_like_id(&words(&["https://marquee.internal.tatari.dev/p/~scott-idler/x/"])),
        None
    );
    assert_eq!(looks_like_id(&words(&["b8024af2", "beef"])), None);
    assert_eq!(looks_like_id(&words(&["fix-auth"])), None);
    // Uppercase hex is not what the catalog stores, and LIKE wildcards must never reach resolve_id.
    assert_eq!(looks_like_id(&words(&["B8024AF2"])), None);
    assert_eq!(looks_like_id(&words(&["b80%"])), None);
    assert_eq!(looks_like_id(&words(&["b80_"])), None);
    assert_eq!(looks_like_id(&words(&[""])), None);
    assert_eq!(looks_like_id(&[]), None);
}

#[test]
fn resolution_from_id_exact_full_id_is_silent() {
    let r = resolution_from_id(&words(&[FULL_ID]), FULL_ID);
    assert!(
        matches!(r, Some(Resolution::One { ref id, announce: false }) if id == FULL_ID),
        "{r:?}"
    );
}

#[test]
fn resolution_from_id_prefix_match_announces() {
    let r = resolution_from_id(&words(&[FULL_ID]), "b8024af2");
    assert!(
        matches!(r, Some(Resolution::One { ref id, announce: true }) if id == FULL_ID),
        "{r:?}"
    );
}

#[test]
fn resolution_from_id_zero_or_many_matches_fall_through_to_search() {
    assert!(resolution_from_id(&[], "beef").is_none());
    let two = words(&[
        "abcd1111-0000-0000-0000-000000000000",
        "abcd2222-0000-0000-0000-000000000000",
    ]);
    assert!(resolution_from_id(&two, "abcd").is_none());
}

#[test]
fn resolution_from_search_zero_hits_is_no_match() {
    let r = resolution_from_search(results(&[], None, false), "qqxz");
    assert!(matches!(r, Resolution::NoMatch), "{r:?}");
}

#[test]
fn resolution_from_search_single_and_hit_resumes_with_announce() {
    let r = resolution_from_search(results(&[FULL_ID], None, false), "marquee");
    assert!(
        matches!(r, Resolution::One { ref id, announce: true } if id == FULL_ID),
        "{r:?}"
    );
}

#[test]
fn resolution_from_search_single_or_fallback_hit_goes_to_picker() {
    let r = resolution_from_search(results(&[FULL_ID], Some(Fallback::Or), false), "marquee qqxz");
    match r {
        Resolution::Pick { results, header } => {
            assert_eq!(results.count, 1);
            assert_eq!(header, "no session matched all terms; showing any-term matches");
        }
        other => panic!("expected Pick, got {other:?}"),
    }
}

#[test]
fn resolution_from_search_single_truncated_hit_goes_to_picker() {
    let r = resolution_from_search(results(&[FULL_ID], None, true), "review");
    match r {
        Resolution::Pick { header, .. } => {
            assert_eq!(header, "showing the first 1 matches (truncated); refine the query");
        }
        other => panic!("expected Pick, got {other:?}"),
    }
}

#[test]
fn resolution_from_search_several_hits_go_to_picker_with_full_results() {
    let ids = [
        "e0fd2451-0000-0000-0000-000000000000",
        "d18ded3d-0000-0000-0000-000000000000",
        FULL_ID,
    ];
    let r = resolution_from_search(results(&ids, None, false), "marquee security audit");
    match r {
        Resolution::Pick { results, header } => {
            assert_eq!(results.count, 3);
            assert_eq!(results.results.len(), 3);
            assert_eq!(header, r#"3 sessions match "marquee security audit""#);
        }
        other => panic!("expected Pick, got {other:?}"),
    }
}

#[test]
fn candidates_phrase_pluralizes() {
    assert_eq!(candidates_phrase(1), "1 session matches");
    assert_eq!(candidates_phrase(3), "3 sessions match");
}
