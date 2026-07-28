use eyre::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Where a permission rule comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleSource {
    Global,
    Local,
}

impl std::fmt::Display for RuleSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleSource::Global => write!(f, "global"),
            RuleSource::Local => write!(f, "local"),
        }
    }
}

/// A parsed permission rule with its source.
#[derive(Debug, Clone)]
pub struct PermissionRule {
    pub rule: String,
    pub list: PermissionList,
    pub source: RuleSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionList {
    Allow,
    Deny,
}

impl std::fmt::Display for PermissionList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PermissionList::Allow => write!(f, "allow"),
            PermissionList::Deny => write!(f, "deny"),
        }
    }
}

/// Partial representation of Claude Code settings - just the permissions block.
#[derive(Debug, Deserialize, Default)]
struct SettingsFile {
    #[serde(default)]
    permissions: Permissions,
}

#[derive(Debug, Deserialize, Default)]
struct Permissions {
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    deny: Vec<String>,
}

/// Load permission rules from both settings files, deduplicating and tracking source.
pub fn load_settings(settings_path: &Path, settings_local_path: &Path) -> Result<Vec<PermissionRule>> {
    let mut rules = Vec::new();

    // Load global settings
    if settings_path.exists() {
        let global = parse_settings_file(settings_path)
            .with_context(|| format!("Failed to parse {}", settings_path.display()))?;

        for rule in global.permissions.allow {
            rules.push(PermissionRule {
                rule,
                list: PermissionList::Allow,
                source: RuleSource::Global,
            });
        }
        for rule in global.permissions.deny {
            rules.push(PermissionRule {
                rule,
                list: PermissionList::Deny,
                source: RuleSource::Global,
            });
        }
    }

    // Load local settings
    if settings_local_path.exists() {
        let local = parse_settings_file(settings_local_path)
            .with_context(|| format!("Failed to parse {}", settings_local_path.display()))?;

        for rule in local.permissions.allow {
            rules.push(PermissionRule {
                rule,
                list: PermissionList::Allow,
                source: RuleSource::Local,
            });
        }
        for rule in local.permissions.deny {
            rules.push(PermissionRule {
                rule,
                list: PermissionList::Deny,
                source: RuleSource::Local,
            });
        }
    }

    Ok(rules)
}

/// Walk up from `start_dir` looking for `.claude/settings.local.json`.
/// Falls back to `~/.claude/settings.local.json` if no project-level file found.
/// Only matches regular files so a directory named `settings.local.json` is skipped.
///
/// The walk stops AT (never inside) a shared root: the OS temp dir or `$HOME`. Neither is ever a
/// project - a stray `.claude/settings.local.json` dropped directly in `/tmp` or `$HOME` must not
/// be laundered as this invocation's project-level settings just because some ancestor of
/// `start_dir` happens to be one of them. A project genuinely nested under one of those roots
/// (e.g. a repo checked out at `/tmp/some-project`) is unaffected: only the boundary directory
/// itself is skipped, not its descendants.
pub fn discover_settings_local(start_dir: &Path) -> PathBuf {
    // The walk stops on PathBuf EQUALITY, so a symlinked or trailing-slash `$TMPDIR` naming the
    // same directory as the walked path would never compare equal and the boundary would silently
    // stop applying (issue #69). Canonicalize both sides: canonicalizing `start_dir` also
    // canonicalizes every parent the loop visits, so the comparison is like with like. A path that
    // fails to canonicalize (does not exist) keeps its raw form rather than dropping the boundary.
    let canonical = |p: PathBuf| p.canonicalize().unwrap_or(p);
    let home = dirs::home_dir();
    let boundary: Vec<PathBuf> = [Some(std::env::temp_dir()), home.clone()]
        .into_iter()
        .flatten()
        .map(canonical)
        .collect();
    let fallback = home
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
        .join("settings.local.json");
    discover_settings_local_bounded(&canonical(start_dir.to_path_buf()), &boundary, fallback)
}

