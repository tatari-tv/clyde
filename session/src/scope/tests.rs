#![allow(clippy::unwrap_used)]

use super::*;
use std::path::PathBuf;

fn classify_str(s: &str) -> Scope {
    classify(Some(&PathBuf::from(s)))
}

#[test]
fn work_paths_classify_work() {
    assert_eq!(classify_str("/home/saidler/repos/tatari-tv/clyde/main"), Scope::Work);
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
    // slot -- it must classify personal, never get shipped to the work account.
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

/// `repos_touched` from `(slug, count)` pairs, the `<org>/<repo>` attribution shape the catalog stores
/// (measured live: `tatari-tv/thoughts`, `scottidler/claude`).
fn touched(pairs: &[(&str, u64)]) -> BTreeMap<String, u64> {
    pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
}

/// The touch-set cases: no repo attribution at all, so the classifier must fall through to the evidence
/// branch. Returns the scope alone; the basis is asserted separately in
/// `classify_with_evidence_reports_the_deciding_basis`.
fn with_evidence(cwd: Option<&str>, pairs: &[(&str, u64)], files_edited: u64) -> Scope {
    let path = cwd.map(PathBuf::from);
    classify_with_evidence(path.as_deref(), None, None, &touched(pairs), files_edited).scope
}

/// The same, with a repo attribution present. `source` is the stored `repo_source` spelling, parsed the
/// way `sessions::enrich` parses it, so a typo'd variant name fails here rather than silently becoming
/// `None` in production.
fn with_repo(cwd: Option<&str>, repo: &str, source: &str, pairs: &[(&str, u64)], files_edited: u64) -> Decision {
    let path = cwd.map(PathBuf::from);
    let parsed: RepoSource = source.parse().expect("the test names a real RepoSource spelling");
    classify_with_evidence(path.as_deref(), Some(repo), Some(parsed), &touched(pairs), files_edited)
}

/// The Phase 4 table test. Five rows, one per branch of the widened rule.
///
/// BITES on every row: drop the unanchored-cwd requirement and row 3 flips to Work; drop the unanimity
/// rule and row 2 flips; drop the non-empty requirement and row 5 flips.
#[test]
fn classify_with_evidence_table() {
    // 1. Unanchored cwd + all-work touches, fully accounted -> Work. The whole point of the phase.
    assert_eq!(
        with_evidence(Some("/home/saidler/notes"), &[("tatari-tv/philo", 2)], 2),
        Scope::Work
    );
    // 2. Unanchored + MIXED -> Personal. One personal repo in the set refuses the whole session.
    assert_eq!(
        with_evidence(
            Some("/home/saidler/notes"),
            &[("tatari-tv/philo", 2), ("scottidler/claude", 1)],
            3
        ),
        Scope::Personal
    );
    // 3. Personal-ANCHORED cwd + all-work touches -> Personal. A positive personal signal is never
    // overturned by evidence; the widening only reaches "unclassifiable".
    assert_eq!(
        with_evidence(
            Some("/home/saidler/repos/scottidler/claude"),
            &[("tatari-tv/philo", 2)],
            2
        ),
        Scope::Personal
    );
    // 4. Work-anchored cwd + anything -> Work, by the unchanged cwd rule.
    assert_eq!(
        with_evidence(
            Some("/home/saidler/repos/tatari-tv/clyde"),
            &[("scottidler/claude", 5)],
            5
        ),
        Scope::Work
    );
    // 5. Unanchored + EMPTY touches -> Personal. No evidence is not evidence of work.
    assert_eq!(with_evidence(Some("/home/saidler/notes"), &[], 0), Scope::Personal);
    // And no cwd at all, with no evidence, is still personal.
    assert_eq!(with_evidence(None, &[], 0), Scope::Personal);
}

/// THE fail-open the review panel caught, and the totality rule that closes it.
///
/// `repos_touched` silently DROPS every edited path that does not resolve under `repo_root`, so a
/// session whose cwd is `~/notes` and which edits `~/notes/journal.md`, `~/Documents/taxes.md`, and one
/// file under `~/repos/tatari-tv/philo` yields `{tatari-tv/philo: 1}` -- unanchored, non-empty,
/// unanimously work. Without `sum(repos_touched) == files_edited`, that whole transcript, tax
/// discussion included, would be sent to the work Anthropic account.
///
/// BITES: drop the totality check and the first assertion below flips to Work.
#[test]
fn an_unaccounted_for_edit_refuses_the_widening() {
    // 3 files edited, only 1 attributed: the other 2 were outside repo_root and silently dropped.
    assert_eq!(
        with_evidence(Some("/home/saidler/notes"), &[("tatari-tv/philo", 1)], 3),
        Scope::Personal,
        "an unaccounted-for edit means the touch set is not the whole story"
    );
    // Fully accounted for, so the unanimity is real and the widening fires.
    assert_eq!(
        with_evidence(Some("/home/saidler/notes"), &[("tatari-tv/philo", 3)], 3),
        Scope::Work
    );
    // Accounted counts must MATCH, not merely reach: a touch set claiming more edits than the session
    // made is as incoherent as one claiming fewer, and incoherent evidence is not trusted.
    assert_eq!(
        with_evidence(Some("/home/saidler/notes"), &[("tatari-tv/philo", 4)], 3),
        Scope::Personal
    );
}

/// The org test on a touched repo is a DIFFERENT matching form from the cwd test, and each gets its own
/// test because "unifying" them would break one of the two.
///
/// A `repos_touched` key is an `<org>/<repo>` slug, so the org is the segment before the first `/`. The
/// cwd test walks path COMPONENTS for the slot after `repos`, which is what makes
/// `~/repos/scottidler/tatari-tv` personal. Both read the same `WORK_ORGS`.
#[test]
fn a_touched_slugs_org_is_the_segment_before_the_first_slash() {
    assert!(is_work_slug("tatari-tv/philo"));
    assert!(is_work_slug("tatari-tv/clyde"));
    // The org slot is the FIRST segment: a personal org owning a repo NAMED `tatari-tv` is personal,
    // the mirror of the cwd rule's `~/repos/scottidler/tatari-tv` case.
    assert!(!is_work_slug("scottidler/tatari-tv"));
    assert!(!is_work_slug("danielmiessler/fabric"));
    // Substring is not a match.
    assert!(!is_work_slug("tatari-tv-notes/x"));
    // A key that is not an `<org>/<repo>` slug at all fails CLOSED.
    assert!(!is_work_slug("tatari-tv"));
    assert!(!is_work_slug(""));
}

/// Malformed slugs that still contain a slash fail CLOSED. Found by the implementation-audit panel:
/// `is_work_slug` originally ignored the repo segment entirely, so `"tatari-tv/"` passed the org test on
/// an EMPTY repo name. The no-slash case already failed closed, which is what made the hole easy to miss.
///
/// Not reachable from clyde's own writer (`common::repo::slug_under_root` needs two normal path
/// components, so it can never emit an empty segment or a second slash), but this reads a STORED blob
/// and it is the gate that decides whether a session body leaves the machine.
///
/// BITES: restore `Some((org, _)) => WORK_ORGS.contains(&org)` and the first two assertions flip.
#[test]
fn a_malformed_slug_with_a_slash_fails_closed() {
    assert!(!is_work_slug("tatari-tv/"), "an empty repo segment is not a repo");
    assert!(
        !is_work_slug("tatari-tv/a/b"),
        "two slashes is not the documented shape"
    );
    assert!(!is_work_slug("/philo"), "an empty org segment is not a work org");
    assert!(!is_work_slug("/"));
    // The well-formed case is unaffected, so this is a narrowing and not a break.
    assert!(is_work_slug("tatari-tv/philo"));
}

/// A ZERO count is not a touch. The design's condition is "the session touched at least one repo", which
/// a map like `{"tatari-tv/philo": 0}` satisfies structurally while representing no touch at all. Found
/// by the implementation-audit panel: with `files_edited: 0` the totality check passed too, so a session
/// that edited NOTHING classified Work.
///
/// BITES: drop the `*count > 0` conjunct and the first assertion flips to Work.
#[test]
fn a_zero_count_touch_set_never_widens_to_work() {
    assert_eq!(
        with_evidence(Some("/home/saidler/notes"), &[("tatari-tv/philo", 0)], 0),
        Scope::Personal,
        "a session that edited nothing has no work evidence"
    );
    // One real touch plus one zero-count entry is still refused: EVERY entry must be a real touch, so
    // the unanimity is over actual evidence rather than over placeholders.
    assert_eq!(
        with_evidence(
            Some("/home/saidler/notes"),
            &[("tatari-tv/philo", 1), ("tatari-tv/clyde", 0)],
            1
        ),
        Scope::Personal
    );
    // And a positive count with matching totality still widens, so this narrowed nothing real.
    assert_eq!(
        with_evidence(Some("/home/saidler/notes"), &[("tatari-tv/philo", 1)], 1),
        Scope::Work
    );
}

/// An overflowing count sum fails CLOSED rather than wrapping. `sum::<u64>()` wraps silently in a
/// release build, and a wrapped total that happened to equal `files_edited` would satisfy the totality
/// check on nonsense evidence.
///
/// BITES: restore the plain `values().sum()` and the first assertion classifies Work in release, because
/// `u64::MAX + 2` wraps to 1.
#[test]
fn an_overflowing_count_sum_fails_closed() {
    assert_eq!(
        with_evidence(
            Some("/home/saidler/notes"),
            &[("tatari-tv/philo", u64::MAX), ("tatari-tv/clyde", 2)],
            1
        ),
        Scope::Personal,
        "a wrapped sum must never satisfy the totality check"
    );
}

/// `has_repos_anchor` answers "was this cwd placeable at all", which is the gate on whether evidence
/// gets a say. It must be true for a personal anchor (so evidence cannot overturn it) and false for a
/// path with no `repos/` component.
#[test]
fn has_repos_anchor_detects_any_org_slot_not_just_work() {
    assert!(has_repos_anchor(&PathBuf::from(
        "/home/saidler/repos/scottidler/claude"
    )));
    assert!(has_repos_anchor(&PathBuf::from("/home/saidler/repos/tatari-tv/clyde")));
    // `repos` with something after it is an anchor even if that something is not a known org.
    assert!(has_repos_anchor(&PathBuf::from("/home/saidler/repos/whoever/x")));
    // No `repos` component, or nothing after it, is unanchored.
    assert!(!has_repos_anchor(&PathBuf::from("/home/saidler/notes")));
    assert!(!has_repos_anchor(&PathBuf::from("/tmp/scratch")));
    assert!(!has_repos_anchor(&PathBuf::from("/home/saidler/repos")));
}

/// The git-origin branch, which is the fix for the `~/repos/<org>/<repo>` layout assumption.
///
/// Each row is one of the four layouts measured on 2026-07-31, none of which has an org slot a path walk
/// can read. Before this branch every one of them classified Personal and sat at 0% coverage.
///
/// BITES: delete the `RepoSource::GitOrigin` branch and every Work row here flips to Personal.
#[test]
fn git_origin_classifies_every_real_world_layout() {
    for cwd in [
        "/Users/stephen/code/work/philo", // Stephen
        "/Users/luke/Projects/philo",     // Luke
        "/home/keegan/git/tatari/philo",  // Keegan: an org slot, but it reads `tatari`, not `tatari-tv`
        "/home/patrick",                  // Patrick: no structure at all
        "/Users/someone/wt/philo",        // a bare worktree root
    ] {
        let d = with_repo(Some(cwd), "tatari-tv/philo", "git-origin", &[], 0);
        assert_eq!(d.scope, Scope::Work, "the remote must place a session run from {cwd}");
        assert_eq!(d.basis, Basis::GitOrigin);
    }
}

/// The branch is definitive in BOTH directions, and that is the safe choice.
///
/// A personal remote returns Personal outright rather than falling through to the touch set. That is
/// strictly safer than today: without it, a session whose own checkout is provably personal could still
/// be widened to Work by a unanimous work touch set.
///
/// BITES: make the branch fall through on a non-work slug instead of returning, and the second case
/// flips to Work.
#[test]
fn git_origin_refuses_a_personal_remote_and_outranks_the_touch_set() {
    let d = with_repo(
        Some("/Users/luke/Projects/claude"),
        "scottidler/claude",
        "git-origin",
        &[],
        0,
    );
    assert_eq!(d.scope, Scope::Personal);
    assert_eq!(d.basis, Basis::GitOrigin);

    // A personal remote, and a touch set that WOULD satisfy unanimity + totality on its own.
    let d = with_repo(
        Some("/Users/luke/Projects/claude"),
        "scottidler/claude",
        "git-origin",
        &[("tatari-tv/philo", 2)],
        2,
    );
    assert_eq!(
        d.scope,
        Scope::Personal,
        "a provably personal checkout must not be widened by its touch set"
    );
}

/// Only `git-origin` confers scope. The other three sources must not.
///
/// `known-path` and `path-guess` are path conventions, which is the thing being fixed, and
/// `files-touched` is the touch set -- routing it through here would bypass the unanimity and totality
/// checks the touch-set branch applies. Each is asserted with a WORK slug and an empty touch set, so a
/// Work answer could only come from the source being wrongly trusted.
///
/// BITES: widen the branch's condition to `repo_source.is_some()` and all three flip to Work.
#[test]
fn only_git_origin_confers_scope() {
    for source in ["known-path", "files-touched", "path-guess"] {
        let d = with_repo(
            Some("/Users/stephen/code/work/philo"),
            "tatari-tv/philo",
            source,
            &[],
            0,
        );
        assert_eq!(
            d.scope,
            Scope::Personal,
            "{source} must not confer work scope on its own"
        );
        assert_eq!(d.basis, Basis::TouchSet, "{source} must fall through to the touch set");
    }
}

/// A work-anchored cwd still wins outright, and a personal-anchored cwd is still judged by its anchor
/// before the remote is consulted. The precedence in the doc comment is the precedence in the code.
#[test]
fn cwd_anchor_outranks_the_remote_in_both_directions() {
    // Work anchor + personal remote -> Work by the anchor, unchanged from before this branch existed.
    let d = with_repo(
        Some("/home/saidler/repos/tatari-tv/clyde"),
        "scottidler/claude",
        "git-origin",
        &[],
        0,
    );
    assert_eq!(d.scope, Scope::Work);
    assert_eq!(d.basis, Basis::CwdAnchor);

    // Personal anchor + work remote -> Personal by the anchor. This is the conservative placement: the
    // remote never overturns a positive personal signal, matching the touch set's own restriction.
    let d = with_repo(
        Some("/home/saidler/repos/scottidler/claude"),
        "tatari-tv/philo",
        "git-origin",
        &[],
        0,
    );
    assert_eq!(d.scope, Scope::Personal);
    assert_eq!(d.basis, Basis::CwdAnchor);
}

/// The basis is what the caller's provisional-`scope_version` rule keys on, so a wrong basis is a real
/// defect and not a cosmetic label. Only `TouchSet` may report that it read stored evidence.
///
/// BITES: return `Basis::TouchSet` from the git-origin branch and `sessions::enrich` marks those
/// decisions provisional, re-offering every such row on every pass forever.
#[test]
fn classify_with_evidence_reports_the_deciding_basis() {
    assert!(Basis::TouchSet.reads_stored_evidence());
    assert!(!Basis::GitOrigin.reads_stored_evidence());
    assert!(!Basis::CwdAnchor.reads_stored_evidence());

    // No cwd and no attribution: the touch set is the only signal left.
    let path: Option<PathBuf> = None;
    let d = classify_with_evidence(path.as_deref(), None, None, &touched(&[]), 0);
    assert_eq!(d.basis, Basis::TouchSet);
    assert_eq!(d.scope, Scope::Personal, "no signal at all must stay fail-safe");

    // A malformed slug on an otherwise-trusted source fails closed rather than panicking or widening.
    let d = with_repo(
        Some("/Users/stephen/code/work/philo"),
        "tatari-tv",
        "git-origin",
        &[],
        0,
    );
    assert_eq!(
        d.scope,
        Scope::Personal,
        "a slug with no repo segment is not a work slug"
    );
}

#[test]
fn scope_tokens_are_stable() {
    assert_eq!(Scope::Work.as_str(), "work");
    assert_eq!(Scope::Personal.as_str(), "personal");
    assert!(Scope::Work.is_work());
    assert!(!Scope::Personal.is_work());
}
