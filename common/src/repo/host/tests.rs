#![allow(clippy::unwrap_used)]

use super::*;
use std::cell::RefCell;

/// A resolver with a fixed alias table, and a call counter so the memo can be asserted rather than
/// assumed.
struct Fake {
    table: HashMap<String, String>,
    calls: RefCell<usize>,
}

impl Fake {
    fn new(pairs: &[(&str, &str)]) -> Self {
        Self {
            table: pairs
                .iter()
                .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
                .collect(),
            calls: RefCell::new(0),
        }
    }
    fn calls(&self) -> usize {
        *self.calls.borrow()
    }
}

impl HostResolver for Fake {
    fn hostname(&self, host: &str) -> Option<String> {
        *self.calls.borrow_mut() += 1;
        self.table.get(host).cloned()
    }
}

/// A resolver that always fails, standing in for an absent `ssh`.
struct Absent;

impl HostResolver for Absent {
    fn hostname(&self, _: &str) -> Option<String> {
        None
    }
}

fn allowed() -> Vec<String> {
    DEFAULT_WORK_REMOTE_HOSTS.iter().map(|h| (*h).to_string()).collect()
}

#[test]
fn the_default_allowlist_is_exactly_github_dot_com() {
    assert_eq!(DEFAULT_WORK_REMOTE_HOSTS, &["github.com"]);
}

/// The allowlisted literal, which is the case that must cost nothing.
#[test]
fn an_allowlisted_host_confers_work_without_resolving_anything() {
    let fake = Fake::new(&[]);
    let mut policy = HostPolicy::with_resolver(&allowed(), fake);
    assert!(policy.confers_work("github.com"));
    assert_eq!(
        policy.resolver.calls(),
        0,
        "the common case must short-circuit before spawning a resolver"
    );
}

/// Problem 2's five crafted URLs, end to end: parse the remote, then ask the policy. Every one of
/// them conferred Work scope on v0.22.0, because the host was discarded at the parse.
///
/// BITES: drop the host check from the caller (or make `confers_work` return `true`) and all four
/// hostile rows pass.
#[test]
fn parse_slug_refuses_a_non_allowlisted_host() {
    let mut policy = HostPolicy::with_resolver(&allowed(), Absent);
    let cases = [
        (
            "git@github.com:tatari-tv/philo.git",
            "github.com",
            "tatari-tv/philo",
            true,
        ),
        (
            "git@evil.example.com:tatari-tv/x.git",
            "evil.example.com",
            "tatari-tv/x",
            false,
        ),
        (
            "https://evil.example.com/tatari-tv/x",
            "evil.example.com",
            "tatari-tv/x",
            false,
        ),
        ("http://10.0.0.5:8080/tatari-tv/x", "10.0.0.5", "tatari-tv/x", false),
        (
            "ssh://git@gitea.local:2222/tatari-tv/x.git",
            "gitea.local",
            "tatari-tv/x",
            false,
        ),
    ];
    for (url, expected_host, expected_slug, expected_work) in cases {
        let parsed = super::super::parse_slug(url).unwrap_or_else(|| panic!("{url} must still parse"));
        assert_eq!(parsed.host, expected_host, "host for {url}");
        // The slug still parses, and still looks like a work org. ATTRIBUTION is unchanged; only the
        // SCOPE it may confer is gated. That distinction is the fix: a hostile remote can still say
        // which repo a session was in, it just cannot make the transcript shippable.
        assert_eq!(parsed.slug, expected_slug, "slug for {url}");
        assert_eq!(
            policy.confers_work(&parsed.host),
            expected_work,
            "{url} must {} confer work",
            if expected_work { "" } else { "NOT" }
        );
    }
}

/// Row 20, and the check that this fix did not reintroduce the 0%-coverage bug. An SSH `Host` alias
/// that resolves to an allowlisted host STILL confers work.
///
/// BITES: delete the `self.resolve(&host)` call and compare the literal only; this refuses, and
/// every developer using an alias drops back to 0% enrichment coverage.
#[test]
fn an_ssh_alias_resolving_to_an_allowlisted_host_still_confers_work() {
    let fake = Fake::new(&[("github-work", "github.com")]);
    let mut policy = HostPolicy::with_resolver(&allowed(), fake);

    let parsed = super::super::parse_slug("git@github-work:tatari-tv/x.git").expect("parses");
    assert_eq!(parsed.host, "github-work", "the parser reports the alias verbatim");
    assert!(
        policy.confers_work(&parsed.host),
        "an alias to github.com must confer work"
    );
}

/// An alias that resolves to something NOT on the list is still refused: resolution widens what can
/// be recognized, never what is trusted.
#[test]
fn an_ssh_alias_resolving_elsewhere_is_still_refused() {
    let fake = Fake::new(&[("sneaky", "evil.example.com")]);
    let mut policy = HostPolicy::with_resolver(&allowed(), fake);
    assert!(!policy.confers_work("sneaky"));
}

/// The `ssh`-absent case. Resolution failing must never OPEN the gate.
#[test]
fn an_absent_ssh_fails_closed() {
    let mut policy = HostPolicy::with_resolver(&allowed(), Absent);
    assert!(!policy.confers_work("github-work"));
    assert!(
        policy.confers_work("github.com"),
        "a literal match still works with no resolver at all, so an absent ssh costs coverage only \
         for alias users"
    );
}

/// Memoized per host, including the failure. A catalog with 2,000 sessions behind one alias must
/// spawn `ssh` once, not 2,000 times.
#[test]
fn resolution_is_memoized_per_host_including_failures() {
    let fake = Fake::new(&[("github-work", "github.com")]);
    let mut policy = HostPolicy::with_resolver(&allowed(), fake);
    for _ in 0..5 {
        assert!(policy.confers_work("github-work"));
        assert!(!policy.confers_work("never-resolves"));
    }
    assert_eq!(
        policy.resolver.calls(),
        2,
        "one resolution per distinct host, and the FAILURE is cached too"
    );
}

/// The allowlist and the parsed host are both lowercased, so case cannot smuggle a host past the
/// comparison.
#[test]
fn host_matching_is_case_insensitive_on_both_sides() {
    let mut policy = HostPolicy::with_resolver(&["GitHub.COM".to_string()], Absent);
    assert!(policy.confers_work("github.com"));
    assert!(policy.confers_work("GITHUB.COM"));
}

/// The production resolver actually runs and fails closed for a host nobody has aliased. Cheap, and
/// it is the only assertion here that proves the `ssh -G` spawn works at all; every other test uses
/// the fake, because a test cannot control the invoking user's real `~/.ssh/config`.
#[test]
fn the_real_ssh_resolver_runs_and_fails_closed_on_an_unknown_host() {
    let mut policy = HostPolicy::new(&allowed());
    assert!(
        !policy.confers_work("clyde-matrix-no-such-alias.invalid"),
        "an unaliased host must not confer work"
    );
}
