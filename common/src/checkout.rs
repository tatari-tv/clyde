//! The checkout matrix: one shared, real-`git` fixture every phase asserts against.
//!
//! `docs/design/2026-07-31-attribution-and-routing.md` (Testing Strategy) exists because rule 1 was
//! declared "layout-agnostic" over four rows that were all the same shape. The remedy is a single
//! fixture carrying EVERY checkout shape the design names, built in a [`tempfile::TempDir`] with its
//! own `HOME`, its own `repo-root`, and its own `projects-dir`. A new shape is added here once and
//! every phase inherits it.
//!
//! Real `git init`, never a mocked `run_git`: the defects this doc catalogues are in what git
//! ACTUALLY returns (a container root has no work tree, a plain bare repo reports `.` for its common
//! dir), and a fake would encode the same wrong assumption the design doc did.
//!
//! Gated behind the `testkit` feature so the fixture builders never ship in a release binary. The
//! crates that assert against it take `common = { path = "../common", features = ["testkit"] }` as a
//! DEV-dependency.
//!
//! ## Hermetic by construction
//!
//! Every `git` invocation here runs with `GIT_CONFIG_GLOBAL` and `GIT_CONFIG_SYSTEM` pointed at
//! `/dev/null` and identity supplied inline. Without that the developer's own `~/.gitconfig` reaches
//! the fixture, and an `insteadOf` rule or a `init.defaultBranch` there would make the matrix mean
//! something different on every host. That is the same class of leak the design's `--local` decision
//! closes in production code.

use std::path::{Path, PathBuf};
use std::process::Command;

use log::debug;
use tempfile::TempDir;

/// The work org every work-scoped fixture row is checked out under. The one place the fixture's
/// notion of "work" is spelled; `session::scope::WORK_ORGS` is the production one and the two are
/// deliberately separate, so a test cannot pass by sharing a constant with the code under test.
const WORK_ORG: &str = "tatari-tv";

