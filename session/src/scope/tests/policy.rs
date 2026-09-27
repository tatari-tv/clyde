//! v5: `reposlugs-ptns` through `ScopePolicy`. Every policy below names its patterns; the rows are
//! the design doc's Phase 2 success criteria (`docs/design/2026-09-27-reposlugs-ptns-from-config.md`).

#![allow(clippy::unwrap_used)]

use super::*;

const ROOT: &str = "/home/saidler/repos";

/// A policy over the single root `~/repos` and exactly the named patterns.
fn policy(raw: &[&str]) -> ScopePolicy {
    ScopePolicy::new(&[PathBuf::from(ROOT)], &ptns(raw))
}

/// Classify a stored row: `cwd`, an optional rule-1 git-origin slug, and a touch set.
///
/// Hand-built rather than resolver-driven, because these rows are about what the POLICY does with a
/// stored slug and a path, and the policy reads nothing else. The resolver-to-classifier path is
/// covered by the matrix tests above, and by `the_personal_dir_fixture_*` below.
fn decide(
    policy: &ScopePolicy,
    cwd: &str,
    remote: Option<&str>,
    pairs: &[(&str, u64)],
    files_edited: u64,
    facts: &RoutingFacts<'_>,
) -> Decision {
    classify_with_evidence(
        Some(Path::new(cwd)),
        remote,
        remote.map(|_| RepoSource::GitOrigin),
        &touched(pairs),
        files_edited,
        policy,
        facts,
    )
}

/// [`decide`] with present evidence and nothing else recorded.
fn decide_plain(policy: &ScopePolicy, cwd: &str, remote: Option<&str>) -> Decision {
    decide(policy, cwd, remote, &[], 0, &present())
}

fn work_by(basis: Basis) -> Decision {
    Decision {
        scope: Scope::Work,
        basis,
        settled: true,
    }
}

fn personal(basis: Basis, settled: bool) -> Decision {
    Decision {
        scope: Scope::Personal,
        basis,
        settled,
    }
}

/// The policy is the only thing that names work orgs: `otto-rs/*` makes `otto-rs` wide, and
/// `tatari-tv` is nothing special without a pattern.
///
/// BITES: restore a compiled-in `tatari-tv` lookup in `owner_rule` and row 2 reads Work.
#[test]
fn an_owner_wide_pattern_is_the_only_thing_that_names_a_work_org() {
    let p = policy(&["otto-rs/*"]);
    assert_eq!(
        decide_plain(&p, "/home/saidler/repos/otto-rs/x", None),
        work_by(Basis::CwdAnchor)
    );
    assert_eq!(
        decide_plain(&p, "/home/saidler/repos/tatari-tv/x", None),
        personal(Basis::CwdAnchor, true)
    );
}

/// An exclude on a WIDE owner takes Work away by the path slug and by the stored remote, and leaves
/// the fork-in-a-work-dir precedence alone.
///
/// BITES: drop the exclude check in `wide_anchor` and rows 2 and 3 read Work.
#[test]
fn an_exclude_vetoes_a_wide_owner_by_path_and_by_remote() {
    let p = policy(&["tatari-tv/*", "!tatari-tv/secret"]);
    assert_eq!(
        decide_plain(&p, "/home/saidler/repos/tatari-tv/x", None),
        work_by(Basis::CwdAnchor)
    );
    assert_eq!(
        decide_plain(&p, "/home/saidler/repos/tatari-tv/secret", None),
        personal(Basis::CwdAnchor, false)
    );
    assert_eq!(
        decide_plain(&p, "/home/saidler/repos/tatari-tv/x", Some("tatari-tv/secret")).scope,
        Scope::Personal
    );

    // The fork fixture, a real checkout: `<repo-root>/tatari-tv/clyde-fork`, origin
    // `scottidler/clyde-fork`. No exclude names it, so the wide anchor still places it.
    let m = Matrix::build();
    let d = classify_real(
        &m,
        &m.fork_in_work_dir,
        &ScopePolicy::new(&[m.repo_root()], &ptns(&["tatari-tv/*", "!tatari-tv/secret"])),
        &present(),
    );
    assert_eq!(d, work_by(Basis::CwdAnchor));
}

