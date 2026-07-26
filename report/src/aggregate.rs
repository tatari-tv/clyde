//! Pure, deterministic aggregation over a [`Report`]: by-org, by-repo, by-day, outlier, and
//! cache rollups, all pre-sorted and pre-formatted so `render`'s context block hands Opus numbers
//! to copy, never numbers to compute (design: `docs/design/2026-07-04-report-aggregates-outcomes.md`).
//!
//! Phase split (design "Architecture" section): by-org/by-repo/by-day/outliers need no pricing.
//! The cache-read-share and the list-price/cache-savings counterfactual DO need `&Pricing`, so
//! `compute` takes one and [`Aggregates`] carries a `cache` field ([`CacheStats`]). `compute` is
//! the single aggregate entry point; the counterfactual is the sole sanctioned computation.

use crate::fmt::{format_optional_usd, format_tokens_human, format_usd, short_id};
use crate::outcome::Outcomes;
use crate::report::Report;
use chrono::NaiveDate;
use claude_pricing::{Pricing, TokenUsage};
use common::repo::RepoSource;
use log::{debug, warn};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

/// Org bucket for sessions whose repo could not be detected (`SessionEntry.repo == None`). Doubles
/// as the [`Attribution`] bucket for the same sessions, deliberately: one spelling for one concept.
pub const UNATTRIBUTED_ORG: &str = "(unattributed)";

/// [`Attribution`] bucket for a session that HAS a repo but no recorded provenance: a pre-v10
/// artifact folded in by `report merge`, whose repo was resolved before `repo_source` existed.
/// Parenthesized so it cannot collide with a real `RepoSource` spelling, matching
/// [`UNATTRIBUTED_ORG`]'s precedent. Never produced from a locally-collected window: `to_collected`
/// fails loudly on a slug with no source.
pub const UNKNOWN_SOURCE: &str = "(unknown-source)";

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
    /// Bar-chart geometry (design "Chart truthfulness"): `spend-raw / max(spend-raw across this
    /// series) * 100`, one decimal, `None` (and absent from JSON) when the whole series is $0 - a
    /// series with no scale field renders as a table, never a chart.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spend_percent_of_max: Option<f64>,
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
    /// See [`OrgRow::spend_percent_of_max`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spend_percent_of_max: Option<f64>,
    pub models: Vec<String>,
    /// The STRONGEST evidence any session in this row has for the slug existing at all
    /// (`git-origin` beats `known-path` beats `files-touched` beats `path-guess`), so a row whose
    /// value is `path-guess` is one that NO session ever observed — the fabricated-sibling case
    /// (`<root>/tatari-tv/clyde-ft` guessing `tatari-tv/clyde-ft`) rule 4 can produce.
    ///
    /// Strongest, not weakest, because the question this answers is "is this repo real?", not "was
    /// every session in it observed". One guessed session among five hundred git-origin ones does
    /// not make `tatari-tv/clyde` a fabrication; per-source spend lives in `attribution`.
    ///
    /// `None` on a merged pre-v10 artifact whose sessions carry no provenance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct DayRow {
    pub date: String,
    pub sessions: usize,
    #[serde(skip)]
    pub spend_raw: f64,
    pub spend: String,
    /// See [`OrgRow::spend_percent_of_max`], scaled against the max daily spend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spend_percent_of_max: Option<f64>,
    /// Same formula as `spend-percent-of-max`, scaled against the max daily session count instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sessions_percent_of_max: Option<f64>,
}

/// Bar-chart geometry shared by every chartable aggregate row (design "Chart truthfulness"):
/// `round(value / max * 1000) / 10` - one-decimal percent-of-series-max, 0-100. `None` when `max`
/// is zero (an all-zero series has no meaningful proportion), which callers propagate straight
/// into `skip_serializing_if` so the field is ABSENT from the context JSON - the render prompt's
/// "no scale field -> table" rule then applies with no special-casing.
pub fn percent_of_max(value: f64, max: f64) -> Option<f64> {
    if max == 0.0 {
        None
    } else {
        Some((value / max * 1000.0).round() / 10.0)
    }
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
/// summed across all models, one decimal -- computed via the shared `common::cache_read_share`
/// helper so this crate and `efficiency` cannot drift on the same formula/name (design:
/// `docs/design/2026-07-22-session-efficiency-signals.md`, Phase 2). The helper returns `None`
/// only on a zero denominator; `report` preserves its own pre-existing convention of rendering
/// that as `"0.0%"` (never blank), while `efficiency` keeps the `None` and renders `n/a`. The
/// counterfactual reprices each model as if every cache token (reads AND 5m/1h writes) had been
/// fresh `input` with the cache fields zeroed, reusing the crate's own >200k tiering via
/// `Pricing::calculate_usd`, summed across priced models; `cache-savings` is that minus the
/// report's actual priced spend. If ANY model with nonzero cache tokens is unpriced, BOTH
/// counterfactual fields are `None` (fail closed: never a `$0` stand-in for an unknown).
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
        match common::metrics::price(model, &usage, pricing) {
            Some(cost) => list_price += cost,
            None if cache_tokens > 0 => {
                // A cache-bearing model with no price makes the whole counterfactual unknowable.
                debug!(
                    "aggregate::compute_cache_stats: unpriced cache-bearing model `{}`; counterfactual absent",
                    model
                );
                counterfactual_ok = false;
            }
            None => {}
        }
    }

    let share_pct = common::cache_read_share(total_input, total_cache_read, total_cache_5m, total_cache_1h)
        .map(|r| r * 100.0)
        .unwrap_or(0.0);

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
    debug!("aggregate::compute_by_org: sessions={}", report.sessions.len());
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
            spend_percent_of_max: None,
        })
        .collect();
    let max_spend = rows.iter().map(|r| r.spend_raw).fold(0.0_f64, f64::max);
    for row in &mut rows {
        row.spend_percent_of_max = percent_of_max(row.spend_raw, max_spend);
    }
    sort_by_spend_desc(&mut rows, |r| r.spend_raw);
    debug!("aggregate::compute_by_org: rows={} max-spend={}", rows.len(), max_spend);
    rows
}

