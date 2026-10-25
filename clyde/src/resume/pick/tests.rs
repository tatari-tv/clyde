#![allow(clippy::unwrap_used)]

use super::*;
use chrono::{TimeZone, Utc};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use tempfile::TempDir;

const ID_A: &str = "aaaa1111-0000-0000-0000-000000000000";
const ID_B: &str = "bbbb2222-0000-0000-0000-000000000000";
const ID_C: &str = "cccc3333-0000-0000-0000-000000000000";
const FOREIGN_ID: &str = "ffff9999-0000-0000-0000-000000000000";

/// The env-scrub test sets the two fzf vars on this process; every stub test holds this so no other
/// stub child is spawned while the environ block is being written.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Poison-tolerant: one failed stub test must not cascade into every other one.
fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn record(session_id: &str) -> SessionRecord {
    SessionRecord {
        id: 1,
        session_id: session_id.to_string(),
        cwd: Some("/home/user/repos/org/marquee".to_string()),
        project_dir: "-home-user-repos-org-marquee".to_string(),
        transcript_path: PathBuf::from("/nonexistent/transcript.jsonl"),
        title: Some("Marquee report publishing".to_string()),
        first_prompt: Some("publish the audit report".to_string()),
        summary: Some("Published the security audit to marquee.".to_string()),
        tags: vec!["rust".to_string(), "security-audit".to_string()],
        tags_source: None,
        git_branch: Some("main".to_string()),
        repo: None,
        repo_source: None,
        model: None,
        n_msgs: 3182,
        created: Some(Utc.with_ymd_and_hms(2026, 10, 22, 9, 30, 0).unwrap()),
        activity_at: None,
        modified: Utc.with_ymd_and_hms(2026, 10, 23, 17, 5, 0).unwrap(),
        cost: None,
        host: "testhost".to_string(),
        archived: false,
        staged_path: None,
    }
}

fn item<'a>(rec: &'a SessionRecord, snippet: &'a str) -> PickItem<'a> {
    PickItem { record: rec, snippet }
}

