//! Integration tests for `clyde session resume <query...>`: id-or-search resolution end to end.
//!
//! Driven through the real `clyde` binary against a catalog seeded with `session reindex
//! --projects-dir` (as `tests/search.rs` does). Seeded cwds are real tempdirs and the transcripts
//! exist, so `plan_resume` reaches `Launch`; a stub `claude` first on `PATH` prints its cwd and
//! args, so the exec is observable on stdout. `Command::output` gives the child a null stdin, which
//! is the no-TTY case. The TTY case runs clyde under util-linux `script` with a stub `fzf` that
//! picks the first row.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

use tempfile::TempDir;

// Both ids share the `abcd` prefix, so `abcd` is an ambiguous id prefix.
const SID_ALPHA: &str = "abcd1111-7a3b-4a9c-93b1-6e2a90d1f042";
const SID_BRAVO: &str = "abcd2222-1e22-4f5a-b91c-1234567890ab";

const STUB_MARKER: &str = "STUB-CLAUDE";

struct Fixture {
    tmp: TempDir,
    projects: PathBuf,
    db_path: PathBuf,
    alpha_cwd: PathBuf,
    bravo_cwd: PathBuf,
    stub_dir: PathBuf,
}

fn write_jsonl(path: &Path, lines: &[String]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let mut f = fs::File::create(path).unwrap();
    for line in lines {
        writeln!(f, "{}", line).unwrap();
    }
}

fn session_lines(cwd: &Path, prompt: &str, title: &str) -> Vec<String> {
    vec![
        serde_json::json!({
            "type": "user",
            "cwd": cwd.to_string_lossy(),
            "gitBranch": "main",
            "timestamp": "2026-10-20T10:00:00Z",
            "message": {"content": prompt},
        })
        .to_string(),
        serde_json::json!({"type": "ai-title", "aiTitle": title, "sessionId": "x"}).to_string(),
    ]
}

/// Seed two sessions, reindex them into a fresh catalog, and drop a stub `claude` on disk.
///
/// - alpha: `kubernetes`, `facade` (an id-shaped word no id starts with), `migrate`
/// - bravo: `terraform`, `abcd` (the shared id prefix as a body word), `migrate`
///
/// bravo's transcript mtime is newer, so it is the most recent session.
fn fixture() -> Fixture {
    let tmp = TempDir::new().expect("tempdir");
    let projects = tmp.path().join("projects");
    let db_path = tmp.path().join("sessions.db");
    let alpha_cwd = tmp.path().join("alpha");
    let bravo_cwd = tmp.path().join("bravo");
    fs::create_dir_all(&alpha_cwd).unwrap();
    fs::create_dir_all(&bravo_cwd).unwrap();
    let alpha_cwd = alpha_cwd.canonicalize().unwrap();
    let bravo_cwd = bravo_cwd.canonicalize().unwrap();

    write_jsonl(
        &projects.join("-tmp-alpha").join(format!("{SID_ALPHA}.jsonl")),
        &session_lines(
            &alpha_cwd,
            "migrate the kubernetes ingress behind the facade service",
            "Kubernetes ingress migration",
        ),
    );
    write_jsonl(
        &projects.join("-tmp-bravo").join(format!("{SID_BRAVO}.jsonl")),
        &session_lines(
            &bravo_cwd,
            "migrate the terraform state bucket for abcd",
            "Terraform state migration",
        ),
    );

    let day = std::time::Duration::from_secs(86_400);
    let older = std::time::SystemTime::now() - 2 * day;
    for (sid, dir, mtime) in [(SID_ALPHA, "-tmp-alpha", older), (SID_BRAVO, "-tmp-bravo", older + day)] {
        fs::File::options()
            .write(true)
            .open(projects.join(dir).join(format!("{sid}.jsonl")))
            .unwrap()
            .set_modified(mtime)
            .unwrap();
    }
    reindex(&db_path, &projects);

    let stub_dir = tmp.path().join("bin");
    fs::create_dir_all(&stub_dir).unwrap();
    let stub = stub_dir.join("claude");
    fs::write(
        &stub,
        format!("#!/bin/sh\necho \"{STUB_MARKER} cwd=$(pwd -P)\"\necho \"{STUB_MARKER} args=$*\"\n"),
    )
    .unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();

    Fixture {
        tmp,
        projects,
        db_path,
        alpha_cwd,
        bravo_cwd,
        stub_dir,
    }
}

