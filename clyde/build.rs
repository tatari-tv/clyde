// Canonical fleet build.rs: emits GIT_DESCRIBE for the crate's `--version`.
//
// Resolves the real git directory via `git rev-parse` instead of assuming a `.git/`
// sits next to Cargo.toml, so `cargo:rerun-if-changed` is correct for:
//   - a regular single-crate repo (`.git/` at the crate root),
//   - a workspace member (`.git/` at the workspace root, above the crate), and
//   - a git worktree, including bare-container worktrees (`.git` is a gitdir file).
// Falls back to CARGO_PKG_VERSION when git is unavailable (e.g. a source tarball).
use std::process::Command;

/// Run `git` with the given args; return trimmed stdout, or None if git is absent,
/// the command failed, or the output was empty.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn main() {
    // Precedence: an explicit GIT_DESCRIBE env (CI pinning, or forcing a version in
    // tests) wins; else `git describe`; else the Cargo.toml version.
    println!("cargo:rerun-if-env-changed=GIT_DESCRIBE");
    let describe = std::env::var("GIT_DESCRIBE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| git(&["describe", "--tags", "--always", "--dirty"]))
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    println!("cargo:rustc-env=GIT_DESCRIBE={describe}");

    // Rebuild when HEAD moves or tags change. HEAD lives in the (per-worktree) git
    // dir; refs and packed-refs live in the common dir. cargo resolves a relative
    // rerun path against this crate's manifest dir (= the build script's CWD), the
    // same base `git rev-parse` prints relative paths against - so both a plain
    // `.git` and an absolute worktree gitdir resolve correctly.
    if let Some(git_dir) = git(&["rev-parse", "--git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
    }
    if let Some(common_dir) = git(&["rev-parse", "--git-common-dir"]) {
        println!("cargo:rerun-if-changed={common_dir}/refs");
        println!("cargo:rerun-if-changed={common_dir}/packed-refs");
    }
}
