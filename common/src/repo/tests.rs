#![allow(clippy::unwrap_used)]

use super::*;
use crate::checkout::Matrix;
use std::process::Command;
use tempfile::TempDir;

/// Run one setup `git` command with a SCRUBBED environment.
///
/// The scrub is not cosmetic. Three tests in this file mutate `GIT_DIR` / `GIT_CONFIG_*` / `HOME`
/// process-wide to prove `run_git`'s allowlist works, and cargo runs the tests in one binary
/// CONCURRENTLY. Without this, a leaked `GIT_DIR` redirects a sibling test's `git init` at an
/// unrelated repository and the failure surfaces somewhere else entirely. `ENV_LOCK` cannot help:
/// it only serializes the tests that take it.
fn git_setup(dir: &Path, args: &[&str]) {
    let s = Command::new("git")
        .current_dir(dir)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .args(args)
        .status()
        .unwrap();
    assert!(s.success(), "git {args:?} failed in {}", dir.display());
}

fn git_init(dir: &Path) {
    git_setup(dir, &["init", "-q"]);
    git_setup(dir, &["config", "user.email", "test@example.com"]);
    git_setup(dir, &["config", "user.name", "test"]);
}

fn add_origin(dir: &Path, url: &str) {
    git_setup(dir, &["remote", "add", "origin", url]);
}

/// Set one key in the repo's OWN (`--local`) config. Used to plant an `insteadOf` rule in the one
/// scope no environment scrub can take away.
fn git_config(dir: &Path, key: &str, value: &str) {
    git_setup(dir, &["config", "--local", key, value]);
}

/// `(host, slug)` for a URL, or `None`. The two fields are asserted TOGETHER everywhere below,
/// because discarding the host while keeping the slug is precisely Problem 2.
fn split(url: &str) -> Option<(String, String)> {
    parse_slug(url).map(|r| (r.host, r.slug))
}

#[test]
fn parse_slug_ssh_form() {
    let want = Some(("github.com".to_string(), "tatari-tv/claude-report".to_string()));
    assert_eq!(split("git@github.com:tatari-tv/claude-report.git"), want);
    assert_eq!(split("git@github.com:tatari-tv/claude-report"), want);
}

#[test]
fn parse_slug_https_form() {
    let want = Some(("github.com".to_string(), "scottidler/obsidian".to_string()));
    assert_eq!(split("https://github.com/scottidler/obsidian.git"), want);
    assert_eq!(split("https://github.com/scottidler/obsidian"), want);
}

#[test]
fn parse_slug_git_protocol() {
    assert_eq!(
        split("git://github.com/foo/bar.git"),
        Some(("github.com".to_string(), "foo/bar".to_string()))
    );
}

#[test]
fn parse_slug_garbage_returns_none() {
    assert_eq!(parse_slug(""), None);
    assert_eq!(parse_slug("not-a-url"), None);
    assert_eq!(parse_slug("https://github.com/onlyorg"), None);
    // KILLS: `replace || with && in parse_slug`. The guard is `org.is_empty() || repo.is_empty()`,
    // so it only differs from `&&` when exactly ONE side is empty, and nothing covered that. Both
    // shapes are reachable from a real (if malformed) remote URL.
    assert_eq!(parse_slug("https://github.com//philo"), None, "an empty org");
    assert_eq!(parse_slug("https://github.com/tatari-tv/"), None, "an empty repo");
}

/// The host is normalized before it reaches the allowlist: any `user@` dropped, any `:port` dropped,
/// lowercased. Each of those is a way a host could otherwise miss a literal comparison.
#[test]
fn parse_slug_normalizes_the_host() {
    assert_eq!(
        split("ssh://git@GitHub.com:22/tatari-tv/philo.git"),
        Some(("github.com".to_string(), "tatari-tv/philo".to_string())),
        "user, port and case all normalized away"
    );
    assert_eq!(
        split("http://10.0.0.5:8080/tatari-tv/x"),
        Some(("10.0.0.5".to_string(), "tatari-tv/x".to_string())),
        "a bare IP with a port is a host like any other, and it is not on the allowlist"
    );
}

/// An IPv6 literal is DECLINED rather than mangled. clyde has never seen one in a git remote, and a
/// wrong answer here confers work scope, so the fail-closed direction is to refuse.
#[test]
fn parse_slug_declines_a_bracketed_ipv6_authority() {
    assert_eq!(parse_slug("ssh://git@[::1]:22/tatari-tv/x.git"), None);
}

/// A missing cwd is the archived-session case (matrix row 26). There is nothing to observe, so it
/// must be INDETERMINATE and record nothing: a restored checkout has to be able to recover the row.
#[test]
fn detect_is_indeterminate_for_a_missing_dir() {
    let r = detect_with_blocked_roots(Path::new("/nonexistent/cr-test/missing"), &[]);
    assert_eq!(r, ProbeOutcome::Indeterminate);
    assert!(!r.is_conclusive_negative(), "a vanished cwd is not evidence");
}

