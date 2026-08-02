#![allow(clippy::unwrap_used)]

use super::*;
use common::checkout::Matrix;
use std::path::Path;
use std::path::PathBuf;

/// The default single root, `~/repos`, as `Anchors`. Every path assertion below is written against
/// it because it is the layout the original rule hardcoded; the multi-root and off-layout cases have
/// their own tests and name their own roots.
fn anchors() -> Anchors {
    Anchors::new(&[PathBuf::from("/home/saidler/repos")])
}

/// The anchor's verdict for a path with NO probe recorded, which is every shape except the bare
/// `<root>/<work-org>` one. `None` means unanchored: the cwd expresses no opinion and the repo
/// evidence gets a say.
fn anchor_of(s: &str) -> Option<Scope> {
    anchors().scope_of(&PathBuf::from(s), None)
}

/// The scope a cwd alone produces through the REAL classifier, with no repo evidence at all. The
/// successor to the deleted cwd-only `classify`: that function was a SECOND implementation of this
/// question, and export ran it while the gate ran this one.
fn cwd_only(s: &str) -> Scope {
    classify_with_evidence(
        Some(&PathBuf::from(s)),
        None,
        None,
        &BTreeMap::new(),
        0,
        &anchors(),
        &RoutingFacts {
            evidence_present: true,
            ..Default::default()
        },
    )
    .scope
}

#[test]
fn work_paths_classify_work() {
    assert_eq!(cwd_only("/home/saidler/repos/tatari-tv/clyde/main"), Scope::Work);
    assert_eq!(cwd_only("/home/saidler/repos/tatari-tv/philo"), Scope::Work);
}

/// The org DIR itself, with no repo beneath it. 21 sessions on the live catalog, and the shape a
/// naive "a work org needs a following component" rule silently demotes to Personal.
///
/// It anchors Work ONLY on a conclusive `NotARepo`, because three other things can occupy that path:
/// a flat repo named `tatari-tv`, a bare container of the same name, and an empty repo with no
/// origin. See `bare_work_org_anchors_only_on_a_positively_observed_non_repository`.
///
/// BITES: delete the bare-work-org branch and this flips to Personal, taking the 21 with it.
#[test]
fn the_work_org_directory_itself_is_work_when_the_probe_saw_a_plain_directory() {
    assert_eq!(
        anchors().scope_of(
            &PathBuf::from("/home/saidler/repos/tatari-tv"),
            Some(RecordedProbe::Negative(&ProbeOutcome::NotARepo))
        ),
        Some(Scope::Work)
    );
}

#[test]
fn personal_paths_classify_personal() {
    assert_eq!(cwd_only("/home/saidler/repos/scottidler/loopr"), Scope::Personal);
    assert_eq!(cwd_only("/home/saidler/repos/danielmiessler/fabric"), Scope::Personal);
}

#[test]
fn unknown_and_missing_cwd_fail_safe_to_personal() {
    // No cwd at all -> personal (never assumed shippable to the work account).
    assert_eq!(
        classify_with_evidence(
            None,
            None,
            None,
            &BTreeMap::new(),
            0,
            &anchors(),
            &RoutingFacts {
                evidence_present: true,
                ..Default::default()
            }
        )
        .scope,
        Scope::Personal
    );
    // A bare home dir, /tmp, anything unrecognized -> personal.
    assert_eq!(cwd_only("/home/saidler"), Scope::Personal);
    assert_eq!(cwd_only("/tmp/scratch"), Scope::Personal);
    assert_eq!(cwd_only(""), Scope::Personal);
}

#[test]
fn substring_of_work_org_is_not_work() {
    // Exact-component match only: a personal repo that merely contains the marker as a substring
    // must NOT be misclassified as work.
    assert_eq!(
        cwd_only("/home/saidler/repos/scottidler/tatari-tv-notes"),
        Scope::Personal
    );
    assert_eq!(cwd_only("/home/saidler/tatari-tv-personal/x"), Scope::Personal);
}

#[test]
fn work_org_only_matches_the_org_slot_not_anywhere() {
    // (Codex audit finding) A personal repo *named* `tatari-tv` sits in the repo slot, not the org
    // slot -- it must classify personal, never get shipped to the work account.
    assert_eq!(cwd_only("/home/saidler/repos/scottidler/tatari-tv"), Scope::Personal);
    // A `tatari-tv` component under no configured root is unanchored, and with no evidence that is
    // Personal. Same verdict as before; the BASIS changed from a settled cwd anchor to the fallback.
    assert_eq!(cwd_only("/tmp/tatari-tv/scratch"), Scope::Personal);
    assert_eq!(cwd_only("/home/saidler/work/tatari-tv/x"), Scope::Personal);
    // Only the component immediately under a ROOT is the org; depth below it stays work.
    assert_eq!(cwd_only("/home/saidler/repos/tatari-tv/anything/deep"), Scope::Work);
}

