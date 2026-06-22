//! Parse discovered transcript files into rolled-up [`ParsedSession`] records.
//!
//! Robustness contract (design doc "Edge Cases"): malformed/partial lines, non-UTF-8 bytes,
//! and Claude JSONL schema drift are skip-and-logged, never fatal. Each line is parsed
//! independently from raw bytes so one bad line never truncates the rest of a transcript.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use log::{debug, trace, warn};
use serde_json::Value;

use crate::model::{ParsedSession, SessionFile, SessionFileKind};

/// Cap on the stored first-prompt (some first prompts paste whole files).
const MAX_FIRST_PROMPT_CHARS: usize = 2_000;
/// Cap on the per-session body indexed for content recall, bounding worst-case storage.
const MAX_BODY_CHARS: usize = 500_000;

/// User-message wrappers that are not genuine prompts (slash-command scaffolding, hook output,
/// injected reminders). The first user text that is *not* one of these becomes `first_prompt`.
const NOISE_PREFIXES: &[&str] = &[
    "<local-command-caveat>",
    "<command-name>",
    "<command-message>",
    "<command-args>",
    "<command-stdout>",
    "<system-reminder>",
    "<bash-input>",
    "<bash-stdout>",
    "<bash-stderr>",
    "<user-prompt-submit-hook>",
];

/// Group discovered files by parent session id and parse each group into one record.
/// Subagent transcripts roll up into their parent (the `cr` contract).
pub fn parse_sessions(files: &[SessionFile]) -> Vec<ParsedSession> {
    debug!("parse::parse_sessions: files={}", files.len());
    let mut groups: BTreeMap<String, Vec<&SessionFile>> = BTreeMap::new();
    for f in files {
        groups.entry(f.group_id.clone()).or_default().push(f);
    }
    let sessions: Vec<ParsedSession> = groups
        .into_iter()
        .filter_map(|(gid, group)| parse_group(&gid, &group))
        .collect();
    debug!("parse::parse_sessions: parsed {} sessions", sessions.len());
    sessions
}

fn parse_group(group_id: &str, files: &[&SessionFile]) -> Option<ParsedSession> {
    trace!("parse::parse_group: group_id={} files={}", group_id, files.len());
    let mut acc = Acc::new(group_id);
    // Parents before subagents so project_dir is derived from the canonical location.
    let mut ordered: Vec<&SessionFile> = files.to_vec();
    ordered.sort_by(|a, b| {
        let key = |f: &&SessionFile| (matches!(f.kind, SessionFileKind::Subagent), f.path.clone());
        key(a).cmp(&key(b))
    });
    for f in &ordered {
        acc.ingest_file(f);
    }
    acc.finalize()
}

struct Acc {
    session_id: String,
    cwd: Option<PathBuf>,
    project_dir: Option<PathBuf>,
    git_branch: Option<String>,
    ai_title: Option<String>,
    first_prompt: Option<String>,
    model: Option<String>,
    n_msgs: usize,
    created: Option<DateTime<Utc>>,
    modified: Option<DateTime<Utc>>,
    body: String,
    body_chars: usize,
    paths: Vec<PathBuf>,
}

impl Acc {
    fn new(group_id: &str) -> Self {
        Self {
            session_id: group_id.to_string(),
            cwd: None,
            project_dir: None,
            git_branch: None,
            ai_title: None,
            first_prompt: None,
            model: None,
            n_msgs: 0,
            created: None,
            modified: None,
            body: String::new(),
            body_chars: 0,
            paths: Vec::new(),
        }
    }

