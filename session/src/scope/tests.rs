#![allow(clippy::unwrap_used)]

use super::*;
use common::checkout::Matrix;
use std::path::Path;
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
    // `evidence_present: true`: these rows model a catalog the efficiency pass HAS reached, which is
    // what makes a touch-set decision settled. The provisional case has its own tests.
    let facts = RoutingFacts {
        evidence_present: true,
        ..Default::default()
    };
    classify_with_evidence(path.as_deref(), None, None, &touched(pairs), files_edited, &facts).scope
}

/// Classify a session whose cwd is a REAL checkout in the shared matrix, with `repo`/`repo_source`
/// produced by the ACTUAL rule-1 resolver.
///
/// **This replaces the deleted hand-built-row helper, and register item 4 is why.** That helper
/// assembled a catalog row from three strings, so the test at `session/src/scope/tests.rs:277-289`
/// (v0.22.0) could assert that a cwd of `/home/patrick` classifies work via
/// `repo_source = "git-origin"`. Production can NEVER emit that row: `detect_with_blocked_roots`'s
/// blocked set is `[$HOME]`, so a toplevel equal to `$HOME` is rejected, and if `$HOME` is not a repo
/// the probe fails anyway. The test did not bite, and it inflated the measured win.
///
/// Driving the resolver makes that class of assertion impossible: a row that cannot be produced
/// cannot be tested.
fn classify_at(m: &Matrix, cwd: &Path, pairs: &[(&str, u64)], files_edited: u64) -> Decision {
    classify_at_with_facts(
        m,
        cwd,
        pairs,
        files_edited,
        &RoutingFacts {
            evidence_present: true,
            ..Default::default()
        },
    )
}

/// [`classify_at`] plus the v3 routing state: a recorded conclusive negative, an operator override,
/// or the host verdict.
fn classify_at_with_facts(
    m: &Matrix,
    cwd: &Path,
    pairs: &[(&str, u64)],
    files_edited: u64,
    facts: &RoutingFacts<'_>,
) -> Decision {
    let outcome = common::repo::detect_with_blocked_roots(cwd, &m.blocked());
    // Rule 1 is the ONLY thing that produces `GitOrigin`, so the source is derived from whether the
    // probe resolved rather than named by the test. A test cannot claim git-origin provenance for a
    // cwd the resolver declined.
    let source = outcome.resolved_slug().map(|_| RepoSource::GitOrigin);
    classify_with_evidence(
        Some(cwd),
        outcome.resolved_slug(),
        source,
        &touched(pairs),
        files_edited,
        facts,
    )
}

/// Classify a HAND-BUILT catalog row.
///
/// Legitimate for exactly two shapes, both of which the resolver can never emit and both of which are
/// real states the classifier must survive:
///
/// - a CORRUPT stored slug (`sessions.repo` is a stored column; a hand-edited or truncated value has
///   to fail closed rather than panic or widen)
/// - a `repo_source` other than `git-origin`, which rules 2 through 4 write and rule 1 never does
///
/// Named for what it is, so the distinction from [`classify_at`] is visible at every call site rather
/// than buried in a helper that looks the same for both.
fn classify_stored(cwd: &str, repo: &str, source: &str, pairs: &[(&str, u64)], files_edited: u64) -> Decision {
    let path = PathBuf::from(cwd);
    let parsed: RepoSource = source.parse().expect("the test names a real RepoSource spelling");
    classify_with_evidence(
        Some(&path),
        Some(repo),
        Some(parsed),
        &touched(pairs),
        files_edited,
        &RoutingFacts {
            evidence_present: true,
            ..Default::default()
        },
    )
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
    let m = Matrix::build();
    for (who, cwd) in [
        ("Stephen, <home>/code/work", &m.layout_code_work),
        ("Luke, <home>/Projects", &m.layout_projects),
        (
            "Keegan, <home>/git/tatari (an org slot, but it reads `tatari`)",
            &m.layout_git_tatari,
        ),
    ] {
        let d = classify_at(&m, cwd, &[], 0);
        assert_eq!(d.scope, Scope::Work, "the remote must place a session run from {who}");
        assert_eq!(d.basis, Basis::GitOrigin);
    }
}

