//! Pure, deterministic aggregation over a [`Report`]: by-org, by-repo, by-day, outlier, and
//! cache rollups, all pre-sorted and pre-formatted so `render`'s context block hands Opus numbers
//! to copy, never numbers to compute (design: `docs/design/2026-07-04-report-aggregates-outcomes.md`).
//!
//! Phase split (design "Architecture" section): by-org/by-repo/by-day/outliers need no pricing.
//! The cache-read-share and the list-price/cache-savings counterfactual DO need `&Pricing`, so
//! `compute` takes one and [`Aggregates`] carries a `cache` field ([`CacheStats`]). `compute` is
//! the single aggregate entry point; the counterfactual is the sole sanctioned computation.

use crate::fmt::{format_optional_usd, format_tokens_human, format_usd};
use crate::outcome::Outcomes;
use crate::report::Report;
use chrono::NaiveDate;
use claude_pricing::{Pricing, TokenUsage};
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
    pub cache: CacheStats,
}

/// Cache-efficiency rollup (design "Data Model" / Definitions sections). The two counterfactual
/// fields are `None` (and, via `skip_serializing_if`, ABSENT from the context JSON) when any model
/// with nonzero cache tokens is unpriced: never emit `$0` for an unknown.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct CacheStats {
    /// `cache_read / (input + cache_read + cache_5m_write + cache_1h_write)` summed across all
    /// models, one decimal (e.g. `"96.0%"`).
    pub cache_read_share: String,
    pub input_tokens_human: String,
    pub cache_read_tokens_human: String,
    /// "What if every token were fresh input": all cache tokens folded into `input`, summed across
    /// priced models. `None` when any cache-bearing model is unpriced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_price_equivalent: Option<String>,
    /// `list_price_equivalent` minus actual priced spend. `None` under the same condition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_savings: Option<String>,
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
    /// The session's observed outcomes, when extraction ran and found any; backs the outlier
    /// table's "What it produced" column (prompt: "outcome fields when available").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcomes: Option<Outcomes>,
}

/// Compute all render-time aggregates over `report`. `outliers_n` caps the outlier table (0 is
/// legal: an empty table). `pricing` backs the cache counterfactual only (the sole sanctioned
/// computation); every other rollup is pure over the report's own token/spend numbers.
pub fn compute(report: &Report, outliers_n: usize, pricing: &Pricing) -> Aggregates {
    debug!(
        "aggregate::compute: sessions={} models={} outliers-n={}",
        report.sessions.len(),
        report.totals.models.len(),
        outliers_n
    );
    let by_org = compute_by_org(report);
    let by_repo = compute_by_repo(report);
    let by_day = compute_by_day(report);
    let outliers = compute_outliers(report, outliers_n);
    let cache = compute_cache_stats(report, pricing);
    debug!(
        "aggregate::compute: by-org={} by-repo={} by-day={} outliers={} cache-read-share={} counterfactual={}",
        by_org.len(),
        by_repo.len(),
        by_day.len(),
        outliers.len(),
        cache.cache_read_share,
        cache.list_price_equivalent.is_some(),
    );
    Aggregates {
        by_org,
        by_repo,
        by_day,
        outliers,
        cache,
    }
}

/// Cache-efficiency rollup and the sanctioned list-price counterfactual (design Definitions).
///
/// `cache-read-share` is `cache_read / (input + cache_read + cache_5m_write + cache_1h_write)`
/// summed across all models, one decimal. The counterfactual reprices each model as if every
/// cache token (reads AND 5m/1h writes) had been fresh `input` with the cache fields zeroed,
/// reusing the crate's own >200k tiering via `Pricing::calculate_usd`, summed across priced
/// models; `cache-savings` is that minus the report's actual priced spend. If ANY model with
/// nonzero cache tokens is unpriced, BOTH counterfactual fields are `None` (fail closed: never a
/// `$0` stand-in for an unknown).
fn compute_cache_stats(report: &Report, pricing: &Pricing) -> CacheStats {
    debug!(
        "aggregate::compute_cache_stats: models={} actual-spend={}",
        report.totals.models.len(),
        report.totals.spend_usd
    );
    let mut total_input: u64 = 0;
    let mut total_cache_read: u64 = 0;
    let mut total_cache_5m: u64 = 0;
    let mut total_cache_1h: u64 = 0;

    let mut list_price = 0.0_f64;
    let mut counterfactual_ok = true;

    for (model, m) in &report.totals.models {
        total_input += m.input;
        total_cache_read += m.cache_read;
        total_cache_5m += m.cache_5m_write;
        total_cache_1h += m.cache_1h_write;

        let cache_tokens = m.cache_read + m.cache_5m_write + m.cache_1h_write;
        // "What if every token were fresh input": fold ALL cache tokens into `input`, zero the
        // cache fields. Without caching those writes would not exist either.
        let usage = TokenUsage {
            input_tokens: m.input + cache_tokens,
            output_tokens: m.output,
            cache_5m_write_tokens: 0,
            cache_1h_write_tokens: 0,
            cache_read_tokens: 0,
        };
        match pricing.calculate_usd(model, &usage) {
            Ok(cost) => list_price += cost,
            Err(_) if cache_tokens > 0 => {
                // A cache-bearing model with no price makes the whole counterfactual unknowable.
                debug!(
                    "aggregate::compute_cache_stats: unpriced cache-bearing model `{}`; counterfactual absent",
                    model
                );
                counterfactual_ok = false;
            }
            Err(_) => {}
        }
    }

    let denom = total_input + total_cache_read + total_cache_5m + total_cache_1h;
    let share_pct = if denom == 0 {
        0.0
    } else {
        (total_cache_read as f64 / denom as f64) * 100.0
    };

    let (list_price_equivalent, cache_savings) = if counterfactual_ok {
        (
            Some(format_usd(list_price)),
            Some(format_usd(list_price - report.totals.spend_usd)),
        )
    } else {
        (None, None)
    };

    CacheStats {
        cache_read_share: format!("{:.1}%", share_pct),
        input_tokens_human: format_tokens_human(total_input),
        cache_read_tokens_human: format_tokens_human(total_cache_read),
        list_price_equivalent,
        cache_savings,
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
                outcomes: entry.outcomes.clone(),
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