fn reindex(db_path: &Path, projects: &Path) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_clyde"))
        .arg("--db")
        .arg(db_path)
        .args(["session", "reindex", "--projects-dir"])
        .arg(projects)
        .output()
        .expect("clyde session reindex should run");
    assert!(out.status.success(), "reindex failed: {out:?}");
}

impl Fixture {
    /// Delete a seeded transcript and reindex, so the catalog marks that session archived.
    fn archive(&self, sid: &str) {
        let dir = if sid == SID_ALPHA { "-tmp-alpha" } else { "-tmp-bravo" };
        fs::remove_file(self.projects.join(dir).join(format!("{sid}.jsonl"))).unwrap();
        reindex(&self.db_path, &self.projects);
    }

    fn path_env(&self) -> std::ffi::OsString {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs = vec![self.stub_dir.clone()];
        dirs.extend(std::env::split_paths(&path));
        std::env::join_paths(dirs).unwrap()
    }

    /// Run `clyde session resume --no-reindex` with no query on a pseudo-terminal (util-linux
    /// `script`), with a stub `fzf` that touches a marker file and picks the first row. Returns the
    /// combined terminal output and whether fzf ran.
    fn resume_on_tty(&self) -> (Output, bool) {
        let marker = self.tmp.path().join("fzf-ran");
        let fzf = self.stub_dir.join("fzf");
        fs::write(&fzf, format!("#!/bin/sh\ntouch '{}'\nhead -n 1\n", marker.display())).unwrap();
        fs::set_permissions(&fzf, fs::Permissions::from_mode(0o755)).unwrap();
        let cmd = format!(
            "'{}' --db '{}' session resume --no-reindex",
            env!("CARGO_BIN_EXE_clyde"),
            self.db_path.display()
        );
        // `script` runs `cmd` through `$SHELL`. A login zsh's startup files can put the user's own
        // `claude` ahead of the stub on PATH, so pin a plain sh and a HOME with no startup files.
        let out = std::process::Command::new("script")
            .args(["-qec", &cmd, "/dev/null"])
            .env("SHELL", "/bin/sh")
            .env("HOME", self.tmp.path())
            .env("PATH", self.path_env())
            .current_dir(self.tmp.path())
            .output()
            .expect("util-linux script should run");
        (out, marker.exists())
    }

    /// Run `clyde --db <db> session resume <query...> --no-reindex` with the stub first on `PATH`.
    fn resume(&self, query: &[&str]) -> Output {
        std::process::Command::new(env!("CARGO_BIN_EXE_clyde"))
            .arg("--db")
            .arg(&self.db_path)
            .args(["session", "resume"])
            .args(query)
            .arg("--no-reindex")
            .env("PATH", self.path_env())
            .current_dir(self.tmp.path())
            .output()
            .expect("clyde session resume should run")
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn assert_launched(out: &Output, cwd: &Path, id: &str) {
    let so = stdout(out);
    assert!(out.status.success(), "stub claude should exit 0: {out:?}");
    assert!(
        so.contains(&format!("{STUB_MARKER} cwd={}", cwd.display())),
        "stub must run in the session cwd: {so:?}"
    );
    assert!(
        so.contains(&format!("{STUB_MARKER} args=--resume {id}")),
        "stub must get --resume <id>: {so:?}"
    );
}

fn assert_not_launched(out: &Output) {
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {out:?}");
    assert!(!stdout(out).contains(STUB_MARKER), "stub claude must not run: {out:?}");
}

#[test]
fn one_unique_word_hit_resumes_and_announces() {
    let fx = fixture();
    let out = fx.resume(&["kubernetes"]);
    assert_launched(&out, &fx.alpha_cwd, SID_ALPHA);
    assert!(
        stderr(&out).contains("→ resuming abcd1111"),
        "one-hit announce line expected: {out:?}"
    );
}

#[test]
fn two_hits_without_tty_print_search_json_and_exit_1() {
    let fx = fixture();
    let out = fx.resume(&["migrate"]);
    assert_not_launched(&out);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out))
        .unwrap_or_else(|e| panic!("stdout is not SearchResults JSON: {e}: {out:?}"));
    assert_eq!(v["count"], 2, "{v}");
    assert_eq!(v["results"].as_array().unwrap().len(), 2, "{v}");
    assert!(stderr(&out).contains("2 sessions match; pass an id"), "{out:?}");
}

#[test]
fn or_fallback_single_hit_without_tty_never_launches() {
    let fx = fixture();
    let out = fx.resume(&["kubernetes", "qqxzqqxznomatch"]);
    assert_not_launched(&out);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["fallback"], "or", "{v}");
    assert_eq!(v["count"], 1, "{v}");
    assert!(stderr(&out).contains("1 session matches; pass an id"), "{out:?}");
}