/// P3, the defect. `<root>/<repo>` -- a flat clone with no org level -- used to read
/// `Personal, settled` off the literal `repos` component, which excluded it from
/// `enrich_candidates` until the next `SCOPE_VERSION` bump WITHOUT ever consulting the remote.
///
/// BITES: restore the `windows(2)` literal-`repos` match and this anchors Personal instead of
/// deferring.
///
/// The LIMIT is asserted too, and it is a real one: a SUBDIRECTORY of a flat clone
/// (`<root>/clyde/src`) is shape-identical to `<root>/<org>/<repo>` and still anchors Personal. No
/// rule reading the path alone can separate those two, so the fix covers a session that ran AT a
/// flat clone's root and not one that ran below it. Stated rather than hidden.
#[test]
fn a_flat_repo_under_a_root_is_unanchored_so_the_remote_can_answer() {
    assert_eq!(anchor_of("/home/saidler/repos/clyde"), None);
    assert_eq!(
        anchor_of("/home/saidler/repos/clyde/src"),
        Some(Scope::Personal),
        "indistinguishable from <root>/<org>/<repo> by the path alone; the limit, not the fix"
    );
}

/// P3's pre-existing bug, one level down. `~/repos/scottidler/repos/tatari-tv/x` reads WORK today
/// off the INNER literal `repos`, which is exactly the "contains a work org somewhere in the path"
/// failure the org-slot rule was written to avoid.
///
/// BITES: restore the literal-`repos` match and this reads Work.
#[test]
fn an_inner_repos_component_no_longer_manufactures_a_work_org() {
    assert_eq!(
        anchor_of("/home/saidler/repos/scottidler/repos/tatari-tv/x"),
        Some(Scope::Personal),
        "the org slot is `scottidler`, read under the CONFIGURED root"
    );
}

/// The disclosed NARROWING. A `repos/<work-org>` adjacency OUTSIDE every configured root stops
/// anchoring Work. For a live checkout the remote gives the same verdict by a better route; for a
/// VANISHED cwd there is no remote to ask, so it moves Work -> Personal. Fail-safe direction, one
/// line of config to restore.
#[test]
fn a_repos_work_org_adjacency_outside_every_root_no_longer_anchors_work() {
    assert_eq!(anchor_of("/elsewhere/repos/tatari-tv/x"), None);
    assert_eq!(anchor_of("/tmp/repos/scottidler/x"), None);
}

/// The intended WIDENING, in the leak direction, disclosed rather than quiet: a session under an
/// operator-declared root with a work-org slot gains Work scope where today it depends on the
/// remote. That is the same trust model `~/repos/tatari-tv/*` runs on, extended to the roots the
/// operator named. The operator declaring a root IS the authorization.
#[test]
fn an_off_layout_root_anchors_its_work_org_slot() {
    let off = Anchors::new(&[PathBuf::from("/home/stephen/code")]);
    assert_eq!(
        off.scope_of(&PathBuf::from("/home/stephen/code/tatari-tv/clyde"), None),
        Some(Scope::Work)
    );
    assert_eq!(
        off.scope_of(&PathBuf::from("/home/stephen/code/scottidler/x"), None),
        Some(Scope::Personal)
    );
    // A flat repo under the same root still defers: the path names no org.
    assert_eq!(off.scope_of(&PathBuf::from("/home/stephen/code/clyde"), None), None);
    // And `~/repos/...` is NOT a root here, so it anchors nothing.
    assert_eq!(
        off.scope_of(&PathBuf::from("/home/saidler/repos/tatari-tv/clyde"), None),
        None
    );
}

/// The anchor reads EVERY configured root, which is the whole point of P1 reaching the gate.
#[test]
fn the_anchor_matches_every_configured_root() {
    let both = Anchors::new(&[
        PathBuf::from("/home/stephen/code/work"),
        PathBuf::from("/home/stephen/wt"),
    ]);
    assert_eq!(
        both.scope_of(&PathBuf::from("/home/stephen/code/work/tatari-tv/philo"), None),
        Some(Scope::Work)
    );
    assert_eq!(
        both.scope_of(&PathBuf::from("/home/stephen/wt/tatari-tv/clyde"), None),
        Some(Scope::Work)
    );
    assert_eq!(
        both.scope_of(&PathBuf::from("/home/stephen/other/tatari-tv/x"), None),
        None
    );
}

/// An EMPTY root list anchors nothing. `de_repo_roots` refuses `repo-roots: []`, and
/// `EnrichOptions::default()` carries exactly this, so the fail-safe direction is that a caller which
/// forgets to set the roots LOSES coverage rather than gaining scope.
#[test]
fn no_roots_at_all_anchors_nothing() {
    let none = Anchors::default();
    assert_eq!(
        none.scope_of(&PathBuf::from("/home/saidler/repos/tatari-tv/clyde"), None),
        None
    );
    assert_eq!(
        none.scope_of(
            &PathBuf::from("/home/saidler/repos/tatari-tv"),
            Some(RecordedProbe::Negative(&ProbeOutcome::NotARepo))
        ),
        None
    );
}