/// A directory that is genuinely not a repository IS conclusive: git answered, and there is no
/// repo here to carry a work remote.
#[test]
fn detect_is_conclusively_not_a_repo_for_a_plain_directory() {
    let tmp = TempDir::new().unwrap();
    // PRECONDITION, stated rather than assumed: `NotARepo` means "no repository at or above this
    // cwd", so the temp root must not itself sit under one. `$TMPDIR` decides that, and it is not
    // always `/tmp`: pointing it under this repo (or under `$HOME`, which is a dotfiles repo on the
    // maintainer's machine) puts a `.git` above every temp dir and this cwd legitimately reads
    // `Indeterminate` instead. Found when the mutation task redirected `TMPDIR` and the baseline
    // failed here with no explanation.
    assert!(
        !tmp.path().ancestors().any(|d| d.join(".git").exists()),
        "this test needs a temp root outside any git repository; $TMPDIR resolved to {} and a \
         `.git` exists above it, so a plain directory there is legitimately Indeterminate",
        tmp.path().display()
    );

    let r = detect_with_blocked_roots(tmp.path(), &[]);
    assert_eq!(r, ProbeOutcome::NotARepo);
    assert!(r.is_conclusive_negative());
}

/// The seed state of Problem 1, and the one negative the routing gate exists to record.
#[test]
fn detect_is_conclusively_no_origin_when_the_repo_has_none() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().canonicalize().unwrap();
    git_init(&real);
    let r = detect_with_blocked_roots(&real, &[]);
    assert_eq!(r, ProbeOutcome::NoOrigin);
    assert!(r.is_conclusive_negative());
}

#[test]
fn detect_returns_slug_for_ssh_origin() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().canonicalize().unwrap();
    git_init(&real);
    add_origin(&real, "git@github.com:tatari-tv/claude-report.git");
    let r = detect_with_blocked_roots(&real, &[]);
    assert_eq!(r.resolved_slug(), Some("tatari-tv/claude-report"));
}

#[test]
fn detect_returns_slug_for_https_origin_no_dot_git() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().canonicalize().unwrap();
    git_init(&real);
    add_origin(&real, "https://github.com/scottidler/obsidian");
    let r = detect_with_blocked_roots(&real, &[]);
    assert_eq!(r.resolved_slug(), Some("scottidler/obsidian"));
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
    assert_eq!(r.resolved_slug(), Some("foo/bar"));
}

/// A blocked root is a refusal to ATTRIBUTE, not a statement about a remote, so it must not record.
/// Stamping it would mean a session run under a git-tracked `$HOME` could never be recovered.
#[test]
fn detect_rejects_dotfiles_climb_when_toplevel_is_blocked() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().canonicalize().unwrap();
    git_init(&real);
    add_origin(&real, "git@github.com:user/dotfiles.git");

    let unversioned = real.join("scratch").join("foo");
    std::fs::create_dir_all(&unversioned).unwrap();

    let r = detect_with_blocked_roots(&unversioned, std::slice::from_ref(&real));
    assert_eq!(
        r,
        ProbeOutcome::Blocked,
        "dotfiles climb should be rejected via blocked root"
    );
    assert!(
        !r.is_conclusive_negative(),
        "a blocked root says nothing about a remote, so it must never stamp"
    );

    let r2 = detect_with_blocked_roots(&unversioned, &[]);
    assert_eq!(
        r2.resolved_slug(),
        Some("user/dotfiles"),
        "without blocked roots, the literal rule does not catch the climb"
    );
}

/// AC11's `GIT_DIR` vector. Measured on `main` before the scrub: an exported `GIT_DIR` makes an
/// unrelated cwd resolve to the pointed-at repo's origin, and the containment check does NOT catch
/// it, because git treats the `-C` path as the work tree when `GIT_DIR` is set without
/// `GIT_WORK_TREE`.
///
/// BITES: delete `env_clear()` from `run_git` and this resolves to `tatari-tv/forged` instead.
#[test]
fn git_dir_in_the_environment_cannot_forge_an_attribution() {
    let guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().canonicalize().unwrap();

    let elsewhere = real.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    git_init(&elsewhere);
    add_origin(&elsewhere, "git@github.com:tatari-tv/forged.git");

    // A plain directory with no repository of its own.
    let unrelated = real.join("unrelated");
    std::fs::create_dir_all(&unrelated).unwrap();

    let prior = std::env::var("GIT_DIR").ok();
    unsafe { std::env::set_var("GIT_DIR", elsewhere.join(".git")) };
    let r = detect_with_blocked_roots(&unrelated, &[]);
    match prior {
        Some(v) => unsafe { std::env::set_var("GIT_DIR", v) },
        None => unsafe { std::env::remove_var("GIT_DIR") },
    }
    drop(guard);

    assert_eq!(
        r.resolved_slug(),
        None,
        "an inherited GIT_DIR must not reach the probe; it forged an attribution on main"
    );
    assert_eq!(
        r,
        ProbeOutcome::NotARepo,
        "with the environment scrubbed, the cwd is simply not a repository"
    );
}

/// AC11's `GIT_CONFIG_*` vectors, both of them, in the LEAK direction (a personal origin reading as
/// work). Round 3 demonstrated only the safe direction; this is the one that matters.
///
/// BITES: revert the origin read to `git remote get-url origin` and the `insteadOf` case resolves to
/// `tatari-tv/sideproject`. Drop `--local` and the injection case does the same.
#[test]
fn git_config_in_the_environment_cannot_forge_an_attribution() {
    let guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().canonicalize().unwrap();
    git_init(&real);
    add_origin(&real, "git@github.com:scottidler/sideproject.git");

    let vars = [
        ("GIT_CONFIG_COUNT", "1".to_string()),
        (
            "GIT_CONFIG_KEY_0",
            "url.git@github.com:tatari-tv/.insteadOf".to_string(),
        ),
        ("GIT_CONFIG_VALUE_0", "git@github.com:scottidler/".to_string()),
    ];
    let prior: Vec<(&str, Option<String>)> = vars.iter().map(|(k, _)| (*k, std::env::var(k).ok())).collect();
    for (k, v) in &vars {
        unsafe { std::env::set_var(k, v) };
    }
    let rewritten = detect_with_blocked_roots(&real, &[]);
    for (k, v) in &prior {
        match v {
            Some(v) => unsafe { std::env::set_var(k, v) },
            None => unsafe { std::env::remove_var(k) },
        }
    }
    drop(guard);

    assert_eq!(
        rewritten.resolved_slug(),
        Some("scottidler/sideproject"),
        "an insteadOf rewrite must not turn a personal origin into a work one"
    );
}