#[test]
fn zero_hits_report_no_match_and_exit_1() {
    let fx = fixture();
    let out = fx.resume(&["qqxzqqxznomatch"]);
    assert_not_launched(&out);
    assert!(
        stderr(&out).contains(r#"no session matches "qqxzqqxznomatch""#),
        "{out:?}"
    );
}

#[test]
fn no_query_without_tty_lists_recent_sessions_and_exits_1() {
    let fx = fixture();
    let out = fx.resume(&[]);
    assert_not_launched(&out);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out))
        .unwrap_or_else(|e| panic!("stdout is not a SessionRecord JSON array: {e}: {out:?}"));
    let ids: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["session-id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [SID_BRAVO, SID_ALPHA], "newest first: {v}");
    assert!(
        stderr(&out).contains("listed the 2 most recent sessions; pass an id or search terms"),
        "{out:?}"
    );
}

#[test]
fn no_query_leaves_out_archived_sessions() {
    let fx = fixture();
    fx.archive(SID_BRAVO);
    let out = fx.resume(&[]);
    assert_not_launched(&out);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let ids: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["session-id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [SID_ALPHA], "{v}");
    assert!(stderr(&out).contains("listed the 1 most recent session;"), "{out:?}");
}

#[test]
fn no_query_with_every_session_archived_reports_an_empty_catalog() {
    let fx = fixture();
    fx.archive(SID_ALPHA);
    fx.archive(SID_BRAVO);
    let out = fx.resume(&[]);
    assert_not_launched(&out);
    assert!(stderr(&out).contains("no sessions in the catalog"), "{out:?}");
}

#[test]
fn no_query_on_a_tty_opens_the_picker_newest_first() {
    let fx = fixture();
    let (out, fzf_ran) = fx.resume_on_tty();
    assert!(fzf_ran, "the picker must open: {out:?}");
    assert_launched(&out, &fx.bravo_cwd, SID_BRAVO);
}

#[test]
fn no_query_on_a_tty_opens_the_picker_even_for_one_session() {
    let fx = fixture();
    fx.archive(SID_BRAVO);
    let (out, fzf_ran) = fx.resume_on_tty();
    assert!(fzf_ran, "one session must still go through the picker: {out:?}");
    assert_launched(&out, &fx.alpha_cwd, SID_ALPHA);
}

#[test]
fn full_id_resumes_silently() {
    let fx = fixture();
    let out = fx.resume(&[SID_BRAVO]);
    assert_launched(&out, &fx.bravo_cwd, SID_BRAVO);
    assert!(
        !stderr(&out).contains("resuming"),
        "a full id must not print the announce line: {out:?}"
    );
}

#[test]
fn id_shaped_word_with_no_id_match_resumes_via_search() {
    let fx = fixture();
    // `facade` passes looks_like_id but no seeded id starts with it; it is a body word in alpha.
    let out = fx.resume(&["facade"]);
    assert_launched(&out, &fx.alpha_cwd, SID_ALPHA);
    assert!(stderr(&out).contains("→ resuming abcd1111"), "{out:?}");
}

#[test]
fn ambiguous_id_prefix_goes_to_search_not_id_candidates() {
    let fx = fixture();
    // `abcd` prefixes both seeded ids, so it is not an id; search finds it only in bravo's body.
    let out = fx.resume(&["abcd"]);
    assert_launched(&out, &fx.bravo_cwd, SID_BRAVO);
    let se = stderr(&out);
    assert!(!se.contains("ambiguous"), "must not report id ambiguity: {se:?}");
    assert!(se.contains("→ resuming abcd2222"), "{se:?}");
}

#[test]
fn resume_help_lists_claude_and_fzf_under_required_tools() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_clyde"))
        .args(["session", "resume", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let help = String::from_utf8_lossy(&out.stdout);
    let tools = help.split("REQUIRED TOOLS:").nth(1).expect("REQUIRED TOOLS block");
    assert!(tools.contains("session resume: exec `claude --resume <id>`"), "{help}");
    assert!(tools.contains("session resume: pick among several matches"), "{help}");
}

#[test]
fn resume_help_keeps_the_extra_flags_example_indented() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_clyde"))
        .args(["session", "resume", "--help"])
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    let about = help.split("Usage:").next().unwrap();
    assert!(
        about.contains("\n  clyde session resume <query...> -- --model opus\n"),
        "{help}"
    );
}
