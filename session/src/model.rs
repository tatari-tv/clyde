//! Typed model for a Claude Code session: the discovered files ([`SessionFile`]) and the
//! parsed, rolled-up record ([`ParsedSession`]) the navigational layer indexes.

use std::path::PathBuf;

use chrono::{DateTime, Utc};

/// Whether a discovered JSONL is a top-level parent session or a subagent transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionFileKind {
    /// `<project>/<uuid>.jsonl` — a real top-level session.
    Parent,
    /// `<project>/<uuid>/subagents/*.jsonl` — a subagent transcript that rolls up into the
    /// parent session identified by `<uuid>`. Mirrors `cr`'s rollup contract.
    Subagent,
}

/// A single discovered transcript file, tagged with the parent session id it belongs to.
#[derive(Debug, Clone)]
pub struct SessionFile {
    pub path: PathBuf,
    /// The parent session UUID. Parents and their subagents share this, so grouping by
    /// `group_id` rolls subagents into the parent (the `cr` semantics).
    pub group_id: String,
    pub kind: SessionFileKind,
}

/// One navigational record per session, parsed and rolled up from the parent transcript plus
/// any subagent transcripts. The `sessions` layer maps this into a `sessions.db` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSession {
    /// The session UUID (the parent transcript stem).
    pub session_id: String,
    /// The working directory the session ran in (first `cwd` seen in the transcript).
    pub cwd: Option<PathBuf>,
    /// The slugified-cwd project directory under `~/.claude/projects` that holds the transcript.
    pub project_dir: PathBuf,
    /// Claude's auto-generated title (`ai-title` line). Present for ~96% of sessions.
    pub ai_title: Option<String>,
    /// First genuine user prompt (command/caveat/system wrappers skipped). Title fallback.
    pub first_prompt: Option<String>,
    /// Git branch the session ran on (first `gitBranch` seen).
    pub git_branch: Option<String>,
    /// The most recent assistant model id seen (e.g. `claude-opus-4-8`).
    pub model: Option<String>,
    /// Count of user + assistant messages across parent and subagents.
    pub n_msgs: usize,
    /// Earliest message timestamp in the transcript.
    pub created: Option<DateTime<Utc>>,
    /// Parent transcript file mtime — the incremental-reindex skip key.
    pub modified: DateTime<Utc>,
    /// Concatenated user + assistant text, for the body-FTS content-recall index.
    pub body: String,
    /// All transcript files (parent first, then subagents), for `open`/staging.
    pub jsonl_paths: Vec<PathBuf>,
}

impl ParsedSession {
    /// The display title: Claude's `ai-title` when present, else the first user prompt.
    pub fn title(&self) -> Option<&str> {
        self.ai_title.as_deref().or(self.first_prompt.as_deref())
    }
}