/// **A FOURTH channel, found while writing the deletion proofs and not named in the design.**
/// `insteadOf` does not need an environment variable OR a hostile `~/.gitconfig`: set in the repo's
/// OWN `--local` config it rewrites a personal origin into a work one, and `env_clear` cannot help,
/// because the forge lives in the file `--local` is supposed to read. Measured 2026-07-31:
///
/// ```text
/// $ git config --local 'url.git@github.com:tatari-tv/.insteadOf' 'git@github.com:scottidler/'
/// $ env -i PATH=... git remote get-url origin
/// git@github.com:tatari-tv/sideproject.git      # forged
/// $ env -i PATH=... git config --local --get remote.origin.url
/// git@github.com:scottidler/sideproject.git     # the truth
/// ```
///
/// This is the case the primitive change OWNS, and the only one where it is the sole defense.
///
/// BITES: revert the origin read to `git remote get-url origin` and this resolves to
/// `tatari-tv/sideproject`.
#[test]
fn the_origin_primitive_does_not_apply_insteadof_rewriting() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().canonicalize().unwrap();
    git_init(&real);
    add_origin(&real, "git@github.com:scottidler/sideproject.git");
    git_config(
        &real,
        "url.git@github.com:tatari-tv/.insteadOf",
        "git@github.com:scottidler/",
    );

    assert_eq!(
        detect_with_blocked_roots(&real, &[]).resolved_slug(),
        Some("scottidler/sideproject"),
        "an insteadOf rule in the repo's own config must not rewrite a personal origin into a work \
         one; `git remote get-url` applies it by design and `git config` does not"
    );
}

/// The property `--local` OWNS: the origin read consults ONLY the repo's own config, so a
/// `remote.origin.url` planted in a WIDER scope cannot contribute one.
///
/// Asserted against the argv itself rather than through [`detect_with_blocked_roots`], and that is
/// deliberate. `run_git`'s `env_clear` already denies the child every channel that could carry a
/// wider-scope config, so a test routed through the module would pass with `--local` deleted and
/// prove nothing. The layering means each defense has to be tested at ITS layer:
///
/// - `env_clear` is tested through the module (`git_dir_in_the_environment_...`), because that is
///   where it acts.
/// - `--local` is tested here, against [`ORIGIN_ARGS`], because what it protects is the read's SCOPE.
///   Its live job is to keep the forge closed if a future caller ever inherits an environment, and
///   `/etc/gitconfig` is a scope no `env_clear` removes.
///
/// BITES: drop `--local` from [`ORIGIN_ARGS`] and the first assertion sees the planted URL.
#[test]
fn the_origin_primitive_reads_only_the_repos_own_config() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().canonicalize().unwrap();
    git_init(&real);

    let planted = real.join("planted.cfg");
    std::fs::write(
        &planted,
        "[remote \"origin\"]\n\turl = git@github.com:tatari-tv/forged.git\n",
    )
    .unwrap();

    let run = |args: &[&str]| -> (i32, String) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&real)
            .args(args)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("GIT_CONFIG_GLOBAL", &planted)
            .output()
            .expect("spawn git");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        )
    };

    let (rc, out) = run(ORIGIN_ARGS);
    assert_eq!(
        (rc, out.as_str()),
        (1, ""),
        "the SHIPPED argv must stay conclusively origin-less with a URL planted in a wider scope"
    );

    // The control: the same read without the scope qualifier picks the planted URL straight up.
    // Without this line the assertion above could pass because the plant never worked.
    let (rc, out) = run(&["config", "--get", "remote.origin.url"]);
    assert_eq!(
        (rc, out.as_str()),
        (0, "git@github.com:tatari-tv/forged.git"),
        "control: the plant IS reachable without --local, which is what --local is for"
    );
}

