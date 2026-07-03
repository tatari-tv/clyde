use colored::*;
use eyre::Result;
use std::path::Path;

/// Result of a single check.
pub struct CheckResult {
    pub name: String,
    pub passed: bool,
    pub message: String,
}

/// Run all checks and return results.
pub fn run_checks(db_path: &Path, settings_path: &Path, settings_local_path: &Path) -> Vec<CheckResult> {
    vec![
        check_database(db_path),
        check_hook_registered(settings_path, settings_local_path),
        check_binary_in_path(),
    ]
}

/// Run the `check` subcommand: verify DB, hook registration, and binary availability.
pub fn run_check(db_path: &Path, settings_path: &Path, settings_local_path: &Path) -> Result<bool> {
    let results = run_checks(db_path, settings_path, settings_local_path);
    let mut all_passed = true;

    for result in &results {
        if result.passed {
            println!("{} {}: {}", "PASS".green().bold(), result.name, result.message);
        } else {
            println!("{} {}: {}", "FAIL".red().bold(), result.name, result.message);
            all_passed = false;
        }
    }

    if all_passed {
        println!("\n{}", "All checks passed.".green().bold());
    } else {
        println!("\n{}", "Some checks failed. See above for details.".red().bold());
    }

    Ok(all_passed)
}

fn check_database(db_path: &Path) -> CheckResult {
    if !db_path.exists() {
        return CheckResult {
            name: "database".to_string(),
            passed: false,
            message: format!(
                "Database not found at {}. Run `clyde permit log` once to create it.",
                db_path.display()
            ),
        };
    }

    match crate::db::EventStore::open(db_path) {
        Ok(store) => {
            if store.is_writable() {
                let count = store.count_events().unwrap_or(0);
                CheckResult {
                    name: "database".to_string(),
                    passed: true,
                    message: format!("{} ({} events)", db_path.display(), count),
                }
            } else {
                CheckResult {
                    name: "database".to_string(),
                    passed: false,
                    message: format!("Database at {} is not writable", db_path.display()),
                }
            }
        }
        Err(e) => CheckResult {
            name: "database".to_string(),
            passed: false,
            message: format!("Failed to open database: {e}"),
        },
    }
}

fn check_hook_registered(settings_path: &Path, settings_local_path: &Path) -> CheckResult {
    let paths = [settings_path, settings_local_path];
    // Accept either the legacy standalone form (`claude-permit log`) or the current umbrella
    // form (`clyde permit log`), mirroring the idempotency check in install.rs:has_permit_hook.
    // Match the actual hook command form (with the `log` subcommand) to avoid false positives
    // from unrelated mentions of the binary names elsewhere in the settings file.
    let found_in = paths.iter().filter(|p| p.exists()).find(|p| {
        std::fs::read_to_string(p)
            .map(|content| content.contains("claude-permit log") || content.contains("clyde permit log"))
            .unwrap_or(false)
    });

    match found_in {
        Some(path) => CheckResult {
            name: "hook".to_string(),
            passed: true,
            message: format!("clyde permit log hook found in {}", path.display()),
        },
        None => CheckResult {
            name: "hook".to_string(),
            passed: false,
            message: "No permit hook found. Run `clyde permit install --yes` to add one.".to_string(),
        },
    }
}

fn check_binary_in_path() -> CheckResult {
    // Prefer the clyde umbrella binary; fall back to accepting the legacy claude-permit shim so
    // hosts that haven't run bootstrap yet still pass this check.
    match which::which("clyde").or_else(|_| which::which("claude-permit")) {
        Ok(path) => CheckResult {
            name: "binary".to_string(),
            passed: true,
            message: format!("{}", path.display()),
        },
        Err(_) => CheckResult {
            name: "binary".to_string(),
            passed: false,
            message: "clyde not found in PATH. Install clyde to get permit log support.".to_string(),
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn check_database_missing() {
        let dir = TempDir::new().expect("temp");
        let result = check_database(&dir.path().join("nonexistent.db"));
        assert!(!result.passed);
        assert!(result.message.contains("not found"));
    }

    #[test]
    fn check_database_exists() {
        let dir = TempDir::new().expect("temp");
        let db_path = dir.path().join("test.db");
        // open creates the DB; we just need the side effect
        drop(crate::db::EventStore::open(&db_path).expect("open"));
        let result = check_database(&db_path);
        assert!(result.passed);
        assert!(result.message.contains("0 events"));
    }

    #[test]
    fn check_hook_not_registered() {
        let dir = TempDir::new().expect("temp");
        let settings = dir.path().join("settings.json");
        let settings_local = dir.path().join("settings.local.json");
        std::fs::write(&settings, r#"{"permissions":{}}"#).expect("write");
        let result = check_hook_registered(&settings, &settings_local);
        assert!(!result.passed);
    }

    #[test]
    fn check_hook_found_legacy_form() {
        let dir = TempDir::new().expect("temp");
        let settings = dir.path().join("settings.json");
        let settings_local = dir.path().join("settings.local.json");
        std::fs::write(
            &settings,
            r#"{"hooks":{"PreToolUse":[{"command":"claude-permit log"}]}}"#,
        )
        .expect("write");
        let result = check_hook_registered(&settings, &settings_local);
        assert!(result.passed);
        assert!(result.message.contains("clyde permit log"));
    }

    #[test]
    fn check_hook_found_clyde_form() {
        let dir = TempDir::new().expect("temp");
        let settings = dir.path().join("settings.json");
        let settings_local = dir.path().join("settings.local.json");
        std::fs::write(
            &settings,
            r#"{"hooks":{"PreToolUse":[{"command":"clyde permit log"}]}}"#,
        )
        .expect("write");
        let result = check_hook_registered(&settings, &settings_local);
        assert!(result.passed);
        assert!(result.message.contains("clyde permit log"));
    }
}