/// A NAMED owner defers to the remote, and only the remote: every layout of the same repo reaches
/// Work by its slug, and nothing else does.
///
/// BITES: map `OwnerRule::Named` to `Settled(Personal)` and rows 1 to 5 read Personal; key it on
/// the directory name instead and rows 3 to 5 read Personal and row 6 reads Work.
#[test]
fn a_named_owner_defers_to_its_git_origin_slug() {
    let p = policy(&["scottidler/claude"]);
    let slug = Some("scottidler/claude");
    for cwd in [
        "/home/saidler/repos/scottidler/claude/x",
        "/home/saidler/repos/scottidler/claude/.claude/worktrees/f2",
        "/home/saidler/repos/scottidler/claude-feature",
        "/home/saidler/repos/scottidler/claude.git",
        "/home/saidler/repos/scottidler/claude-main",
    ] {
        assert_eq!(decide_plain(&p, cwd, slug), work_by(Basis::GitOrigin), "{cwd}");
    }
    assert_eq!(
        decide_plain(
            &p,
            "/home/saidler/repos/scottidler/claude",
            Some("scottidler/claude-fork")
        ),
        personal(Basis::GitOrigin, false)
    );
    assert_eq!(
        decide_plain(&p, "/home/saidler/repos/scottidler/claude", None),
        personal(Basis::CwdAnchor, false)
    );
    assert_eq!(
        decide_plain(&p, "/home/saidler/repos/milwaukie-youth-football/x", None),
        personal(Basis::CwdAnchor, true)
    );
}

/// A named owner NEVER reaches Work through the touch set: a `second-brain` session that edited a
/// `scottidler/claude` file is not a `scottidler/claude` session.
///
/// BITES: let `Deferred` fall through to the touch set and row 3 reads Work.
#[test]
fn a_named_owner_never_reaches_the_touch_set() {
    let p = policy(&["scottidler/claude"]);
    let cwd = "/home/saidler/repos/scottidler/second-brain";
    let check = |name: &str, pairs: &[(&str, u64)], files: u64, facts: &RoutingFacts<'_>| {
        let d = decide(&p, cwd, None, pairs, files, facts);
        assert_eq!(d.scope, Scope::Personal, "{name}");
        assert!(!d.settled, "{name}: provisional");
        assert_ne!(d.basis, Basis::TouchSet, "{name}");
    };
    check("no evidence", &[], 0, &RoutingFacts::default());
    check("empty repos_touched", &[], 0, &present());
    check("one claude edit", &[("scottidler/claude", 1)], 1, &present());
    check(
        "claude plus eratosthenes",
        &[("scottidler/claude", 1), ("scottidler/eratosthenes", 1)],
        2,
        &present(),
    );
}

/// Unanchored cwds read the policy on both evidence paths, and an exclude applies on both.
///
/// BITES: restore a compiled-in `tatari-tv` lookup in `ScopePolicy::matches` and rows 1 and 2 read
/// Personal; drop the exclude half of `matches` and rows 4 and 5 read Work.
#[test]
fn unanchored_cwds_read_the_policy_on_the_remote_and_the_touch_set() {
    let p = policy(&["tatari-tv/*", "scottidler/dotfiles"]);
    assert_eq!(
        decide_plain(&p, "/tmp/x", Some("scottidler/dotfiles")),
        work_by(Basis::GitOrigin)
    );
    let mixed = [("scottidler/dotfiles", 2), ("tatari-tv/x", 1)];
    assert_eq!(decide(&p, "/tmp/x", None, &mixed, 3, &present()).scope, Scope::Work);
    let unlisted = [("scottidler/dotfiles", 1), ("scottidler/eratosthenes", 1)];
    assert_eq!(
        decide(&p, "/tmp/x", None, &unlisted, 2, &present()).scope,
        Scope::Personal
    );

    let p = policy(&["tatari-tv/*", "scottidler/dotfiles", "!scottidler/dotfiles"]);
    assert_eq!(
        decide_plain(&p, "/tmp/x", Some("scottidler/dotfiles")).scope,
        Scope::Personal
    );
    assert_eq!(decide(&p, "/tmp/x", None, &mixed, 3, &present()).scope, Scope::Personal);
}