/// The composite: a hostile `~/.gitconfig` setting `remote.origin.url` must not turn a repo with NO
/// origin, which owes the conclusive `NoOrigin`, into a forged work `Resolved`. That is the worst
/// shape in the design, and it needs no `GIT_*` variable at all.
///
/// BITES: add `HOME` to `GIT_ENV_ALLOWLIST` (with `--local` also dropped) and this resolves to
/// `tatari-tv/forged`. With `--local` in place, forwarding `HOME` alone does NOT break it, and AC11
/// says so rather than demanding a failure measurement shows will not happen.
#[test]
fn a_hostile_home_gitconfig_cannot_forge_an_attribution() {
    let guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().canonicalize().unwrap();

    let fake_home = real.join("home");
    std::fs::create_dir_all(&fake_home).unwrap();
    std::fs::write(
        fake_home.join(".gitconfig"),
        "[remote \"origin\"]\n\turl = git@github.com:tatari-tv/forged.git\n",
    )
    .unwrap();

    let repo_dir = real.join("noorigin");
    std::fs::create_dir_all(&repo_dir).unwrap();
    git_init(&repo_dir);

    let prior = std::env::var("HOME").ok();
    unsafe { std::env::set_var("HOME", &fake_home) };
    let r = detect_with_blocked_roots(&repo_dir, &[]);
    match prior {
        Some(v) => unsafe { std::env::set_var("HOME", v) },
        None => unsafe { std::env::remove_var("HOME") },
    }
    drop(guard);

    assert_eq!(
        r,
        ProbeOutcome::NoOrigin,
        "a repo with no origin must stay CONCLUSIVELY origin-less; a hostile ~/.gitconfig forged a \
         work Resolved here on main"
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

// ---- provenance ---------------------------------------------------------------------------------

/// The rank ordering IS the catalog's write rule, so it is pinned here rather than left implicit:
/// `git-origin(0) < known-path(1) < files-touched(2) < path-guess(3)`, and the derived `Ord` must
/// agree with `rank()` or the upgrade-only upsert and an in-memory comparison would disagree.
#[test]
fn repo_source_rank_ordering_is_best_first() {
    assert_eq!(RepoSource::GitOrigin.rank(), 0);
    assert_eq!(RepoSource::KnownPath.rank(), 1);
    assert_eq!(RepoSource::FilesTouched.rank(), 2);
    assert_eq!(RepoSource::PathGuess.rank(), 3);

    let ordered = [
        RepoSource::GitOrigin,
        RepoSource::KnownPath,
        RepoSource::FilesTouched,
        RepoSource::PathGuess,
    ];
    for pair in ordered.windows(2) {
        let (better, worse) = (pair[0], pair[1]);
        assert!(better < worse, "{better} must outrank {worse}");
        assert!(better.rank() < worse.rank(), "rank() must agree with Ord");
    }
}

/// The kebab spellings are a persistence contract (a TEXT column), not a display label, so the
/// round trip is asserted both ways.
#[test]
fn repo_source_kebab_spellings_round_trip() {
    for (source, spelling) in [
        (RepoSource::GitOrigin, "git-origin"),
        (RepoSource::KnownPath, "known-path"),
        (RepoSource::FilesTouched, "files-touched"),
        (RepoSource::PathGuess, "path-guess"),
    ] {
        assert_eq!(source.as_str(), spelling);
        assert_eq!(source.to_string(), spelling);
        assert_eq!(RepoSource::from_str(spelling).unwrap(), source);
    }
}

/// An unrecognized provenance fails loudly and names the offending value: silently dropping it
/// would let a guess be read back as an observation.
#[test]
fn repo_source_rejects_an_unknown_spelling() {
    let err = format!("{:#}", RepoSource::from_str("GitOrigin").unwrap_err());
    assert!(err.contains("GitOrigin"), "must name the bad value: {err}");
    assert!(err.contains("git-origin"), "must name the legal set: {err}");
}

// ---- rule 2: the learned path map ---------------------------------------------------------------

fn map(entries: &[(&Path, &str)]) -> BTreeMap<PathBuf, String> {
    entries
        .iter()
        .map(|(p, r)| (p.to_path_buf(), (*r).to_string()))
        .collect()
}

/// AC: with `<root>/tatari-tv/clyde/main -> tatari-tv/clyde` in the injected map and the directory
/// ABSENT from disk, resolution returns `tatari-tv/clyde` with `KnownPath`. This is the whole point
/// of the phase: a worktree that has been cleaned up still attributes.
///
/// BITES: delete the rule-2 arm from the chain and this falls through to rule 4, which would answer
/// `tatari-tv/clyde` for THIS layout but with source `PathGuess`, failing the source assertion.
#[test]
fn known_path_resolves_a_vanished_directory() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repos");
    let cwd = root.join("tatari-tv").join("clyde").join("main");
    assert!(!cwd.exists(), "the fixture must model a deleted worktree");

    let paths = map(&[(cwd.as_path(), "tatari-tv/clyde")]);
    let mut resolver = Resolver::new();
    let resolved = resolver.resolve(&cwd, &paths, &BTreeMap::new(), &root);

    assert_eq!(
        resolved,
        Some(Resolved {
            repo: "tatari-tv/clyde".into(),
            source: RepoSource::KnownPath,
        })
    );
}

/// Longest prefix wins, and a prefix hit above the cwd still resolves: a subdirectory of a
/// recorded path inherits its repo.
#[test]
fn known_path_takes_the_longest_prefix() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repos");
    let outer = root.join("tatari-tv").join("clyde");
    let inner = outer.join("main");

    let paths = map(&[
        (outer.as_path(), "tatari-tv/clyde-outer"),
        (inner.as_path(), "tatari-tv/clyde-inner"),
    ]);
    let deep = inner.join("report").join("src");

    let resolved = from_known_path(&deep, &paths, &[]).unwrap();
    assert_eq!(resolved.repo, "tatari-tv/clyde-inner");
    assert_eq!(resolved.source, RepoSource::KnownPath);
}

/// The walk stops at a blocked root: nothing at or above `$HOME` attributes a session, even if a
/// stray row put an entry there.
#[test]
fn known_path_stops_at_a_blocked_ancestor() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let cwd = home.join("scratch");

    let paths = map(&[(home.as_path(), "someone/dotfiles")]);
    assert_eq!(from_known_path(&cwd, &paths, std::slice::from_ref(&home)), None);
    assert!(
        from_known_path(&cwd, &paths, &[]).is_some(),
        "without the blocked root the entry would have matched, so the block is what rejected it"
    );
}