/// Run one `git` command inside the fixture, hermetically. Panics with the full stderr on failure,
/// because a fixture that half-built is worse than one that did not build: the test that consumes it
/// would report a routing defect that is really a setup bug.
fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        // `env_clear` FIRST, then an explicit set. Naming only `GIT_CONFIG_*` was not enough: the
        // routing tests in `repo::tests` mutate `GIT_DIR` and `HOME` process-wide under `ENV_LOCK`,
        // and cargo runs tests in the same binary CONCURRENTLY, so a leaked `GIT_DIR` reached this
        // builder and every `git init` here failed. A fixture that can be broken by an unrelated
        // test in the same binary is not a fixture.
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "clyde matrix")
        .env("GIT_AUTHOR_EMAIL", "matrix@example.invalid")
        .env("GIT_COMMITTER_NAME", "clyde matrix")
        .env("GIT_COMMITTER_EMAIL", "matrix@example.invalid")
        .output()
        .unwrap_or_else(|e| panic!("checkout::git: failed to spawn git {args:?} in {}: {e}", cwd.display()));
    assert!(
        out.status.success(),
        "checkout::git: git {args:?} in {} failed: {}",
        cwd.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `mkdir -p`, panicking with the path on failure.
fn mkdir(path: &Path) -> PathBuf {
    std::fs::create_dir_all(path).unwrap_or_else(|e| panic!("checkout::mkdir: {}: {e}", path.display()));
    path.to_path_buf()
}

/// A non-bare repo at `path` with one empty commit, and `origin` set to `remote` when given.
/// The commit matters: an EMPTY repo is its own matrix row (30), so every other row must have
/// history or the two shapes would be indistinguishable.
fn repo(path: &Path, remote: Option<&str>) -> PathBuf {
    mkdir(path);
    git(path, &["init", "-q", "-b", "main"]);
    git(path, &["commit", "-q", "--allow-empty", "-m", "init"]);
    if let Some(url) = remote {
        git(path, &["remote", "add", "origin", url]);
    }
    path.to_path_buf()
}

/// A bare repo at `path`, with `origin` set to `remote` when given.
fn bare(path: &Path, remote: Option<&str>) -> PathBuf {
    mkdir(path);
    git(path, &["init", "-q", "--bare", "-b", "main"]);
    if let Some(url) = remote {
        git(path, &["remote", "add", "origin", url]);
    }
    path.to_path_buf()
}

/// The shared checkout matrix. Every field is an absolute path inside one [`TempDir`], so a test
/// that mutates a row cannot reach outside the fixture and cannot see `~/repos`.
///
/// Rows the design lists that are NOT fields here are CONDITIONS rather than shapes, applied by the
/// test that needs them: the row-10 flip (`git remote add` on [`Self::no_origin`]), row 22 (delete
/// [`Self::deletable`] after indexing), row 23 (`safe.directory` refusal), rows 25 to 28 and 31 to 32
/// (catalog state, or a hostile environment). Each has a named helper on this struct where the setup
/// is more than one call.
pub struct Matrix {
    root: TempDir,
    /// Row 1: flat clone, ssh remote, under the work org. The baseline that already works.
    pub flat_ssh: PathBuf,
    /// Row 2: flat clone, https remote. `parse_slug`'s other main branch.
    pub flat_https: PathBuf,
    /// Row 3: a subdirectory of row 1. `--show-toplevel` sits above the cwd.
    pub subdir: PathBuf,
    /// Row 4: a linked worktree beside its main checkout, at org level. The v0.21.0 shape.
    pub worktree: PathBuf,
    /// Row 5: the CHILD of a bare-repo container. The row the old verification table did test.
    pub container_child: PathBuf,
    /// Row 6: the ROOT of a bare-repo container. Problem 4, the row it did NOT test.
    pub container_root: PathBuf,
    /// Row 7: a plain bare mirror. `--git-common-dir` returns `.`, which is why the design needs the
    /// `common == cwd` guard.
    pub bare_mirror: PathBuf,
    /// Row 8: a directory inside the container's `.bare`. Exercises the containment check.
    pub inside_bare: PathBuf,
    /// Row 9: a git repo with NO origin. Problem 1's seed state, and the conclusive `NoOrigin`.
    pub no_origin: PathBuf,
    /// Row 11: a plain directory that is not a git repo at all.
    pub not_a_repo: PathBuf,
    /// Row 14: `<home>/code/work/philo`. Stephen's layout: no org level.
    pub layout_code_work: PathBuf,
    /// Row 15: `<home>/Projects/philo`. Luke's layout: no org level.
    pub layout_projects: PathBuf,
    /// Row 16: `<home>/git/tatari/philo`. Keegan's layout: the org slot reads `tatari`.
    pub layout_git_tatari: PathBuf,
    /// Row 18: a PERSONAL fork of a work repo, checked out in a work directory. The case that killed
    /// the precedence change (register item 5).
    pub fork_in_work_dir: PathBuf,
    /// Row 19: a remote on a host that is not allowlisted. Problem 2.
    pub host_not_allowed: PathBuf,
    /// Row 20: a remote reached through an ssh `Host` alias. Problem 2's fix must not break this.
    pub host_ssh_alias: PathBuf,
    /// Row 21: a checkout carrying a `.gitmodules` that names a hostile remote. Problem 2's
    /// attacker-authored vector.
    pub hostile_submodule: PathBuf,
    /// Row 22: a checkout the test DELETES after indexing, to exercise rule 2's learned path map.
    pub deletable: PathBuf,
    /// Row 24: a symlink whose target is row 1. `--show-toplevel` returns the CANONICAL path, so the
    /// lexical containment check rejects it today. Confirmed live bug.
    pub symlinked: PathBuf,
    /// Row 29: a repo whose `.git/config` cannot be read. Must be `Indeterminate`, never `NotARepo`.
    pub unreadable_config: PathBuf,
    /// Row 30: an empty repo: no commits, no origin. Must be `NoOrigin` (conclusive), distinguished
    /// from a repo-discovery failure.
    pub empty_repo: PathBuf,
    /// A directory holding a `.git` FILE that points at a bare repo in a SIBLING tree, so the
    /// resolved root is not an ancestor of the cwd. The only env-var-free shape that genuinely fails
    /// the containment check; see the module docs on row 8.
    pub outside_root: PathBuf,
}

impl Matrix {
    /// Build every shared shape. One `TempDir`, torn down when the returned value drops.
    pub fn build() -> Self {
        let root = tempfile::tempdir().expect("checkout::Matrix: temp root");
        let home = mkdir(&root.path().join("home"));
        let repos = mkdir(&home.join("repos"));
        let work = mkdir(&repos.join(WORK_ORG));
        let personal = mkdir(&home.join("personal"));
        mkdir(&home.join("projects"));
        debug!("checkout::Matrix::build: home={}", home.display());

        let flat_ssh = repo(&work.join("philo"), Some("git@github.com:tatari-tv/philo.git"));
        let subdir = mkdir(&flat_ssh.join("src"));
        let flat_https = repo(
            &work.join("philo-https"),
            Some("https://github.com/tatari-tv/philo.git"),
        );

        // Row 4: a linked worktree beside its main checkout. `worktree add` needs a committed HEAD,
        // which `repo` above supplies.
        let clyde = repo(&work.join("clyde"), Some("git@github.com:tatari-tv/clyde.git"));
        let worktree = work.join("clyde-ft");
        git(
            &clyde,
            &["worktree", "add", "-q", "-b", "ft", &worktree.to_string_lossy()],
        );

        // Rows 5, 6 and 8: `git init --bare` plus branch directories, which is exactly what
        // `git init --bare` and a worktree per branch produces. `clyde/build.rs` already resolves
        // this shape for `cargo:rerun-if-changed`, so it is in-house precedent, not exotic.
        let container_root = mkdir(&work.join("dags"));
        let dot_bare = bare(
            &container_root.join(".bare"),
            Some("git@github.com:tatari-tv/airflow-dags.git"),
        );
        std::fs::write(container_root.join(".git"), "gitdir: ./.bare\n").expect("checkout: container .git file");
        // The commit lands in the CHILD, never at the container root: the root has no work tree, so
        // `git commit` there fails with the very `must be run in a work tree` error that makes this
        // row a matrix row in the first place.
        let container_child = container_root.join("main");
        git(
            &container_root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "main",
                &container_child.to_string_lossy(),
            ],
        );
        git(&container_child, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let inside_bare = dot_bare.join("refs");

        let bare_mirror = bare(&work.join("mirror.git"), Some("git@github.com:tatari-tv/mirror.git"));

        let no_origin = repo(&personal.join("noorigin"), None);
        let not_a_repo = mkdir(&personal.join("plain"));
        let empty_repo = mkdir(&personal.join("empty"));
        git(&empty_repo, &["init", "-q", "-b", "main"]);

        // Rows 14 to 16: the three teammate layouts with no readable org slot. Real checkouts with
        // real `tatari-tv` origins, and NONE of them under `repo-root`.
        let layout_code_work = repo(
            &home.join("code").join("work").join("philo"),
            Some("git@github.com:tatari-tv/philo.git"),
        );
        let layout_projects = repo(
            &home.join("Projects").join("philo"),
            Some("git@github.com:tatari-tv/philo.git"),
        );
        let layout_git_tatari = repo(
            &home.join("git").join("tatari").join("philo"),
            Some("git@github.com:tatari-tv/philo.git"),
        );

        let fork_in_work_dir = repo(
            &work.join("clyde-fork"),
            Some("git@github.com:scottidler/clyde-fork.git"),
        );
        let host_not_allowed = repo(&work.join("elsewhere"), Some("git@evil.example.com:tatari-tv/x.git"));
        let host_ssh_alias = repo(&work.join("aliased"), Some("git@github-work:tatari-tv/x.git"));

        // Row 21: `.gitmodules` is attacker-authored content in a third-party clone, and v0.22.0
        // newly feeds a remote-derived string to `is_work_slug`. The file is what matters; no
        // `submodule add` is needed to make the content reachable.
        let hostile_submodule = repo(&work.join("withsub"), Some("git@github.com:tatari-tv/withsub.git"));
        std::fs::write(
            hostile_submodule.join(".gitmodules"),
            "[submodule \"vendor\"]\n\tpath = vendor\n\turl = git@evil.example.com:tatari-tv/x.git\n",
        )
        .expect("checkout: hostile .gitmodules");

        let deletable = repo(&work.join("ephemeral"), Some("git@github.com:tatari-tv/ephemeral.git"));

        // Row 24: a symlink to row 1. `--show-toplevel` canonicalizes, so the lexical
        // `cwd.starts_with(&toplevel)` in `detect_with_blocked_roots` fails and rule 1 declines for
        // every symlink-reached session. Confirmed live, 2026-07-31.
        let symlinked = home.join("philo-link");
        symlink(&flat_ssh, &symlinked);

        let unreadable_config = repo(&personal.join("locked"), Some("git@github.com:scottidler/locked.git"));

        // The containment-decline shape. A `.git` FILE pointing at a bare repo in a SIBLING tree, so
        // the computed root (`<sibling>`) is not an ancestor of the cwd (`<x>/pointer`). Measured
        // 2026-07-31: this is the only shape without an environment variable that makes the check
        // bite; every in-tree cwd resolves to an ancestor by construction, because git's discovery
        // walks UP.
        let sibling = bare(
            &home.join("sibling").join("detached.git"),
            Some("git@github.com:tatari-tv/detached.git"),
        );
        let outside_root = mkdir(&home.join("x").join("pointer"));
        std::fs::write(outside_root.join(".git"), format!("gitdir: {}\n", sibling.display()))
            .expect("checkout: pointer .git file");

        Self {
            root,
            flat_ssh,
            flat_https,
            subdir,
            worktree,
            container_child,
            container_root,
            bare_mirror,
            inside_bare,
            no_origin,
            not_a_repo,
            layout_code_work,
            layout_projects,
            layout_git_tatari,
            fork_in_work_dir,
            host_not_allowed,
            host_ssh_alias,
            hostile_submodule,
            deletable,
            symlinked,
            unreadable_config,
            empty_repo,
            outside_root,
        }
    }

    /// The fixture's `HOME`. Rule 1's blocked-root set is `[$HOME]`, so a test that wants the guard
    /// exercised passes this rather than the real home directory.
    pub fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }

    /// The fixture's `repo-root` (rule 4's `<repo-root>/<org>/<repo>` anchor).
    pub fn repo_root(&self) -> PathBuf {
        self.home().join("repos")
    }

    /// The fixture's `projects-dir`. Empty until a test writes a transcript into it.
    pub fn projects_dir(&self) -> PathBuf {
        self.home().join("projects")
    }

    /// The blocked-root set rule 1 must be given for this fixture: exactly `[$HOME]`, mirroring
    /// production's `home_dir_as_blocked`.
    pub fn blocked(&self) -> Vec<PathBuf> {
        vec![self.home()]
    }

    /// Row 10: the flip. Give [`Self::no_origin`] an origin, exactly as `git remote add origin` or
    /// `gh repo create --source=.` would. The seed state classified `personal`; after this a live
    /// probe succeeds and returns a WORK slug.
    pub fn add_origin_to_no_origin(&self) {
        git(
            &self.no_origin,
            &["remote", "add", "origin", "git@github.com:tatari-tv/side-project.git"],
        );
    }

    /// Row 13: a bare repo AT the fixture's `$HOME`. Built on demand rather than in
    /// [`Self::build`], because making `$HOME` itself a repo changes what every OTHER row's git
    /// discovery finds when it walks up.
    pub fn make_home_a_bare_repo(&self) {
        bare(&self.home(), Some("git@github.com:scottidler/dotfiles.git"));
    }

    /// Row 12: `$HOME` itself a non-bare git repo. Same on-demand reasoning as
    /// [`Self::make_home_a_bare_repo`].
    pub fn make_home_a_repo(&self) {
        repo(&self.home(), Some("git@github.com:scottidler/dotfiles.git"));
    }

    /// Row 29: make [`Self::unreadable_config`]'s `.git/config` unreadable, so git fails at the
    /// origin read for a reason that is NOT "there is no origin". Returns `false` when the platform
    /// cannot express it (running as root, where mode 0 is still readable), so the caller skips
    /// rather than asserting a condition it failed to create.
    pub fn make_config_unreadable(&self) -> bool {
        let config = self.unreadable_config.join(".git").join("config");
        set_unreadable(&config);
        std::fs::read_to_string(&config).is_err()
    }

    /// Row 32: a hostile `~/.gitconfig` inside the fixture home, setting `remote.origin.url`. With
    /// `HOME` forwarded and a plain `--get`, this turns [`Self::no_origin`] (which must produce the
    /// conclusive `NoOrigin`) into a forged work `Resolved`. Returns the path written.
    pub fn write_hostile_gitconfig(&self) -> PathBuf {
        let path = self.home().join(".gitconfig");
        std::fs::write(
            &path,
            "[remote \"origin\"]\n\turl = git@github.com:tatari-tv/forged.git\n",
        )
        .expect("checkout: hostile .gitconfig");
        path
    }
}

/// Create a symlink at `link` pointing to `target`. Unix only, which is every platform clyde's CI
/// and its users run on; a Windows build would need `std::os::windows::fs::symlink_dir` and a
/// developer-mode privilege, and no such host exists for this project.
#[cfg(unix)]
fn symlink(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link)
        .unwrap_or_else(|e| panic!("checkout::symlink: {} -> {}: {e}", link.display(), target.display()));
}

/// Strip every read bit from `path`.
#[cfg(unix)]
fn set_unreadable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o000);
    std::fs::set_permissions(path, perms)
        .unwrap_or_else(|e| panic!("checkout::set_unreadable: {}: {e}", path.display()));
}

#[cfg(test)]
mod tests;
