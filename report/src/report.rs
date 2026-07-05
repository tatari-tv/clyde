use crate::outcome::{self, OutcomeTotals, Outcomes};
use crate::session::{SessionSummary, TokenTotals};
use chrono::{DateTime, Utc};
use claude_pricing::Pricing;
use eyre::{Context, Result};
use log::debug;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
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
    /// `Some(true)` when collect ran outcome extraction over every transcript in this report,
    /// `Some(false)` for `--no-outcomes` (Phase 5), `None` on JSONs from a pre-Phase-4 binary.
    /// The merge coverage rules (`merge::recompute_totals`) key on this flag, not on whether
    /// `totals.outcomes` happens to be present, so a mixed-capability merge can't read as complete.
    #[serde(default)]
    pub outcomes_enabled: Option<bool>,
    pub totals: Totals,
    pub sessions: BTreeMap<String, SessionEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Totals {
    pub sessions: usize,
    pub spend_usd: f64,
    #[serde(default)]
    pub untracked_models: Vec<String>,
    pub models: BTreeMap<String, ModelTokens>,
    /// Deduped outcome rollup (global dedupe by sha / PR url across every session). Absent on
    /// pre-Phase-4 JSONs and on a merge that mixes outcomes-capable and incapable inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcomes: Option<OutcomeTotals>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SessionEntry {
    pub title: Option<String>,
    pub repo: Option<String>,
    pub begin: DateTime<Utc>,
    pub end: DateTime<Utc>,
    #[serde(default)]
    pub spend_usd: Option<f64>,
    #[serde(default)]
    pub untracked_models: Vec<String>,
    #[serde(default)]
    pub jsonl_paths: Vec<PathBuf>,
    pub models: BTreeMap<String, ModelTokens>,
    /// Present only when at least one outcome was observed for this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcomes: Option<Outcomes>,
}

impl SessionEntry {
    /// Sum of `total` tokens across every model this session used. Shared by `aggregate` (row
    /// rollups) and `render`'s built-in/custom template paths.
    pub fn total_tokens(&self) -> u64 {
        self.models.values().map(|m| m.total).sum()
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "kebab-case")]
pub struct ModelTokens {
    pub input: u64,
    pub output: u64,
    pub cache_5m_write: u64,
    pub cache_1h_write: u64,
    pub cache_read: u64,
    pub total: u64,
    #[serde(default)]
    pub spend_usd: Option<f64>,
}

impl ModelTokens {
    pub fn from_totals(model: &str, t: &TokenTotals, pricing: &Pricing) -> Self {
        let spend_usd = match pricing.calculate_usd(model, &t.as_usage()) {
            Ok(f) => Some(round_cents(f)),
            Err(_) => None,
        };
        Self {
            input: t.input,
            output: t.output,
            cache_5m_write: t.cache_5m_write,
            cache_1h_write: t.cache_1h_write,
            cache_read: t.cache_read,
            total: t.total,
            spend_usd,
        }
    }
}

fn round_cents(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
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
    let report: Report = match serde_json::from_str(&body) {
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

/// Build the pretty-printed report JSON and the session count, without performing any I/O.
/// Shared by the file-output path ([`write_json`]) and the stdout-streaming path so both emit
/// byte-identical JSON.
pub fn build_json(
    summaries: &[SessionSummary],
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    host: &str,
    pricing: &Pricing,
) -> Result<(String, usize)> {
    debug!(
        "report::build_json: sessions={} since={} until={} host={}",
        summaries.len(),
        since,
        until,
        host
    );
    let report = build_report(summaries, since, until, host, pricing);
    let json = serde_json::to_string_pretty(&report).context("failed to serialize report to JSON")?;
    Ok((json, report.totals.sessions))
}

pub fn write_json(
    path: &Path,
    summaries: &[SessionSummary],
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    host: &str,
    pricing: &Pricing,
) -> Result<usize> {
    debug!(
        "report::write_json: path={} sessions={} since={} until={} host={}",
        path.display(),
        summaries.len(),
        since,
        until,
        host
    );

    let (json, count) = build_json(summaries, since, until, host, pricing)?;

    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(dir).with_context(|| format!("failed to create output dir {}", dir.display()))?;

    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("failed to create temp file in {}", dir.display()))?;
    {
        use std::io::Write;
        tmp.write_all(json.as_bytes())
            .context("failed to write JSON to temp file")?;
        tmp.flush().context("failed to flush temp file")?;
    }
    tmp.persist(path)
        .with_context(|| format!("failed to atomically rename temp file to {}", path.display()))?;

    Ok(count)
}

fn build_report(
    summaries: &[SessionSummary],
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    host: &str,
    pricing: &Pricing,
) -> Report {
    let mut sessions = BTreeMap::new();
    let mut totals_models: BTreeMap<String, TokenTotals> = BTreeMap::new();
    let mut untracked: BTreeSet<String> = BTreeSet::new();

    for s in summaries {
        let entry = to_entry(s, pricing);
        for name in &entry.untracked_models {
            untracked.insert(name.clone());
        }
        sessions.insert(s.session_id.clone(), entry);
        for (model, totals) in &s.models {
            totals_models.entry(model.clone()).or_default().merge(totals);
        }
    }

    let totals_model_entries: BTreeMap<String, ModelTokens> = totals_models
        .iter()
        .map(|(m, t)| (m.clone(), ModelTokens::from_totals(m, t, pricing)))
        .collect();
    let totals_spend: f64 = totals_model_entries.values().filter_map(|m| m.spend_usd).sum();

    // Outcome rollup: global dedupe by sha / PR url across every session in this report. Collect
    // always runs extraction today (the `--no-outcomes` escape hatch lands in Phase 5), so
    // `outcomes-enabled` is unconditionally `Some(true)` here.
    let outcomes_rollup = outcome::rollup(sessions.values().map(|e| e.outcomes.as_ref()));

    let totals = Totals {
        sessions: sessions.len(),
        spend_usd: round_cents(totals_spend),
        untracked_models: untracked.into_iter().collect(),
        models: totals_model_entries,
        outcomes: Some(outcomes_rollup),
    };

    Report {
        schema_version: SCHEMA_VERSION,
        generated: Utc::now(),
        host: host.to_string(),
        since,
        until,
        outcomes_enabled: Some(true),
        totals,
        sessions,
    }
}

fn to_entry(s: &SessionSummary, pricing: &Pricing) -> SessionEntry {
    let models: BTreeMap<String, ModelTokens> = s
        .models
        .iter()
        .map(|(m, t)| (m.clone(), ModelTokens::from_totals(m, t, pricing)))
        .collect();
    let mut priced_sum = 0.0_f64;
    let mut priced_count = 0usize;
    let mut untracked_models: Vec<String> = Vec::new();
    for (name, mt) in &models {
        match mt.spend_usd {
            Some(v) => {
                priced_sum += v;
                priced_count += 1;
            }
            None => untracked_models.push(name.clone()),
        }
    }
    let spend_usd = if priced_count == 0 {
        None
    } else {
        Some(round_cents(priced_sum))
    };
    SessionEntry {
        title: s.title.clone(),
        repo: s.repo.clone(),
        begin: s.begin,
        end: s.end,
        spend_usd,
        untracked_models,
        jsonl_paths: s.jsonl_paths.clone(),
        models,
        outcomes: s.outcomes.clone(),
    }
}

#[cfg(test)]
mod tests;
