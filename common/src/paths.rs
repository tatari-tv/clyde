//! THE definition of clyde's XDG data root.
//!
//! `xdg_data_dir` had five independent implementations across five crates (`session::paths`,
//! `cost::config`, `permit::config`, `report::config`, and a private copy in `common::scan`), each with
//! its own copy of the same eight lines and the same rationale comment. Five bodies resolving one env
//! var is five chances for one of them to drift, and the drift would be silent: each crate would keep
//! working while naming a different directory than its siblings.
//!
//! `common` is the workspace's lowest crate -- `permit`, `report`, `cost`, and `session` all already
//! depend on it -- so it is the only place all five can reach. The four public wrappers are KEPT as
//! delegations rather than deleted, so no caller changes; this file holds the one body that actually
//! reads the environment.
//!
//! The precedent is `session::paths::staged_dir`, which already delegates to
//! `common::scan::default_staged_dir` for exactly this reason.

use std::path::PathBuf;

/// XDG data dir, honoring `$XDG_DATA_HOME` and falling back to `$HOME/.local/share`.
///
/// `dirs::data_local_dir()` is deliberately NOT used: it honors `$XDG_DATA_HOME` only on Linux, and on
/// macOS resolves via system APIs to `~/Library/Application Support`, ignoring the env var. A tool that
/// advertises `~/.local/share/clyde/...` in `--help` would be lying on a Mac, and config dropped in
/// `~/.config` would silently never be found. This resolves to the same XDG layout on every platform.
///
/// A relative `$XDG_DATA_HOME` is IGNORED rather than honored, per the XDG spec: a relative data root
/// would resolve differently for every process depending on its cwd.
///
/// `None` only when both `$XDG_DATA_HOME` and `$HOME` are unset, an environment in which nothing in the
/// tool works. Callers decide whether that is a panic-with-a-clear-message or a default.
pub fn xdg_data_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        let path = PathBuf::from(dir);
        if path.is_absolute() {
            return Some(path);
        }
    }
    dirs::home_dir().map(|h| h.join(".local").join("share"))
}

/// XDG config dir, honoring `$XDG_CONFIG_HOME` and falling back to `$HOME/.config`.
///
/// The sibling of [`xdg_data_dir`], and it had the SAME five-copy problem for the same reason: the
/// consolidation above moved the data root here and left the config root behind, so four crates
/// (`permit::config`, `common::config`, `cost::config`, `session::paths`) each kept resolving
/// `$XDG_CONFIG_HOME` from their own byte-identical body. `pricing` keeps a private copy on purpose:
/// it is a `[lib]`-only crate consumed externally by git tag and deliberately does not depend on
/// `common`.
///
/// `dirs::config_dir()` is deliberately NOT used, for the reason [`xdg_data_dir`] gives about
/// `dirs::data_local_dir()`: it honors `$XDG_CONFIG_HOME` only on Linux, and on macOS resolves to
/// `~/Library/Application Support`, so config dropped in `~/.config` would silently never be found.
///
/// A relative `$XDG_CONFIG_HOME` is IGNORED rather than honored, per the XDG spec.
pub fn xdg_config_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        let path = PathBuf::from(dir);
        if path.is_absolute() {
            return Some(path);
        }
    }
    dirs::home_dir().map(|h| h.join(".config"))
}

#[cfg(test)]
mod tests;
