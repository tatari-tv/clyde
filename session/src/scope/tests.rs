#![allow(clippy::unwrap_used)]

use super::*;
use std::path::PathBuf;

fn classify_str(s: &str) -> Scope {
    classify(Some(&PathBuf::from(s)))
}

#[test]
fn work_paths_classify_work() {
    assert_eq!(classify_str("/home/saidler/repos/tatari-tv/klod/main"), Scope::Work);
    assert_eq!(classify_str("/home/saidler/repos/tatari-tv/philo"), Scope::Work);
    // The org dir itself (no repo beneath) is still work.
    assert_eq!(classify_str("/home/saidler/repos/tatari-tv"), Scope::Work);
}

#[test]
fn personal_paths_classify_personal() {
    assert_eq!(classify_str("/home/saidler/repos/scottidler/loopr"), Scope::Personal);
    assert_eq!(
        classify_str("/home/saidler/repos/danielmiessler/fabric"),
        Scope::Personal
    );
}

#[test]
fn unknown_and_missing_cwd_fail_safe_to_personal() {
    // No cwd at all -> personal (never assumed shippable to the work account).
    assert_eq!(classify(None), Scope::Personal);
    // A bare home dir, /tmp, anything unrecognized -> personal.
    assert_eq!(classify_str("/home/saidler"), Scope::Personal);
    assert_eq!(classify_str("/tmp/scratch"), Scope::Personal);
    assert_eq!(classify_str(""), Scope::Personal);
}

#[test]
fn substring_of_work_org_is_not_work() {
    // Exact-component match only: a personal repo that merely contains the marker as a substring
    // must NOT be misclassified as work.
    assert_eq!(
        classify_str("/home/saidler/repos/scottidler/tatari-tv-notes"),
        Scope::Personal
    );
    assert_eq!(classify_str("/home/saidler/tatari-tv-personal/x"), Scope::Personal);
}

#[test]
fn work_org_only_matches_the_org_slot_not_anywhere() {
    // (Codex audit finding) A personal repo *named* `tatari-tv` sits in the repo slot, not the org
    // slot — it must classify personal, never get shipped to the work account.
    assert_eq!(
        classify_str("/home/saidler/repos/scottidler/tatari-tv"),
        Scope::Personal
    );
    // A `tatari-tv` component with no `repos/` anchor (scratchpad, alt root) is personal.
    assert_eq!(classify_str("/tmp/tatari-tv/scratch"), Scope::Personal);
    assert_eq!(classify_str("/home/saidler/work/tatari-tv/x"), Scope::Personal);
    // Only the component immediately after `repos` is the org; depth below it stays work.
    assert_eq!(classify_str("/home/saidler/repos/tatari-tv/anything/deep"), Scope::Work);
}

fn repo_str(s: &str) -> Option<String> {
    repo_slug(Some(&PathBuf::from(s)))
}

#[test]
fn repo_slug_derives_org_and_name_from_repos_anchor() {
    assert_eq!(
        repo_str("/home/saidler/repos/tatari-tv/drata-cli").as_deref(),
        Some("tatari-tv/drata-cli")
    );
    assert_eq!(
        repo_str("/home/saidler/repos/scottidler/manifest").as_deref(),
        Some("scottidler/manifest")
    );
    // A deeper working dir resolves to its top repo slot (org + the component after it); the first
    // `repos` anchor wins.
    assert_eq!(
        repo_str("/home/saidler/repos/tatari-tv/clyde/main").as_deref(),
        Some("tatari-tv/clyde")
    );
}

#[test]
fn repo_slug_is_null_without_a_repos_anchor() {
    // No cwd at all -> None (the export `repo` field is null).
    assert_eq!(repo_slug(None), None);
    // A bare home dir / scratch path has no `repos/<org>/<repo>` anchor -> None.
    assert_eq!(repo_str("/home/saidler"), None);
    assert_eq!(repo_str("/tmp/scratch"), None);
    // `repos` present but no repo component after the org slot -> None (needs org AND name).
    assert_eq!(repo_str("/home/saidler/repos/tatari-tv"), None);
}

#[test]
fn scope_tokens_are_stable() {
    assert_eq!(Scope::Work.as_str(), "work");
    assert_eq!(Scope::Personal.as_str(), "personal");
    assert!(Scope::Work.is_work());
    assert!(!Scope::Personal.is_work());
}
