#![allow(clippy::unwrap_used)]

use super::*;
use std::sync::Mutex;
use tempfile::TempDir;

// Env-var mutation is process-global; serialize every env-touching test.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// The env-honoring platform test, moved here with the definition it covers: this is now the ONE body
/// that reads `$XDG_DATA_HOME`, so this is where the behavior is pinned. The four public wrappers keep
/// their own tests, which check the DELEGATION rather than re-testing this.
///
/// Asserts behavior, never a platform-specific path: no `#[cfg(target_os)]` branch and no
/// `~/Library/Application Support` assertion. That is the whole point of not using `dirs::data_local_dir`.
#[test]
fn xdg_data_dir_honors_env_and_falls_back() {
    let guard = ENV_LOCK.lock().unwrap();
    let prior = std::env::var("XDG_DATA_HOME").ok();

    let dir = TempDir::new().unwrap();
    unsafe { std::env::set_var("XDG_DATA_HOME", dir.path()) };
    assert_eq!(xdg_data_dir().as_deref(), Some(dir.path()));

    // Unset -> `$HOME/.local/share` on EVERY platform, macOS included.
    unsafe { std::env::remove_var("XDG_DATA_HOME") };
    assert!(xdg_data_dir().unwrap().ends_with(".local/share"));

    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
    }
    drop(guard);
}

/// A RELATIVE `$XDG_DATA_HOME` is ignored, falling back to `$HOME/.local/share`. Per the XDG spec, and
/// load-bearing: honoring it would make the data root resolve differently for every process depending
/// on its cwd, so two clyde invocations from different directories would use different catalogs.
///
/// BITES: drop the `path.is_absolute()` guard and this returns the relative path.
#[test]
fn a_relative_xdg_data_home_is_ignored() {
    let guard = ENV_LOCK.lock().unwrap();
    let prior = std::env::var("XDG_DATA_HOME").ok();

    unsafe { std::env::set_var("XDG_DATA_HOME", "relative/not/absolute") };
    let resolved = xdg_data_dir().unwrap();
    assert!(
        resolved.is_absolute(),
        "a relative XDG_DATA_HOME must not become the data root: {resolved:?}"
    );
    assert!(resolved.ends_with(".local/share"));

    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
    }
    drop(guard);
}
