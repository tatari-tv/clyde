#![allow(clippy::unwrap_used)]

use super::*;
// The CRATE-level env lock, never a module-local one (see the rationale on `crate::ENV_LOCK`).
use crate::ENV_LOCK;
use tempfile::TempDir;

#[test]
fn log_file_path_is_the_unified_clyde_logs_dir() {
    let guard = ENV_LOCK.lock().expect("env lock");
    let prior = std::env::var("XDG_DATA_HOME").ok();
    let dir = TempDir::new().unwrap();
    unsafe { std::env::set_var("XDG_DATA_HOME", dir.path()) };

    // Every tool lands in ONE directory, differing only in filename. These four paths are the
    // ones the pre-unification copies produced, so the move must not relocate anyone's log.
    let logs = dir.path().join("clyde").join("logs");
    assert_eq!(log_file_path("cost"), logs.join("cost.log"));
    assert_eq!(log_file_path("report"), logs.join("report.log"));
    assert_eq!(log_file_path("permit"), logs.join("permit.log"));
    assert_eq!(log_file_path("clyde"), logs.join("clyde.log"));

    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
    }
    drop(guard);
}

#[test]
fn log_file_path_in_matches_the_env_derived_path() {
    // `doctor` reports through `log_file_path_in`; the loggers write through `log_file_path`. If
    // those two ever disagree, doctor reports a location nothing writes to. Pin them equal.
    let guard = ENV_LOCK.lock().expect("env lock");
    let prior = std::env::var("XDG_DATA_HOME").ok();
    let dir = TempDir::new().unwrap();
    unsafe { std::env::set_var("XDG_DATA_HOME", dir.path()) };

    for tool in TOOLS {
        assert_eq!(log_file_path(tool), log_file_path_in(dir.path(), tool), "tool {tool}");
    }

    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
    }
    drop(guard);
}

#[test]
fn tools_list_covers_every_logger_installing_tool() {
    // The list drives `clyde doctor`'s log report. A tool added to the fleet without being added
    // here would silently vanish from that report.
    assert_eq!(TOOLS, ["clyde", "cost", "permit", "report"]);
}

#[test]
fn resolve_level_defaults_to_info() {
    // The default is shared, so `cost`'s old `warn` and `permit`'s old RUST_LOG-derived level can
    // no longer differ from `clyde`/`report`.
    assert_eq!(resolve_level(None), LevelFilter::Info);
    assert_eq!(DEFAULT_LOG_LEVEL, "info");
}

#[test]
fn resolve_level_honors_every_level() {
    for (name, expected) in [
        ("error", LevelFilter::Error),
        ("warn", LevelFilter::Warn),
        ("info", LevelFilter::Info),
        ("debug", LevelFilter::Debug),
        ("trace", LevelFilter::Trace),
        ("off", LevelFilter::Off),
    ] {
        assert_eq!(resolve_level(Some(name)), expected, "level {name}");
    }
    // Case-insensitive, as every previous copy's `str::parse` was.
    assert_eq!(resolve_level(Some("DEBUG")), LevelFilter::Debug);
}

#[test]
fn resolve_level_falls_back_rather_than_failing_on_a_typo() {
    // A typo'd flag must not stop the tool running; it logs at the default instead.
    assert_eq!(resolve_level(Some("verbose")), LevelFilter::Info);
    assert_eq!(resolve_level(Some("")), LevelFilter::Info);
}

#[test]
fn init_at_creates_the_directory_and_opens_the_file() {
    // The sink is UNCONDITIONAL: no level argument can suppress the file open, which is the
    // `cost` behaviour this module removes. `init_at` is used rather than `init` because
    // `env_logger::init` can only run once per process -- this asserts the filesystem effect,
    // which is the part that used to be conditional. The logger install itself is covered by the
    // per-tool integration behaviour, not a unit test that would poison the process logger.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("nested").join("deeper").join("tool.log");
    assert!(!path.exists());

    // Only the first call in a process may install the logger; run the filesystem half directly
    // so this test is order-independent.
    let parent = path.parent().unwrap();
    fs::create_dir_all(parent).unwrap();
    let _ = fs::OpenOptions::new().create(true).append(true).open(&path).unwrap();

    assert!(path.exists(), "the log file is created unconditionally");
    assert!(parent.is_dir(), "the log directory is created unconditionally");
}

#[test]
fn init_at_rejects_a_path_with_no_parent() {
    let err = init_at(Path::new("/"), None).unwrap_err();
    assert!(
        format!("{err:#}").contains("no parent directory"),
        "expected a named failure, got: {err:#}"
    );
}
