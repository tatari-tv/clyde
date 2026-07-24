use crate::outcome::{FileOutcomes, Outcomes, PrRef};
use crate::repo::Resolver;
use crate::scan::{SessionFile, SessionFileKind};
use chrono::{DateTime, Utc};
use claude_pricing::{AssistantEntry, ParseResult, normalize_model_id};
use log::debug;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

/// The shared token accumulator, lifted into `common` (Phase 1,
/// `docs/design/2026-07-24-report-collect-once-render-from-data.md`) so `report` and `efficiency`
/// share one `add`/`merge`/pricing path. Re-exported here so existing call sites in this crate
/// (`crate::session::TokenTotals`) are unaffected by the move.
pub use common::metrics::TokenTotals;

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub session_id: String,
    pub repo: Option<String>,
    pub cwd: Option<PathBuf>,
    pub begin: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub models: BTreeMap<String, TokenTotals>,
    pub jsonl_paths: Vec<PathBuf>,
    pub title: Option<String>,
    /// Union of extracted outcomes across the session group's files (parent + subagents), deduped
    /// (commits by sha, PRs by url, files by path). `None` when no outcome was observed.
    pub outcomes: Option<Outcomes>,
}

impl SessionSummary {
    pub fn total_tokens(&self) -> u64 {
        self.models.values().map(|t| t.total).sum()
    }
}

pub fn fold(
    files: &[SessionFile],
    parsed: &HashMap<PathBuf, ParseResult>,
    outcomes: &HashMap<PathBuf, FileOutcomes>,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    no_rollup: bool,
    resolver: &mut Resolver,
    existing_titles: &HashMap<String, String>,
) -> Vec<SessionSummary> {
    let mut groups: HashMap<String, GroupBuilder> = HashMap::new();

    for file in files {
        let group_id = group_id_for(file, no_rollup);
        let parse_result = match parsed.get(&file.path) {
            Some(p) => p,
            None => continue,
        };

        let entry = groups
            .entry(group_id.clone())
            .or_insert_with(|| GroupBuilder::new(group_id.clone()));

        entry.note_file(file);
        if let Some(cwd) = parse_result.cwd.as_ref()
            && entry.cwd.is_none()
        {
            entry.cwd = Some(cwd.clone());
        }
        for e in &parse_result.entries {
            entry.add_entry(e.clone());
        }
    }

    let mut out = Vec::with_capacity(groups.len());
    for mut g in groups.into_values() {
        let summary = match g.finalize(since, until, resolver, existing_titles, outcomes) {
            Some(s) => s,
            None => continue,
        };
        out.push(summary);
    }
    debug!("session::fold: emitted {} session summaries", out.len());
    out
}

fn group_id_for(file: &SessionFile, no_rollup: bool) -> String {
    if !no_rollup {
        return file.group_id.clone();
    }
    match file.kind {
        SessionFileKind::Parent => file.group_id.clone(),
        SessionFileKind::Subagent => file
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| file.group_id.clone()),
    }
}

#[derive(Debug)]
struct GroupBuilder {
    session_id: String,
    cwd: Option<PathBuf>,
    parent_paths: Vec<PathBuf>,
    subagent_paths: Vec<PathBuf>,
    entries: Vec<AssistantEntry>,
}

impl GroupBuilder {
    fn new(session_id: String) -> Self {
        Self {
            session_id,
            cwd: None,
            parent_paths: Vec::new(),
            subagent_paths: Vec::new(),
            entries: Vec::new(),
        }
    }

    fn note_file(&mut self, file: &SessionFile) {
        match file.kind {
            SessionFileKind::Parent => self.parent_paths.push(file.path.clone()),
            SessionFileKind::Subagent => self.subagent_paths.push(file.path.clone()),
        }
    }

    fn add_entry(&mut self, entry: AssistantEntry) {
        self.entries.push(entry);
    }

