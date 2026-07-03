#![allow(clippy::unwrap_used)]

use super::*;
use tempfile::TempDir;

#[test]
fn creates_a_new_file() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("settings.json");
    assert!(!target.exists());

    write_atomic(&target, b"{}").unwrap();

    assert_eq!(std::fs::read(&target).unwrap(), b"{}");
}

#[test]
fn overwrites_an_existing_file() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("settings.json");
    std::fs::write(&target, "old-content").unwrap();

    write_atomic(&target, b"new-content").unwrap();

    assert_eq!(std::fs::read(&target).unwrap(), b"new-content");
}

#[cfg(unix)]
#[test]
fn overwrite_preserves_existing_file_mode() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let target = dir.path().join("statusline.sh");
    std::fs::write(&target, "#!/bin/sh\necho old\n").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();

    write_atomic(&target, b"#!/bin/sh\necho new\n").unwrap();

    let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "exec bit must survive an atomic overwrite");
}

// This proves the temp file is created in the target's OWN parent directory, not the OS temp dir.
//
// It deliberately does NOT prove the point by making the parent directory read-only: CI runs the
// test suite as root, and root bypasses directory permission bits (CAP_DAC_OVERRIDE), so a 0o500
// dir stays writable there and the write would wrongly succeed. It also must NOT mutate a
// process-global env var (e.g. `TMPDIR`): that would make every OTHER concurrently-running test's
// own `TempDir::new()` (which consults the same system temp dir) flaky, since Rust runs tests in
// parallel by default.
//
// Instead the target's own parent directory is *missing*. `stat` of the target then reports
// NotFound (ENOENT), so `write_atomic` proceeds past its existing-file probe, and
// `NamedTempFile::new_in(parent)` fails with ENOENT because the directory isn't there. The error
// names "failed to create temp file in <parent>". ENOENT is uid-independent, so this holds under
// CI's root too. An implementation that instead created its temp file in the OS temp dir (which
// exists) would get past creation and only fail later at `persist`'s rename step, with a different
// message. Pinning the former message proves the temp file was attempted directly inside the
// target's own directory.
#[cfg(unix)]
#[test]
fn temp_file_is_created_in_targets_own_directory_not_system_tmp() {
    let dir = TempDir::new().unwrap();
    let target_parent = dir.path().join("does-not-exist");
    let target = target_parent.join("settings.json");

    let err = write_atomic(&target, b"hello").expect_err("temp-file creation in a missing parent must fail");

    let message = err.to_string();
    assert!(
        message.contains("failed to create temp file in"),
        "expected the temp-file *creation* step to fail (proving it was attempted in the \
         target's own directory, not the system temp dir): {message}"
    );
    assert!(
        message.contains(target_parent.to_str().unwrap()),
        "error must name the target's own parent directory: {message}"
    );
}

// A parent that is a regular file (not a directory) makes `stat` of the target return ENOTDIR,
// exercising `write_atomic`'s non-NotFound stat-error arm; the whole thing must surface as a typed
// error, never a panic. Uses this uid-independent failure rather than a read-only directory because
// CI runs as root and root bypasses read-only directory bits.
#[cfg(unix)]
#[test]
fn non_directory_parent_returns_error_not_panic() {
    let dir = TempDir::new().unwrap();
    let not_a_dir = dir.path().join("not-a-directory");
    std::fs::write(&not_a_dir, b"regular file").unwrap();

    let target = not_a_dir.join("settings.json");
    let result = write_atomic(&target, b"{}");

    assert!(result.is_err(), "a non-directory parent must error, not panic");
}