#[derive(Default)]
struct RepoAcc {
    sessions: usize,
    tokens: u64,
    spend: f64,
    models: BTreeSet<String>,
    /// The best (lowest-rank) [`RepoSource`] seen for this repo; see [`RepoRow::repo_source`].
    best_source: Option<RepoSource>,
}

impl RepoAcc {
    /// Fold one session's provenance in, keeping the strongest. `RepoSource`'s derived `Ord` is
    /// confidence order (best first), so `min` IS "strongest evidence" with no rank arithmetic here.
    /// An unparseable value is ignored rather than allowed to weaken the row: it is not evidence of
    /// a guess, it is a merged artifact from before provenance existed.
    fn observe(&mut self, source: Option<&str>) {
        let Some(parsed) = source.and_then(|s| RepoSource::from_str(s).ok()) else {
            return;
        };
        self.best_source = Some(match self.best_source {
            Some(current) => current.min(parsed),
            None => parsed,
        });
    }
}

fn compute_by_repo(report: &Report) -> Vec<RepoRow> {
    debug!("aggregate::compute_by_repo: sessions={}", report.sessions.len());
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
        acc.observe(entry.repo_source.as_deref());
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
                spend_percent_of_max: None,
                models: acc.models.into_iter().collect(),
                repo_source: acc.best_source.map(|s| s.as_str().to_string()),
            }
        })
        .collect();
    let max_spend = rows.iter().map(|r| r.spend_raw).fold(0.0_f64, f64::max);
    for row in &mut rows {
        row.spend_percent_of_max = percent_of_max(row.spend_raw, max_spend);
    }
    sort_by_spend_desc(&mut rows, |r| r.spend_raw);
    debug!(
        "aggregate::compute_by_repo: rows={} max-spend={}",
        rows.len(),
        max_spend
    );
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
    debug!("aggregate::compute_by_day: sessions={}", report.sessions.len());
    let since_date = report.since.date_naive();
    let until_date = report.until.date_naive();
    let mut days: BTreeMap<NaiveDate, DayAcc> = BTreeMap::new();
    for entry in report.sessions.values() {
        let date = entry.begin.date_naive().clamp(since_date, until_date);
        let acc = days.entry(date).or_default();
        acc.sessions += 1;
        acc.spend += entry.spend_usd.unwrap_or(0.0);
    }
    let mut rows: Vec<DayRow> = days
        .into_iter()
        .map(|(date, acc)| DayRow {
            date: date.format("%Y-%m-%d").to_string(),
            sessions: acc.sessions,
            spend_raw: acc.spend,
            spend: format_usd(acc.spend),
            spend_percent_of_max: None,
            sessions_percent_of_max: None,
        })
        .collect();
    let max_spend = rows.iter().map(|r| r.spend_raw).fold(0.0_f64, f64::max);
    let max_sessions = rows.iter().map(|r| r.sessions).max().unwrap_or(0);
    for row in &mut rows {
        row.spend_percent_of_max = percent_of_max(row.spend_raw, max_spend);
        row.sessions_percent_of_max = percent_of_max(row.sessions as f64, max_sessions as f64);
    }
    debug!(
        "aggregate::compute_by_day: rows={} max-spend={} max-sessions={}",
        rows.len(),
        max_spend,
        max_sessions
    );
    rows
}

/// How much of the window's money carries a repo, and on what evidence.
///
/// This is what lets the prose state coverage as a FACT instead of the report quietly presenting a
/// fraction of the money as if it were all of it. [`Attribution::rows`] sum to `totals.spend-usd`
/// by construction (see [`compute_attribution`]), so a reader can check the partition themselves.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Attribution {
    /// One row per observed `repo-source`, plus [`UNATTRIBUTED_ORG`]. Pre-sorted by spend
    /// descending, ties broken by source name for determinism.
    pub rows: Vec<AttributionRow>,
    /// Display: spend carrying a repo, on ANY evidence.
    pub covered: String,
    /// Display: spend still carrying none.
    pub uncovered: String,
    /// Display percent: `covered` as a share of `totals.spend-usd`.
    pub covered_share: String,
}

