#![allow(clippy::unwrap_used)]

use super::*;
use std::process::Command;
use tempfile::TempDir;

fn git_init(dir: &Path) {
    let s = Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(s.success());
    let s = Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(s.success());
    let s = Command::new("git")
        .args(["config", "user.name", "test"])
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(s.success());
}

fn add_origin(dir: &Path, url: &str) {
    let s = Command::new("git")
        .args(["remote", "add", "origin", url])
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(s.success());
}

#[test]
fn parse_slug_ssh_form() {
    assert_eq!(
        parse_slug("git@github.com:tatari-tv/claude-report.git"),
        Some("tatari-tv/claude-report".into())
    );
    assert_eq!(
        parse_slug("git@github.com:tatari-tv/claude-report"),
        Some("tatari-tv/claude-report".into())
    );
}

#[test]
fn parse_slug_https_form() {
    assert_eq!(
        parse_slug("https://github.com/scottidler/obsidian.git"),
        Some("scottidler/obsidian".into())
    );
    assert_eq!(
        parse_slug("https://github.com/scottidler/obsidian"),
        Some("scottidler/obsidian".into())
    );
}

#[test]
fn parse_slug_git_protocol() {
    assert_eq!(parse_slug("git://github.com/foo/bar.git"), Some("foo/bar".into()));
}

#[test]
fn parse_slug_garbage_returns_none() {
    assert_eq!(parse_slug(""), None);
    assert_eq!(parse_slug("not-a-url"), None);
    assert_eq!(parse_slug("https://github.com/onlyorg"), None);
}

#[test]
fn detect_returns_none_for_missing_dir() {
    let r = detect_with_blocked_roots(Path::new("/nonexistent/cr-test/missing"), &[]);
    assert_eq!(r, None);
}

#[test]
fn detect_returns_none_for_non_repo() {
    let tmp = TempDir::new().unwrap();
    let r = detect_with_blocked_roots(tmp.path(), &[]);
    assert_eq!(r, None);
}

#[test]
fn detect_returns_none_when_repo_has_no_origin() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().canonicalize().unwrap();
    git_init(&real);
    let r = detect_with_blocked_roots(&real, &[]);
    assert_eq!(r, None);
}

#[test]
fn detect_returns_slug_for_ssh_origin() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().canonicalize().unwrap();
    git_init(&real);
    add_origin(&real, "git@github.com:tatari-tv/claude-report.git");
    let r = detect_with_blocked_roots(&real, &[]);
    assert_eq!(r, Some("tatari-tv/claude-report".into()));
}

#[test]
fn detect_returns_slug_for_https_origin_no_dot_git() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().canonicalize().unwrap();
    git_init(&real);
    add_origin(&real, "https://github.com/scottidler/obsidian");
    let r = detect_with_blocked_roots(&real, &[]);
    assert_eq!(r, Some("scottidler/obsidian".into()));
}

#[test]
fn detect_finds_slug_from_subdirectory_of_repo() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().canonicalize().unwrap();
    git_init(&real);
    add_origin(&real, "git@github.com:foo/bar.git");
    let sub = real.join("src");
    std::fs::create_dir_all(&sub).unwrap();
    let r = detect_with_blocked_roots(&sub, &[]);
    assert_eq!(r, Some("foo/bar".into()));
}

#[test]
fn detect_rejects_dotfiles_climb_when_toplevel_is_blocked() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().canonicalize().unwrap();
    git_init(&real);
    add_origin(&real, "git@github.com:user/dotfiles.git");

    let unversioned = real.join("scratch").join("foo");
    std::fs::create_dir_all(&unversioned).unwrap();

    let r = detect_with_blocked_roots(&unversioned, std::slice::from_ref(&real));
    assert_eq!(r, None, "dotfiles climb should be rejected via blocked root");

    let r2 = detect_with_blocked_roots(&unversioned, &[]);
    assert_eq!(
        r2,
        Some("user/dotfiles".into()),
        "without blocked roots, the literal rule does not catch the climb"
    );
}

#[test]
fn resolver_caches_repeated_lookups() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().canonicalize().unwrap();
    git_init(&real);
    add_origin(&real, "git@github.com:foo/bar.git");

    let mut r = Resolver::new();
    let a = r.detect(&real);
    let b = r.detect(&real);
    assert_eq!(a, b);
    assert!(r.cache.contains_key(&real));
}
