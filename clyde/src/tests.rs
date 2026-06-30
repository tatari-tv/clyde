#![allow(clippy::unwrap_used)]

use super::*;
use chrono::Utc;
use sessions::SessionRecord;
use std::path::PathBuf;
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
        model: None,
        n_msgs: 0,
        created: None,
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

#[test]
fn truncate_title_is_char_boundary_safe() {
    let s = "héllo wörld ".repeat(20);
    // Must not panic on multibyte boundaries.
    let out = truncate_title(&s);
    assert!(out.chars().count() <= TITLE_DISPLAY_WIDTH);
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
