//! The work-remote host allowlist: which hosts a remote-derived slug may confer WORK scope from.
//!
//! Problem 2. [`super::parse_slug`] now returns the host, and this decides whether that host is
//! trusted.
//!
//! **Not a strict `github.com` string test, and that is the whole design.** SSH `Host` aliases are
//! ordinary config: a developer with
//!
//! ```text
//! Host github-work
//!     HostName github.com
//!     IdentityFile ~/.ssh/id_work
//! ```
//!
//! has remotes spelled `git@github-work:tatari-tv/x.git`, and a literal test would refuse every one
//! of them. That reintroduces the exact 0%-coverage bug v0.22.0 fixed, for everyone who juggles two
//! GitHub accounts. So the alias is RESOLVED rather than rejected.
//!
//! Fails CLOSED at every step: an unresolvable host, an absent `ssh`, or a host simply not on the
//! list yields a slug that can still ATTRIBUTE a repo but can never confer Work scope.

use std::collections::HashMap;
use std::process::Command;

use log::{debug, trace, warn};

/// The default allowlist when `work-remote-hosts` is unset.
///
/// Measured rather than assumed: all 59 `origin` remotes across `~/repos/tatari-tv/*` on desk.lan
/// resolve to `github.com`, and zero to anything else. A shop with an internal GitHub Enterprise
/// says so in config; it does not need a code change.
pub const DEFAULT_WORK_REMOTE_HOSTS: &[&str] = &["github.com"];

/// Turns a possible SSH `Host` alias into the real hostname it names.
///
/// A PORT, injected as a generic (never `dyn`), for the reason the house rules give and for one
/// specific to this module: the production implementation reads the invoking user's real
/// `~/.ssh/config`, which a test cannot control (see [`SshResolver`]). Without the seam,
/// [`HostPolicy`]'s logic could only be tested on hosts that happen to have no alias configured,
/// which is not a test of alias handling at all.
pub trait HostResolver {
    /// The real hostname `host` names, or `None` when it cannot be resolved.
    fn hostname(&self, host: &str) -> Option<String>;
}

/// The production resolver: `ssh -G <host>`, read the `hostname` line.
///
/// `ssh -G` prints the effective config WITHOUT connecting, so this touches no network.
///
/// **`HOME` is deliberately NOT forwarded, because ssh ignores it.** Measured 2026-07-31: run under
/// `env -i` with `HOME` pointed at a temp directory holding a `Host github-work` block, `ssh -G`
/// still reported `userknownhostsfile /home/saidler/.ssh/known_hosts` and did NOT apply the alias.
/// ssh resolves the user's home from the passwd database, not from `$HOME`. So forwarding it would
/// be cargo cult: it buys nothing, and it would wrongly imply to a reader that the operator's config
/// is reachable through the environment.
///
/// The useful half of that same measurement: because ssh uses the passwd home, alias resolution
/// WORKS under a scrubbed environment with `PATH` alone, which is what production needs.
#[derive(Debug, Default)]
pub struct SshResolver;

impl HostResolver for SshResolver {
    fn hostname(&self, host: &str) -> Option<String> {
        let out = Command::new("ssh")
            .args(["-G", host])
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .output()
            .map_err(|e| {
                warn!("host: could not run `ssh -G {host}`: {e}; falling back to the literal");
                e
            })
            .ok()?;
        if !out.status.success() {
            warn!(
                "host: `ssh -G {host}` exited {:?}; falling back to the literal",
                out.status.code()
            );
            return None;
        }
        // `ssh -G` emits one lowercase `key value` per line.
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .find_map(|line| line.strip_prefix("hostname "))
            .map(|h| h.trim().to_ascii_lowercase())
    }
}

/// Decides whether a remote host may confer Work scope, resolving SSH aliases and memoizing.
///
/// `&mut` on the query because resolution spawns a subprocess and caches the answer: one spawn per
/// distinct alias per process, and only for hosts that are not already a literal match.
#[derive(Debug)]
pub struct HostPolicy<R: HostResolver> {
    allowed: Vec<String>,
    resolver: R,
    /// host -> resolved hostname. Caches the FAILURE too (as the host itself), so an absent `ssh`
    /// costs one spawn for the whole run rather than one per session.
    resolved: HashMap<String, String>,
}

impl HostPolicy<SshResolver> {
    /// The production policy: the configured allowlist, resolving aliases through `ssh -G`.
    pub fn new(allowed: &[String]) -> Self {
        Self::with_resolver(allowed, SshResolver)
    }
}

impl<R: HostResolver> HostPolicy<R> {
    /// Build a policy over an explicit resolver. Entries are lowercased to match
    /// [`super::normalize_host`]'s output, so a `GitHub.com` in `clyde.yml` still matches.
    pub fn with_resolver(allowed: &[String], resolver: R) -> Self {
        Self {
            allowed: allowed.iter().map(|h| h.to_ascii_lowercase()).collect(),
            resolver,
            resolved: HashMap::new(),
        }
    }

    /// Whether `host` may confer Work scope.
    ///
    /// A literal match short-circuits, so the overwhelmingly common case (`github.com`) spawns
    /// nothing at all. Anything else is treated as a possible alias and resolved once.
    pub fn confers_work(&mut self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        if self.allowed.iter().any(|a| a == &host) {
            trace!("host::confers_work: {host} is allowlisted literally");
            return true;
        }
        let real = self.resolve(&host);
        let allowed = self.allowed.iter().any(|a| a == &real);
        debug!("host::confers_work: {host} resolves to {real}; allowed={allowed}");
        allowed
    }

    /// The real hostname behind a possible alias, memoized.
    ///
    /// Falls back to the literal when the resolver answers nothing. That IS the fail-closed
    /// direction on its own: the literal is what just failed the allowlist test, so falling back to
    /// it means the host still confers nothing.
    fn resolve(&mut self, host: &str) -> String {
        if let Some(cached) = self.resolved.get(host) {
            return cached.clone();
        }
        let real = self.resolver.hostname(host).unwrap_or_else(|| {
            trace!("host::resolve: no answer for {host}; using the literal");
            host.to_string()
        });
        self.resolved.insert(host.to_string(), real.clone());
        real
    }
}

#[cfg(test)]
mod tests;
