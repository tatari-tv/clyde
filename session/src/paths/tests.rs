#![allow(clippy::unwrap_used)]

use super::*;
use std::sync::Mutex;
use tempfile::TempDir;

// Env-var mutation is process-global; serialize every env-touching test.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// This wrapper DELEGATES to `common::paths::xdg_data_dir`, so what is checked here is the delegation:
/// the two agree, always. The env-honoring platform behavior itself is pinned once, in
/// `common/src/paths/tests.rs`, beside the one body that reads the variable.
///
/// BITES: give this wrapper its own body again and the two answers diverge the moment either changes,
/// which is exactly the five-copy drift this consolidation removed.
#[test]
fn xdg_data_dir_delegates_to_common() {
    let guard = ENV_LOCK.lock().unwrap();
    let prior = std::env::var("XDG_DATA_HOME").ok();

    let dir = TempDir::new().unwrap();
    unsafe { std::env::set_var("XDG_DATA_HOME", dir.path()) };
    assert_eq!(xdg_data_dir(), common::paths::xdg_data_dir());
    assert_eq!(xdg_data_dir().as_deref(), Some(dir.path()));

    // And they still agree on the fallback, not just on the env-set path.
    unsafe { std::env::remove_var("XDG_DATA_HOME") };
    assert_eq!(xdg_data_dir(), common::paths::xdg_data_dir());
    assert!(xdg_data_dir().unwrap().ends_with(".local/share"));

    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
    }
    drop(guard);
}

#[test]
fn xdg_config_dir_honors_env_and_falls_back() {
    let guard = ENV_LOCK.lock().unwrap();
    let prior = std::env::var("XDG_CONFIG_HOME").ok();

    let dir = TempDir::new().unwrap();
    unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
    assert_eq!(xdg_config_dir().as_deref(), Some(dir.path()));

    unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
    assert!(xdg_config_dir().unwrap().ends_with(".config"));

    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
    }
    drop(guard);
}

#[test]
fn data_root_and_db_path_sit_under_clyde_namespace() {
    let guard = ENV_LOCK.lock().unwrap();
    let prior = std::env::var("XDG_DATA_HOME").ok();

    let dir = TempDir::new().unwrap();
    unsafe { std::env::set_var("XDG_DATA_HOME", dir.path()) };
    assert_eq!(data_root(), dir.path().join("clyde"));
    assert_eq!(sessions_db_path(), dir.path().join("clyde").join("sessions.db"));
    assert_eq!(staged_dir(), dir.path().join("clyde").join("staged"));

    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
    }
    drop(guard);
}

#[test]
fn claude_projects_dir_ends_with_expected_suffix() {
    let dir = claude_projects_dir().unwrap();
    assert!(dir.ends_with(".claude/projects"));
}
