//! `clyde doctor`'s `reposlugs-ptns:` line (design
//! `docs/design/2026-09-27-reposlugs-ptns-from-config.md`, Phase 4) and its REQUIRED TOOLS block.
//!
//! Spawns the real binary against a hermetic `$HOME`/`$XDG_*_HOME`, the same pattern
//! `serve.rs`/`matrix.rs` use, because the line is printed unconditionally in the paths block --
//! it must appear even with no `clyde.yml` and no catalog, which is exactly what these fixtures are.

use std::path::Path;
use std::process::Command;

/// Write a `clyde.yml` under `<config_home>/clyde/clyde.yml`; `config_home` is passed as
/// `$XDG_CONFIG_HOME` so the loader resolves exactly this file.
fn write_clyde_yml(config_home: &Path, body: &str) {
    let dir = config_home.join("clyde");
    std::fs::create_dir_all(&dir).expect("create config dir");
    std::fs::write(dir.join("clyde.yml"), body).expect("write clyde.yml");
}

/// Run `clyde doctor` against a hermetic environment, pointed at a `--db` that does not exist (no
/// catalog), and return its stdout. `doctor` exits non-zero on a bare-bones host (nothing
/// bootstrapped), which is expected here and not what this test is about.
fn run_doctor(home: &Path, config_home: &Path, data_home: &Path, cache_home: &Path) -> String {
    let output = doctor_output(home, config_home, data_home, cache_home, None);
    String::from_utf8(output.stdout).expect("doctor stdout is utf8")
}

/// Spawn `clyde doctor` hermetically; `path` overrides `PATH` when given.
fn doctor_output(
    home: &Path,
    config_home: &Path,
    data_home: &Path,
    cache_home: &Path,
    path: Option<&Path>,
) -> std::process::Output {
    let db_path = data_home.join("sessions-that-do-not-exist.db");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_clyde"));
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", config_home)
        .env("XDG_DATA_HOME", data_home)
        .env("XDG_CACHE_HOME", cache_home)
        .arg("--db")
        .arg(&db_path)
        .arg("doctor")
        .output()
        .expect("spawn clyde doctor")
}

#[test]
fn no_clyde_yml_and_no_catalog_prints_the_built_in_default() {
    let home = tempfile::tempdir().expect("temp home");
    let config_home = tempfile::tempdir().expect("temp config home");
    let data_home = tempfile::tempdir().expect("temp data home");
    let cache_home = tempfile::tempdir().expect("temp cache home");
    // No clyde.yml written at all, and the `--db` path above does not exist: this is the exact
    // "no clyde.yml and no catalog" acceptance-criteria fixture.

    let stdout = run_doctor(home.path(), config_home.path(), data_home.path(), cache_home.path());

    assert!(
        stdout.contains("reposlugs-ptns: tatari-tv/*"),
        "expected the built-in default patterns, got:\n{stdout}"
    );
}

#[test]
fn an_empty_reposlugs_ptns_list_prints_none() {
    let home = tempfile::tempdir().expect("temp home");
    let config_home = tempfile::tempdir().expect("temp config home");
    let data_home = tempfile::tempdir().expect("temp data home");
    let cache_home = tempfile::tempdir().expect("temp cache home");
    write_clyde_yml(config_home.path(), "reposlugs-ptns: []\n");

    let stdout = run_doctor(home.path(), config_home.path(), data_home.path(), cache_home.path());

    assert!(
        stdout.contains("reposlugs-ptns: none"),
        "an explicit empty list means zero work repos, got:\n{stdout}"
    );
}

#[test]
fn a_configured_list_prints_normalized_sorted_and_deduped() {
    let home = tempfile::tempdir().expect("temp home");
    let config_home = tempfile::tempdir().expect("temp config home");
    let data_home = tempfile::tempdir().expect("temp data home");
    let cache_home = tempfile::tempdir().expect("temp cache home");
    write_clyde_yml(
        config_home.path(),
        "reposlugs-ptns: [\"Scottidler/Claude\", \"otto-rs/otto\", \"scottidler/claude\"]\n",
    );

    let stdout = run_doctor(home.path(), config_home.path(), data_home.path(), cache_home.path());

    assert!(
        stdout.contains("reposlugs-ptns: otto-rs/otto, scottidler/claude"),
        "expected lowercased, sorted, deduped patterns, got:\n{stdout}"
    );
}

#[test]
fn doctor_lists_every_subcommands_required_tools() {
    let home = tempfile::tempdir().expect("temp home");
    let config_home = tempfile::tempdir().expect("temp config home");
    let data_home = tempfile::tempdir().expect("temp data home");
    let cache_home = tempfile::tempdir().expect("temp cache home");

    let stdout = run_doctor(home.path(), config_home.path(), data_home.path(), cache_home.path());

    assert!(stdout.contains("REQUIRED TOOLS:"), "tools block missing:\n{stdout}");
    for tool in [
        "claude",
        "fzf",
        "persona",
        "pandoc",
        "marquee",
        "git",
        "jq",
        "rkvr",
        "systemctl",
    ] {
        assert!(
            stdout.lines().any(|l| l.split_whitespace().nth(1) == Some(tool)),
            "no REQUIRED TOOLS row for {tool}:\n{stdout}"
        );
    }
}

#[test]
fn missing_tools_never_change_doctors_exit_code() {
    let home = tempfile::tempdir().expect("temp home");
    let config_home = tempfile::tempdir().expect("temp config home");
    let data_home = tempfile::tempdir().expect("temp data home");
    let cache_home = tempfile::tempdir().expect("temp cache home");
    let empty_path = tempfile::tempdir().expect("empty PATH dir");
    let (h, c, d, k) = (home.path(), config_home.path(), data_home.path(), cache_home.path());

    let with_tools = doctor_output(h, c, d, k, None);
    let without_tools = doctor_output(h, c, d, k, Some(empty_path.path()));

    let stdout = String::from_utf8_lossy(&without_tools.stdout);
    assert!(stdout.contains("❌ fzf"), "fzf must read as not found:\n{stdout}");
    assert_eq!(
        with_tools.status.code(),
        without_tools.status.code(),
        "a missing tool changed doctor's exit:\n{stdout}"
    );
}

#[test]
fn root_help_points_at_doctor_for_required_tools() {
    let out = Command::new(env!("CARGO_BIN_EXE_clyde"))
        .arg("--help")
        .output()
        .expect("spawn clyde --help");
    let stdout = String::from_utf8(out.stdout).expect("help stdout is utf8");
    assert!(
        stdout.contains("Run `clyde doctor` to check required tools."),
        "root help must point at doctor:\n{stdout}"
    );
}
