#![allow(clippy::unwrap_used)]

use crate::OutputDest;
use crate::config::{CollectConfig, Config, Output, ResolvedCommand};
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

fn make_collect_config(projects_dir: &Path, output: &Path) -> Config {
    Config {
        log_level: "info".into(),
        command: ResolvedCommand::Collect(CollectConfig {
            since: "2026-01-01T00:00:00Z".parse().unwrap(),
            until: "2030-01-01T00:00:00Z".parse().unwrap(),
            output: Output::File(output.to_path_buf()),
            projects_dir: projects_dir.to_path_buf(),
            no_rollup: false,
            skip_title: true,
        }),
    }
}

#[test]
fn end_to_end_collect_writes_json() {
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

    let output = tmp.path().join("claude-report.json");
    let cfg = make_collect_config(&projects, &output);

    let result = crate::run_with_config(&cfg).unwrap();
    assert_eq!(result.sessions_emitted, 2);
    match result.output {
        OutputDest::File(p) => assert_eq!(p, output),
        OutputDest::Stdout => panic!("expected file output, got stdout"),
    }

    let body = fs::read_to_string(&output).unwrap();
    let report: Report = serde_json::from_str(&body).unwrap();
    assert_eq!(report.totals.sessions, 2);
    assert!(report.sessions.contains_key(SID_A));
    assert!(report.sessions.contains_key(SID_B));

    let a = &report.sessions[SID_A];
    assert_eq!(a.models.len(), 2);
    let opus = a.models.get("claude-opus-4-7").unwrap();
    let sonnet = a.models.get("claude-sonnet-4-6").unwrap();
    assert_eq!(opus.input, 10);
    assert_eq!(opus.output, 5);
    assert_eq!(sonnet.input, 20);
    assert_eq!(sonnet.output, 15);
    assert!(a.title.is_none());
}

#[test]
fn latest_prior_report_picks_newest_excluding_self() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    for name in [
        "claude-report-2026-06-20-101010.json",
        "claude-report-2026-06-21-090000.json",
        "claude-report-2026-06-21-235959.json",
        "not-a-report.json",
        "claude-report-latest.txt",
    ] {
        fs::write(dir.join(name), "{}").unwrap();
    }
    // The output we are about to write (does not exist yet).
    let output = Output::File(dir.join("claude-report-2026-06-22-000000.json"));
    let prior = crate::latest_prior_report_in(dir, &output).unwrap();
    assert_eq!(prior, dir.join("claude-report-2026-06-21-235959.json"));
}

#[test]
fn latest_prior_report_none_when_no_prior() {
    let tmp = TempDir::new().unwrap();
    let output = Output::File(tmp.path().join("claude-report-2026-06-22-000000.json"));
    assert!(crate::latest_prior_report_in(tmp.path(), &output).is_none());
}

#[test]
fn latest_prior_report_excludes_output_itself() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let path = dir.join("claude-report-2026-06-22-000000.json");
    fs::write(&path, "{}").unwrap();
    let output = Output::File(path);
    // Only the output file is present; it must be excluded, so no prior.
    assert!(crate::latest_prior_report_in(dir, &output).is_none());
}

#[test]
fn title_carries_forward_across_timestamped_outputs() {
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let project = projects.join("-home-saidler-repos-foo");
    write_jsonl(
        &project.join(format!("{}.jsonl", SID_A)),
        &[
            r#"{"type":"assistant","sessionId":"abc","timestamp":"2026-04-10T10:00:00Z","requestId":"r1","message":{"id":"m1","model":"claude-opus-4-7","usage":{"input_tokens":1,"output_tokens":1}}}"#,
        ],
    );

    let reports = tmp.path().join("reports");
    let first = reports.join("claude-report-2026-06-21-090000.json");
    let cfg1 = make_collect_config(&projects, &first);
    crate::run_with_config(&cfg1).unwrap();

    // Hand-edit the first (older) report to carry a title.
    let mut report: Report = serde_json::from_str(&fs::read_to_string(&first).unwrap()).unwrap();
    report.sessions.get_mut(SID_A).unwrap().title = Some("carried title".into());
    fs::write(&first, serde_json::to_string_pretty(&report).unwrap()).unwrap();

    // A later run with a *different* timestamped output must inherit the title.
    let second = reports.join("claude-report-2026-06-21-235959.json");
    let cfg2 = make_collect_config(&projects, &second);
    crate::run_with_config(&cfg2).unwrap();

    let report: Report = serde_json::from_str(&fs::read_to_string(&second).unwrap()).unwrap();
    assert_eq!(report.sessions[SID_A].title.as_deref(), Some("carried title"));
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

    let output = tmp.path().join("claude-report.json");
    let cfg = make_collect_config(&projects, &output);
    crate::run_with_config(&cfg).unwrap();

    let body = fs::read_to_string(&output).unwrap();
    let mut report: Report = serde_json::from_str(&body).unwrap();
    let entry = report.sessions.get_mut(SID_A).unwrap();
    entry.title = Some("hand-written title".into());
    let edited = serde_json::to_string_pretty(&report).unwrap();
    fs::write(&output, edited).unwrap();

    crate::run_with_config(&cfg).unwrap();

    let body = fs::read_to_string(&output).unwrap();
    let report: Report = serde_json::from_str(&body).unwrap();
    assert_eq!(report.sessions[SID_A].title.as_deref(), Some("hand-written title"));
}

// Serialize env-var-touching tests (XDG_DATA_HOME) so parallel runs can't race.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn stdout_mode_resolves_title_cache_source() {
    // HAZARD 2 (financial): Stdout mode (no `-o`) must still resolve a title-cache SOURCE so the
    // paid Haiku titling carries forward and does NOT re-bill the Anthropic API every run. The
    // source is the newest prior report in the default report dir under XDG data.
    let guard = ENV_LOCK.lock().unwrap();
    let prior_xdg = std::env::var("XDG_DATA_HOME").ok();

    let tmp = TempDir::new().unwrap();
    unsafe { std::env::set_var("XDG_DATA_HOME", tmp.path()) };

    let report_dir = tmp.path().join("claude-report");
    fs::create_dir_all(&report_dir).unwrap();
    let prior = report_dir.join("claude-report-2026-06-21-235959.json");
    fs::write(&prior, "{}").unwrap();
    // An older prior, to confirm the newest is chosen.
    fs::write(report_dir.join("claude-report-2026-06-20-101010.json"), "{}").unwrap();

    let resolved = crate::resolve_titles_source(&Output::Stdout).unwrap();
    assert_eq!(
        resolved,
        Some(prior),
        "stdout mode must seed the title cache from the newest prior report in the default dir"
    );

    match prior_xdg {
        Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
    }
    drop(guard);
}

#[test]
fn log_file_path_resolves_under_unified_clyde_logs_dir() {
    // Phase 8 (D3): report's log moves off the legacy `claude-report/logs/` dir onto the unified
    // `<xdg-data>/clyde/logs/report.log` location shared with cost and permit.
    let guard = ENV_LOCK.lock().unwrap();
    let prior_xdg = std::env::var("XDG_DATA_HOME").ok();

    let tmp = TempDir::new().unwrap();
    unsafe { std::env::set_var("XDG_DATA_HOME", tmp.path()) };

    let path = crate::log_file_path();
    assert_eq!(path, tmp.path().join("clyde").join("logs").join("report.log"));

    match prior_xdg {
        Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
    }
    drop(guard);
}
