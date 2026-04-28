use log::{debug, trace};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn detect(cwd: &Path) -> Option<String> {
    let blocked = home_dir_as_blocked();
    detect_with_blocked_roots(cwd, &blocked)
}

#[derive(Debug, Default)]
pub struct Resolver {
    cache: HashMap<PathBuf, Option<String>>,
    blocked: Vec<PathBuf>,
}

impl Resolver {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            blocked: home_dir_as_blocked(),
        }
    }

    pub fn detect(&mut self, cwd: &Path) -> Option<String> {
        if let Some(cached) = self.cache.get(cwd) {
            return cached.clone();
        }
        let result = detect_with_blocked_roots(cwd, &self.blocked);
        self.cache.insert(cwd.to_path_buf(), result.clone());
        result
    }
}

fn home_dir_as_blocked() -> Vec<PathBuf> {
    dirs::home_dir().map(|h| vec![h]).unwrap_or_default()
}

pub fn detect_with_blocked_roots(cwd: &Path, blocked: &[PathBuf]) -> Option<String> {
    trace!("repo::detect: cwd={}", cwd.display());

    if !cwd.exists() {
        debug!("repo::detect: cwd missing on disk: {}", cwd.display());
        return None;
    }

    let toplevel = run_git(cwd, &["rev-parse", "--show-toplevel"])?;
    let toplevel = PathBuf::from(toplevel.trim());

    if !(toplevel == cwd || cwd.starts_with(&toplevel)) {
        debug!(
            "repo::detect: toplevel {} is not at or above cwd {}; rejecting",
            toplevel.display(),
            cwd.display()
        );
        return None;
    }

    if blocked.iter().any(|b| b == &toplevel) {
        debug!(
            "repo::detect: toplevel {} matches a blocked root (e.g. $HOME); rejecting",
            toplevel.display()
        );
        return None;
    }

    let origin = run_git(cwd, &["remote", "get-url", "origin"])?;
    parse_slug(origin.trim())
}

fn run_git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(cwd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn parse_slug(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    let path = if let Some(rest) = url.strip_prefix("git@") {
        let (_, path) = rest.split_once(':')?;
        path.to_string()
    } else if let Some(rest) = url.strip_prefix("https://") {
        let (_, path) = rest.split_once('/')?;
        path.to_string()
    } else if let Some(rest) = url.strip_prefix("http://") {
        let (_, path) = rest.split_once('/')?;
        path.to_string()
    } else if let Some(rest) = url.strip_prefix("git://") {
        let (_, path) = rest.split_once('/')?;
        path.to_string()
    } else if let Some(rest) = url.strip_prefix("ssh://") {
        let after_user = rest.split_once('@').map(|(_, r)| r).unwrap_or(rest);
        let (_, path) = after_user.split_once('/')?;
        path.to_string()
    } else {
        return None;
    };

    let path = path.strip_suffix(".git").unwrap_or(&path);
    let (org, repo) = path.split_once('/')?;
    if org.is_empty() || repo.is_empty() {
        return None;
    }
    if repo.contains('/') {
        return None;
    }
    Some(format!("{}/{}", org, repo))
}

#[cfg(test)]
mod tests;