/// AC: with an EMPTY map, `<root>/tatari-tv/clyde-ft` returns `tatari-tv/clyde-ft` with `PathGuess`
/// and NEVER `KnownPath`. Rule 2 is learned, never a pattern match, so an unseen sibling worktree
/// falls through to the labeled guess instead of being laundered as an observation.
///
/// BITES: make rule 2 pattern-match the repo root and this returns `KnownPath`.
#[test]
fn an_empty_map_falls_through_to_a_labeled_guess() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repos");
    let cwd = root.join("tatari-tv").join("clyde-ft");

    let empty: BTreeMap<PathBuf, String> = BTreeMap::new();
    assert_eq!(from_known_path(&cwd, &empty, &[]), None, "rule 2 must not guess");

    let mut resolver = Resolver::new();
    let resolved = resolver.resolve(&cwd, &empty, &BTreeMap::new(), &root).unwrap();
    assert_eq!(resolved.repo, "tatari-tv/clyde-ft");
    assert_eq!(
        resolved.source,
        RepoSource::PathGuess,
        "a fabricated sibling slug must be marked a guess"
    );
}

// ---- rule 3: files touched ----------------------------------------------------------------------

fn touched(entries: &[(&str, u64)]) -> BTreeMap<String, u64> {
    entries.iter().map(|(s, n)| ((*s).to_string(), *n)).collect()
}

#[test]
fn files_touched_takes_a_unique_argmax() {
    let counts = touched(&[("tatari-tv/clyde", 7), ("scottidler/obsidian", 2)]);
    assert_eq!(
        from_files_touched(&counts),
        Some(Resolved {
            repo: "tatari-tv/clyde".into(),
            source: RepoSource::FilesTouched,
        })
    );
}

/// A tie is evidence of ambiguity, not of a resolvable repo. A slug-ordered tie-break would hand
/// all the spend to the lexicographically first repo, so rule 3 abstains and the chain falls
/// through.
///
/// BITES: replace the tie check with `max_by_key` and this returns `tatari-tv/appsec-hiring-plan`.
#[test]
fn files_touched_abstains_on_a_tie() {
    let counts = touched(&[("tatari-tv/appsec-hiring-plan", 1), ("tatari-tv/appsec-screening", 1)]);
    assert_eq!(from_files_touched(&counts), None);
}

#[test]
fn files_touched_ignores_zero_counts_and_an_empty_map() {
    assert_eq!(from_files_touched(&BTreeMap::new()), None);
    assert_eq!(from_files_touched(&touched(&[("tatari-tv/clyde", 0)])), None);
    // A zero-count entry alongside a real one is not a tie: it is no evidence at all.
    let counts = touched(&[("tatari-tv/clyde", 3), ("scottidler/obsidian", 0)]);
    assert_eq!(from_files_touched(&counts).unwrap().repo, "tatari-tv/clyde");
}

// ---- rule 4: the path guess ---------------------------------------------------------------------

#[test]
fn path_guess_matches_org_and_repo_under_the_root() {
    let root = Path::new("/home/someone/repos");
    assert_eq!(
        from_path_guess(&root.join("tatari-tv").join("clyde"), root),
        Some(Resolved {
            repo: "tatari-tv/clyde".into(),
            source: RepoSource::PathGuess,
        })
    );
    // Deeper paths keep the first two components: the 136-session dominant case.
    assert_eq!(
        from_path_guess(&root.join("tatari-tv/clyde/main/report/src"), root)
            .unwrap()
            .repo,
        "tatari-tv/clyde"
    );
}

#[test]
fn path_guess_declines_outside_the_root_or_too_shallow() {
    let root = Path::new("/home/someone/repos");
    assert_eq!(from_path_guess(Path::new("/home/someone"), root), None);
    assert_eq!(from_path_guess(Path::new("/tmp/scratch/foo/bar"), root), None);
    assert_eq!(from_path_guess(root, root), None, "the root itself is not a repo");
    assert_eq!(
        from_path_guess(&root.join("tatari-tv"), root),
        None,
        "an org with no repo component is not a slug"
    );
}

// ---- the chain ----------------------------------------------------------------------------------

/// AC: a `$HOME` cwd still returns `None`. Asserted against a synthetic home (deterministic on any
/// machine) and again against the real one, which is what the shipped `Resolver::new` blocks.
#[test]
fn a_home_cwd_resolves_to_none() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(home.join("repos")).unwrap();
    // A git repo AT $HOME (the dotfiles case) must still be refused.
    git_init(&home);
    add_origin(&home, "git@github.com:someone/dotfiles.git");

    let mut resolver = Resolver {
        cache: HashMap::new(),
        blocked: vec![home.clone()],
    };
    assert_eq!(
        resolver.resolve(&home, &BTreeMap::new(), &BTreeMap::new(), &home.join("repos")),
        None
    );

    if let Some(real_home) = dirs::home_dir() {
        let mut real = Resolver::new();
        assert_eq!(
            real.resolve(&real_home, &BTreeMap::new(), &BTreeMap::new(), &real_home.join("repos")),
            None,
            "the real $HOME must not attribute to any repo"
        );
    }
}

