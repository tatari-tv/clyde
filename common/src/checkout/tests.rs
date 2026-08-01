//! Ground-truth assertions on the fixture itself: what real `git` returns for each shape.
//!
//! These are NOT tests of clyde. They pin the git behavior the whole design rests on, so a git
//! upgrade that changes `--git-common-dir`'s answer for a plain bare repo fails CI here, next to the
//! measurement, rather than three phases downstream as a mysterious attribution regression.
//!
//! Measured against git 2.53.0, 2026-07-31. Every expectation below was OBSERVED before it was
//! written; none was predicted.

use super::*;

/// Run a probe against the fixture and return `(rc, trimmed stdout)`.
fn probe(cwd: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("probe: spawn git");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    )
}

fn toplevel(cwd: &Path) -> (i32, String) {
    probe(cwd, &["rev-parse", "--show-toplevel"])
}

fn common_dir(cwd: &Path) -> (i32, String) {
    probe(cwd, &["rev-parse", "--git-common-dir"])
}

fn origin(cwd: &Path) -> (i32, String) {
    probe(cwd, &["config", "--local", "--get", "remote.origin.url"])
}

#[test]
fn the_fixture_builds_every_shape_it_advertises() {
    let m = Matrix::build();
    for (label, path) in [
        ("flat_ssh", &m.flat_ssh),
        ("flat_https", &m.flat_https),
        ("subdir", &m.subdir),
        ("worktree", &m.worktree),
        ("container_child", &m.container_child),
        ("container_root", &m.container_root),
        ("bare_mirror", &m.bare_mirror),
        ("inside_bare", &m.inside_bare),
        ("no_origin", &m.no_origin),
        ("not_a_repo", &m.not_a_repo),
        ("layout_code_work", &m.layout_code_work),
        ("layout_projects", &m.layout_projects),
        ("layout_git_tatari", &m.layout_git_tatari),
        ("fork_in_work_dir", &m.fork_in_work_dir),
        ("host_not_allowed", &m.host_not_allowed),
        ("host_ssh_alias", &m.host_ssh_alias),
        ("hostile_submodule", &m.hostile_submodule),
        ("deletable", &m.deletable),
        ("symlinked", &m.symlinked),
        ("unreadable_config", &m.unreadable_config),
        ("empty_repo", &m.empty_repo),
        ("outside_root", &m.outside_root),
    ] {
        assert!(path.exists(), "matrix row {label} was not built at {}", path.display());
    }
}

/// The row-6 measurement that changed the design: a container root has NO work tree, so
/// `--show-toplevel` fails, while the origin read succeeds. Rule 1 gives up between the two today.
#[test]
fn a_container_root_has_no_work_tree_but_does_have_an_origin() {
    let m = Matrix::build();
    let (tl_rc, _) = toplevel(&m.container_root);
    assert_ne!(tl_rc, 0, "--show-toplevel must FAIL at a bare-repo container root");

    let (cd_rc, cd) = common_dir(&m.container_root);
    assert_eq!(cd_rc, 0);
    assert_eq!(
        Path::new(&cd).file_name().and_then(|n| n.to_str()),
        Some(".bare"),
        "--git-common-dir at a container root names the .bare dir, got {cd}"
    );

    let (o_rc, o) = origin(&m.container_root);
    assert_eq!(o_rc, 0, "the origin read must succeed where --show-toplevel failed");
    assert_eq!(o, "git@github.com:tatari-tv/airflow-dags.git");
}

/// The measurement that AMENDED the reported fix. A plain bare repo returns `.` for its common dir,
/// so `cwd.join(common).parent()` walks one level too high and would root a bare repo at `$HOME` at
/// `$HOME`'s PARENT, bypassing the blocked-root guard entirely.
#[test]
fn a_plain_bare_repo_reports_a_dot_for_its_common_dir() {
    let m = Matrix::build();
    let (rc, cd) = common_dir(&m.bare_mirror);
    assert_eq!(rc, 0);
    assert_eq!(cd, ".", "a plain bare repo's --git-common-dir is `.`, got {cd}");
    // `Path` equality compares components, so the guard is one comparison.
    assert_eq!(m.bare_mirror.join(&cd), m.bare_mirror);
}

