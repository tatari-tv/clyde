use crate::session::SessionSummary;
use chrono::{DateTime, Utc};
use eyre::{Context, Result};
use log::debug;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Report {
    pub schema_version: u32,
    pub generated: DateTime<Utc>,
    pub host: String,
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub session_count: usize,
    pub sessions: BTreeMap<String, SessionEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SessionEntry {
    pub repo: Option<String>,
    pub begin: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub models: Vec<String>,
    pub tokens: TokenEntry,
    pub jsonl_paths: Vec<PathBuf>,
    pub title: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TokenEntry {
    pub input: u64,
    pub output: u64,
    pub cache_5m_write: u64,
    pub cache_1h_write: u64,
    pub cache_read: u64,
    pub total: u64,
}

pub fn load_existing_titles(path: &Path) -> HashMap<String, String> {
    debug!("report::load_existing_titles: path={}", path.display());
    if !path.exists() {
        return HashMap::new();
    }
    let body = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("report: failed to read existing report at {}: {}", path.display(), e);
            return HashMap::new();
        }
    };
    let report: Report = match serde_yaml::from_str(&body) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("report: failed to parse existing report at {}: {}", path.display(), e);
            return HashMap::new();
        }
    };
    let mut out = HashMap::new();
    for (sid, entry) in report.sessions {
        if let Some(title) = entry.title {
            out.insert(sid, title);
        }
    }
    out
}

pub fn write_yaml(
    path: &Path,
    summaries: &[SessionSummary],
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    host: &str,
) -> Result<usize> {
    debug!(
        "report::write_yaml: path={} sessions={} since={} until={} host={}",
        path.display(),
        summaries.len(),
        since,
        until,
        host
    );

    let report = build_report(summaries, since, until, host);

    let yaml = serde_yaml::to_string(&report).context("failed to serialize report to YAML")?;

    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(dir).with_context(|| format!("failed to create output dir {}", dir.display()))?;

    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("failed to create temp file in {}", dir.display()))?;
    {
        use std::io::Write;
        tmp.write_all(yaml.as_bytes())
            .context("failed to write YAML to temp file")?;
        tmp.flush().context("failed to flush temp file")?;
    }
    tmp.persist(path)
        .with_context(|| format!("failed to atomically rename temp file to {}", path.display()))?;

    Ok(report.session_count)
}

fn build_report(summaries: &[SessionSummary], since: DateTime<Utc>, until: DateTime<Utc>, host: &str) -> Report {
    let mut sessions = BTreeMap::new();
    for s in summaries {
        sessions.insert(s.session_id.clone(), to_entry(s));
    }
    let session_count = sessions.len();
    Report {
        schema_version: SCHEMA_VERSION,
        generated: Utc::now(),
        host: host.to_string(),
        since,
        until,
        session_count,
        sessions,
    }
}

fn to_entry(s: &SessionSummary) -> SessionEntry {
    SessionEntry {
        repo: s.repo.clone(),
        begin: s.begin,
        end: s.end,
        models: s.models.iter().cloned().collect(),
        tokens: TokenEntry {
            input: s.tokens.input,
            output: s.tokens.output,
            cache_5m_write: s.tokens.cache_5m_write,
            cache_1h_write: s.tokens.cache_1h_write,
            cache_read: s.tokens.cache_read,
            total: s.tokens.total,
        },
        jsonl_paths: s.jsonl_paths.clone(),
        title: s.title.clone(),
    }
}

#[cfg(test)]
mod tests;
