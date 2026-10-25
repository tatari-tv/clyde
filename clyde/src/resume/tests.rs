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
