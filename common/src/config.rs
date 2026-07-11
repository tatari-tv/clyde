//! The top-level `clyde.yml` config loader.
//!
//! This is the FIRST file-backed config clyde reads (nothing read `clyde.yml` before). It is
//! intentionally minimal: one field today (`date-tz`), strict schema (`deny_unknown_fields`), and
//! a missing file is NOT an error — it yields defaults. The CLI layer loads this once and threads
//! the resolved [`DateTz`](crate::DateTz) into [`parse_since`](crate::parse_since), keeping the
//! parser pure.

use std::path::PathBuf;

use eyre::{Context, Result};
use serde::Deserialize;

use crate::since::DateTz;

/// Project name, used to resolve `~/.config/<project>/<project>.yml`.
const PROJECT: &str = "clyde";

/// The serde view of `date-tz: utc | local`. Kept separate from [`DateTz`] (which is a pure
/// parser input with no serde derives) so the config schema and the parser type stay decoupled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum DateTzConfig {
    #[default]
    Utc,
    Local,
}

impl From<DateTzConfig> for DateTz {
    fn from(value: DateTzConfig) -> Self {
        match value {
            DateTzConfig::Utc => DateTz::Utc,
            DateTzConfig::Local => DateTz::Local,
        }
    }
}

/// The serde view of `render.format`: the default output format for `report render` when
/// `--format` is omitted. Mirrors the `report` crate's `cli::Format` variants (kebab-case), but
/// lives here because `common` cannot depend on `report`; the mapping to `cli::Format` is done in
/// `report`. Defaults to `markdown`, matching the built-in default, so an absent config is a no-op.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FormatConfig {
    #[default]
    Markdown,
    Pdf,
    Html,
    MarqueeHtml,
    MarqueeMarkdown,
}

/// The `render:` section of `clyde.yml`: defaults for `report render`. Every field defaults, so an
/// absent section is all-defaults.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct RenderConfig {
    /// Default output format when `--format` is omitted on the command line. Defaults to markdown.
    #[serde(default)]
    format: FormatConfig,
}

/// The parsed `clyde.yml`. Every field defaults, so an absent file deserializes to all-defaults.
///
/// `Default` is hand-written (NOT derived): `reindex_on_start` defaults to `true`, but a derived
/// `Default` would give the `bool` zero value `false` and diverge from the serde default a missing
/// file resolves to. Hand-writing keeps `Config::default()` and a from-scratch deserialize in lock
/// step.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// How a bare `YYYY-MM-DD` date's midnight is interpreted by `--since`. Defaults to `utc`.
    #[serde(default)]
    date_tz: DateTzConfig,
    /// Defaults for `report render` (currently just the output format).
    #[serde(default)]
    render: RenderConfig,
    /// The Claude session transcript root `clyde mcp serve` reindexes and reads. Absent -> the
    /// platform default `~/.claude/projects`. A `clyde mcp serve` is spawned by an MCP host with
    /// fixed args (`mcp serve`, no flags reachable), so this config field is the only way to point
    /// it elsewhere; it replaces the old `session serve --projects-dir` flag.
    #[serde(default)]
    projects_dir: Option<PathBuf>,
    /// Whether `clyde mcp serve` runs a one-shot incremental reindex at startup (default `true`),
    /// so today's sessions are findable. Set `false` to serve a possibly-stale catalog and skip the
    /// startup scan (e.g. a very large catalog whose reindex would delay the MCP handshake). It
    /// replaces the old `session serve --no-reindex` flag.
    #[serde(default = "default_reindex_on_start")]
    reindex_on_start: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            date_tz: DateTzConfig::default(),
            render: RenderConfig::default(),
            projects_dir: None,
            reindex_on_start: default_reindex_on_start(),
        }
    }
}

/// The serde default for `reindex-on-start`: on. A one-shot startup reindex keeps the served
/// catalog current for the common case.
fn default_reindex_on_start() -> bool {
    true
}

/// The platform default projects root when `projects-dir` is unset: `~/.claude/projects`.
/// `dirs::home_dir()` is correct on every platform, and this is a Claude-owned path (not a clyde
/// XDG path). Mirrors `session::paths::claude_projects_dir`; `common` cannot depend on the
/// `session` crate, so the fallback is inlined here.
fn default_projects_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".claude").join("projects"))
        .unwrap_or_else(|| PathBuf::from(".claude").join("projects"))
}

impl Config {
    /// The bare-date timezone interpretation for `--since`, as the pure-parser type.
    pub fn date_tz(&self) -> DateTz {
        self.date_tz.into()
    }

    /// The configured default output format for `report render` (`markdown` when unset).
    pub fn render_format(&self) -> FormatConfig {
        self.render.format
    }

    /// The resolved projects root for `clyde mcp serve`: the configured `projects-dir`, else the
    /// platform default `~/.claude/projects`.
    pub fn projects_dir(&self) -> PathBuf {
        self.projects_dir.clone().unwrap_or_else(default_projects_dir)
    }

    /// Whether `clyde mcp serve` runs a one-shot incremental reindex at startup (default `true`).
    pub fn reindex_on_start(&self) -> bool {
        self.reindex_on_start
    }
}

/// Load `clyde.yml` from the XDG config dir, falling back to defaults when the file is absent.
///
/// Resolution: `$XDG_CONFIG_HOME/clyde/clyde.yml`, else `$HOME/.config/clyde/clyde.yml`. A missing
/// file is the common case and is NOT an error. An *unreadable* or *malformed* file IS an error
/// (a typo'd key, thanks to `deny_unknown_fields`, surfaces loudly rather than silently widening
/// behavior).
pub fn load() -> Result<Config> {
    match config_path() {
        Some(path) => load_from(&path),
        // No HOME and no XDG_CONFIG_HOME: nothing to read, use defaults.
        None => Ok(Config::default()),
    }
}

/// Load from an explicit path. A nonexistent path yields defaults; any other error propagates.
fn load_from(path: &std::path::Path) -> Result<Config> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => return Err(e).with_context(|| format!("failed to read config {}", path.display())),
    };
    serde_yaml::from_str(&text).with_context(|| format!("failed to parse config {}", path.display()))
}

/// Path to `clyde.yml`: `<xdg-config>/clyde/clyde.yml`.
fn config_path() -> Option<PathBuf> {
    xdg_config_dir().map(|d| d.join(PROJECT).join(format!("{PROJECT}.yml")))
}

/// XDG config dir, honoring `$XDG_CONFIG_HOME` and falling back to `$HOME/.config`.
///
/// We deliberately do NOT use `dirs::config_dir()`: it honors `$XDG_CONFIG_HOME` only on Linux and
/// returns `~/Library/Application Support` on macOS, so config a user drops in `~/.config` would be
/// silently never found there. This resolves to the same XDG layout on every platform.
fn xdg_config_dir() -> Option<PathBuf> {
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
