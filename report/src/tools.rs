//! External-tool validation for `report`'s `--help`. Renders a "REQUIRED TOOLS" block showing
//! which optional external binaries (`persona`, `pandoc`, `marquee`, `git`, `jq`) are installed
//! and what each is used for. Surfaced by the `clyde` umbrella as the `after_help` of
//! `clyde report --help` (the probes spawn `--version`, so the umbrella computes this only when
//! that help is actually requested).

use log::debug;
use std::process::Command;

struct ToolStatus {
    version: String,
    icon: &'static str,
}

fn check_tool(tool: &str, version_arg: &str) -> ToolStatus {
    match Command::new(tool).arg(version_arg).output() {
        Ok(output) if output.status.success() => {
            let body = String::from_utf8_lossy(&output.stdout);
            let version = extract_version(&body);
            ToolStatus {
                version: if version.is_empty() {
                    "installed".to_string()
                } else {
                    version
                },
                icon: "✅",
            }
        }
        _ => ToolStatus {
            version: "not found".to_string(),
            icon: "❌",
        },
    }
}

fn extract_version(output: &str) -> String {
    let Some(line) = output.lines().next() else {
        return String::new();
    };
    for word in line.split_whitespace() {
        if let Some(v) = looks_like_version(word.trim_start_matches('v')) {
            return v.to_string();
        }
        // Handle single-token formats like `jq-1.8.1` where the version
        // sits after a dash with no whitespace before it.
        if let Some((_, suffix)) = word.rsplit_once('-')
            && let Some(v) = looks_like_version(suffix)
        {
            return v.to_string();
        }
    }
    if let Some(v) = looks_like_version(line.trim()) {
        return v.to_string();
    }
    String::new()
}

fn looks_like_version(s: &str) -> Option<&str> {
    if s.contains('.') && s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        Some(s)
    } else {
        None
    }
}

/// Render the "REQUIRED TOOLS" help block, probing each optional external binary for its version.
/// Spawns one `--version` per tool, so call this only when rendering `clyde report --help`.
pub fn tool_validation_help() -> String {
    debug!("tools::tool_validation_help: probing external tools");
    let tools: &[(&str, &str, &str)] = &[
        ("persona", "--version", "report render: persona block in context"),
        ("pandoc", "--version", "report render --format pdf / marquee-html"),
        (
            "marquee",
            "--version",
            "report render --format marquee-html / marquee-markdown",
        ),
        ("git", "--version", "report collect: repo detection"),
        ("jq", "--version", "report collect: query JSON report output"),
    ];

    let entries: Vec<(ToolStatus, &str, &str)> = tools
        .iter()
        .map(|(name, arg, purpose)| (check_tool(name, arg), *name, *purpose))
        .collect();

    let max_name = entries.iter().map(|(_, n, _)| n.len()).max().unwrap_or(0);
    let max_ver = entries.iter().map(|(s, _, _)| s.version.len()).max().unwrap_or(0);

    let mut help = String::from("REQUIRED TOOLS:\n");
    for (status, name, purpose) in &entries {
        help.push_str(&format!(
            "  {} {:<name_w$}  {:>ver_w$}  ({})\n",
            status.icon,
            name,
            status.version,
            purpose,
            name_w = max_name,
            ver_w = max_ver,
        ));
    }
    help
}

#[cfg(test)]
mod tests;