    fn ingest_file(&mut self, file: &SessionFile) {
        self.paths.push(file.path.clone());

        if matches!(file.kind, SessionFileKind::Parent) && self.project_dir.is_none() {
            self.project_dir = file.path.parent().map(Path::to_path_buf);
        }
        if let Some(mtime) = file_mtime(&file.path) {
            self.modified = Some(self.modified.map_or(mtime, |cur| cur.max(mtime)));
        }

        let bytes = match fs::read(&file.path) {
            Ok(b) => b,
            Err(e) => {
                warn!("parse: failed to read {}: {}", file.path.display(), e);
                return;
            }
        };
        for line in bytes.split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            match serde_json::from_slice::<Value>(line) {
                Ok(v) => self.ingest_line(&v),
                Err(e) => trace!("parse: skipping malformed line in {}: {}", file.path.display(), e),
            }
        }
    }

    fn ingest_line(&mut self, v: &Value) {
        if self.cwd.is_none()
            && let Some(c) = v.get("cwd").and_then(Value::as_str)
            && !c.is_empty()
        {
            self.cwd = Some(PathBuf::from(c));
        }
        if self.git_branch.is_none()
            && let Some(b) = v.get("gitBranch").and_then(Value::as_str)
            && !b.is_empty()
        {
            self.git_branch = Some(b.to_string());
        }
        if let Some(dt) = v.get("timestamp").and_then(Value::as_str).and_then(parse_ts) {
            self.created = Some(self.created.map_or(dt, |cur| cur.min(dt)));
        }

        match v.get("type").and_then(Value::as_str) {
            Some("ai-title") => {
                if let Some(t) = v.get("aiTitle").and_then(Value::as_str) {
                    let t = t.trim();
                    if !t.is_empty() {
                        self.ai_title = Some(t.to_string());
                    }
                }
            }
            Some("user") => {
                self.n_msgs += 1;
                let text = extract_text(v.get("message").and_then(|m| m.get("content")));
                let trimmed = text.trim();
                if !is_command_noise(trimmed) {
                    if self.first_prompt.is_none() {
                        self.first_prompt = Some(cap_chars(trimmed, MAX_FIRST_PROMPT_CHARS));
                    }
                    self.append_body(trimmed);
                }
            }
            Some("assistant") => {
                self.n_msgs += 1;
                if let Some(m) = v.get("message").and_then(|m| m.get("model")).and_then(Value::as_str)
                    && !m.is_empty()
                {
                    self.model = Some(m.to_string());
                }
                let text = extract_text(v.get("message").and_then(|m| m.get("content")));
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    self.append_body(trimmed);
                }
            }
            _ => {}
        }
    }

    fn append_body(&mut self, text: &str) {
        if self.body_chars >= MAX_BODY_CHARS {
            return;
        }
        if !self.body.is_empty() {
            self.body.push('\n');
            self.body_chars += 1;
        }
        self.body.push_str(text);
        self.body_chars += text.chars().count();
    }

    fn finalize(self) -> Option<ParsedSession> {
        let project_dir = self
            .project_dir
            .or_else(|| self.paths.first().and_then(|p| project_dir_from_subagent(p)))?;
        let modified = match self.modified {
            Some(m) => m,
            None => {
                warn!("parse: session {} has no stat-able file; dropping", self.session_id);
                return None;
            }
        };
        Some(ParsedSession {
            session_id: self.session_id,
            cwd: self.cwd,
            project_dir,
            ai_title: self.ai_title,
            first_prompt: self.first_prompt,
            git_branch: self.git_branch,
            model: self.model,
            n_msgs: self.n_msgs,
            created: self.created,
            modified,
            body: self.body,
            jsonl_paths: self.paths,
        })
    }
}

/// Subagent path `<projects>/<slug>/<uuid>/subagents/x.jsonl` → `<projects>/<slug>`.
fn project_dir_from_subagent(path: &Path) -> Option<PathBuf> {
    path.ancestors().nth(3).map(Path::to_path_buf)
}

fn file_mtime(path: &Path) -> Option<DateTime<Utc>> {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(DateTime::<Utc>::from)
        .ok()
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

fn extract_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn is_command_noise(s: &str) -> bool {
    let t = s.trim();
    t.is_empty() || NOISE_PREFIXES.iter().any(|p| t.starts_with(p))
}

fn cap_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests;
