//! Typed model for a Claude Code session: the discovered files ([`SessionFile`]) and the
//! parsed, rolled-up record ([`ParsedSession`]) the navigational layer indexes.

use std::path::PathBuf;

use chrono::{DateTime, Utc};

/// Whether a discovered JSONL is a top-level parent session or a subagent transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionFileKind {
    /// `<project>/<uuid>.jsonl` -- a real top-level session.
    Parent,
    /// `<project>/<uuid>/subagents/*.jsonl` -- a subagent transcript that rolls up into the
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

/// Cap on the DISPLAY title (see [`ParsedSession::title`]). Deliberately far below
/// `parse::MAX_FIRST_PROMPT_CHARS`: `first_prompt` is a stored prompt and legitimately wants 2,000
/// chars, a title is one line in a table.
const MAX_TITLE_CHARS: usize = 120;

/// The marker appended to a truncated title. ASCII, so it survives every terminal, FTS tokenizer, and
/// JSON consumer without an encoding question.
const TITLE_ELLIPSIS: &str = "...";

/// `s` bounded to `max` CHARACTERS, cutting at the last word boundary that fits and marking the cut.
///
/// Counts chars, never bytes: `&s[..max]` panics the moment a multibyte character straddles the
/// boundary, and session titles carry plenty (em-dashes, box drawing, CJK, emoji). Falls back to a hard
/// char-boundary cut when the text has no space inside the cap, so a single enormous token is still
/// bounded.
fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    // `rsplit_once` on the accumulated HEAD, so the boundary search never reads past the cap.
    let cut = match head.rsplit_once(char::is_whitespace) {
        // Guard against a boundary so early that the title becomes useless: a word break in the first
        // quarter of the cap loses more than the ragged edge costs, so take the hard cut instead.
        Some((left, _)) if left.chars().count() > max / 4 => left.trim_end().to_string(),
        _ => head,
    };
    format!("{cut}{TITLE_ELLIPSIS}")
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
    /// The invoked slash-command / skill name (last one, excluding `/clear`), for sessions that
    /// opened with a command and so have neither an ai-title nor a typed first prompt.
    pub command_name: Option<String>,
    /// Git branch the session ran on (first `gitBranch` seen).
    pub git_branch: Option<String>,
    /// The most recent assistant model id seen (e.g. `claude-opus-4-8`).
    pub model: Option<String>,
    /// Count of user + assistant messages across parent and subagents.
    pub n_msgs: usize,
    /// Earliest message timestamp in the transcript.
    pub created: Option<DateTime<Utc>>,
    /// LATEST message timestamp in the transcript: real activity time, the mirror of [`Self::created`].
    /// `None` when no record in the transcript carried a parseable `timestamp`, which is legitimate
    /// and NOT the same as "not yet computed" (that distinction is what `parse_version` records).
    ///
    /// Distinct from [`Self::modified`] on purpose. `modified` is filesystem mtime, which a Syncthing
    /// sync, a restore, or a `cp -r` resets wholesale; this is what the session actually did and when.
    /// Dormancy measures from this (see `sessions::SessionRecord::dormancy_at`); report windowing,
    /// `--since`, `sort=recency` and export all keep reading `modified` unchanged.
    pub activity_at: Option<DateTime<Utc>>,
    /// Parent transcript file mtime -- the incremental-reindex skip key.
    pub modified: DateTime<Utc>,
    /// Concatenated user + assistant text, for the body-FTS content-recall index.
    pub body: String,
    /// All transcript files (parent first, then subagents), for `open`/staging.
    pub jsonl_paths: Vec<PathBuf>,
}

impl ParsedSession {
    /// The display title: Claude's `ai-title` when present, else the first genuine user prompt,
    /// else the invoked command/skill name (for command-opened sessions Claude never titled).
    ///
    /// Shaped as a TITLE, which the raw source often is not. Claude emits no `ai-title` for ~4% of
    /// sessions, and the `first_prompt` fallback is a whole prompt capped at
    /// `parse::MAX_FIRST_PROMPT_CHARS` (2,000) -- so an agent-launch prompt or a context-compaction
    /// summary became a 2,000-character "title" that rendered as a wall of text in the report's outlier
    /// table and in every search hit. Measured on desk.lan 2026-07-31: 61 rows over 200 chars, the worst
    /// sitting exactly at the 2,000 cap.
    ///
    /// Two transforms, both needed. The FIRST NON-BLANK LINE handles the multi-line case (a compaction
    /// summary's later paragraphs are not title material at any length), then [`MAX_TITLE_CHARS`] bounds
    /// what a single very long line can do. Truncation prefers the last word boundary inside the cap so
    /// the result does not end mid-word, and marks itself with an ellipsis.
    ///
    /// Applied to whichever source wins, not just the fallback: a short `ai_title` passes through
    /// unchanged, so a uniform rule costs nothing and cannot be defeated by a future source.
    pub fn title(&self) -> Option<String> {
        let raw = self
            .ai_title
            .as_deref()
            .or(self.first_prompt.as_deref())
            .or(self.command_name.as_deref())?;
        let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
        Some(shorten(line, MAX_TITLE_CHARS))
    }
}

/// Who spoke a [`Message`]: a Claude Code transcript carries only user and assistant turns
/// (tool-result/system lines are not surfaced as messages).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// One role-labeled message from the served index space: the noise-excluded user + assistant
/// sequence -- parent transcript in order first, then each subagent file in path order -- that
/// `session::parse::parse_messages` yields. This is EXACTLY what `ParsedSession.body` folded into
/// the body-FTS index (same `extract_text` + `NOISE_PREFIXES` filter), so grep/read (Phases 6/7)
/// see what search already matched on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub text: String,
    /// `true` when this message came from a subagent transcript rather than the parent. Subagent
    /// text is included (it is already rolled into the parent's body FTS) and flagged so callers
    /// can label it distinctly.
    pub subagent: bool,
}

#[cfg(test)]
mod tests;
