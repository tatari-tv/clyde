#![allow(clippy::unwrap_used)]

use crate::config::{Config, ResolvedCommand, ScanConfig};
use crate::report::Report;
use std::fs;
use std::io::Write;
use std::path::Path;
use tempfile::TempDir;

const SID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const SID_B: &str = "8b21c34d-1e22-4f5a-b91c-1234567890ab";

fn write_jsonl(path: &Path, lines: &[&str]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let mut f = fs::File::create(path).unwrap();
    for line in lines {
        writeln!(f, "{}", line).unwrap();
    }
}

fn make_scan_config(projects_dir: &Path, output: &Path) -> Config {
    Config {
        log_level: "info".into(),
        command: ResolvedCommand::Scan(ScanConfig {
            since: "2026-01-01T00:00:00Z".parse().unwrap(),
            until: "2030-01-01T00:00:00Z".parse().unwrap(),
            output: output.to_path_buf(),
            projects_dir: projects_dir.to_path_buf(),
            no_rollup: false,
            no_title: false,
        }),
    }
}

#[test]
fn end_to_end_scan_writes_yaml() {
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let project_a = projects.join("-home-saidler-repos-foo-bar");

    write_jsonl(
        &project_a.join(format!("{}.jsonl", SID_A)),
        &[
            r#"{"type":"user","cwd":"/home/saidler/repos/foo/bar","message":{"role":"user","content":"hi"}}"#,
            r#"{"type":"assistant","sessionId":"abc","timestamp":"2026-04-10T10:00:00Z","cwd":"/home/saidler/repos/foo/bar","requestId":"r1","message":{"id":"m1","model":"claude-opus-4-7","usage":{"input_tokens":10,"output_tokens":5}}}"#,
        ],
    );
    write_jsonl(
        &project_a.join(SID_A).join("subagents").join("agent-aabbccdd.jsonl"),
        &[
            r#"{"type":"assistant","sessionId":"sub","timestamp":"2026-04-10T10:30:00Z","requestId":"r2","message":{"id":"m2","model":"claude-sonnet-4-6","usage":{"input_tokens":20,"output_tokens":15}}}"#,
        ],
    );

    let project_b = projects.join("-home-saidler-scratch");
    write_jsonl(
        &project_b.join(format!("{}.jsonl", SID_B)),
        &[
            r#"{"type":"assistant","sessionId":"abc2","timestamp":"2026-04-15T12:00:00Z","requestId":"r3","message":{"id":"m3","model":"claude-opus-4-7","usage":{"input_tokens":1,"output_tokens":2}}}"#,
        ],
    );

    let output = tmp.path().join("claude-report.yml");
    let cfg = make_scan_config(&projects, &output);

    let result = crate::run(&cfg).unwrap();
    assert_eq!(result.sessions_emitted, 2);
    assert_eq!(result.output_path, output);

    let body = fs::read_to_string(&output).unwrap();
    let report: Report = serde_yaml::from_str(&body).unwrap();
    assert_eq!(report.session_count, 2);
    assert!(report.sessions.contains_key(SID_A));
    assert!(report.sessions.contains_key(SID_B));

    let a = &report.sessions[SID_A];
    assert_eq!(a.tokens.output, 5 + 15);
    assert_eq!(a.tokens.input, 10 + 20);
    assert_eq!(a.models.len(), 2);
    assert_eq!(a.jsonl_paths.len(), 2);
    assert!(a.title.is_none());
}

#[test]
fn end_to_end_title_preserved_across_runs() {
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let project = projects.join("-home-saidler-repos-foo");

    write_jsonl(
        &project.join(format!("{}.jsonl", SID_A)),
        &[
            r#"{"type":"assistant","sessionId":"abc","timestamp":"2026-04-10T10:00:00Z","requestId":"r1","message":{"id":"m1","model":"claude-opus-4-7","usage":{"input_tokens":1,"output_tokens":1}}}"#,
        ],
    );

    let output = tmp.path().join("claude-report.yml");
    let cfg = make_scan_config(&projects, &output);
    crate::run(&cfg).unwrap();

    let body = fs::read_to_string(&output).unwrap();
    let mut report: Report = serde_yaml::from_str(&body).unwrap();
    let entry = report.sessions.get_mut(SID_A).unwrap();
    entry.title = Some("hand-written title".into());
    let edited = serde_yaml::to_string(&report).unwrap();
    fs::write(&output, edited).unwrap();

    crate::run(&cfg).unwrap();

    let body = fs::read_to_string(&output).unwrap();
    let report: Report = serde_yaml::from_str(&body).unwrap();
    assert_eq!(report.sessions[SID_A].title.as_deref(), Some("hand-written title"));
}