/// The synthetic `<repo-root>/scottidler/philo` with remote `tatari-tv/philo`, under a policy that
/// makes `scottidler` NAMED. The slug matches `tatari-tv/*`, but its owner is not the cwd's, so the
/// deferred arm refuses it, and the disclosure still reports personal-vs-work.
///
/// BITES: drop the owner comparison in `deferred_slug_is_work` and this reads Work.
#[test]
fn the_personal_dir_fixture_with_a_work_remote_stays_personal_under_a_named_owner() {
    let m = Matrix::build();
    let p = ScopePolicy::new(&[m.repo_root()], &ptns(&["tatari-tv/*", "scottidler/claude"]));
    let d = classify_real(&m, &m.work_remote_in_personal_dir, &p, &present());
    assert_eq!(d.scope, Scope::Personal);
    assert!(!d.settled);
    assert_eq!(
        anchor_disagrees_with_remote(&m.work_remote_in_personal_dir, "tatari-tv/philo", &p),
        Some(Disagreement {
            anchor: Scope::Personal,
            remote: Scope::Work
        })
    );
    // The listed sibling worktree agrees with its remote.
    assert_eq!(
        anchor_disagrees_with_remote(
            &m.repo_root().join("scottidler").join("claude-feature"),
            "scottidler/claude",
            &p
        ),
        None
    );
}

/// A resolver that resolves nothing: an ssh alias with no `Host` entry.
struct UnresolvedAlias;

impl common::repo::host::HostResolver for UnresolvedAlias {
    fn hostname(&self, _: &str) -> Option<String> {
        None
    }
}

/// The git-origin guards at the DEFERRED shape, which is newly reachable for named owners.
///
/// BITES: route `Deferred` around `git_origin_decision` and rows 1 to 4 read Work.
#[test]
fn the_git_origin_guards_still_refuse_at_the_deferred_shape() {
    let p = policy(&["scottidler/claude"]);
    let cwd = "/home/saidler/repos/scottidler/claude-feature";
    let slug = Some("scottidler/claude");
    let at = |facts: RoutingFacts<'_>| {
        decide(
            &p,
            cwd,
            slug,
            &[],
            0,
            &RoutingFacts {
                evidence_present: true,
                ..facts
            },
        )
    };

    let untrusted = at(RoutingFacts {
        host_confers_work: Some(false),
        ..Default::default()
    });
    assert_eq!(untrusted, personal(Basis::HostRefused, false));

    let mut hosts = common::repo::host::HostPolicy::with_resolver(&["github.com".to_string()], UnresolvedAlias);
    let alias = at(RoutingFacts {
        host_confers_work: Some(hosts.confers_work("github-work")),
        ..Default::default()
    });
    assert_eq!(
        alias,
        personal(Basis::HostRefused, false),
        "an unresolved alias refuses"
    );

    let negative = ProbeOutcome::NoOrigin;
    let refused = at(RoutingFacts {
        repo_probe: Some(RecordedProbe::Negative(&negative)),
        ..Default::default()
    });
    assert_eq!(refused, personal(Basis::ProbeRefused, false));
    let unreadable = at(RoutingFacts {
        repo_probe: Some(RecordedProbe::Unreadable),
        ..Default::default()
    });
    assert_eq!(unreadable, personal(Basis::ProbeRefused, false));

    // A NULL host (every pre-v13 row) keeps its inherited trust: the strip-only rule, stated.
    assert_eq!(at(RoutingFacts::default()), work_by(Basis::GitOrigin));
}

