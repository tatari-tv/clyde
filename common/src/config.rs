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

/// The `efficiency:` section of `clyde.yml`: the thresholds `clyde efficiency` scores a session's
/// aggregate signals against (design "Config", `docs/design/2026-07-22-session-efficiency-signals.md`).
///
/// These define the *what* (where the line sits), never the *whether* (that scoping is CLI-flag
/// scope, per the house config rule). Every field defaults, so an absent `efficiency:` section is
/// all-defaults. `Default` is HAND-WRITTEN (not derived): the numeric thresholds have meaningful
/// non-zero defaults, and a derived `Default` would silently substitute the type's zero value
/// (`0.0` floor / `0.0` ceiling / `false` flag / `0` gates) — a floor of 0.0 flags nothing and a
/// ceiling of 0.0 flags everything, both of which diverge from what a missing file must resolve to.
/// Hand-writing keeps `EfficiencyConfig::default()` and a from-scratch deserialize in lock step.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct EfficiencyConfig {
    /// `cache-read-share` below this fraction flags the session as cache-wasteful — but ONLY when
    /// the session is eligible (see the two gates below). Default `0.6`.
    #[serde(default = "default_cache_read_share_floor", deserialize_with = "de_fraction")]
    cache_read_share_floor: f64,
    /// `tool-error rate` (`tool_errors / tool_calls`) above this fraction flags the session as
    /// error-prone. Default `0.05`.
    #[serde(default = "default_tool_error_rate_ceiling", deserialize_with = "de_fraction")]
    tool_error_rate_ceiling: f64,
    /// When `true`, ANY auto-compaction in the session raises a flag (a session that ran the context
    /// to the wall). Independent of the eligibility gates. Default `true`.
    #[serde(default = "default_auto_compaction_flag")]
    auto_compaction_flag: bool,
    /// Eligibility gate: a session with fewer than this many total tokens cannot meaningfully reuse
    /// cache, so it is NOT scored for cache waste (prevents false positives on short one-shots).
    /// Default `20000`.
    #[serde(default = "default_minimum_total_tokens")]
    minimum_total_tokens: u64,
    /// Eligibility gate: a session with fewer than this many assistant turns is a quick one-shot
    /// where cache reads are structurally impossible, so it is NOT scored for cache waste. Default
    /// `3`.
    #[serde(default = "default_minimum_turns")]
    minimum_turns: u64,
}

impl Default for EfficiencyConfig {
    fn default() -> Self {
        Self {
            cache_read_share_floor: default_cache_read_share_floor(),
            tool_error_rate_ceiling: default_tool_error_rate_ceiling(),
            auto_compaction_flag: default_auto_compaction_flag(),
            minimum_total_tokens: default_minimum_total_tokens(),
            minimum_turns: default_minimum_turns(),
        }
    }
}

/// Deserialize a threshold that must be a finite fraction in `0.0..=1.0`. `cache-read-share-floor`
/// and `tool-error-rate-ceiling` are both compared against ratios that live in `[0, 1]`; a typo like
/// `1.1` (flag nothing) or `-0.1` (flag everything) or a non-finite `.nan`/`.inf` would silently
/// invert the scoring. Reject it loudly at parse time (fail closed) rather than quietly mis-scoring.
fn de_fraction<'de, D>(deserializer: D) -> std::result::Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = f64::deserialize(deserializer)?;
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(serde::de::Error::custom(format!(
            "must be a finite fraction in 0.0..=1.0, got {value}"
        )));
    }
    Ok(value)
}

/// Serde default for `cache-read-share-floor`: below 60% cache reuse is the cache-waste line.
fn default_cache_read_share_floor() -> f64 {
    0.6
}

/// Serde default for `tool-error-rate-ceiling`: above 5% of tool calls erroring is the line.
fn default_tool_error_rate_ceiling() -> f64 {
    0.05
}

/// Serde default for `auto-compaction-flag`: on. Any auto-compaction is worth surfacing.
fn default_auto_compaction_flag() -> bool {
    true
}

/// Serde default for `minimum-total-tokens`: the cache-waste eligibility floor on token volume.
fn default_minimum_total_tokens() -> u64 {
    20000
}

/// Serde default for `minimum-turns`: the cache-waste eligibility floor on turn count.
fn default_minimum_turns() -> u64 {
    3
}

impl EfficiencyConfig {
    /// The `cache-read-share` floor: below it, an eligible session is flagged cache-wasteful.
    pub fn cache_read_share_floor(&self) -> f64 {
        self.cache_read_share_floor
    }

    /// The tool-error-rate ceiling: above it, a session is flagged error-prone.
    pub fn tool_error_rate_ceiling(&self) -> f64 {
        self.tool_error_rate_ceiling
    }

    /// Whether any auto-compaction raises a flag.
    pub fn auto_compaction_flag(&self) -> bool {
        self.auto_compaction_flag
    }

    /// The token-volume eligibility gate for cache-waste flagging.
    pub fn minimum_total_tokens(&self) -> u64 {
        self.minimum_total_tokens
    }

    /// The turn-count eligibility gate for cache-waste flagging.
    pub fn minimum_turns(&self) -> u64 {
        self.minimum_turns
    }
}

/// The parsed `clyde.yml`. Every field defaults, so an absent file deserializes to all-defaults.
///
/// `Default` is hand-written (NOT derived): `reindex_on_start` defaults to `true`, but a derived
/// `Default` would give the `bool` zero value `false` and diverge from the serde default a missing
/// file resolves to. Hand-writing keeps `Config::default()` and a from-scratch deserialize in lock
/// step.
///
/// `Eq` is deliberately NOT derived: the `efficiency:` section carries `f64` thresholds, which do
/// not implement `Eq`. `PartialEq` (what the tests' `assert_eq!` needs) is sufficient.
#[derive(Debug, Clone, PartialEq, Deserialize)]
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
    /// Thresholds `clyde efficiency` scores a session against. Absent -> all-defaults.
    #[serde(default)]
    efficiency: EfficiencyConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            date_tz: DateTzConfig::default(),
            render: RenderConfig::default(),
            projects_dir: None,
            reindex_on_start: default_reindex_on_start(),
            efficiency: EfficiencyConfig::default(),
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

    /// The `efficiency:` scoring thresholds (all-defaults when the section is absent).
    pub fn efficiency(&self) -> &EfficiencyConfig {
        &self.efficiency
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