/// One `repo-source` bucket of the window's spend.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct AttributionRow {
    /// `git-origin` | `known-path` | `files-touched` | `path-guess` | [`UNKNOWN_SOURCE`] |
    /// [`UNATTRIBUTED_ORG`].
    pub source: String,
    pub sessions: usize,
    /// Raw bucket spend, kept ONLY to sort the rows; not serialized, so no numeric operand reaches
    /// the model (the string-only context rule).
    #[serde(skip)]
    pub spend_raw: f64,
    pub spend: String,
    /// How much weight the row's evidence carries: `observed` (rules 1 and 2 both SAW the repo),
    /// `inferred` (rule 3 read it off the files the session edited), `guessed` (rule 4 pattern-matched
    /// a path and may have invented the slug), `unknown`, `unattributed`.
    pub confidence: String,
}

/// The confidence word for a `repo-source`, spelled once here so the vocabulary cannot drift between
/// the row and the prompt that quotes it.
fn confidence_of(source: RepoSource) -> &'static str {
    match source {
        RepoSource::GitOrigin | RepoSource::KnownPath => "observed",
        RepoSource::FilesTouched => "inferred",
        RepoSource::PathGuess => "guessed",
    }
}

/// Partition `totals.spend-usd` across the rules that resolved each session's repo.
///
/// The buckets are measured from the per-session spends, with ONE deliberate adjustment: the
/// `(unattributed)` row absorbs the difference between `totals.spend-usd` and the sum of the
/// per-session spends, so the rows sum to the headline figure exactly. That difference is a pricing
/// artifact, not attribution: `totals.spend-usd` prices the UNIONED per-model token counts once,
/// while each session is priced on its own, and the two disagree whenever a model's long-context
/// (>200k) tier is crossed by the union but not by an individual session. Folding it anywhere else
/// would attribute money to a repo that no session's own price supports; folding it into
/// "unattributed" says exactly what it is. It is WARNed when it exceeds a cent, so it can never grow
/// unnoticed into a number that matters.
pub fn compute_attribution(report: &Report) -> Attribution {
    debug!("aggregate::compute_attribution: sessions={}", report.sessions.len());
    let mut buckets: BTreeMap<String, (usize, f64)> = BTreeMap::new();
    let mut measured_total = 0.0_f64;
    let mut covered = 0.0_f64;

    for entry in report.sessions.values() {
        let spend = entry.spend_usd.unwrap_or(0.0);
        measured_total += spend;
        let key = match (&entry.repo, entry.repo_source.as_deref()) {
            (None, _) => UNATTRIBUTED_ORG.to_string(),
            (Some(_), Some(src)) => match RepoSource::from_str(src) {
                Ok(parsed) => {
                    covered += spend;
                    parsed.as_str().to_string()
                }
                Err(_) => {
                    covered += spend;
                    UNKNOWN_SOURCE.to_string()
                }
            },
            (Some(_), None) => {
                covered += spend;
                UNKNOWN_SOURCE.to_string()
            }
        };
        let bucket = buckets.entry(key).or_insert((0, 0.0));
        bucket.0 += 1;
        bucket.1 += spend;
    }

    // Reconcile to the headline figure. See the doc comment: this is a pricing-basis residual, and
    // the unattributed bucket is where an unattributable dollar belongs.
    let residual = report.totals.spend_usd - measured_total;
    if residual.abs() > 0.01 {
        warn!(
            "aggregate::compute_attribution: per-session spends sum to {measured_total:.2} but \
             totals.spend-usd is {:.2}; the {residual:.2} difference (long-context tiering across \
             the unioned token counts) is carried in the ({UNATTRIBUTED_ORG}) bucket",
            report.totals.spend_usd
        );
    }
    if residual != 0.0 {
        let bucket = buckets.entry(UNATTRIBUTED_ORG.to_string()).or_insert((0, 0.0));
        bucket.1 += residual;
    }
    let uncovered = report.totals.spend_usd - covered;

    let mut rows: Vec<AttributionRow> = buckets
        .into_iter()
        .map(|(source, (sessions, spend))| {
            let confidence = match RepoSource::from_str(&source) {
                Ok(parsed) => confidence_of(parsed).to_string(),
                Err(_) if source == UNATTRIBUTED_ORG => "unattributed".to_string(),
                Err(_) => "unknown".to_string(),
            };
            AttributionRow {
                source,
                sessions,
                spend_raw: spend,
                spend: format_usd(spend),
                confidence,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.spend_raw
            .partial_cmp(&a.spend_raw)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.source.cmp(&b.source))
    });

    let covered_share = if report.totals.spend_usd == 0.0 {
        "0.0%".to_string()
    } else {
        format!("{:.1}%", covered / report.totals.spend_usd * 100.0)
    };
    debug!(
        "aggregate::compute_attribution: rows={} covered={covered:.2} uncovered={uncovered:.2} share={covered_share}",
        rows.len()
    );
    Attribution {
        rows,
        covered: format_usd(covered),
        uncovered: format_usd(uncovered),
        covered_share,
    }
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
                short_id: short_id(sid).to_string(),
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