/// AC: the existing rule-1 cases resolve unchanged through the chain, under `GitOrigin`.
#[test]
fn a_live_worktree_resolves_via_git_origin() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().canonicalize().unwrap();
    git_init(&real);
    add_origin(&real, "git@github.com:tatari-tv/claude-report.git");

    // A map and a repo root that would BOTH answer differently, so a rule-1 regression is visible
    // rather than masked by a lower rule.
    let paths = map(&[(real.as_path(), "wrong/answer")]);
    let mut resolver = Resolver::new();
    let resolved = resolver
        .resolve(&real, &paths, &touched(&[("also/wrong", 9)]), real.parent().unwrap())
        .unwrap();

    assert_eq!(resolved.repo, "tatari-tv/claude-report");
    assert_eq!(resolved.source, RepoSource::GitOrigin);
}

/// Precedence between the three fallback rules: the learned map outranks both the touched-file
/// argmax and the pattern guess, and the argmax outranks the guess.
#[test]
fn the_chain_prefers_the_higher_confidence_rule() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repos");
    let cwd = root.join("tatari-tv").join("clyde-ft");
    let counts = touched(&[("scottidler/obsidian", 4)]);

    let mut resolver = Resolver::new();
    let known = resolver
        .resolve(&cwd, &map(&[(cwd.as_path(), "tatari-tv/clyde")]), &counts, &root)
        .unwrap();
    assert_eq!(known.repo, "tatari-tv/clyde");
    assert_eq!(known.source, RepoSource::KnownPath);

    let files = resolver.resolve(&cwd, &BTreeMap::new(), &counts, &root).unwrap();
    assert_eq!(files.repo, "scottidler/obsidian");
    assert_eq!(files.source, RepoSource::FilesTouched);

    // A tie in rule 3 falls through to rule 4 rather than picking a slug-ordered winner.
    let tie = touched(&[("scottidler/obsidian", 1), ("tatari-tv/clyde", 1)]);
    let guessed = resolver.resolve(&cwd, &BTreeMap::new(), &tie, &root).unwrap();
    assert_eq!(guessed.repo, "tatari-tv/clyde-ft");
    assert_eq!(guessed.source, RepoSource::PathGuess);
}

/// Everything declines: a temp-dir cwd with no map, no edited files, and nothing under the repo
/// root. `None` is the honest answer, and the caller renders it as `(unattributed)`.
#[test]
fn the_chain_returns_none_when_every_rule_declines() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("scratch").join("nowhere");
    let mut resolver = Resolver::new();
    assert_eq!(
        resolver.resolve(
            &cwd,
            &BTreeMap::new(),
            &BTreeMap::new(),
            Path::new("/home/someone/repos")
        ),
        None
    );
}

/// `slug_under_root` is the ONE definition of the `<repo-root>/<org>/<repo>` shape: rule 4 reads a
/// session's cwd through it, and `efficiency::outcome::union` reads every edited file's parent
/// directory through it to build rule 3's input. Two readers deriving the shape independently is
/// exactly how the two would drift.
#[test]
fn slug_under_root_reads_the_first_two_components_only() {
    let root = Path::new("/home/saidler/repos");
    assert_eq!(
        slug_under_root(&root.join("tatari-tv/clyde"), root).as_deref(),
        Some("tatari-tv/clyde")
    );
    assert_eq!(
        slug_under_root(&root.join("tatari-tv/clyde/report/src"), root).as_deref(),
        Some("tatari-tv/clyde"),
        "depth below the repo slot does not change the slug"
    );
}

#[test]
fn slug_under_root_declines_anything_that_is_not_the_shape() {
    let root = Path::new("/home/saidler/repos");
    assert_eq!(slug_under_root(root, root), None, "the root itself names no repo");
    assert_eq!(
        slug_under_root(&root.join("tatari-tv"), root),
        None,
        "an org with no repo component is not a slug"
    );
    assert_eq!(
        slug_under_root(Path::new("/tmp/scratch/a/b"), root),
        None,
        "matching is confined to the configured root, so an arbitrary path cannot manufacture an org"
    );
}

// ---------------------------------------------------------------------------------------------
// Mutation-driven coverage (Phase 5). Every test below closes a SURVIVING mutant: the mutation run
// found code whose behavior no test observed. Each names the mutant it kills, so a future reader can
// tell a coverage test from a behavior test.
// ---------------------------------------------------------------------------------------------

/// KILLS: `replace home_dir_as_blocked -> Vec<PathBuf> with vec![]` and `with vec![Default::default()]`.
///
/// The blocked set is the ONLY thing stopping a git-tracked `$HOME` from attributing every session
/// to the dotfiles repo, and nothing asserted it was actually populated from `$HOME`. Every existing
/// blocked-root test passes its own list, so an empty default was invisible.
#[test]
fn a_fresh_resolver_blocks_the_real_home_directory() {
    // This test READS `HOME` twice indirectly (`Resolver::new` computes `home_dir_as_blocked`, and
    // `dirs::home_dir` computes the expected value) while `a_hostile_home_gitconfig_cannot_forge_an_
    // attribution` sets `HOME` process-wide in the same binary. `ENV_LOCK` serializes only the tests
    // that TAKE it, so an indirect reader has to take it too or it can observe the fake home and
    // fail for a reason that has nothing to do with what it asserts.
    let guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let resolver = Resolver::new();
    let home = dirs::home_dir().expect("this test needs a HOME");
    assert_eq!(
        resolver.blocked,
        vec![home],
        "Resolver::new must block $HOME; an empty set silently attributes a git-tracked home"
    );
    drop(guard);
}

