use crate::repo::Resolver;
use crate::scan::{SessionFile, SessionFileKind};
use chrono::{DateTime, Utc};
use claude_pricing::{AssistantEntry, ParseResult, TokenUsage, normalize_model_id};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct TokenTotals {
    pub input: u64,
    pub output: u64,
    pub cache_5m_write: u64,
    pub cache_1h_write: u64,
    pub cache_read: u64,
    pub total: u64,
}

impl TokenTotals {
    pub fn add(&mut self, usage: &TokenUsage) {
        self.input += usage.input_tokens;
        self.output += usage.output_tokens;
        self.cache_5m_write += usage.cache_5m_write_tokens;
        self.cache_1h_write += usage.cache_1h_write_tokens;
        self.cache_read += usage.cache_read_tokens;
        self.total = self.input + self.output + self.cache_5m_write + self.cache_1h_write + self.cache_read;
    }

    pub fn merge(&mut self, other: &TokenTotals) {
        self.input += other.input;
        self.output += other.output;
        self.cache_5m_write += other.cache_5m_write;
        self.cache_1h_write += other.cache_1h_write;
        self.cache_read += other.cache_read;
        self.total = self.input + self.output + self.cache_5m_write + self.cache_1h_write + self.cache_read;
    }

    pub fn as_usage(&self) -> TokenUsage {
        TokenUsage {
            input_tokens: self.input,
            output_tokens: self.output,
            cache_5m_write_tokens: self.cache_5m_write,
            cache_1h_write_tokens: self.cache_1h_write,
            cache_read_tokens: self.cache_read,
        }
    }
}

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
}

impl SessionSummary {
    pub fn total_tokens(&self) -> u64 {
        self.models.values().map(|t| t.total).sum()
    }
}

pub fn fold(
    files: &[SessionFile],
    parsed: &HashMap<PathBuf, ParseResult>,
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
        let summary = match g.finalize(since, until, resolver, existing_titles) {
            Some(s) => s,
            None => continue,
        };
        out.push(summary);
    }
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
    ) -> Option<SessionSummary> {
        let kept: Vec<AssistantEntry> = dedupe(std::mem::take(&mut self.entries))
            .into_iter()
            .filter(|e| e.timestamp >= since && e.timestamp <= until)
            .collect();
        if kept.is_empty() {
            return None;
        }

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
        })
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
