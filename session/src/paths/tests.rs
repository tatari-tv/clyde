#![allow(clippy::unwrap_used)]

use super::*;
use std::sync::MutexGuard;
use tempfile::TempDir;

/// Holds the env lock and restores one variable on DROP, so the original value comes back during
/// unwinding as well as on the happy path.
///
/// A restore written as the last statements of a test only runs when the assertions above it passed. One
/// failure then leaks the altered value into every later test in the same process, and the resulting
/// failures point at innocent tests. Found by CodeRabbit on PR #81; mirrors
/// `common/src/paths/tests.rs`'s guard, which covers the same hazard for the definition this file
/// delegates to.
struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    key: &'static str,
    prior: Option<String>,
}

impl EnvGuard {
    fn new(key: &'static str) -> Self {
        let lock = crate::ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        Self {
            _lock: lock,
            key,
            prior: std::env::var(key).ok(),
        }
    }

    fn set(&self, value: &str) {
        unsafe { std::env::set_var(self.key, value) };
    }

    fn unset(&self) {
        unsafe { std::env::remove_var(self.key) };
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.prior {
            Some(v) => unsafe { std::env::set_var(self.key, v) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

/// This wrapper DELEGATES to `common::paths::xdg_data_dir`, so what is checked here is the delegation:
/// the two agree, always. The env-honoring platform behavior itself is pinned once, in
/// `common/src/paths/tests.rs`, beside the one body that reads the variable.
///
/// BITES: give this wrapper its own body again and the two answers diverge the moment either changes,
/// which is exactly the five-copy drift this consolidation removed.
#[test]
fn xdg_data_dir_delegates_to_common() {
    let env = EnvGuard::new("XDG_DATA_HOME");

    let dir = TempDir::new().unwrap();
    env.set(&dir.path().to_string_lossy());
    assert_eq!(xdg_data_dir(), common::paths::xdg_data_dir());
    assert_eq!(xdg_data_dir().as_deref(), Some(dir.path()));

    // And they still agree on the fallback, not just on the env-set path.
    env.unset();
    assert_eq!(xdg_data_dir(), common::paths::xdg_data_dir());
    assert!(xdg_data_dir().unwrap().ends_with(".local/share"));
}

#[test]
fn xdg_config_dir_honors_env_and_falls_back() {
    let env = EnvGuard::new("XDG_CONFIG_HOME");

    let dir = TempDir::new().unwrap();
    env.set(&dir.path().to_string_lossy());
    assert_eq!(xdg_config_dir().as_deref(), Some(dir.path()));

    env.unset();
    assert!(xdg_config_dir().unwrap().ends_with(".config"));
}

#[test]
fn data_root_and_db_path_sit_under_clyde_namespace() {
    let env = EnvGuard::new("XDG_DATA_HOME");

    let dir = TempDir::new().unwrap();
    env.set(&dir.path().to_string_lossy());
    assert_eq!(data_root(), dir.path().join("clyde"));
    assert_eq!(sessions_db_path(), dir.path().join("clyde").join("sessions.db"));
    assert_eq!(staged_dir(), dir.path().join("clyde").join("staged"));
}

#[test]
fn claude_projects_dir_ends_with_expected_suffix() {
    let dir = claude_projects_dir().unwrap();
    assert!(dir.ends_with(".claude/projects"));
}
