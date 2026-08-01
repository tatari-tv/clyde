//! The ONE resolution of the Claude projects root, shared by every command that needs it.
//!
//! Register item 8, and the register understated it: before this module there were THREE paths and
//! three different answers.
//!
//! | path | resolved from | honored config? |
//! |---|---|---|
//! | `cmd_reindex` | `--projects-dir`, else the platform default | no |
//! | `lazy_reindex` | the platform default only | no |
//! | `mcp serve` | `cfg.projects_dir()` | yes |
//!
//! So an operator who set `projects-dir` in `clyde.yml` and ran `reindex` or `enrich` got the real
//! `~/.claude/projects` instead, which is how the register's own sandbox testing corrupted a live
//! catalog. Three callers agreeing is the fix; ONE function they all call is what stops a fourth
//! caller from diverging again.
//!
//! Precedence is the house standard (`rules/general.md`): CLI flag, then config file, then default.

use std::path::{Path, PathBuf};

use common::Config;
use eyre::{Result, eyre};
use log::debug;

/// Resolve the projects root: the `--projects-dir` flag, else `clyde.yml`'s `projects-dir`, else the
/// platform default `~/.claude/projects`.
///
/// Errors only in the environment where the default itself cannot be computed (no `$HOME`), which is
/// the one case where there is genuinely no answer. Fail LOUD there rather than falling back to a
/// relative path that would silently scan the current directory.
pub fn resolve(flag: Option<&Path>, cfg: &Config) -> Result<PathBuf> {
    if let Some(path) = flag {
        debug!("projects::resolve: {} (from --projects-dir)", path.display());
        return Ok(path.to_path_buf());
    }
    if let Some(path) = cfg.configured_projects_dir() {
        debug!("projects::resolve: {} (from clyde.yml projects-dir)", path.display());
        return Ok(path.to_path_buf());
    }
    let path = session::paths::claude_projects_dir()
        .ok_or_else(|| eyre!("could not determine ~/.claude/projects (set HOME, or set projects-dir in clyde.yml)"))?;
    debug!("projects::resolve: {} (platform default)", path.display());
    Ok(path)
}

#[cfg(test)]
mod tests;