/// Core walk, parameterized on the boundary roots and the fallback path so the boundary
/// enforcement can be pinned in tests without mutating the real `$TMPDIR`/`$HOME` (which would
/// race every other test's own `TempDir::new()` in the same process). `discover_settings_local`
/// is the only real caller and always wires in the true OS temp dir and home dir.
fn discover_settings_local_bounded(start_dir: &Path, boundary: &[PathBuf], fallback: PathBuf) -> PathBuf {
    let mut dir = start_dir.to_path_buf();
    loop {
        if boundary.iter().any(|b| b == &dir) {
            break;
        }
        let candidate = dir.join(".claude").join("settings.local.json");
        if candidate.is_file() {
            return candidate;
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => break,
        }
    }
    fallback
}

fn parse_settings_file(path: &Path) -> Result<SettingsFile> {
    let content = std::fs::read_to_string(path).context("Failed to read file")?;
    let settings: SettingsFile = serde_json::from_str(&content).context("Failed to parse JSON")?;
    Ok(settings)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parse_global_settings() {
        let dir = TempDir::new().expect("temp");
        let global = dir.path().join("settings.json");
        let local = dir.path().join("settings.local.json");
        std::fs::write(
            &global,
            r#"{"permissions":{"allow":["Bash(ls:*)","WebSearch"],"deny":["Bash(git tag -d *)"]}}"#,
        )
        .expect("write");

        let rules = load_settings(&global, &local).expect("load");
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].rule, "Bash(ls:*)");
        assert_eq!(rules[0].source, RuleSource::Global);
        assert_eq!(rules[0].list, PermissionList::Allow);
        assert_eq!(rules[2].list, PermissionList::Deny);
    }

    #[test]
    fn parse_both_files() {
        let dir = TempDir::new().expect("temp");
        let global = dir.path().join("settings.json");
        let local = dir.path().join("settings.local.json");
        std::fs::write(&global, r#"{"permissions":{"allow":["Bash(ls:*)"]}}"#).expect("write");
        std::fs::write(&local, r#"{"permissions":{"allow":["Bash(curl:*)"],"deny":[]}}"#).expect("write");

        let rules = load_settings(&global, &local).expect("load");
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].source, RuleSource::Global);
        assert_eq!(rules[1].source, RuleSource::Local);
    }

    #[test]
    fn missing_files_ok() {
        let dir = TempDir::new().expect("temp");
        let global = dir.path().join("nonexistent.json");
        let local = dir.path().join("also-nonexistent.json");
        let rules = load_settings(&global, &local).expect("load");
        assert!(rules.is_empty());
    }

    #[test]
    fn discover_finds_project_settings_local() {
        let root = TempDir::new().expect("temp");
        let claude_dir = root.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).expect("mkdir");
        let expected = claude_dir.join("settings.local.json");
        std::fs::write(&expected, r#"{"permissions":{}}"#).expect("write");

        let subdir = root.path().join("project").join("src");
        std::fs::create_dir_all(&subdir).expect("mkdir sub");

        let found = discover_settings_local(&subdir);
        // `discover_settings_local` canonicalizes `start_dir`, so the found path is canonical;
        // canonicalize the expectation too or this fails wherever the temp dir is symlinked
        // (macOS `/tmp` -> `/private/tmp`).
        assert_eq!(found, expected.canonicalize().expect("canonicalize"));
    }

    #[test]
    fn discover_falls_back_to_home() {
        let empty = TempDir::new().expect("temp");
        let result = discover_settings_local(empty.path());
        let expected = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".claude")
            .join("settings.local.json");
        assert_eq!(result, expected);
    }

    /// BITES: a `.claude/settings.local.json` sitting directly in a shared/system root (the OS
    /// temp dir here, standing in for the boundary) must never be treated as this invocation's
    /// project settings, even though it is a real ancestor of `start_dir`. Drop the boundary check
    /// from `discover_settings_local_bounded` and this returns the stray file instead of falling
    /// back - which is exactly the bug a real `/tmp/.claude/settings.local.json` (created by some
    /// unrelated tool or session rooted at `/tmp`) would trigger for any invocation whose cwd
    /// lives under `/tmp`, silently discarding the caller's real project-level settings.
    #[test]
    fn discover_stops_at_a_shared_boundary_root() {
        let shared_root = TempDir::new().expect("temp");
        let stray_claude = shared_root.path().join(".claude");
        std::fs::create_dir_all(&stray_claude).expect("mkdir");
        std::fs::write(stray_claude.join("settings.local.json"), r#"{"permissions":{}}"#).expect("write");

        let start = shared_root.path().join("nested").join("cwd");
        std::fs::create_dir_all(&start).expect("mkdir sub");

        let boundary = vec![shared_root.path().to_path_buf()];
        let fallback = PathBuf::from("/fallback/settings.local.json");

        let found = discover_settings_local_bounded(&start, &boundary, fallback.clone());
        assert_eq!(
            found, fallback,
            "the boundary root's own .claude/settings.local.json must never be matched"
        );
    }

    /// A project genuinely nested under a boundary root (e.g. a repo checked out under `/tmp`) is
    /// unaffected: only the boundary directory itself is skipped, never its descendants.
    #[test]
    fn discover_still_finds_a_project_nested_under_a_boundary_root() {
        let shared_root = TempDir::new().expect("temp");
        let project = shared_root.path().join("real-project");
        let claude_dir = project.join(".claude");
        std::fs::create_dir_all(&claude_dir).expect("mkdir");
        let expected = claude_dir.join("settings.local.json");
        std::fs::write(&expected, r#"{"permissions":{}}"#).expect("write");

        let start = project.join("src");
        std::fs::create_dir_all(&start).expect("mkdir sub");

        let boundary = vec![shared_root.path().to_path_buf()];
        let found = discover_settings_local_bounded(&start, &boundary, PathBuf::from("/fallback"));
        assert_eq!(found, expected);
    }

    /// BITES: spell the boundary root as a SYMLINK to the real directory (a symlinked `$TMPDIR`
    /// is exactly this) and the walk's `PathBuf` equality never fires -- it escapes the boundary
    /// and launders the stray file (issue #69). `discover_settings_local` therefore canonicalizes
    /// the boundary AND `start_dir` before walking; this pins both the defect and the fix.
    #[test]
    fn a_symlinked_boundary_root_stops_the_walk_only_once_canonicalized() {
        let real_root = TempDir::new().expect("temp");
        let stray_claude = real_root.path().join(".claude");
        std::fs::create_dir_all(&stray_claude).expect("mkdir");
        let stray = stray_claude.join("settings.local.json");
        std::fs::write(&stray, r#"{"permissions":{}}"#).expect("write");
        let start = real_root.path().join("nested").join("cwd");
        std::fs::create_dir_all(&start).expect("mkdir sub");

        let alias_holder = TempDir::new().expect("temp");
        let alias = alias_holder.path().join("tmp-alias");
        std::os::unix::fs::symlink(real_root.path(), &alias).expect("symlink");

        let fallback = PathBuf::from("/fallback/settings.local.json");

        // The raw pair IS the defect: the symlink never compares equal to the walked real path,
        // so the boundary silently stops applying and the stray file is matched.
        let escaped = discover_settings_local_bounded(&start, std::slice::from_ref(&alias), fallback.clone());
        assert_eq!(
            escaped, stray,
            "precondition: a raw symlink boundary does not stop the walk"
        );

        // Canonicalized the way `discover_settings_local` does it, the boundary holds.
        let boundary = vec![alias.canonicalize().expect("canonicalize boundary")];
        let start = start.canonicalize().expect("canonicalize start");
        let found = discover_settings_local_bounded(&start, &boundary, fallback.clone());
        assert_eq!(found, fallback, "the canonicalized symlink boundary must stop the walk");
    }

    #[test]
    fn settings_with_extra_fields() {
        let dir = TempDir::new().expect("temp");
        let global = dir.path().join("settings.json");
        let local = dir.path().join("settings.local.json");
        std::fs::write(
            &global,
            r#"{"model":"opus","env":{},"permissions":{"allow":["Bash(ls:*)"],"deny":[],"additionalDirectories":["/tmp"]},"hooks":{}}"#,
        )
        .expect("write");

        let rules = load_settings(&global, &local).expect("load");
        assert_eq!(rules.len(), 1);
    }
}
