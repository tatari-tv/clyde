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

// Deliberately does NOT mutate a process-global env var (e.g. `TMPDIR`) to prove this property:
// doing so would make every OTHER concurrently-running test's own `TempDir::new()` (which
// consults the same system temp dir) flaky, since Rust runs tests in parallel by default.
// Instead, this makes the target's own parent directory read-only (write+execute stays clear on
// nothing) and inspects *which stage* of `write_atomic` fails. A correct implementation calls
// `NamedTempFile::new_in(parent)`, so the temp-file *creation* itself fails inside `parent` and
// the error names "failed to create temp file in <parent>". An implementation that instead
// created its temp file in the OS temp dir (which stays writable here) would get past creation
// and only fail later at `persist`'s rename step, with a different message. Pinning the former
// message proves the temp file was attempted directly inside the target's own directory.
#[cfg(unix)]
#[test]
fn temp_file_is_created_in_targets_own_directory_not_system_tmp() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let target_parent = dir.path().join("only-writable-by-nobody");
    std::fs::create_dir(&target_parent).unwrap();
    std::fs::set_permissions(&target_parent, std::fs::Permissions::from_mode(0o500)).unwrap();

    let target = target_parent.join("settings.json");
    let err = write_atomic(&target, b"hello").expect_err("a read-only target directory must fail the write");

    // Restore write permission so TempDir can clean up the directory on drop.
    std::fs::set_permissions(&target_parent, std::fs::Permissions::from_mode(0o700)).unwrap();

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

#[cfg(unix)]
#[test]
fn readonly_parent_dir_returns_error_not_panic() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let sub = dir.path().join("readonly");
    std::fs::create_dir(&sub).unwrap();
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o500)).unwrap();

    let target = sub.join("settings.json");
    let result = write_atomic(&target, b"{}");

    // Restore write permission so TempDir can clean up the directory on drop.
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o700)).unwrap();

    assert!(result.is_err(), "a read-only parent directory must error, not panic");
}
