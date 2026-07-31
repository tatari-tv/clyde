#![allow(clippy::unwrap_used)]

use super::*;
use std::sync::{Mutex, MutexGuard};
use tempfile::TempDir;

// Env-var mutation is process-global; serialize every env-touching test.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Holds the env lock and restores `$XDG_DATA_HOME` on DROP, so the original value comes back during
/// unwinding as well as on the happy path.
///
/// A restore written as the last statements of a test only runs when the assertions above it passed. One
/// failure then leaks the altered value into every later test in the same process, and the resulting
/// failures point at innocent tests -- the worst kind to debug. Found by CodeRabbit on PR #81.
///
/// The lock is held for the guard's lifetime, so a panicking test poisons it and later env-touching
/// tests fail loudly on `unwrap()` rather than reading a corrupted environment.
struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    prior: Option<String>,
}

impl EnvGuard {
    fn new() -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        Self {
            _lock: lock,
            prior: std::env::var("XDG_DATA_HOME").ok(),
        }
    }

    fn set(&self, value: &str) {
        unsafe { std::env::set_var("XDG_DATA_HOME", value) };
    }

    fn unset(&self) {
        unsafe { std::env::remove_var("XDG_DATA_HOME") };
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.prior {
            Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
        }
    }
}

/// The env-honoring platform test, moved here with the definition it covers: this is now the ONE body
/// that reads `$XDG_DATA_HOME`, so this is where the behavior is pinned. The four public wrappers keep
/// their own tests, which check the DELEGATION rather than re-testing this.
///
/// Asserts behavior, never a platform-specific path: no `#[cfg(target_os)]` branch and no
/// `~/Library/Application Support` assertion. That is the whole point of not using `dirs::data_local_dir`.
#[test]
fn xdg_data_dir_honors_env_and_falls_back() {
    let env = EnvGuard::new();

    let dir = TempDir::new().unwrap();
    env.set(&dir.path().to_string_lossy());
    assert_eq!(xdg_data_dir().as_deref(), Some(dir.path()));

    // Unset -> `$HOME/.local/share` on EVERY platform, macOS included.
    env.unset();
    assert!(xdg_data_dir().unwrap().ends_with(".local/share"));
}

/// A RELATIVE `$XDG_DATA_HOME` is ignored, falling back to `$HOME/.local/share`. Per the XDG spec, and
/// load-bearing: honoring it would make the data root resolve differently for every process depending
/// on its cwd, so two clyde invocations from different directories would use different catalogs.
///
/// BITES: drop the `path.is_absolute()` guard and this returns the relative path.
#[test]
fn a_relative_xdg_data_home_is_ignored() {
    let env = EnvGuard::new();

    env.set("relative/not/absolute");
    let resolved = xdg_data_dir().unwrap();
    assert!(
        resolved.is_absolute(),
        "a relative XDG_DATA_HOME must not become the data root: {resolved:?}"
    );
    assert!(resolved.ends_with(".local/share"));
}
