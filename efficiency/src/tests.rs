#![allow(clippy::unwrap_used)]

use tempfile::TempDir;

use super::*;
use crate::cli::Command;

fn args(path: std::path::PathBuf, json: bool, worst: Option<usize>, command: Option<Command>) -> EfficiencyArgs {
    EfficiencyArgs {
        path: Some(path),
        json,
        worst,
        command,
    }
}

#[test]
fn run_exits_zero_with_no_subcommand_and_no_worst() {
    // No subcommand and no --worst: nothing to report, matching the Phase 1 scaffold's contract.
    let tmp = TempDir::new().expect("tempdir");
    let code = run(
        args(tmp.path().to_path_buf(), false, None, None),
        common::Globals::default(),
    )
    .expect("run should not error");
    assert_eq!(code, 0);
}

#[test]
fn run_exits_zero_with_an_explicit_log_level() {
    let tmp = TempDir::new().expect("tempdir");
    let globals = common::Globals {
        log_level: Some("debug".to_string()),
    };
    let code =
        run(args(tmp.path().to_path_buf(), false, None, None), globals).expect("run should not error with a level");
    assert_eq!(code, 0);
}

#[test]
fn run_worst_exits_zero_on_an_empty_projects_dir() {
    let tmp = TempDir::new().expect("tempdir");
    let code = run(
        args(tmp.path().to_path_buf(), true, Some(3), None),
        common::Globals::default(),
    )
    .expect("run should not error");
    assert_eq!(code, 0);
}

#[test]
fn run_session_reports_no_match_on_an_empty_projects_dir() {
    let tmp = TempDir::new().expect("tempdir");
    let command = Some(Command::Session {
        id: "none-such".to_string(),
        by_subagent: false,
    });
    let code = run(
        args(tmp.path().to_path_buf(), true, None, command),
        common::Globals::default(),
    )
    .expect("run should not error");
    assert_eq!(code, 0);
}

#[test]
fn run_daily_and_weekly_exit_zero_on_an_empty_projects_dir() {
    let tmp = TempDir::new().expect("tempdir");

    let daily_command = Some(Command::Daily { days: 7 });
    let code = run(
        args(tmp.path().to_path_buf(), true, None, daily_command),
        common::Globals::default(),
    )
    .expect("daily should not error");
    assert_eq!(code, 0);

    let weekly_command = Some(Command::Weekly { weeks: 4 });
    let code = run(
        args(tmp.path().to_path_buf(), true, None, weekly_command),
        common::Globals::default(),
    )
    .expect("weekly should not error");
    assert_eq!(code, 0);
}