    fn finalize(
        &mut self,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
        resolver: &mut Resolver,
        existing_titles: &HashMap<String, String>,
        outcomes: &HashMap<PathBuf, FileOutcomes>,
    ) -> Option<SessionSummary> {
        let kept: Vec<AssistantEntry> = dedupe(std::mem::take(&mut self.entries))
            .into_iter()
            .filter(|e| e.timestamp >= since && e.timestamp <= until)
            .collect();
        if kept.is_empty() {
            return None;
        }

        // Union outcomes across all of this group's files (parent + subagents) BEFORE the path
        // vecs are drained into `jsonl_paths` below.
        let outcomes_union = union_outcomes(&self.parent_paths, &self.subagent_paths, outcomes);

        let mut models: BTreeMap<String, TokenTotals> = BTreeMap::new();
        let mut begin = kept[0].timestamp;
        let mut end = kept[0].timestamp;
        for e in &kept {
            let key = normalize_model_id(&e.model).to_string();
            models.entry(key).or_default().add(&e.usage);
            if e.timestamp < begin {
                begin = e.timestamp;
            }
            if e.timestamp > end {
                end = e.timestamp;
            }
        }

        let mut jsonl_paths = Vec::new();
        let mut sorted_parents = std::mem::take(&mut self.parent_paths);
        sorted_parents.sort();
        let mut sorted_subs = std::mem::take(&mut self.subagent_paths);
        sorted_subs.sort();
        jsonl_paths.extend(sorted_parents);
        jsonl_paths.extend(sorted_subs);

        let repo = self.cwd.as_deref().and_then(|c| resolver.detect(c));
        let title = existing_titles.get(&self.session_id).cloned();

        Some(SessionSummary {
            session_id: self.session_id.clone(),
            repo,
            cwd: self.cwd.clone(),
            begin,
            end,
            models,
            jsonl_paths,
            title,
            outcomes: outcomes_union,
        })
    }
}

/// Union the per-file [`FileOutcomes`] for every file in a session group into the persisted,
/// per-session [`Outcomes`] shape: commits deduped by sha, PRs deduped by url, edited file paths
/// deduped then counted, and the MCP counts summed (they carry no cross-file identity to dedupe
/// on). Returns `None` when the group observed no outcome at all.
fn union_outcomes(
    parent_paths: &[PathBuf],
    subagent_paths: &[PathBuf],
    outcomes: &HashMap<PathBuf, FileOutcomes>,
) -> Option<Outcomes> {
    let mut commits: BTreeSet<String> = BTreeSet::new();
    let mut prs: Vec<PrRef> = Vec::new();
    let mut seen_urls: HashSet<String> = HashSet::new();
    let mut files: BTreeSet<String> = BTreeSet::new();
    let mut confluence_writes: u64 = 0;
    let mut jira_writes: u64 = 0;
    let mut slack_messages: u64 = 0;

    for path in parent_paths.iter().chain(subagent_paths.iter()) {
        let Some(fo) = outcomes.get(path) else {
            continue;
        };
        commits.extend(fo.commits.iter().cloned());
        for pr in &fo.prs {
            if seen_urls.insert(pr.url.clone()) {
                prs.push(pr.clone());
            }
        }
        files.extend(fo.files_edited.iter().cloned());
        confluence_writes += fo.confluence_writes;
        jira_writes += fo.jira_writes;
        slack_messages += fo.slack_messages;
    }

    let result = Outcomes {
        commits: commits.into_iter().collect(),
        prs,
        confluence_writes,
        jira_writes,
        slack_messages,
        files_edited: files.len() as u64,
    };

    if result == Outcomes::default() {
        None
    } else {
        Some(result)
    }
}

fn dedupe(entries: Vec<AssistantEntry>) -> Vec<AssistantEntry> {
    let mut by_key: HashMap<(Option<String>, Option<String>), AssistantEntry> = HashMap::new();
    let mut without_key: Vec<AssistantEntry> = Vec::new();

    for e in entries {
        if e.message_id.is_none() && e.request_id.is_none() {
            without_key.push(e);
            continue;
        }
        let key = (e.message_id.clone(), e.request_id.clone());
        match by_key.get(&key) {
            Some(existing) if existing.usage.output_tokens >= e.usage.output_tokens => {}
            _ => {
                by_key.insert(key, e);
            }
        }
    }

    let mut out: Vec<AssistantEntry> = by_key.into_values().collect();
    out.extend(without_key);
    out
}

#[cfg(test)]
mod tests;