/// Case is ignored per extracted segment on every path, and nothing lowercases a whole path or
/// re-keys the touch set.
///
/// BITES: compare with `==` instead of `eq_ignore_ascii_case` in `ptn_matches` and every row reads
/// Personal.
#[test]
fn mixed_case_matches_on_every_path() {
    let raw = policy(&["Scottidler/Claude"]);
    assert_eq!(
        raw,
        policy(&["scottidler/claude"]),
        "the pattern is lowercased on parse"
    );
    assert_eq!(
        decide_plain(&raw, "/home/saidler/repos/scottidler/claude", Some("scottidler/CLAUDE")),
        work_by(Basis::GitOrigin)
    );
    assert_eq!(
        decide_plain(&raw, "/tmp/x", Some("scottidler/CLAUDE")),
        work_by(Basis::GitOrigin)
    );
    assert_eq!(
        decide_plain(&policy(&["otto-rs/*"]), "/home/saidler/repos/Otto-RS/x", None),
        work_by(Basis::CwdAnchor)
    );
    assert_eq!(
        decide(
            &policy(&["otto-rs/otto"]),
            "/tmp/x",
            None,
            &[("Otto-RS/otto", 1)],
            1,
            &present()
        )
        .scope,
        Scope::Work
    );
}

/// v5 is the logic change this phase makes; policy changes after it ride the fingerprint.
#[test]
fn scope_version_is_five() {
    assert_eq!(SCOPE_VERSION, 5);
}

/// `Excluded` is TERMINAL: the touch set cannot reclaim it, and an owner-wide exclude takes the
/// bare org dir too.
///
/// BITES: make `Excluded` fall through like `Unanchored` and row 1 reads Work by `TouchSet`; drop
/// the `!<owner>/*` check on the bare shape and row 2 reads Work by the org-dir probe rule.
#[test]
fn an_exclude_is_terminal() {
    let p = policy(&["tatari-tv/*", "!tatari-tv/secret"]);
    let d = decide(
        &p,
        "/home/saidler/repos/tatari-tv/secret",
        None,
        &[("tatari-tv/x", 1)],
        1,
        &present(),
    );
    assert_eq!(d, personal(Basis::CwdAnchor, false));

    let p = policy(&["tatari-tv/*", "!tatari-tv/*"]);
    let facts = RoutingFacts {
        repo_probe: Some(RecordedProbe::Negative(&ProbeOutcome::NotARepo)),
        evidence_present: true,
        ..Default::default()
    };
    let bare = decide(&p, "/home/saidler/repos/tatari-tv", None, &[], 0, &facts);
    assert_eq!(bare.scope, Scope::Personal);
    assert_eq!(
        p.scope_of(Path::new("/home/saidler/repos/tatari-tv"), None, &facts),
        Anchor::Excluded
    );
}

/// The combination rules: exclude wins over a named include, wide wins over named, excludes are
/// case-insensitive, and a stranded exclude survives normalization.
///
/// BITES: drop excludes with no matching include in `ScopePolicy::new` and the last row reads Work.
#[test]
fn includes_and_excludes_combine_as_documented() {
    let both = policy(&["scottidler/claude", "!scottidler/claude"]);
    assert_eq!(
        decide_plain(
            &both,
            "/home/saidler/repos/scottidler/claude",
            Some("scottidler/claude")
        )
        .scope,
        Scope::Personal
    );
    assert!(!both.matches("scottidler/claude"));

    let wide_and_named = policy(&["tatari-tv/*", "tatari-tv/x"]);
    assert_eq!(wide_and_named.owner_rule("tatari-tv"), OwnerRule::Wide);
    assert_eq!(wide_and_named.owner_rule("TATARI-TV"), OwnerRule::Wide);
    assert_eq!(policy(&["tatari-tv/x"]).owner_rule("tatari-tv"), OwnerRule::Named);
    assert_eq!(
        policy(&["!tatari-tv/*"]).owner_rule("tatari-tv"),
        OwnerRule::Unlisted,
        "an exclude never makes an owner wide or named"
    );

    let mixed = policy(&["tatari-tv/*", "!Tatari-TV/Secret"]);
    assert!(!mixed.matches("tatari-tv/secret"));
    assert_eq!(
        decide_plain(&mixed, "/home/saidler/repos/tatari-tv/secret", None).scope,
        Scope::Personal
    );

    let stranded = policy(&["tatari-tv/*", "!scottidler/private"]);
    assert!(stranded.fingerprint().contains("!scottidler/private"));
    assert_eq!(
        decide_plain(
            &stranded,
            "/home/saidler/repos/tatari-tv/fork",
            Some("scottidler/private")
        ),
        personal(Basis::CwdAnchor, false)
    );
}