/// KILLS: the four `ProbeOutcome::resolved_host` mutants, including the deleted match arm.
///
/// The host is what Phase 3's allowlist reads, so an accessor that always answered `None` would
/// silently make every row pre-v13 (never refusing), and one that answered a constant would confer
/// work from the wrong host.
#[test]
fn resolved_host_reports_the_host_only_for_a_resolved_probe() {
    let resolved = ProbeOutcome::Resolved {
        slug: "tatari-tv/philo".into(),
        host: "github.com".into(),
    };
    assert_eq!(resolved.resolved_host(), Some("github.com"));
    assert_eq!(resolved.resolved_slug(), Some("tatari-tv/philo"));

    for other in [
        ProbeOutcome::NoOrigin,
        ProbeOutcome::NotARepo,
        ProbeOutcome::Blocked,
        ProbeOutcome::OutsideRoot,
        ProbeOutcome::Indeterminate,
    ] {
        assert_eq!(
            other.resolved_host(),
            None,
            "{} observed no remote, so it has no host to report",
            other.as_str()
        );
    }
}

/// KILLS: both `ProbeOutcome::as_str` mutants.
///
/// These tokens are PERSISTED in `sessions.repo_probe`, so they are a stored contract rather than a
/// label: renaming one silently orphans every row written under the old spelling, and `clyde doctor`
/// would stop counting them.
#[test]
fn probe_outcome_tokens_are_a_stable_contract() {
    assert_eq!(
        ProbeOutcome::Resolved {
            slug: "a/b".into(),
            host: "h".into()
        }
        .as_str(),
        "resolved"
    );
    assert_eq!(ProbeOutcome::NoOrigin.as_str(), "no-origin");
    assert_eq!(ProbeOutcome::NotARepo.as_str(), "not-a-repo");
    assert_eq!(ProbeOutcome::Blocked.as_str(), "blocked");
    assert_eq!(ProbeOutcome::OutsideRoot.as_str(), "outside-root");
    assert_eq!(ProbeOutcome::Indeterminate.as_str(), "indeterminate");
}

/// KILLS: `replace == with != in detect_with_blocked_roots` (the containment check).
///
/// The check is `!(toplevel == cwd || cwd.starts_with(&toplevel))`, and it only DIFFERS from the
/// mutant when the toplevel is neither the cwd nor an ancestor of it. Every ordinary shape is
/// contained by construction, because git's discovery walks UP, so nothing exercised the rejection.
///
/// `core.worktree` is the reproducible way to get there. Measured 2026-07-31:
///
/// ```text
/// $ git -C <proj> config --local core.worktree <elsewhere>
/// $ git -C <proj> rev-parse --show-toplevel
/// <elsewhere>                     # not the cwd, and not an ancestor of it
/// ```
///
/// This is exactly the "git finds a repo that does not contain X" case the containment check exists
/// for, and it must decline rather than attribute the cwd to that repo.
#[test]
fn detect_declines_a_toplevel_that_does_not_contain_the_cwd() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().canonicalize().unwrap();
    let elsewhere = real.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();

    let proj = real.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    add_origin(&proj, "git@github.com:tatari-tv/philo.git");
    git_config(&proj, "core.worktree", &elsewhere.to_string_lossy());

    let outcome = detect_with_blocked_roots(&proj, &[]);
    assert_eq!(
        outcome,
        ProbeOutcome::OutsideRoot,
        "a toplevel outside the cwd must be rejected, not attributed"
    );
    assert!(
        !outcome.is_conclusive_negative(),
        "and it must not stamp: a containment rejection says nothing about a remote"
    );
}

// ---------------------------------------------------------------------------------------------
// Phase 6: rule 1 resolves where there is no work tree, and stops declining symlinked cwds.
// ---------------------------------------------------------------------------------------------

/// Matrix row 6, and Problem 4. At the root of a bare-repo container there is no work tree, so
/// `rev-parse --show-toplevel` fails and rule 1 used to give up BEFORE the call that answers.
///
/// Not exotic: it is what `git init --bare` plus branch directories produces, and clyde's own
/// `build.rs` already resolves it. Cost on Keegan's catalog: 12 sessions, $326.87 of July spend, and
/// `tatari-tv/airflow-dags` absent from every by-repo table in his month's report.
///
/// BITES: delete the `--git-common-dir` fallback and this declines again.
#[test]
fn detect_resolves_at_a_bare_repo_container_root() {
    let m = Matrix::build();
    assert_eq!(
        detect_with_blocked_roots(&m.container_root, &m.blocked()).resolved_slug(),
        Some("tatari-tv/airflow-dags")
    );
}

/// Matrix row 7. A plain bare repo reports `.` for its common dir, which is the case the design's
/// own snippet got wrong: `cwd.join(common).parent()` walks one level too high.
///
/// BITES: delete the `common == cwd` branch and the root becomes the cwd's PARENT, which fails
/// containment and declines.
#[test]
fn detect_resolves_at_a_plain_bare_repo() {
    let m = Matrix::build();
    assert_eq!(
        detect_with_blocked_roots(&m.bare_mirror, &m.blocked()).resolved_slug(),
        Some("tatari-tv/mirror")
    );
}

