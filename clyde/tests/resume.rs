//! Integration tests for `clyde session resume <query...>`: id-or-search resolution end to end.
//!
//! Driven through the real `clyde` binary against a catalog seeded with `session reindex
//! --projects-dir` (as `tests/search.rs` does). Seeded cwds are real tempdirs and the transcripts
//! exist, so `plan_resume` reaches `Launch`; a stub `claude` first on `PATH` prints its cwd and
//! args, so the exec is observable on stdout. `Command::output` gives the child a null stdin, which
//! is the no-TTY case.

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

    let reindex = std::process::Command::new(env!("CARGO_BIN_EXE_clyde"))
        .arg("--db")
        .arg(&db_path)
        .args(["session", "reindex", "--projects-dir"])
        .arg(&projects)
        .output()
        .expect("clyde session reindex should run");
    assert!(reindex.status.success(), "reindex failed: {:?}", reindex);

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
        db_path,
        alpha_cwd,
        bravo_cwd,
        stub_dir,
    }
}

impl Fixture {
    /// Run `clyde --db <db> session resume <query...> --no-reindex` with the stub first on `PATH`.
    fn resume(&self, query: &[&str]) -> Output {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs = vec![self.stub_dir.clone()];
        dirs.extend(std::env::split_paths(&path));
        std::process::Command::new(env!("CARGO_BIN_EXE_clyde"))
            .arg("--db")
            .arg(&self.db_path)
            .args(["session", "resume"])
            .args(query)
            .arg("--no-reindex")
            .env("PATH", std::env::join_paths(dirs).unwrap())
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