/// The measurement that amended the amendment. `common == cwd` ALSO holds for a cwd sitting in a
/// NON-bare repo's `.git` directory, where rooting at the cwd would put the root at `<repo>/.git`
/// and a repo at `$HOME` would slip past the `b == root` blocked check. `--is-bare-repository` is
/// what tells the two apart.
#[test]
fn a_dot_common_dir_means_bare_only_when_git_says_it_is_bare() {
    let m = Matrix::build();
    let git_dir = m.flat_ssh.join(".git");

    let (rc, cd) = common_dir(&git_dir);
    assert_eq!(rc, 0);
    assert_eq!(cd, ".", "a cwd inside a non-bare repo's .git also reports `.`");

    let (_, bare_here) = probe(&git_dir, &["rev-parse", "--is-bare-repository"]);
    assert_eq!(
        bare_here, "false",
        "the repo is NOT bare, so the root is the cwd's parent"
    );

    let (_, bare_mirror) = probe(&m.bare_mirror, &["rev-parse", "--is-bare-repository"]);
    assert_eq!(bare_mirror, "true", "the mirror IS bare, so the root is the cwd itself");
}

/// A repo with no origin answers CONCLUSIVELY: git ran, the key is absent, rc=1. That is evidence.
/// Any other non-zero code at this stage means git did not answer the question asked.
#[test]
fn a_repo_with_no_origin_exits_one_not_a_hundred_and_twenty_eight() {
    let m = Matrix::build();
    let (rc, out) = origin(&m.no_origin);
    assert_eq!(rc, 1, "no-origin must be rc=1 (conclusive), got rc={rc} out={out:?}");
    assert!(out.is_empty());
}

/// Row 30. An empty repo (no commits) is still a repo with no origin, and must be indistinguishable
/// from row 9 at the origin read: `NoOrigin`, conclusive. It must NOT read as a discovery failure.
#[test]
fn an_empty_repo_with_no_commits_is_still_a_conclusive_no_origin() {
    let m = Matrix::build();
    let (tl_rc, _) = toplevel(&m.empty_repo);
    assert_eq!(tl_rc, 0, "an empty repo still has a work tree");
    let (rc, _) = origin(&m.empty_repo);
    assert_eq!(rc, 1, "an empty repo's missing origin is conclusive, not an anomaly");
}

/// Row 11. A directory that is not a repo fails the `--local` read with git's own 128, which is why
/// the design refuses to collapse 128 into `NotARepo` at the ORIGIN-read stage: by then `rev-parse`
/// has already established the cwd IS a repo, so a 128 there is an anomaly.
#[test]
fn a_non_repo_directory_fails_the_local_read_with_a_hundred_and_twenty_eight() {
    let m = Matrix::build();
    let (rc, _) = origin(&m.not_a_repo);
    assert_eq!(rc, 128, "a non-repo cwd is git's own 128");
}

/// Row 24, the confirmed live bug. `--show-toplevel` returns the CANONICAL path, so a lexical
/// `cwd.starts_with(&toplevel)` against the symlink path fails and rule 1 declines silently.
#[test]
fn a_symlinked_cwd_reports_its_canonical_toplevel() {
    let m = Matrix::build();
    let (rc, tl) = toplevel(&m.symlinked);
    assert_eq!(rc, 0);
    assert_ne!(
        Path::new(&tl),
        m.symlinked.as_path(),
        "the toplevel is canonical, NOT the symlink path: that is the whole bug"
    );
    assert_eq!(
        std::fs::canonicalize(&m.symlinked).expect("canonicalize the link"),
        std::fs::canonicalize(&tl).expect("canonicalize the toplevel"),
        "both sides canonicalize to the same real path, which is the fix"
    );
}

/// The containment-decline shape. Every in-tree cwd resolves to an ANCESTOR by construction (git
/// discovery walks up), so the only env-var-free way to make the containment check bite is a `.git`
/// pointer into a sibling tree.
#[test]
fn a_gitdir_pointer_into_a_sibling_tree_resolves_outside_the_cwd() {
    let m = Matrix::build();
    let (tl_rc, _) = toplevel(&m.outside_root);
    assert_ne!(tl_rc, 0, "the pointed-at repo is bare, so there is no work tree");
    let (rc, cd) = common_dir(&m.outside_root);
    assert_eq!(rc, 0);
    let root = Path::new(&cd).parent().expect("the common dir has a parent");
    assert!(
        !m.outside_root.starts_with(root),
        "the computed root {} must NOT be an ancestor of the cwd {}",
        root.display(),
        m.outside_root.display()
    );
}

/// Row 10 is a SEQUENCE, and this pins its two halves at the git level: the same cwd answers
/// conclusively-no-origin, then answers with a WORK slug, with nothing about the session changed.
#[test]
fn adding_an_origin_flips_the_same_cwd_from_conclusive_to_resolved() {
    let m = Matrix::build();
    assert_eq!(origin(&m.no_origin).0, 1, "seed state: conclusively no origin");
    m.add_origin_to_no_origin();
    let (rc, url) = origin(&m.no_origin);
    assert_eq!(rc, 0, "one `git remote add` later, the probe succeeds");
    assert_eq!(url, "git@github.com:tatari-tv/side-project.git");
}