/// Matrix row 8. A cwd inside the container's `.bare` resolves through the container, because the
/// common dir's parent IS the container and the cwd is under it.
///
/// The design labels this row "containment" and Phase 6 names its test
/// `detect_declines_a_repo_found_above_the_cwd`, but measurement says it RESOLVES: git's discovery
/// walks up, so whatever it finds is an ancestor by construction. The genuine decline needs a
/// gitdir pointer out of the tree, which is
/// `detect_declines_a_repo_found_outside_the_cwd` below. Both are asserted rather than picking one.
#[test]
fn detect_resolves_from_inside_the_bare_dir() {
    let m = Matrix::build();
    assert_eq!(
        detect_with_blocked_roots(&m.inside_bare, &m.blocked()).resolved_slug(),
        Some("tatari-tv/airflow-dags")
    );
}

/// The containment check, on the NO-WORK-TREE branch. A `.git` FILE pointing at a bare repo in a
/// SIBLING tree resolves to a root that is not an ancestor of the cwd, and must be refused rather
/// than attributed.
///
/// BITES: drop the mirrored containment check from the fallback and this attributes
/// `tatari-tv/detached` to a directory that has nothing to do with it.
#[test]
fn detect_declines_a_repo_found_outside_the_cwd() {
    let m = Matrix::build();
    let outcome = detect_with_blocked_roots(&m.outside_root, &m.blocked());
    assert_eq!(outcome, ProbeOutcome::OutsideRoot);
    assert!(
        !outcome.is_conclusive_negative(),
        "a containment rejection says nothing about a remote, so it must never stamp"
    );
}

/// Matrix row 13, and the reason the fallback needs the `--is-bare-repository` refinement. A BARE
/// repo at `$HOME` must still be refused: the blocked-root guard is what stops a git-tracked home
/// attributing every session to the dotfiles repo.
///
/// BITES: root the `common == cwd` case at the cwd's parent unconditionally and the computed root
/// stops equalling `$HOME`, so the guard misses and this resolves.
#[test]
fn detect_still_blocks_a_bare_repo_at_a_blocked_root() {
    let m = Matrix::build();
    m.make_home_a_bare_repo();
    assert_eq!(
        detect_with_blocked_roots(&m.home(), &m.blocked()),
        ProbeOutcome::Blocked
    );
}

/// The OTHER half of the same refinement, and a hole this phase would have introduced rather than
/// exposed. A cwd inside a NON-bare repo's `.git` also reports `.` for its common dir, so the
/// design's unconditional `common == cwd -> root = cwd` would root at `<repo>/.git`. The blocked
/// check compares the root, so a repo at `$HOME` probed from `$HOME/.git` would compute
/// `$HOME/.git`, MISS the guard, and attribute the dotfiles repo.
///
/// BITES: drop the `--is-bare-repository` branch and this resolves instead of being blocked.
#[test]
fn detect_blocks_a_cwd_inside_a_blocked_repos_git_dir() {
    let m = Matrix::build();
    m.make_home_a_repo();
    let git_dir = m.home().join(".git");
    assert_eq!(
        detect_with_blocked_roots(&git_dir, &m.blocked()),
        ProbeOutcome::Blocked,
        "a cwd inside $HOME/.git must root at $HOME and hit the guard"
    );
}

/// Matrix row 24, a CONFIRMED pre-existing bug rather than one this design introduced.
/// `--show-toplevel` returns the CANONICAL path, so the lexical containment check rejected every
/// session whose cwd was reached through a symlink, silently.
///
/// BITES: revert `contains` to a lexical comparison and this declines.
#[test]
fn detect_resolves_a_symlinked_cwd() {
    let m = Matrix::build();
    assert_eq!(
        detect_with_blocked_roots(&m.symlinked, &m.blocked()).resolved_slug(),
        Some("tatari-tv/philo"),
        "a symlink-reached cwd must resolve; the toplevel is canonical and the cwd is not"
    );
}

/// A `.git` DIRECTORY that carries no `HEAD` is not a repository, and git says so. Testing only for
/// existence made a stray one downgrade a conclusive `NotARepo` to `Indeterminate`.
///
/// Found on a live host: `~/.git` there is a plain directory holding only `info/` (the
/// `info/exclude` global-ignore trick), and it turned 21 conclusive answers into a `clyde doctor`
/// line telling the operator to go check `safe.directory` for a problem that did not exist.
///
/// BITES: revert `has_git_marker` to a bare `.exists()` and this reports `Indeterminate`.
#[test]
fn a_git_directory_with_no_head_is_not_a_marker() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().canonicalize().unwrap();
    assert!(
        !real.ancestors().any(|d| d.join(".git").exists()),
        "this test needs a temp root outside any git repository"
    );

    // The stray shape: a `.git` directory with only `info/` inside.
    std::fs::create_dir_all(real.join(".git").join("info")).unwrap();
    let under = real.join("some").join("workdir");
    std::fs::create_dir_all(&under).unwrap();

    let outcome = detect_with_blocked_roots(&under, &[]);
    assert_eq!(
        outcome,
        ProbeOutcome::NotARepo,
        "git reports `not a git repository` here, so clyde must agree and record it"
    );
    assert!(outcome.is_conclusive_negative());

    // And the real shape still counts: a `.git` dir WITH a HEAD suppresses the conclusive answer,
    // because a repository genuinely is present even if git could not use it.
    std::fs::write(real.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
    assert!(
        !detect_with_blocked_roots(&under, &[]).is_conclusive_negative(),
        "a real git dir above the cwd must still suppress the conclusive answer"
    );
}
