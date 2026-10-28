//! Each tool-bearing subcommand's `--help` ends with its REQUIRED TOOLS block. `session resume` is
//! covered in `resume.rs`, `report` in `report::tools`.

use std::process::Command;

fn help(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_clyde"))
        .args(args)
        .arg("--help")
        .output()
        .expect("spawn clyde --help");
    assert!(out.status.success(), "{args:?} --help failed: {out:?}");
    String::from_utf8(out.stdout).expect("help stdout is utf8")
}

fn assert_tool_row(help: &str, tool: &str) {
    let block = help
        .split_once("REQUIRED TOOLS:")
        .unwrap_or_else(|| panic!("no REQUIRED TOOLS block:\n{help}"))
        .1;
    assert!(
        block.lines().any(|l| l.split_whitespace().nth(1) == Some(tool)),
        "no REQUIRED TOOLS row for {tool}:\n{help}"
    );
}

#[test]
fn bootstrap_help_lists_systemctl() {
    assert_tool_row(&help(&["bootstrap"]), "systemctl");
}

#[test]
fn permit_apply_help_lists_rkvr() {
    assert_tool_row(&help(&["permit", "apply"]), "rkvr");
}
