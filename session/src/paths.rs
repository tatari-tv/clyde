//! Path resolution for klod. Single source of truth; hardcoded paths or `dirs::config_dir()`
//! / `dirs::data_local_dir()` anywhere else are a code-review reject.
//!
//! `dirs::config_dir()` / `dirs::data_local_dir()` honor `$XDG_*_HOME` only on Linux; on macOS
//! they return `~/Library/...`, ignoring the env vars. The helpers here resolve to the XDG
//! layout on every platform. `dirs::home_dir()` is fine (correct everywhere) and the helpers
//! are built on it.
//!
//! On-disk layout (resolved at runtime):
//!
//! ```text
//! $XDG_DATA_HOME/klod/        # ~/.local/share/klod/   — authoritative
//!     sessions.db             #   the navigational index (integration contract)
//!     reports/                #   cr output lands here (Phase 4)
//!     staged/                 #   durable transcript copies (Phase 1.5)
//! $XDG_CONFIG_HOME/klod/      # ~/.config/klod/         — shared config
//! $XDG_CACHE_HOME/klod/       # ~/.cache/klod/          — regenerable caches (rm-safe)
//! ```

use std::path::PathBuf;

/// Subdirectory name under each XDG root that owns klod's data, config, and cache.
pub const KLOD_DIR: &str = "klod";

/// XDG config dir, honoring `$XDG_CONFIG_HOME` and falling back to `$HOME/.config`.
pub fn xdg_config_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        let path = PathBuf::from(dir);
        if path.is_absolute() {
            return Some(path);
        }
    }
    dirs::home_dir().map(|h| h.join(".config"))
}

/// XDG data dir, honoring `$XDG_DATA_HOME` and falling back to `$HOME/.local/share`.
pub fn xdg_data_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        let path = PathBuf::from(dir);
        if path.is_absolute() {
            return Some(path);
        }
    }
    dirs::home_dir().map(|h| h.join(".local").join("share"))
}

/// XDG cache dir, honoring `$XDG_CACHE_HOME` and falling back to `$HOME/.cache`.
pub fn xdg_cache_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_CACHE_HOME") {
        let path = PathBuf::from(dir);
        if path.is_absolute() {
            return Some(path);
        }
    }
    dirs::home_dir().map(|h| h.join(".cache"))
}

/// `~/.local/share/klod/` (XDG on every platform).
///
/// Panics only if `xdg_data_dir()` returns `None`, which means both `$HOME` and
/// `$XDG_DATA_HOME` are unset - a broken environment where nothing else in klod works either.
/// We never fabricate a `~/`-prefixed fallback: a literal `~` is not expanded by the OS and
/// would create a directory named `~` under CWD.
pub fn data_root() -> PathBuf {
    xdg_data_dir()
        .expect("xdg_data_dir() returned None (set HOME or XDG_DATA_HOME)")
        .join(KLOD_DIR)
}

/// `~/.config/klod/`. Panics only when `xdg_config_dir()` returns `None` (see [`data_root`]).
pub fn config_root() -> PathBuf {
    xdg_config_dir()
        .expect("xdg_config_dir() returned None (set HOME or XDG_CONFIG_HOME)")
        .join(KLOD_DIR)
}

/// `~/.cache/klod/`. Panics only when `xdg_cache_dir()` returns `None` (see [`data_root`]).
pub fn cache_root() -> PathBuf {
    xdg_cache_dir()
        .expect("xdg_cache_dir() returned None (set HOME or XDG_CACHE_HOME)")
        .join(KLOD_DIR)
}

/// The navigational index DB. THE integration contract between klod subcommands.
pub fn sessions_db_path() -> PathBuf {
    data_root().join("sessions.db")
}

/// Where `cr` reports land once `cr` migrates into klod (Phase 4).
pub fn reports_dir() -> PathBuf {
    data_root().join("reports")
}

/// Where Phase 1.5 stages durable copies of transcripts to beat the 30-day TTL.
pub fn staged_dir() -> PathBuf {
    data_root().join("staged")
}

/// The Claude-owned session transcript root: `~/.claude/projects`. `dirs::home_dir()` is
/// correct on every platform; this is not a klod-owned path so it is not under the XDG namespace.
pub fn claude_projects_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

#[cfg(test)]
mod tests;