/// The bare `<root>/<work-org>` shape, one row per [`ProbeOutcome`] variant. SIX in total, because
/// the rule is stated as an exhaustive table and it has to be asserted as one: two rounds of "defer
/// unless X" produced two holes, and a third was found only by walking the shape's occupants.
///
/// The match in `bare_work_org_is_an_org_dir` has no wildcard, so a SEVENTH variant is a compile
/// error and this test cannot silently stop covering the enum.
///
/// BITES, per row: widen the condition from `NotARepo` to "no slug resolved" and the `NoOrigin` and
/// `Indeterminate` rows flip to Work -- an EMPTY personal repo named `tatari-tv` shipped to the work
/// account on a directory-name coincidence. Narrow it to nothing and the `NotARepo` row flips to
/// None, which is the 21-session regression.
#[test]
fn bare_work_org_anchors_only_on_a_positively_observed_non_repository() {
    let bare = PathBuf::from("/home/saidler/repos/tatari-tv");
    let at = |probe: Option<RecordedProbe<'_>>| anchors().scope_of(&bare, probe);

    // The org DIRECTORY: git answered, and there is no repository here.
    assert_eq!(
        at(Some(RecordedProbe::Negative(&ProbeOutcome::NotARepo))),
        Some(Scope::Work),
        "the org dir"
    );
    // A checkout with a parseable origin: defer, and let the git-origin branch's guards apply. This
    // is the flat repo literally NAMED `tatari-tv`, the leak found in round 2.
    assert_eq!(
        at(Some(RecordedProbe::Negative(&ProbeOutcome::Resolved {
            slug: "scottidler/tatari-tv".into(),
            host: "github.com".into(),
        }))),
        None,
        "the remote decides, not the directory name"
    );
    // It IS a repo, it just has no remote. The EMPTY personal repo, the hole found in round 3.
    assert_eq!(
        at(Some(RecordedProbe::Negative(&ProbeOutcome::NoOrigin))),
        None,
        "an empty repo is not an org dir"
    );
    // The cwd is gone, or git could not answer. Absence of evidence.
    assert_eq!(at(Some(RecordedProbe::Negative(&ProbeOutcome::Indeterminate))), None);
    // The repo boundary is not at or above the cwd.
    assert_eq!(at(Some(RecordedProbe::Negative(&ProbeOutcome::OutsideRoot))), None);
    // The nearest boundary is a blocked root. Fails closed AGAINST one reviewer's advice: the
    // inference that `Blocked` implies "not its own checkout" is probably right, but a Work-granting
    // branch does not rest on one line of reasoning. The lost coverage is recoverable; a leak is not.
    assert_eq!(at(Some(RecordedProbe::Negative(&ProbeOutcome::Blocked))), None);
    // And nothing recorded at all, which is what every non-negative outcome persists as.
    assert_eq!(at(None), None);
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
    classify_with_evidence(
        path.as_deref(),
        None,
        None,
        &touched(pairs),
        files_edited,
        &anchors(),
        &facts,
    )
    .scope
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
        // The fixture's OWN repo root is the configured anchor, so a `<repo-root>/<org>/<repo>` row
        // IS anchored, which is exactly what `cwd_anchor_outranks_the_remote_in_both_directions`
        // asserts. The off-layout rows (`layout_code_work`, `layout_projects`, `layout_git_tatari`)
        // sit under no configured root and are therefore unanchored, which is what makes
        // `git_origin_classifies_every_real_world_layout` a test of the remote rather than of a path
        // convention that happens to agree.
        &Anchors::new(&[m.repo_root()]),
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
        &anchors(),
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

/// The anchor answers "was this cwd placeable at all", which is the gate on whether evidence gets a
/// say. It must decide for an anchored cwd (so evidence cannot overturn it) and DEFER for a path
/// with no org slot under a configured root.
///
/// The v3 version of this test asserted `/home/saidler/repos/whoever/x` was anchored and never asked
/// about `/home/saidler/repos/clyde`. That omission is P3: the flat shape read `Personal, settled`
/// and nothing falsified it.
#[test]
fn the_anchor_decides_for_any_org_slot_and_defers_otherwise() {
    assert_eq!(
        anchor_of("/home/saidler/repos/scottidler/claude"),
        Some(Scope::Personal)
    );
    assert_eq!(anchor_of("/home/saidler/repos/tatari-tv/clyde"), Some(Scope::Work));
    // A root component with something after it is an org slot even if that something is not a known
    // org, and a non-work org decides Personal.
    assert_eq!(anchor_of("/home/saidler/repos/whoever/x"), Some(Scope::Personal));
    // No configured root above it, or nothing under the root: unanchored.
    assert_eq!(anchor_of("/home/saidler/notes"), None);
    assert_eq!(anchor_of("/tmp/scratch"), None);
    assert_eq!(anchor_of("/home/saidler/repos"), None);
    // The personal ORG dir. Reaches the same Personal by a different route (the fallback, unsettled)
    // rather than by a settled anchor: it names no repo, so it is not a statement about scope.
    assert_eq!(anchor_of("/home/saidler/repos/scottidler"), None);
    // THE DEFECT: a flat clone under the root. `whoever/x` above passed for a release while this
    // silently read settled-personal.
    assert_eq!(anchor_of("/home/saidler/repos/clyde"), None);
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
    let d = classify_with_evidence(path.as_deref(), None, None, &touched(&[]), 0, &anchors(), &facts);
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

/// Register item 5's predicate, and the two mutants that survived the first mutation run.
///
/// KILLS: `delete ! in anchor_disagrees_with_remote` (which would report a disagreement for every
/// UNANCHORED cwd and none for anchored ones, exactly inverting what is counted) and
/// `replace != with == ` (which would report a disagreement whenever the two AGREE).
///
/// Both directions are asserted, because they mean different things to an operator: a work anchor
/// with a personal remote is usually a fork, a personal anchor with a work remote is usually a
/// misfiled clone.
#[test]
fn anchor_disagreement_is_reported_only_when_an_anchored_cwd_conflicts() {
    // Work anchor, personal remote: the fork.
    assert_eq!(
        anchor_disagrees_with_remote(
            &PathBuf::from("/home/saidler/repos/tatari-tv/clyde-fork"),
            "scottidler/clyde-fork",
            &anchors()
        ),
        Some(Disagreement {
            anchor: Scope::Work,
            remote: Scope::Personal
        })
    );
    // Personal anchor, work remote: the misfiled clone.
    assert_eq!(
        anchor_disagrees_with_remote(
            &PathBuf::from("/home/saidler/repos/scottidler/philo"),
            "tatari-tv/philo",
            &anchors()
        ),
        Some(Disagreement {
            anchor: Scope::Personal,
            remote: Scope::Work
        })
    );
    // AGREEMENT is not a disagreement, in both directions.
    assert_eq!(
        anchor_disagrees_with_remote(
            &PathBuf::from("/home/saidler/repos/tatari-tv/clyde"),
            "tatari-tv/clyde",
            &anchors()
        ),
        None
    );
    assert_eq!(
        anchor_disagrees_with_remote(
            &PathBuf::from("/home/saidler/repos/scottidler/loopr"),
            "scottidler/loopr",
            &anchors()
        ),
        None
    );
    // An UNANCHORED cwd expresses no opinion, so it can never disagree, however the remote reads.
    assert_eq!(
        anchor_disagrees_with_remote(
            &PathBuf::from("/Users/stephen/code/work/philo"),
            "tatari-tv/philo",
            &anchors()
        ),
        None
    );
    assert_eq!(
        anchor_disagrees_with_remote(
            &PathBuf::from("/Users/stephen/code/work/philo"),
            "scottidler/x",
            &anchors()
        ),
        None,
        "no anchor means nothing to conflict WITH, so this is silence rather than a conflict"
    );
}

/// The operator override, in the classifier itself.
///
/// KILLS: `replace == with != in classify_with_evidence` at the override comparison. With `!=`, an
/// override of `personal` reads as WORK, which is a leak introduced by the escape hatch meant to
/// prevent one.
///
/// `sessions` asserts the same behavior end to end through the real gate, but a mutant in THIS crate
/// is only checked by THIS crate's tests, so the end-to-end assertion cannot close it. That is the
/// general lesson: the biting test has to live in the crate that owns the code.
#[test]
fn an_operator_override_decides_in_both_directions() {
    let m = Matrix::build();
    let work = RoutingFacts {
        scope_override: Some("work"),
        evidence_present: true,
        ..Default::default()
    };
    let personal = RoutingFacts {
        scope_override: Some("personal"),
        evidence_present: true,
        ..Default::default()
    };

    // `personal` over a cwd the anchor would call WORK.
    let d = classify_at_with_facts(&m, &m.flat_ssh, &[], 0, &personal);
    assert_eq!(
        d.scope,
        Scope::Personal,
        "an override of `personal` must not read as work"
    );
    assert_eq!(d.basis, Basis::Override);
    assert!(d.settled, "a human is the highest-confidence evidence there is");

    // `work` over a cwd nothing else can place.
    let d = classify_at_with_facts(&m, &m.home(), &[], 0, &work);
    assert_eq!(d.scope, Scope::Work);
    assert_eq!(d.basis, Basis::Override);

    // An unrecognized token fails CLOSED. `Db::set_scope_override` rejects anything but the two
    // legal spellings, so reaching this means a hand-edited catalog.
    let garbage = RoutingFacts {
        scope_override: Some("WORK"),
        evidence_present: true,
        ..Default::default()
    };
    let d = classify_at_with_facts(&m, &m.flat_ssh, &[], 0, &garbage);
    assert_eq!(
        d.scope,
        Scope::Personal,
        "an unrecognized override token must fail closed, never open"
    );
}

// ---------------------------------------------------------------------------------------------
// P3 through the REAL gate: real `git` fixtures, the real resolver, the real classifier. Each row
// names the deletion that breaks it, so a future reader can check the test still bites.
// ---------------------------------------------------------------------------------------------

/// Classify a real checkout with an explicit root list and explicit routing facts.
///
/// Separate from [`classify_at_with_facts`] because these rows are ABOUT which roots are configured:
/// folding the roots into the fixture helper would make the parameter under test invisible.
fn classify_under(m: &Matrix, cwd: &Path, roots: &[PathBuf], facts: &RoutingFacts<'_>) -> Decision {
    let outcome = common::repo::detect_with_blocked_roots(cwd, &m.blocked());
    let source = outcome.resolved_slug().map(|_| RepoSource::GitOrigin);
    classify_with_evidence(
        Some(cwd),
        outcome.resolved_slug(),
        source,
        &BTreeMap::new(),
        0,
        &Anchors::new(roots),
        facts,
    )
}

fn present() -> RoutingFacts<'static> {
    RoutingFacts {
        evidence_present: true,
        ..Default::default()
    }
}

/// **P3, the defect.** `<root>/<repo>` -- a flat clone with no org level -- carrying a WORK origin
/// classifies Work via `GitOrigin`, not Personal via `CwdAnchor`.
///
/// The old rule matched the literal component `repos`, found no work org after it, and returned
/// `Decision { scope: Personal, basis: CwdAnchor, settled: true }`. Settled excluded the row from
/// `enrich_candidates` on every disjunct, so the remote sitting right there was never consulted and
/// would not be until the next `SCOPE_VERSION` bump.
///
/// BITES: restore the literal-`repos` anchor and both the scope AND the basis flip.
#[test]
fn a_flat_repo_under_the_root_reaches_the_remote_instead_of_settling_personal() {
    let m = Matrix::build();
    let d = classify_under(&m, &m.flat_under_root, &[m.repo_root()], &present());
    assert_eq!(d.scope, Scope::Work);
    assert_eq!(
        d.basis,
        Basis::GitOrigin,
        "the REMOTE must be what decided; a Work by CwdAnchor would mean the path guessed right"
    );
}

/// The org DIRECTORY, through the real gate. 21 measured sessions at `~/repos/tatari-tv`, and the
/// row a naive "a work org needs a following component" rule silently demotes.
///
/// Its probe is a genuine `NotARepo`: there is no `.git` marker at or above it in the fixture.
///
/// BITES: delete the bare-work-org branch and this flips to Personal, taking the 21 with it.
#[test]
fn the_work_org_directory_stays_work_through_the_real_gate() {
    let m = Matrix::build();
    let org_dir = m.repo_root().join("tatari-tv");
    let probe = common::repo::detect_with_blocked_roots(&org_dir, &m.blocked());
    assert_eq!(
        probe,
        common::repo::ProbeOutcome::NotARepo,
        "precondition: a plain directory"
    );

    let d = classify_under(
        &m,
        &org_dir,
        &[m.repo_root()],
        &RoutingFacts {
            repo_probe: Some(RecordedProbe::Negative(&probe)),
            evidence_present: true,
            ..Default::default()
        },
    );
    assert_eq!(d.scope, Scope::Work);
    assert_eq!(d.basis, Basis::CwdAnchor);
}

/// **The leak Gemini found in round 2.** A flat repo literally NAMED `tatari-tv` under a configured
/// root, with a PERSONAL origin. Granting Work to any bare work-org component sends this to the work
/// account on a directory-name coincidence.
///
/// BITES: widen the anchor from `NotARepo` to "any bare work-org component" and this reads Work.
#[test]
fn a_flat_repo_named_after_the_work_org_is_personal_by_its_remote() {
    let m = Matrix::build();
    let probe = common::repo::detect_with_blocked_roots(&m.work_org_named_repo, &m.blocked());
    assert_eq!(
        probe.resolved_slug(),
        Some("scottidler/tatari-tv"),
        "precondition: a real checkout with a personal origin"
    );

    let d = classify_under(
        &m,
        &m.work_org_named_repo,
        &[m.alt_root()],
        &RoutingFacts {
            repo_probe: Some(RecordedProbe::Negative(&probe)),
            evidence_present: true,
            ..Default::default()
        },
    );
    assert_eq!(d.scope, Scope::Personal);
    assert_eq!(d.basis, Basis::GitOrigin, "the remote decided, not the directory name");
}

/// **The hole found in round 3, by walking the shape's occupants rather than by the panel.** An
/// EMPTY repo named `tatari-tv` with no origin resolves no slug AND is not a plain directory, so a
/// rule keyed on "no slug resolved" hands it Work.
///
/// BITES: key the anchor on `repo.is_none()` instead of `ProbeOutcome::NotARepo` and this flips to
/// Work, shipping a personal repository's content to the work account.
#[test]
fn an_empty_repo_named_after_the_work_org_is_not_an_org_dir() {
    let m = Matrix::build();
    let probe = common::repo::detect_with_blocked_roots(&m.empty_repo_named_work_org, &m.blocked());
    assert_eq!(
        probe,
        common::repo::ProbeOutcome::NoOrigin,
        "precondition: conclusive, and NOT a plain directory"
    );
    assert_eq!(
        probe.resolved_slug(),
        None,
        "and it resolves no slug, which is the trap"
    );

    let d = classify_under(
        &m,
        &m.empty_repo_named_work_org,
        &[m.alt_root2()],
        &RoutingFacts {
            repo_probe: Some(RecordedProbe::Negative(&probe)),
            evidence_present: true,
            ..Default::default()
        },
    );
    assert_eq!(d.scope, Scope::Personal);
}

/// **The guard paths the first draft failed to assert (Codex).** The anchor change makes MORE
/// sessions reach the `git-origin` branch, so the two guards that branch leans on need falsifying
/// tests at the newly-reachable shape -- otherwise a regression in either is invisible.
///
/// BITES: delete the host check and row 1 reads Work; delete the probe check and row 2 reads Work.
#[test]
fn the_git_origin_guards_still_refuse_at_the_newly_reachable_flat_shape() {
    let m = Matrix::build();

    // A work slug from a host that is not allowlisted.
    let d = classify_under(
        &m,
        &m.flat_under_root_bad_host,
        &[m.repo_root()],
        &RoutingFacts {
            host_confers_work: Some(false),
            evidence_present: true,
            ..Default::default()
        },
    );
    assert_eq!(d.scope, Scope::Personal);
    assert_eq!(d.basis, Basis::HostRefused);

    // A work slug preceded by a CONCLUSIVE negative probe: the remote appeared after the session ran.
    let negative = common::repo::ProbeOutcome::NoOrigin;
    let d = classify_under(
        &m,
        &m.flat_under_root,
        &[m.repo_root()],
        &RoutingFacts {
            repo_probe: Some(RecordedProbe::Negative(&negative)),
            evidence_present: true,
            ..Default::default()
        },
    );
    assert_eq!(d.scope, Scope::Personal);
    assert_eq!(d.basis, Basis::ProbeRefused);
}

/// The anchor's own rows, asserted together because they are one rule read at four shapes and
/// splitting them hides that.
///
/// BITES: restore the literal-`repos` match and rows 1 and 2 flip.
#[test]
fn the_anchor_table_holds_at_every_shape_the_path_can_answer() {
    let a = Anchors::new(&[PathBuf::from("/home/saidler/repos")]);
    let at = |p: &str| a.scope_of(&PathBuf::from(p), None);

    // The inner-`repos` bug, one level down from the org slot. Reads Work today.
    assert_eq!(
        at("/home/saidler/repos/scottidler/repos/tatari-tv/x"),
        Some(Scope::Personal)
    );
    // The disclosed NARROWING: a `repos/<work-org>` adjacency outside every configured root.
    assert_eq!(at("/elsewhere/repos/tatari-tv/x"), None);
    // Unchanged: a personal repo NAMED after the work org, in the repo slot.
    assert_eq!(at("/home/saidler/repos/scottidler/tatari-tv"), Some(Scope::Personal));
    // Unchanged: the ordinary work checkout.
    assert_eq!(at("/home/saidler/repos/tatari-tv/clyde"), Some(Scope::Work));
}

// ---------------------------------------------------------------------------------------------
// Implementation-audit findings (Codex, 2026-08-01). Both are leak-direction regressions this
// branch introduced by making the probe signal TYPED without keeping presence as its own signal.
// ---------------------------------------------------------------------------------------------

/// **An UNREADABLE `repo_probe` still refuses a git-origin work slug.**
///
/// v3 keyed this branch on the column being non-NULL, so any stored value refused. The first version
/// of v4 keyed it on the PARSED outcome, so a stamp this binary could not read collapsed to "nothing
/// recorded" and the slug was granted Work -- a leak introduced by the very change meant to make the
/// signal more precise. The column is written only for a conclusive negative, so its presence is the
/// evidence; reading it only ever says WHICH negative.
///
/// BITES: key the git-origin guard on `facts.repo_probe.and_then(RecordedProbe::outcome)` and this
/// returns Work.
#[test]
fn an_unreadable_probe_stamp_still_refuses_a_work_slug() {
    let m = Matrix::build();
    let d = classify_under(
        &m,
        &m.flat_under_root,
        &[m.repo_root()],
        &RoutingFacts {
            repo_probe: Some(RecordedProbe::Unreadable),
            evidence_present: true,
            ..Default::default()
        },
    );
    assert_eq!(
        d.scope,
        Scope::Personal,
        "a recorded negative we cannot parse is still a recorded negative"
    );
    assert_eq!(d.basis, Basis::ProbeRefused);
}

/// **An UNREADABLE `repo_probe` does not anchor a bare `<root>/<work-org>` as Work either.**
///
/// The mirror of the case above, one branch up. `NotARepo` is the ONLY thing that means "this is the
/// org directory", and a string we just admitted we cannot parse is not that.
///
/// BITES: treat `Unreadable` as `NotARepo` (or drop the `outcome()` guard) and this returns Work.
#[test]
fn an_unreadable_probe_stamp_does_not_anchor_the_bare_work_org() {
    let a = Anchors::new(&[PathBuf::from("/home/saidler/repos")]);
    assert_eq!(
        a.scope_of(
            &PathBuf::from("/home/saidler/repos/tatari-tv"),
            Some(RecordedProbe::Unreadable)
        ),
        None,
        "unreadable defers, exactly like every outcome that is not NotARepo"
    );
}

/// **`RecordedProbe::of` joins PRESENCE and CONTENT, and presence comes from the RAW column.**
///
/// The pairing used to live at `sessions::routing::classify_row` as two locals combined under a
/// comment, so nothing stopped a caller reporting `Unreadable` for a readable stamp or -- the leak
/// the audit found -- collapsing a present-but-unparseable column to `None`, telling the classifier
/// nothing was recorded and letting the git-origin branch grant Work on a string it could not read.
///
/// All four combinations are pinned. The third row is the leak; the fourth cannot occur from the one
/// caller (a parse only succeeds when there is a column to parse) and is asserted anyway, because the
/// constructor's contract is that RAW alone decides presence.
///
/// BITES: key presence on `parsed` instead of `raw` and rows 2 and 3 both change.
#[test]
fn recorded_probe_of_takes_presence_from_the_column_and_content_from_the_parse() {
    let not_a_repo = ProbeOutcome::NotARepo;

    // No column: nothing was recorded, so there is nothing to refuse.
    assert_eq!(RecordedProbe::of(None, None), None);

    // A column that parsed: the outcome is available to the bare-work-org anchor.
    assert_eq!(
        RecordedProbe::of(Some("not-a-repo@2026-08-01T12:00:00+00:00"), Some(&not_a_repo)),
        Some(RecordedProbe::Negative(&not_a_repo)),
    );

    // A column that did NOT parse: still a RECORDED negative, so `Unreadable`, never `None`.
    assert_eq!(
        RecordedProbe::of(Some("garbage"), None),
        Some(RecordedProbe::Unreadable)
    );

    // Raw decides presence even if a parsed value is somehow handed in without one.
    assert_eq!(RecordedProbe::of(None, Some(&not_a_repo)), None);
}

/// **The doc's `~/code/repos/tatari-tv/x` row**, which both audit seats found unasserted. A literal
/// `repos` component sitting INSIDE a configured off-layout root is an ordinary org slot named
/// `repos`, not an anchor: the org is `repos`, which is not a work org, so it settles Personal.
///
/// This is the narrowing half of the same fix that kills the inner-`repos` bug one row below it.
///
/// BITES: restore the literal-`repos` match and this reads Work.
#[test]
fn a_repos_component_inside_an_off_layout_root_is_an_ordinary_org_slot() {
    let off = Anchors::new(&[PathBuf::from("/home/stephen/code")]);
    assert_eq!(
        off.scope_of(&PathBuf::from("/home/stephen/code/repos/tatari-tv/x"), None),
        Some(Scope::Personal),
        "the org slot is the literal directory `repos`, and `repos` is not a work org"
    );
}

/// **The class-killing test, and the reason it exists.**
///
/// The regression above (an unreadable stamp granting Work) shipped through a full `otto ci` because
/// every existing probe test fed a WELL-FORMED stamp. The change preserved behavior for well-formed
/// input and altered it only for malformed input, so the suite was blind to it BY CONSTRUCTION: no
/// test named the input class that changed. Mutation testing does not close this either -- it asks
/// "does a test observe this code", not "does a test observe this INPUT".
///
/// So the column's states are enumerated here and each one's DIRECTION is pinned. A new state (or a
/// new reading of an existing one) has to come through this table, which is what makes the bug class
/// unable to recur rather than merely fixed once.
///
/// The invariant, in one line: **only a NULL column may leave a git-origin work slug at Work.**
///
/// BITES: any change that lets a non-NULL column reach Work flips a row here.
#[test]
fn every_state_of_the_repo_probe_column_pins_its_direction() {
    let m = Matrix::build();
    let no_origin = common::repo::ProbeOutcome::NoOrigin;
    let not_a_repo = common::repo::ProbeOutcome::NotARepo;

    // (what the column holds, what the git-origin branch must do with a WORK slug)
    let rows: &[(&str, Option<RecordedProbe<'_>>, Scope)] = &[
        ("NULL: nothing was ever recorded", None, Scope::Work),
        (
            "a readable no-origin negative",
            Some(RecordedProbe::Negative(&no_origin)),
            Scope::Personal,
        ),
        (
            "a readable not-a-repo negative",
            Some(RecordedProbe::Negative(&not_a_repo)),
            Scope::Personal,
        ),
        (
            "a value this binary cannot read",
            Some(RecordedProbe::Unreadable),
            Scope::Personal,
        ),
    ];

    for (name, probe, want) in rows {
        let d = classify_under(
            &m,
            &m.flat_under_root,
            &[m.repo_root()],
            &RoutingFacts {
                repo_probe: *probe,
                evidence_present: true,
                ..Default::default()
            },
        );
        assert_eq!(
            d.scope,
            *want,
            "repo_probe state {name:?} must classify {:?}",
            want.as_str()
        );
    }

    // And the same totality at the bare `<root>/<work-org>` anchor, where the question is narrower:
    // only a READABLE NotARepo may grant Work, because only it means "a plain directory".
    let bare = m.repo_root().join("tatari-tv");
    let anchors = Anchors::new(&[m.repo_root()]);
    assert_eq!(
        anchors.scope_of(&bare, Some(RecordedProbe::Negative(&not_a_repo))),
        Some(Scope::Work)
    );
    for (name, probe) in [
        (
            "a readable no-origin negative",
            Some(RecordedProbe::Negative(&no_origin)),
        ),
        ("a value this binary cannot read", Some(RecordedProbe::Unreadable)),
        ("NULL", None),
    ] {
        assert_eq!(
            anchors.scope_of(&bare, probe),
            None,
            "only a readable not-a-repo anchors the bare work org; {name} must defer"
        );
    }
}

/// `Scope::from_stored` is the export contract's reader for the persisted `scope` column, and both
/// of its arms survived mutation: no test drove the function directly.
///
/// It matters that this is pinned HERE rather than only end-to-end through export: a mutant is only
/// checked by the tests that run for its own crate, which is the lesson the v0.23.0 mutation pass
/// already recorded for `scope_override`.
///
/// BITES: delete either arm and its row fails; the `None` rows guard the frozen vocabulary.
#[test]
fn from_stored_reads_exactly_the_frozen_vocabulary() {
    assert_eq!(Scope::from_stored("work"), Some(Scope::Work));
    assert_eq!(Scope::from_stored("personal"), Some(Scope::Personal));
    // Anything else is a hand-edited or forward-dated catalog. `None` is not a verdict: the export
    // contract turns it into a LOUD error and the classifier's override step fails closed to
    // personal, which are different obligations and deliberately so.
    for bad in ["Work", "PERSONAL", "", " work", "wor", "unknown"] {
        assert_eq!(
            Scope::from_stored(bad),
            None,
            "{bad:?} is outside the frozen work|personal vocabulary"
        );
    }
}

/// `RecordedProbe::token` is log-only, and both of its string mutants survived. It is asserted
/// rather than skipped because it is what an operator reads when a work slug was refused, and a
/// token that says the wrong thing sends them to the wrong remedy -- the exact failure the orphaned
/// worktree warning was fixed for.
///
/// BITES: replace either arm's string and its row fails.
#[test]
fn recorded_probe_token_names_what_was_recorded() {
    assert_eq!(RecordedProbe::Negative(&ProbeOutcome::NoOrigin).token(), "no-origin");
    assert_eq!(RecordedProbe::Negative(&ProbeOutcome::NotARepo).token(), "not-a-repo");
    assert_eq!(
        RecordedProbe::Unreadable.token(),
        "unreadable",
        "an operator must be able to tell a stamp we could not parse from one we could"
    );
    // The readable variants report the OUTCOME's own token, so the two spellings cannot drift.
    assert_eq!(
        RecordedProbe::Negative(&ProbeOutcome::NotARepo).token(),
        ProbeOutcome::NotARepo.as_str()
    );
}

/// The DEEPEST matching root decides the org slot, which is what makes the symlink expansion safe:
/// a configured spelling can sit under another root's real path, and there the longer one names the
/// org.
///
/// BITES: take the shallowest instead and this reads `link` as the org slot, yielding Personal for a
/// work checkout.
#[test]
fn the_anchor_takes_the_deepest_matching_root() {
    let nested = Anchors::new(&[PathBuf::from("/a"), PathBuf::from("/a/link")]);
    assert_eq!(
        nested.scope_of(&PathBuf::from("/a/link/tatari-tv/clyde"), None),
        Some(Scope::Work),
        "under `/a` the org slot would read `link`; under `/a/link` it reads `tatari-tv`"
    );
    // Order in the list must not matter, only depth.
    let reversed = Anchors::new(&[PathBuf::from("/a/link"), PathBuf::from("/a")]);
    assert_eq!(
        reversed.scope_of(&PathBuf::from("/a/link/tatari-tv/clyde"), None),
        Some(Scope::Work)
    );
}