fn ids(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

// ---------------------------------------------------------------------------
// fzf_line
// ---------------------------------------------------------------------------

#[test]
fn fzf_line_formats_id_field_then_display_columns() {
    let rec = record(ID_A);
    let line = fzf_line(&item(&rec, ""));
    assert_eq!(
        line,
        format!("{ID_A}\taaaa1111  2026-10-23  marquee:main  3182  Marquee report publishing  [rust security-audit]")
    );
}

#[test]
fn fzf_line_flattens_tabs_newlines_and_keeps_multibyte_chars() {
    let mut rec = record(ID_A);
    rec.title = Some("caf\u{e9}\tdeploy\nstep two\r\n\u{1f680} launch".to_string());
    rec.tags = vec!["a\tb".to_string(), "c\nd".to_string()];
    let line = fzf_line(&item(&rec, ""));
    assert!(!line.contains('\n') && !line.contains('\r'), "one line: {line:?}");
    assert_eq!(line.matches('\t').count(), 1, "exactly one id delimiter: {line:?}");
    let (id, display) = line.split_once('\t').unwrap();
    assert_eq!(id, ID_A);
    assert!(
        display.contains("caf\u{e9} deploy step two \u{1f680} launch"),
        "{display:?}"
    );
    assert!(display.contains("[a b c d]"), "{display:?}");
}

#[test]
fn fzf_line_omits_empty_tags_and_detached_branch() {
    let mut rec = record(ID_A);
    rec.tags.clear();
    rec.git_branch = Some("HEAD".to_string());
    let line = fzf_line(&item(&rec, ""));
    assert!(line.ends_with("  marquee  3182  Marquee report publishing"), "{line:?}");
}

// ---------------------------------------------------------------------------
// preview_text
// ---------------------------------------------------------------------------

#[test]
fn preview_text_lists_facts_summary_and_snippet() {
    let rec = record(ID_A);
    let text = preview_text(&item(&rec, "the **audit** run"));
    assert_eq!(
        text,
        "Marquee report publishing\n\
         marquee:main\n\
         cwd: /home/user/repos/org/marquee\n\
         created: 2026-10-22 09:30 UTC\n\
         modified: 2026-10-23 17:05 UTC\n\
         msgs: 3182\n\
         tags: rust security-audit\n\
         \n\
         Published the security audit to marquee.\n\
         \n\
         match: the **audit** run\n"
    );
}

#[test]
fn preview_text_falls_back_to_first_prompt_without_summary() {
    let mut rec = record(ID_A);
    rec.summary = None;
    let text = preview_text(&item(&rec, ""));
    assert!(text.contains("\n\npublish the audit report\n"), "{text:?}");
    assert!(
        !text.contains("match:"),
        "no snippet line when the hit has none: {text:?}"
    );
}

#[test]
fn preview_text_placeholders_when_fields_missing() {
    let mut rec = record(ID_A);
    rec.summary = None;
    rec.first_prompt = None;
    rec.title = None;
    rec.cwd = None;
    rec.created = None;
    rec.tags.clear();
    let text = preview_text(&item(&rec, ""));
    assert!(text.starts_with("(untitled)\n"), "{text:?}");
    assert!(text.contains("cwd: -\ncreated: -\n"), "{text:?}");
    assert!(text.contains("tags: -\n"), "{text:?}");
    assert!(text.contains("(no summary or first prompt)"), "{text:?}");
}

// ---------------------------------------------------------------------------
// parse_selection
// ---------------------------------------------------------------------------

#[test]
fn parse_selection_takes_field_one_and_trims_newline() {
    let got = parse_selection(&format!("{ID_B}\tbbbb2222  2026-10-23  x\n"), &ids(&[ID_A, ID_B])).unwrap();
    assert_eq!(got, ID_B);
}

#[test]
fn parse_selection_rejects_empty_output() {
    let err = parse_selection("", &ids(&[ID_A])).unwrap_err();
    assert!(err.to_string().contains("no selection"), "{err}");
    assert!(parse_selection("\n", &ids(&[ID_A])).is_err());
}

#[test]
fn parse_selection_rejects_tabless_line() {
    let err = parse_selection(&format!("{ID_A}\n"), &ids(&[ID_A])).unwrap_err();
    assert!(err.to_string().contains("no tab"), "{err}");
}

#[test]
fn parse_selection_rejects_id_not_among_candidates() {
    let err = parse_selection(&format!("{FOREIGN_ID}\tx\n"), &ids(&[ID_A, ID_B])).unwrap_err();
    assert!(err.to_string().contains("not one of the 2 candidates"), "{err}");
}

// ---------------------------------------------------------------------------
// preview_command / write_preview
// ---------------------------------------------------------------------------

#[test]
fn preview_command_quotes_dir_and_leaves_field_placeholder() {
    assert_eq!(
        preview_command(Path::new("/tmp/clyde-resume-x")).unwrap(),
        "cat '/tmp/clyde-resume-x'/{1}"
    );
}

#[test]
fn preview_command_refuses_dir_with_single_quote() {
    let err = preview_command(Path::new("/tmp/it's")).unwrap_err();
    assert!(err.to_string().contains("single quote"), "{err}");
}

#[test]
fn write_preview_refuses_id_that_is_not_a_plain_file_name() {
    let dir = TempDir::new().unwrap();
    for bad in ["../escape", "a/b", "..", "", "a\tb"] {
        let rec = record(bad);
        assert!(
            write_preview(dir.path(), &item(&rec, "")).is_err(),
            "{bad:?} must be refused"
        );
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

// ---------------------------------------------------------------------------
// run_picker with a stub fzf
// ---------------------------------------------------------------------------

/// A stub `fzf` that records its argv (one per line) to `<scratch>/args`, then runs `body` with the
/// rows on stdin.
struct Stub {
    scratch: TempDir,
    fzf: PathBuf,
}

impl Stub {
    fn new(body: &str) -> Self {
        let scratch = TempDir::new().unwrap();
        let fzf = scratch.path().join("fzf");
        let args = scratch.path().join("args");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n{}\n",
            args.display(),
            body.replace("@SCRATCH@", &scratch.path().display().to_string())
        );
        std::fs::write(&fzf, script).unwrap();
        std::fs::set_permissions(&fzf, std::fs::Permissions::from_mode(0o755)).unwrap();
        Stub { scratch, fzf }
    }

    fn args(&self) -> Vec<String> {
        std::fs::read_to_string(self.scratch.path().join("args"))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// The preview tempdir run_picker handed fzf, parsed out of `--preview "cat '<dir>'/{1}"`.
    fn preview_dir(&self) -> PathBuf {
        let args = self.args();
        let at = args.iter().position(|a| a == "--preview").unwrap();
        let cmd = &args[at + 1];
        let dir = cmd.strip_prefix("cat '").unwrap().strip_suffix("'/{1}").unwrap();
        PathBuf::from(dir)
    }

    fn assert_preview_dir_removed(&self) {
        let dir = self.preview_dir();
        assert!(dir.starts_with(std::env::temp_dir()), "{}", dir.display());
        assert!(!dir.exists(), "preview tempdir {} outlived run_picker", dir.display());
    }
}

fn three_records() -> Vec<SessionRecord> {
    vec![record(ID_A), record(ID_B), record(ID_C)]
}

fn items(recs: &[SessionRecord]) -> Vec<PickItem<'_>> {
    recs.iter().map(|r| item(r, "")).collect()
}

#[test]
fn run_picker_returns_the_row_fzf_printed() {
    let guard = env_lock();
    let stub = Stub::new("sed -n 2p");
    let recs = three_records();
    let outcome = run_picker(&stub.fzf, &items(&recs), "3 sessions match \"x\"").unwrap();
    assert_eq!(outcome, PickOutcome::Picked(ID_B.to_string()));
    stub.assert_preview_dir_removed();
    drop(guard);
}

#[test]
fn run_picker_passes_the_designed_fzf_args() {
    let guard = env_lock();
    let stub = Stub::new("head -n 1");
    let recs = three_records();
    run_picker(&stub.fzf, &items(&recs), "the header").unwrap();
    let args = stub.args();
    let dir = stub.preview_dir();
    let expected: Vec<String> = [
        "--delimiter",
        "\t",
        "--with-nth",
        "2..",
        "--no-multi",
        "--layout",
        "reverse",
        "--header",
        "the header",
        "--with-shell",
        "sh -c",
        "--preview",
        &format!("cat '{}'/{{1}}", dir.display()),
        "--preview-window",
        "down,50%,wrap",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(args, expected);
    stub.assert_preview_dir_removed();
    drop(guard);
}

#[test]
fn run_picker_writes_rows_and_one_preview_per_candidate() {
    let guard = env_lock();
    // Copy stdin and the preview files out before run_picker removes the dir.
    let stub = Stub::new(
        r#"cat > '@SCRATCH@/rows'
prev=
for a in "$@"; do [ "$prev" = --preview ] && p=$a; prev=$a; done
d=${p#cat \'}
d=${d%\'/\{1\}}
mkdir '@SCRATCH@/previews'
cp "$d"/* '@SCRATCH@/previews/'
head -n 1 '@SCRATCH@/rows'"#,
    );
    let recs = three_records();
    run_picker(&stub.fzf, &items(&recs), "h").unwrap();
    let rows = std::fs::read_to_string(stub.scratch.path().join("rows")).unwrap();
    let expected: String = recs.iter().map(|r| format!("{}\n", fzf_line(&item(r, "")))).collect();
    assert_eq!(rows, expected);
    let previews = stub.scratch.path().join("previews");
    assert_eq!(std::fs::read_dir(&previews).unwrap().count(), recs.len());
    for rec in &recs {
        let got = std::fs::read_to_string(previews.join(&rec.session_id)).unwrap();
        assert_eq!(got, preview_text(&item(rec, "")));
    }
    stub.assert_preview_dir_removed();
    drop(guard);
}

#[test]
fn run_picker_exit_130_is_cancelled() {
    let guard = env_lock();
    let stub = Stub::new("cat > /dev/null\nexit 130");
    let recs = three_records();
    let outcome = run_picker(&stub.fzf, &items(&recs), "h").unwrap();
    assert_eq!(outcome, PickOutcome::Cancelled);
    stub.assert_preview_dir_removed();
    drop(guard);
}

#[test]
fn run_picker_rejects_an_id_it_did_not_supply() {
    let guard = env_lock();
    let stub = Stub::new(&format!("cat > /dev/null\nprintf '{FOREIGN_ID}\\tforged row\\n'"));
    let recs = three_records();
    let err = run_picker(&stub.fzf, &items(&recs), "h").unwrap_err();
    assert!(err.to_string().contains(FOREIGN_ID), "{err}");
    stub.assert_preview_dir_removed();
    drop(guard);
}

#[test]
fn run_picker_other_exit_code_is_an_error_carrying_fzf_stderr() {
    let guard = env_lock();
    let stub = Stub::new("echo 'unknown option: --bogus' >&2\nexit 2");
    let recs = three_records();
    let err = run_picker(&stub.fzf, &items(&recs), "h").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("code 2") && msg.contains("unknown option: --bogus"),
        "{msg}"
    );
    stub.assert_preview_dir_removed();
    drop(guard);
}

#[test]
fn run_picker_scrubs_user_fzf_defaults_from_the_child_env() {
    let guard = env_lock();
    // SAFETY: every test that spawns a stub child holds ENV_LOCK, and no other test in this binary
    // reads these two vars. They are removed again before the lock is released.
    unsafe {
        std::env::set_var("FZF_DEFAULT_OPTS", "--select-1");
        std::env::set_var("FZF_DEFAULT_OPTS_FILE", "/nonexistent/fzfrc");
    }
    let stub = Stub::new("env > '@SCRATCH@/env'\nhead -n 1");
    let recs = three_records();
    let result = run_picker(&stub.fzf, &items(&recs), "h");
    // SAFETY: as above.
    unsafe {
        std::env::remove_var("FZF_DEFAULT_OPTS");
        std::env::remove_var("FZF_DEFAULT_OPTS_FILE");
    }
    assert_eq!(result.unwrap(), PickOutcome::Picked(ID_A.to_string()));
    let env = std::fs::read_to_string(stub.scratch.path().join("env")).unwrap();
    assert!(
        env.lines().any(|l| l.starts_with("PATH=")),
        "stub recorded its env: {env}"
    );
    for var in FZF_SCRUBBED_ENV {
        assert!(
            !env.lines().any(|l| l.starts_with(&format!("{var}="))),
            "{var} reached fzf:\n{env}"
        );
    }
    stub.assert_preview_dir_removed();
    drop(guard);
}

#[test]
fn run_picker_missing_binary_is_an_error() {
    let recs = three_records();
    let err = run_picker(Path::new("/nonexistent/fzf"), &items(&recs), "h").unwrap_err();
    assert!(err.to_string().contains("failed to launch fzf"), "{err}");
}
