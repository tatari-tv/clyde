//! Shared "REQUIRED TOOLS" help rendering for clyde subcommands.
//!
//! Some subcommands shell out to external binaries that are NOT linked libraries (e.g. `pandoc`,
//! `marquee`, `claude`, `rkvr`, `systemctl`). Their `--help` should advertise those binaries and
//! whether each is currently installed. [`required_tools_help`] renders that block, probing each
//! tool's `--version` with a bounded wait so a hung binary can't stall the help render.

use log::debug;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

/// Wall-clock ceiling for a single `--version` probe. A tool that hangs or blocks on stdin must
/// not stall a `--help` render; on timeout the tool is simply reported unavailable.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// One external binary a subcommand depends on: the executable `name` and a short `purpose`
/// describing what the subcommand uses it for.
pub struct Tool {
    pub name: &'static str,
    pub purpose: &'static str,
}

struct ToolStatus {
    version: String,
    icon: &'static str,
}

impl ToolStatus {
    fn unavailable() -> Self {
        ToolStatus {
            version: "not found".to_string(),
            icon: "❌",
        }
    }
}

/// Render the "REQUIRED TOOLS" help block for `tools`, probing each for its version. Spawns one
/// `--version` per tool, so call this only when actually rendering the owning subcommand's help.
pub fn required_tools_help(tools: &[Tool]) -> String {
    debug!("tools::required_tools_help: probing {} tool(s)", tools.len());
    let entries: Vec<(ToolStatus, &Tool)> = tools.iter().map(|t| (check_tool(t.name), t)).collect();

    let max_name = tools.iter().map(|t| t.name.len()).max().unwrap_or(0);
    let max_ver = entries.iter().map(|(s, _)| s.version.len()).max().unwrap_or(0);

    let mut help = String::from("REQUIRED TOOLS:\n");
    for (status, tool) in &entries {
        help.push_str(&format!(
            "  {} {:<name_w$}  {:>ver_w$}  ({})\n",
            status.icon,
            tool.name,
            status.version,
            tool.purpose,
            name_w = max_name,
            ver_w = max_ver,
        ));
    }
    help
}

fn check_tool(tool: &str) -> ToolStatus {
    // stdin=null so a probe that reads stdin can't block; bounded wait so one that hangs can't
    // stall the help render. Output is a short version line, well under the pipe buffer.
    let spawn = Command::new(tool)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match spawn {
        Ok(c) => c,
        Err(_) => return ToolStatus::unavailable(),
    };
    match child.wait_timeout(PROBE_TIMEOUT) {
        Ok(Some(status)) if status.success() => {
            let mut body = String::new();
            if let Some(mut out) = child.stdout.take() {
                let _ = out.read_to_string(&mut body);
            }
            let version = extract_version(&body);
            ToolStatus {
                version: if version.is_empty() { "installed".to_string() } else { version },
                icon: "✅",
            }
        }
        Ok(Some(_)) => ToolStatus::unavailable(),
        // Timed out or wait failed: reap the child so it can't linger, report unavailable.
        Ok(None) | Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            ToolStatus::unavailable()
        }
    }
}

/// Pull a version-looking token (`1.2.3`, `v1.2.3`, `jq-1.8.1`) from the first line of a tool's
/// `--version` output. Empty string when none is found.
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

#[cfg(test)]
mod tests;
