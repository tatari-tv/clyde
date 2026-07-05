//! Pure, deterministic aggregation over a [`Report`]: by-org, by-repo, by-day, and outlier
//! rollups, all pre-sorted and pre-formatted so `render`'s context block hands Opus numbers to
//! copy, never numbers to compute (design: `docs/design/2026-07-04-report-aggregates-outcomes.md`).
//!
//! Phase split (design "Architecture" section): cache-read-share and the list-price/
//! cache-savings counterfactual need `&Pricing` and land in Phase 2, which will add a `cache`
//! field to [`Aggregates`] and change this module's signature. This phase's `compute` takes
//! only `&Report` and the outlier count.

use crate::fmt::{format_optional_usd, format_tokens_human, format_usd};
use crate::report::Report;
use chrono::NaiveDate;
use log::debug;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Org bucket for sessions whose repo could not be detected (`SessionEntry.repo == None`).
pub const UNATTRIBUTED_ORG: &str = "(unattributed)";

/// Default outlier-table size until Phase 5 wires `--outliers <N>` through to this value.
pub const DEFAULT_OUTLIERS: usize = 10;

/// Render-time-only aggregation over a [`Report`]. Never persisted; rebuilt on every render from
/// the (possibly merged) report JSON.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Aggregates {
    pub by_org: Vec<OrgRow>,
    pub by_repo: Vec<RepoRow>,
    pub by_day: Vec<DayRow>,
    pub outliers: Vec<OutlierRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct OrgRow {
    pub org: String,
    pub repos: usize,
    pub sessions: usize,
    #[serde(skip)]
    pub tokens: u64,
    pub tokens_human: String,
    #[serde(skip)]
    pub spend_raw: f64,
    pub spend: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RepoRow {
    pub repo: String,
    pub org: String,
    pub sessions: usize,
    #[serde(skip)]
    pub tokens: u64,
    pub tokens_human: String,
    #[serde(skip)]
    pub spend_raw: f64,
    pub spend: String,
    pub models: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct DayRow {
    pub date: String,
    pub sessions: usize,
    #[serde(skip)]
    pub spend_raw: f64,
    pub spend: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct OutlierRow {
    pub short_id: String,
    pub title: Option<String>,
    pub repo: Option<String>,
    #[serde(skip)]
    pub tokens: u64,
    pub tokens_human: String,
    #[serde(skip)]
    pub spend_raw: Option<f64>,
    pub spend: String,
}

/// Compute all render-time aggregates over `report`. `outliers_n` caps the outlier table (0 is
/// legal: an empty table).
pub fn compute(report: &Report, outliers_n: usize) -> Aggregates {
    debug!(
        "aggregate::compute: sessions={} outliers-n={}",
        report.sessions.len(),
        outliers_n
    );
    let by_org = compute_by_org(report);
    let by_repo = compute_by_repo(report);
    let by_day = compute_by_day(report);
    let outliers = compute_outliers(report, outliers_n);
    debug!(
        "aggregate::compute: by-org={} by-repo={} by-day={} outliers={}",
        by_org.len(),
        by_repo.len(),
        by_day.len(),
        outliers.len()
    );
    Aggregates {
        by_org,
        by_repo,
        by_day,
        outliers,
    }
}

/// `repo.split_once('/')` first component per the Definitions section; `None` repos (and repos
/// with no `/`, defensively) fall into a bucket rather than panicking or losing the session.
fn org_of(repo: Option<&str>) -> String {
    match repo {
        Some(r) => r
            .split_once('/')
            .map(|(org, _)| org.to_string())
            .unwrap_or_else(|| r.to_string()),
        None => UNATTRIBUTED_ORG.to_string(),
    }
}

fn sort_by_spend_desc<T>(rows: &mut [T], spend: impl Fn(&T) -> f64) {
    rows.sort_by(|a, b| spend(b).partial_cmp(&spend(a)).unwrap_or(std::cmp::Ordering::Equal));
}

#[derive(Default)]
struct OrgAcc {
    repos: BTreeSet<String>,
    sessions: usize,
    tokens: u64,
    spend: f64,
}

fn compute_by_org(report: &Report) -> Vec<OrgRow> {
    let mut orgs: BTreeMap<String, OrgAcc> = BTreeMap::new();
    for entry in report.sessions.values() {
        let org = org_of(entry.repo.as_deref());
        let acc = orgs.entry(org).or_default();
        if let Some(repo) = &entry.repo {
            acc.repos.insert(repo.clone());
        }
        acc.sessions += 1;
        acc.tokens += entry.total_tokens();
        acc.spend += entry.spend_usd.unwrap_or(0.0);
    }
    let mut rows: Vec<OrgRow> = orgs
        .into_iter()
        .map(|(org, acc)| OrgRow {
            org,
            repos: acc.repos.len(),
            sessions: acc.sessions,
            tokens: acc.tokens,
            tokens_human: format_tokens_human(acc.tokens),
            spend_raw: acc.spend,
            spend: format_usd(acc.spend),
        })
        .collect();
    sort_by_spend_desc(&mut rows, |r| r.spend_raw);
    rows
}

#[derive(Default)]
struct RepoAcc {
    sessions: usize,
    tokens: u64,
    spend: f64,
    models: BTreeSet<String>,
}

fn compute_by_repo(report: &Report) -> Vec<RepoRow> {
    let mut repos: BTreeMap<String, RepoAcc> = BTreeMap::new();
    for entry in report.sessions.values() {
        let Some(repo) = entry.repo.as_deref() else {
            continue;
        };
        let acc = repos.entry(repo.to_string()).or_default();
        acc.sessions += 1;
        acc.tokens += entry.total_tokens();
        acc.spend += entry.spend_usd.unwrap_or(0.0);
        acc.models.extend(entry.models.keys().cloned());
    }
    let mut rows: Vec<RepoRow> = repos
        .into_iter()
        .map(|(repo, acc)| {
            let org = org_of(Some(&repo));
            RepoRow {
                repo,
                org,
                sessions: acc.sessions,
                tokens: acc.tokens,
                tokens_human: format_tokens_human(acc.tokens),
                spend_raw: acc.spend,
                spend: format_usd(acc.spend),
                models: acc.models.into_iter().collect(),
            }
        })
        .collect();
    sort_by_spend_desc(&mut rows, |r| r.spend_raw);
    rows
}

#[derive(Default)]
struct DayAcc {
    sessions: usize,
    spend: f64,
}

/// By-day attribution per the Definitions section: a session's counts and spend attribute to its
/// `begin` UTC date, CLAMPED into `[since, until]` (as dates). This is defensive: it never trusts
/// that a `SessionEntry.begin` already lies in period (a boundary fixture pins this), because
/// otherwise a session begun before `since` with in-period tokens would leak an out-of-period
/// date into a citation-bearing table. Only active days (>= 1 session) appear.
fn compute_by_day(report: &Report) -> Vec<DayRow> {
    let since_date = report.since.date_naive();
    let until_date = report.until.date_naive();
    let mut days: BTreeMap<NaiveDate, DayAcc> = BTreeMap::new();
    for entry in report.sessions.values() {
        let date = entry.begin.date_naive().clamp(since_date, until_date);
        let acc = days.entry(date).or_default();
        acc.sessions += 1;
        acc.spend += entry.spend_usd.unwrap_or(0.0);
    }
    days.into_iter()
        .map(|(date, acc)| DayRow {
            date: date.format("%Y-%m-%d").to_string(),
            sessions: acc.sessions,
            spend_raw: acc.spend,
            spend: format_usd(acc.spend),
        })
        .collect()
}

/// Top-`outliers_n` sessions by spend (untracked/unpriced sessions rank as $0, ties broken by
/// short-id for determinism).
fn compute_outliers(report: &Report, outliers_n: usize) -> Vec<OutlierRow> {
    let mut rows: Vec<OutlierRow> = report
        .sessions
        .iter()
        .map(|(sid, entry)| {
            let tokens = entry.total_tokens();
            OutlierRow {
                short_id: sid.get(..8).unwrap_or(sid).to_string(),
                title: entry.title.clone(),
                repo: entry.repo.clone(),
                tokens,
                tokens_human: format_tokens_human(tokens),
                spend_raw: entry.spend_usd,
                spend: format_optional_usd(entry.spend_usd),
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.spend_raw
            .unwrap_or(0.0)
            .partial_cmp(&a.spend_raw.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.short_id.cmp(&b.short_id))
    });
    rows.truncate(outliers_n);
    rows
}

#[cfg(test)]
mod tests;