/// The touch-set check runs over every ORIGINAL key: a mixed-case excluded key still vetoes, and two
/// case variants of one repo are two keys, each checked.
///
/// BITES: lowercase and merge the map before the check and the exclude reads against a key it was
/// never written as; skip excluded keys and row 1 reads Work.
#[test]
fn the_touch_set_checks_every_original_key() {
    let p = policy(&["tatari-tv/*", "!tatari-tv/secret"]);
    let excluded = [("tatari-tv/x", 1), ("Tatari-TV/Secret", 1)];
    assert_eq!(
        decide(&p, "/tmp/x", None, &excluded, 2, &present()).scope,
        Scope::Personal
    );
    let variants = [("Tatari-TV/X", 1), ("tatari-tv/x", 1)];
    assert_eq!(decide(&p, "/tmp/x", None, &variants, 2, &present()).scope, Scope::Work);
}

/// `matches` is any-include AND no-exclude, and every malformed shape fails closed.
#[test]
fn matches_is_any_include_and_no_exclude() {
    let p = policy(&["tatari-tv/*", "otto-rs/otto", "!tatari-tv/secret"]);
    assert!(p.matches("tatari-tv/philo"));
    assert!(p.matches("otto-rs/otto"));
    assert!(!p.matches("otto-rs/other"));
    assert!(!p.matches("tatari-tv/secret"));
    assert!(!p.matches("scottidler/claude"));
    for bad in ["", "tatari-tv", "tatari-tv/", "/x", "tatari-tv/a/b"] {
        assert!(!p.matches(bad), "{bad:?}");
    }
    assert!(
        !ScopePolicy::default().matches("tatari-tv/philo"),
        "empty policy confers nothing"
    );
}

/// The fingerprint is the whole normalized policy: order, case and duplicates do not change it, and
/// any distinct entry does.
#[test]
fn the_fingerprint_is_the_normalized_policy() {
    let roots = [PathBuf::from("/b"), PathBuf::from("/a"), PathBuf::from("/a")];
    let one = ScopePolicy::new(&roots, &ptns(&["tatari-tv/*", "!Tatari-TV/X", "otto-rs/otto"]));
    let two = ScopePolicy::new(
        &[PathBuf::from("/a"), PathBuf::from("/b")],
        &ptns(&["otto-rs/otto", "!tatari-tv/x", "TATARI-TV/*", "otto-rs/otto"]),
    );
    assert_eq!(one, two);
    assert_eq!(one.fingerprint(), two.fingerprint());
    assert_eq!(
        one.fingerprint(),
        r#"{"ptns":["!tatari-tv/x","otto-rs/otto","tatari-tv/*"],"roots":["/a","/b"]}"#
    );
    let narrower = ScopePolicy::new(&[PathBuf::from("/a"), PathBuf::from("/b")], &ptns(&["tatari-tv/*"]));
    assert_ne!(one.fingerprint(), narrower.fingerprint());
    assert_eq!(ScopePolicy::default().fingerprint(), r#"{"ptns":[],"roots":[]}"#);
}
