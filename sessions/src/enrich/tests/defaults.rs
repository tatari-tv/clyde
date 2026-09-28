//! The fail-closed default: `EnrichOptions::default()` carries the EMPTY scope policy.

use super::*;

/// `EnrichOptions::default()` is the EMPTY policy, so it never yields Work: not by the cwd anchor,
/// not by a work remote, not by a unanimous, fully-accounted touch set. Fail closed.
///
/// BITES: default `scope_policy` to anything carrying `tatari-tv/*` and this reads Work.
#[test]
fn default_options_never_classify_work() {
    let opts = EnrichOptions::default();
    assert_eq!(opts.scope_policy, session::ScopePolicy::default());
    let touched: std::collections::BTreeMap<String, u64> = [("tatari-tv/x".to_string(), 1)].into();
    let d = session::classify_with_evidence(
        Some(std::path::Path::new("/home/saidler/repos/tatari-tv/x")),
        Some("tatari-tv/x"),
        Some(common::repo::RepoSource::GitOrigin),
        &touched,
        1,
        &opts.scope_policy,
        &session::RoutingFacts {
            evidence_present: true,
            ..Default::default()
        },
    );
    assert_eq!(d.scope, session::Scope::Personal);
    let d = session::classify_with_evidence(
        Some(std::path::Path::new("/tmp/x")),
        None,
        None,
        &touched,
        1,
        &opts.scope_policy,
        &session::RoutingFacts {
            evidence_present: true,
            ..Default::default()
        },
    );
    assert_eq!(d.scope, session::Scope::Personal);
}
