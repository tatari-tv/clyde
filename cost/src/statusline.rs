use chrono::Local;
use eyre::{Context, Result, eyre};
use include_dir::{Dir, include_dir};
use log::info;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

static STATUSLINE_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/statusline.d");

const DEFAULT_NAME: &str = "scottidler";

/// `pub(crate)` (not private) so the crate-level tests can assert on the shipped segment
/// scripts' content directly (e.g. the stale-feed sidecar path they define, AC7) without
/// duplicating the embedded-file lookup.
pub(crate) fn find_entry(name: &str) -> Result<&'static str> {
    STATUSLINE_DIR
        .get_file(name)
        .and_then(|f| f.contents_utf8())
        .ok_or_else(|| {
            eyre!(
                "Unknown statusline '{}'. Use 'ccu statusline --list' to see available options.",
                name
            )
        })
}

fn entry_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = STATUSLINE_DIR
        .files()
        .filter_map(|f| f.path().file_name()?.to_str())
        .filter(|name| !name.starts_with('.') && *name != "default")
        .collect();
    names.sort_unstable();
    names
}

pub fn list() {
    println!("Available statuslines:");
    for name in entry_names() {
        let marker = if name == DEFAULT_NAME { " (default)" } else { "" };
        println!("  {}{}", name, marker);
    }
}

fn install_to(name: &str, dest_dir: &Path) -> Result<()> {
    let content = find_entry(name)?;
    let target = dest_dir.join("statusline.sh");

    // Back up existing file
    if target.exists() {
        let timestamp = Local::now().format("%Y%m%d-%H%M%S");
        let backup = dest_dir.join(format!("statusline.sh.{}.bak", timestamp));
        fs::rename(&target, &backup)
            .with_context(|| format!("Failed to back up {} to {}", target.display(), backup.display()))?;
        info!("Backed up existing statusline to {}", backup.display());
        println!("Backed up existing statusline to {}", backup.display());
    }

    // Write new statusline
    fs::write(&target, content).with_context(|| format!("Failed to write {}", target.display()))?;

    // Make executable
    let mut perms = fs::metadata(&target)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&target, perms)?;

    info!("Installed '{}' statusline to {}", name, target.display());
    println!("Installed '{}' statusline to {}", name, target.display());

    Ok(())
}

fn claude_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| eyre!("Could not determine home directory"))?;
    Ok(home.join(".claude"))
}

pub fn install(name: Option<&str>) -> Result<()> {
    let name = match name {
        None | Some("default") => DEFAULT_NAME,
        Some(n) => n,
    };
    let dest = claude_dir()?;
    install_to(name, &dest)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_entries_not_empty() {
        assert!(!entry_names().is_empty());
    }

    #[test]
    fn test_default_entry_exists() {
        assert!(find_entry(DEFAULT_NAME).is_ok());
    }

    #[test]
    fn test_unknown_entry_errors() {
        assert!(find_entry("nonexistent").is_err());
    }

    #[test]
    fn test_install_creates_file() {
        let dir = TempDir::new().unwrap();
        install_to("scottidler", dir.path()).unwrap();

        let target = dir.path().join("statusline.sh");
        assert!(target.exists());

        let content = fs::read_to_string(&target).unwrap();
        assert!(content.contains("#!/usr/bin/env bash"));

        let perms = fs::metadata(&target).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o755);
    }

    #[test]
    fn test_install_default() {
        let dir = TempDir::new().unwrap();
        install_to(DEFAULT_NAME, dir.path()).unwrap();
        assert!(dir.path().join("statusline.sh").exists());
    }

    #[test]
    fn test_install_backs_up_existing() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("statusline.sh");

        fs::write(&target, "old content").unwrap();
        assert!(target.exists());

        install_to("scottidler", dir.path()).unwrap();

        let content = fs::read_to_string(&target).unwrap();
        assert!(content.contains("#!/usr/bin/env bash"));
        assert_ne!(content, "old content");

        let backups: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name().to_string_lossy().starts_with("statusline.sh.")
                    && e.file_name().to_string_lossy().ends_with(".bak")
            })
            .collect();
        assert_eq!(backups.len(), 1);

        let backup_content = fs::read_to_string(backups[0].path()).unwrap();
        assert_eq!(backup_content, "old content");
    }

    #[test]
    fn test_install_unknown_errors() {
        let dir = TempDir::new().unwrap();
        assert!(install_to("nonexistent", dir.path()).is_err());
    }

    #[test]
    fn test_embedded_content_is_executable_script() {
        let content = find_entry("scottidler").unwrap();
        assert!(content.starts_with("#!/usr/bin/env bash"));
        // Fresh installs invoke the clyde umbrella form, not the bare `ccu` shim.
        assert!(content.contains("clyde cost"));
    }
}