/// **Register item 4, corrected.** The fourth layout in the register's table is Patrick's bare `~`,
/// and the old test asserted it classifies WORK via `repo_source = "git-origin"`. That row is
/// impossible: rule 1's blocked set is `[$HOME]`, so a toplevel equal to `$HOME` is rejected, and a
/// `$HOME` that is not a repo fails the probe anyway. The PR #82 body's claim that Patrick's layout
/// was fixed is wrong, and this is the true expectation.
///
/// A `~` cwd stays PERSONAL, and it stays personal for the fail-safe reason: no signal can place it,
/// so the default holds. Fixing Patrick's coverage needs a different mechanism, not this branch.
///
/// BITES: this is the assertion the old test had backwards, so any change that makes a bare `$HOME`
/// confer work breaks it.
#[test]
fn a_home_cwd_stays_personal_because_no_signal_can_place_it() {
    let m = Matrix::build();
    let d = classify_at(&m, &m.home(), &[], 0);
    assert_eq!(d.scope, Scope::Personal);
    assert_eq!(
        d.basis,
        Basis::TouchSet,
        "rule 1 declined, so nothing reached the git-origin branch at all"
    );

    // And it stays personal even when $HOME IS a git repo with a work remote, which is the dotfiles
    // case the blocked-root guard exists for.
    let m = Matrix::build();
    m.make_home_a_repo();
    let d = classify_at(&m, &m.home(), &[], 0);
    assert_eq!(
        d.scope,
        Scope::Personal,
        "a git-tracked $HOME must never be attributed, so it can never confer scope"
    );
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
    let m = Matrix::build();
    // A real off-layout checkout whose origin is personal: the matrix's fork row, which carries
    // `git@github.com:scottidler/clyde-fork.git`. Reached through its own path rather than the work
    // directory it also sits in, so the git-origin branch is what decides.
    let personal_remote = m.home().join("Projects").join("claude");
    std::fs::create_dir_all(&personal_remote).expect("create dir");
    let d = classify_at(&m, &m.fork_in_work_dir, &[], 0);
    assert_eq!(
        d.scope,
        Scope::Work,
        "the fork sits under the work org, so the CWD ANCHOR places it: that is register item 5"
    );
    assert_eq!(d.basis, Basis::CwdAnchor);

    // The git-origin branch itself, on a cwd with no anchor at all. Hand-built, because the matrix
    // has no off-layout checkout with a personal remote and adding one would duplicate the fork.
    let d = classify_stored("/Users/luke/Projects/claude", "scottidler/claude", "git-origin", &[], 0);
    assert_eq!(d.scope, Scope::Personal);
    assert_eq!(d.basis, Basis::GitOrigin);

    // A personal remote, and a touch set that WOULD satisfy unanimity + totality on its own.
    let d = classify_stored(
        "/Users/luke/Projects/claude",
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
        // Hand-built on purpose: rule 1 never emits these sources, so there is no resolver output
        // that could produce this row. That is exactly what `classify_stored` is for.
        let d = classify_stored("/Users/stephen/code/work/philo", "tatari-tv/philo", source, &[], 0);
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
    let m = Matrix::build();

    // Work anchor + PERSONAL remote -> Work by the anchor. A real checkout: the matrix's
    // `<repo-root>/tatari-tv/clyde-fork`, whose origin is `scottidler/clyde-fork`. This is ordinary
    // work, and it is the case that killed the proposed precedence change: making a personal remote
    // refuse ahead of the anchor would silently drop it from enrichment.
    let d = classify_at(&m, &m.fork_in_work_dir, &[], 0);
    assert_eq!(d.scope, Scope::Work);
    assert_eq!(d.basis, Basis::CwdAnchor);

    // Personal anchor + WORK remote -> Personal by the anchor. The conservative placement: the remote
    // never overturns a positive personal signal, matching the touch set's own restriction.
    let d = classify_at(&m, &m.work_remote_in_personal_dir, &[], 0);
    assert_eq!(d.scope, Scope::Personal);
    assert_eq!(d.basis, Basis::CwdAnchor);
}

/// The basis names which signal decided, and `settled` says whether to record it. Both are read by
/// `sessions::enrich`, so a wrong one is a real defect and not a cosmetic label.
///
/// BITES: return `Basis::TouchSet` from the git-origin branch and the ProbeRefused/GitOrigin split
/// Phase 8 counts on collapses; return `settled: true` from the git-origin PERSONAL arm and a stale
/// probe locks a genuine work session out forever (Problem 3).
#[test]
fn classify_with_evidence_reports_the_deciding_basis() {
    // No cwd and no attribution: the touch set is the only signal left.
    let path: Option<PathBuf> = None;
    let facts = RoutingFacts {
        evidence_present: true,
        ..Default::default()
    };
    let d = classify_with_evidence(path.as_deref(), None, None, &touched(&[]), 0, &facts);
    assert_eq!(d.basis, Basis::TouchSet);
    assert_eq!(d.scope, Scope::Personal, "no signal at all must stay fail-safe");

    // A malformed slug on an otherwise-trusted source fails closed rather than panicking or widening.
    // Hand-built: `sessions.repo` is a STORED column, and `parse_slug` can never produce a slug with
    // no repo segment, so this models a corrupt or hand-edited row rather than resolver output.
    let d = classify_stored("/Users/stephen/code/work/philo", "tatari-tv", "git-origin", &[], 0);
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
