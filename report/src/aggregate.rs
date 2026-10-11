//! Pure, deterministic aggregation over a [`Report`]: by-org, by-repo, by-day, outlier, and
//! cache rollups, all pre-sorted and pre-formatted so `render`'s context block hands Opus numbers
//! to copy, never numbers to compute (design: `docs/design/2026-07-04-report-aggregates-outcomes.md`).
//!
//! Phase split (design "Architecture" section): by-org/by-repo/by-day/outliers need no pricing.
//! The cache-read-share and the list-price/cache-savings counterfactual DO need `&Pricing`, so
//! `compute` takes one and [`Aggregates`] carries a `cache` field ([`CacheStats`]). `compute` is
//! the single aggregate entry point; the counterfactual is the sole sanctioned computation.

use crate::cents;
use crate::chart::{self, Charts};
use crate::fmt::{format_optional_usd, format_tokens_human, format_usd, short_id};
use crate::outcome::{self, Outcomes};
use crate::report::{Report, SessionEntry};
use chrono::NaiveDate;
use claude_pricing::{Pricing, TokenUsage};
use common::repo::RepoSource;
use log::{debug, trace};
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
    /// How the daily series reconciles with the headline: what it shows, what was spent on days
    /// outside the window by sessions the window holds, and how much of it sits on a begin day
    /// because the session has no per-day buckets.
    pub day_coverage: DayCoverage,
    /// Precomputed line-chart geometry over [`Self::by_day`] (design Phase 11): the `viewBox` and
    /// `points` strings the model copies verbatim into one `<svg>`/`<polyline>`, plus their axis
    /// labels. A chart is ABSENT when its series cannot honestly be drawn (see [`chart::Charts`]).
    pub charts: Charts,
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
    /// Raw twin of [`Self::list_price_equivalent`], whole cents. Never serialized.
    #[serde(skip)]
    pub list_price_cents: Option<i64>,
    /// Raw twin of [`Self::cache_savings`], whole cents: exactly `list_price_cents - spend_cents`,
    /// and the display string is formatted from it, so the waterfall's three figures reconcile to
    /// the cent. Negative when the 1h write premium outweighs the read savings.
    #[serde(skip)]
    pub cache_savings_cents: Option<i64>,
    /// Raw twin of the headline spend (`totals.spend-usd`), whole cents.
    #[serde(skip)]
    pub spend_cents: i64,
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
    /// [`Self::spend_cents`] in dollars, the operand chart geometry scales by. Never serialized.
    #[serde(skip)]
    pub spend_raw: f64,
    /// The sum of this org's sessions' spend in whole cents, the same per-session cents the header
    /// adds, so the org rows sum to the headline exactly. Never serialized.
    #[serde(skip)]
    pub spend_cents: i64,
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
    /// See [`OrgRow::spend_raw`].
    #[serde(skip)]
    pub spend_raw: f64,
    /// See [`OrgRow::spend_cents`].
    #[serde(skip)]
    pub spend_cents: i64,
    pub spend: String,
    /// See [`OrgRow::spend_percent_of_max`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spend_percent_of_max: Option<f64>,
    /// What this repo's spend PRODUCED (design Phase 7, gap 8: "`RepoRow` carries no outcomes, so
    /// spend against output per repo cannot be charted"). Absent when the repo observed no outcome
    /// at all, so a zero can never be mistaken for an observation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcomes: Option<RepoOutcomes>,
    /// See [`OrgRow::spend_percent_of_max`], scaled against the max `outcomes.commits` across
    /// `by-repo`. This is the second dimension a spend-against-output chart needs: the spend bar
    /// and the commit bar for one repo are both verbatim copies, so the comparison is drawable
    /// without the model computing a coordinate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commits_percent_of_max: Option<f64>,
    /// Same, scaled against the max `outcomes.prs-opened` across `by-repo`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prs_percent_of_max: Option<f64>,
    pub models: Vec<String>,
    /// The STRONGEST evidence any session in this row has for the slug existing at all
    /// (`git-origin` beats `known-path` beats `files-touched` beats `path-guess`), so a row whose
    /// value is `path-guess` is one that NO session ever observed -- the fabricated-sibling case
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

/// One repo's share of the window's observed output, built by the SAME [`outcome::rollup`] the
/// report-wide totals use, restricted to the sessions attributed to this repo. So commits dedupe by
/// sha and PRs by url WITHIN the row exactly as they do globally.
///
/// **These rows are never summed.** A commit sha or PR url observed in sessions attributed to two
/// different repos counts once in EACH row and once in `totals.outcomes`, which is the deduped
/// global figure. Summing the rows would double-count it; that is why the prompts cite
/// `totals.outcomes` for any period-wide output figure and the per-repo counts only per repo.
///
/// Attribution is by the SESSION's repo, the same key the row's spend is bucketed under, so the two
/// numbers in a spend-against-output comparison describe the same set of sessions. A session in one
/// repo that opened a PR against another therefore lands under the session's repo; `prs[].repository`
/// carries the PR's own slug for anyone who needs the other view.
///
/// Confluence/Jira/Slack writes are deliberately absent: they are not repo-scoped work, so a
/// per-repo row is the wrong place to count them. They stay in `outcomes.totals`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RepoOutcomes {
    /// Distinct commit shas across this repo's sessions.
    pub commits: u64,
    /// Distinct PR urls opened across this repo's sessions.
    pub prs_opened: u64,
    /// Sum of each session's own distinct-file count.
    pub files_edited: u64,
    /// Lines of file content written (see `efficiency::Outcomes::lines_written`).
    pub lines_written: u64,
    /// Lines of file content replaced (see `efficiency::Outcomes::lines_replaced`).
    pub lines_replaced: u64,
}

impl RepoOutcomes {
    /// `true` when nothing at all was observed for the repo, which is what keeps
    /// [`RepoRow::outcomes`] ABSENT rather than a row of zeroes.
    fn is_empty(&self) -> bool {
        self.commits == 0
            && self.prs_opened == 0
            && self.files_edited == 0
            && self.lines_written == 0
            && self.lines_replaced == 0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct DayRow {
    pub date: String,
    /// Sessions ACTIVE on this day: they spent tokens on it (from their per-day buckets), or it is
    /// the begin day of a session with no buckets or no tokens. `0` on an inactive day. A multi-day
    /// session counts on each of its days, so these do not sum to `totals.sessions`.
    pub sessions: usize,
    #[serde(skip)]
    pub spend_raw: f64,
    pub spend: String,
    /// `false` on a zero-fill row: this calendar date fell inside `[since, until]` but no session
    /// was active on it. The prompt may cite these to name a multi-day gap (design finding 3)
    /// instead of inferring one from a missing date -- there is no longer a missing date to infer
    /// from.
    pub active: bool,
    /// See [`OrgRow::spend_percent_of_max`], scaled against the max daily spend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spend_percent_of_max: Option<f64>,
    /// Same formula as `spend-percent-of-max`, scaled against the max daily session count instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sessions_percent_of_max: Option<f64>,
    /// Raw twin of [`Self::spend`], whole cents: the sum of every session's allocated cents on this
    /// day. Never serialized.
    #[serde(skip)]
    pub spend_cents: i64,
    /// This day's cents per model, for the stacked daily chart. Never serialized.
    #[serde(skip)]
    pub model_cents: BTreeMap<String, i64>,
    /// `cache_read / (input + cache_read + cache writes)` over this day's tokens; `None` on a day
    /// with no tokens (no share to speak of, not a zero share). Never serialized.
    #[serde(skip)]
    pub cache_read_share: Option<f64>,
}

/// The daily series against the headline (design "Per-day buckets"): every cent of `totals.spend`
/// is on a `by-day` row OR in [`Self::out_of_window`], so the two figures reconcile on the page.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct DayCoverage {
    /// The spend the `by-day` rows show (their sum).
    pub in_window: String,
    /// Spend the window's sessions made on days before `since` or after `until`. Real spend in
    /// the headline, on no row.
    pub out_of_window: String,
    /// Sessions with any spend on a day outside the window.
    pub out_of_window_sessions: usize,
    /// Sessions with no per-day buckets (their transcripts were gone when the catalog learned to
    /// bucket), placed whole on their begin day.
    pub unbucketed_sessions: usize,
    /// Those sessions' spend.
    pub unbucketed_spend: String,
    #[serde(skip)]
    pub in_window_cents: i64,
    #[serde(skip)]
    pub out_of_window_cents: i64,
    #[serde(skip)]
    pub unbucketed_cents: i64,
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
    let (by_day, day_coverage) = compute_by_day(report);
    let charts = chart::compute_charts(&by_day);
    let outliers = compute_outliers(report, outliers_n);
    let cache = compute_cache_stats(report, pricing);
    debug!(
        "aggregate::compute: by-org={} by-repo={} by-day={} out-of-window-sessions={} outliers={} \
         cache-read-share={} counterfactual={} spend-chart={} sessions-chart={}",
        by_org.len(),
        by_repo.len(),
        by_day.len(),
        day_coverage.out_of_window_sessions,
        outliers.len(),
        cache.cache_read_share,
        cache.list_price_equivalent.is_some(),
        charts.by_day_spend.is_some(),
        charts.by_day_sessions.is_some(),
    );
    Aggregates {
        by_org,
        by_repo,
        by_day,
        day_coverage,
        charts,
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
/// that as `"0.0%"` (never blank), while `efficiency` keeps the `None` and renders `n/a`.
///
/// The counterfactual is priced on the SAME basis as the spend it is compared with: per session,
/// per model, in whole cents, then summed (see [`session_list_cents`]). Each model's tokens are
/// repriced as if every cache token (reads AND 5m/1h writes) had been fresh `input` with the cache
/// fields zeroed. Folding cache into input leaves the model's total input unchanged, so each slice
/// lands on the long-context tier its own session's spend was priced at, never the tier a month of
/// union input would pick. `cache-savings` is list cents minus spend cents, so a window that used
/// no cache saves exactly `$0.00`. If ANY model with nonzero cache tokens is unpriced, BOTH
/// counterfactual fields are `None` (fail closed: never a `$0` stand-in for an unknown).
fn compute_cache_stats(report: &Report, pricing: &Pricing) -> CacheStats {
    debug!(
        "aggregate::compute_cache_stats: sessions={} models={} actual-spend={}",
        report.sessions.len(),
        report.totals.models.len(),
        report.totals.spend_usd
    );
    let mut total_input: u64 = 0;
    let mut total_cache_read: u64 = 0;
    let mut total_cache_5m: u64 = 0;
    let mut total_cache_1h: u64 = 0;
    for m in report.totals.models.values() {
        total_input += m.input;
        total_cache_read += m.cache_read;
        total_cache_5m += m.cache_5m_write;
        total_cache_1h += m.cache_1h_write;
    }

    let mut list_price_cents: Option<i64> = Some(0);
    for entry in report.sessions.values() {
        list_price_cents = match (list_price_cents, session_list_cents(entry, pricing)) {
            (Some(sum), Some(c)) => Some(sum + c),
            _ => None,
        };
    }

    let share_pct = common::cache_read_share(total_input, total_cache_read, total_cache_5m, total_cache_1h)
        .map(|r| r * 100.0)
        .unwrap_or(0.0);

    // Whole cents first, then every display string from those cents: savings are the difference of
    // the two cent sums, so list price minus savings is the displayed spend exactly.
    let spend_cents = crate::report::session_cents_total(report.sessions.values());
    let cache_savings_cents = list_price_cents.map(|list| list - spend_cents);
    let list_price_equivalent = list_price_cents.map(|c| format_usd(cents::to_dollars(c)));
    let cache_savings = cache_savings_cents.map(|c| format_usd(cents::to_dollars(c)));
    debug!(
        "aggregate::compute_cache_stats: list-cents={list_price_cents:?} spend-cents={spend_cents} \
         savings-cents={cache_savings_cents:?}"
    );

    CacheStats {
        cache_read_share: format!("{:.1}%", share_pct),
        input_tokens_human: format_tokens_human(total_input),
        cache_read_tokens_human: format_tokens_human(total_cache_read),
        list_price_equivalent,
        cache_savings,
        list_price_cents,
        cache_savings_cents,
        spend_cents,
    }
}

/// One session's list price ("every token fresh input") in whole cents: the sum of each model's
/// counterfactual, each rounded to cents the way that model's spend was ([`ModelTokens::spend_usd`]
/// is `round_cents` of its price). A model with no cache tokens has a counterfactual identical to
/// its actual usage, so its list cents ARE its recorded spend cents: no repricing, and no drift if
/// the feed moved since collect. `None` when a model with cache tokens is unpriced, in the artifact
/// or in `pricing` (the comparison is unknowable); an unpriced model with no cache tokens adds zero
/// to both sides.
///
/// [`ModelTokens::spend_usd`]: crate::report::ModelTokens::spend_usd
fn session_list_cents(entry: &SessionEntry, pricing: &Pricing) -> Option<i64> {
    let mut total: i64 = 0;
    for (model, m) in &entry.models {
        let cache_tokens = m.cache_read + m.cache_5m_write + m.cache_1h_write;
        if cache_tokens == 0 {
            total += m.spend_usd.map_or(0, cents::to_cents);
            continue;
        }
        if m.spend_usd.is_none() {
            // The artifact has no actual spend to compare a list price with for this model.
            debug!(
                "aggregate::session_list_cents: cache-bearing model `{model}` has no recorded spend; counterfactual absent"
            );
            return None;
        }
        // "What if every token were fresh input": fold ALL cache tokens into `input`, zero the
        // cache fields. Without caching those writes would not exist either.
        let usage = TokenUsage {
            input_tokens: m.input + cache_tokens,
            output_tokens: m.output,
            cache_5m_write_tokens: 0,
            cache_1h_write_tokens: 0,
            cache_read_tokens: 0,
        };
        let Some(cost) = common::metrics::price(model, &usage, pricing) else {
            debug!("aggregate::session_list_cents: unpriced cache-bearing model `{model}`; counterfactual absent");
            return None;
        };
        total += cents::to_cents(cents::round_cents(cost));
    }
    trace!(
        "aggregate::session_list_cents: models={} list-cents={total}",
        entry.models.len()
    );
    Some(total)
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
    cents: i64,
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
        acc.cents += session_cents(entry);
    }
    let mut rows: Vec<OrgRow> = orgs
        .into_iter()
        .map(|(org, acc)| OrgRow {
            org,
            repos: acc.repos.len(),
            sessions: acc.sessions,
            tokens: acc.tokens,
            tokens_human: format_tokens_human(acc.tokens),
            spend_raw: cents::to_dollars(acc.cents),
            spend_cents: acc.cents,
            spend: format_usd(cents::to_dollars(acc.cents)),
            spend_percent_of_max: None,
        })
        .collect();
    let max_spend = rows.iter().map(|r| r.spend_raw).fold(0.0_f64, f64::max);
    for row in &mut rows {
        row.spend_percent_of_max = percent_of_max(row.spend_raw, max_spend);
    }
    // ALWAYS a complete partition, unlike `by-repo`: a session with no repo lands in this table's
    // own `(unattributed)` org bucket rather than being dropped. Each row adds the same per-session
    // cents the headline adds, so the rows sum to it exactly with no display-time reallocation.
    sort_by_spend_desc(&mut rows, |r| r.spend_raw);
    debug!("aggregate::compute_by_org: rows={} max-spend={}", rows.len(), max_spend);
    rows
}

#[derive(Default)]
struct RepoAcc<'a> {
    sessions: usize,
    tokens: u64,
    cents: i64,
    models: BTreeSet<String>,
    /// The best (lowest-rank) [`RepoSource`] seen for this repo; see [`RepoRow::repo_source`].
    best_source: Option<RepoSource>,
    /// Every attributed session's observed outcomes, folded by [`outcome::rollup`] once the repo's
    /// sessions are all in, so the row's dedupe is the global dedupe restricted to this repo.
    outcomes: Vec<&'a Outcomes>,
}

impl RepoAcc<'_> {
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
        acc.cents += session_cents(entry);
        acc.models.extend(entry.models.keys().cloned());
        acc.observe(entry.repo_source.as_deref());
        if let Some(outcomes) = entry.outcomes.as_ref() {
            acc.outcomes.push(outcomes);
        }
    }
    let mut rows: Vec<RepoRow> = repos
        .into_iter()
        .map(|(repo, acc)| {
            let org = org_of(Some(&repo));
            let outcomes = repo_outcomes(&repo, &acc.outcomes);
            RepoRow {
                repo,
                org,
                sessions: acc.sessions,
                tokens: acc.tokens,
                tokens_human: format_tokens_human(acc.tokens),
                spend_raw: cents::to_dollars(acc.cents),
                spend_cents: acc.cents,
                spend: format_usd(cents::to_dollars(acc.cents)),
                spend_percent_of_max: None,
                outcomes,
                commits_percent_of_max: None,
                prs_percent_of_max: None,
                models: acc.models.into_iter().collect(),
                repo_source: acc.best_source.map(|s| s.as_str().to_string()),
            }
        })
        .collect();
    let max_spend = rows.iter().map(|r| r.spend_raw).fold(0.0_f64, f64::max);
    let max_commits = rows.iter().filter_map(|r| r.outcomes.as_ref()).map(|o| o.commits).max();
    let max_prs = rows
        .iter()
        .filter_map(|r| r.outcomes.as_ref())
        .map(|o| o.prs_opened)
        .max();
    for row in &mut rows {
        row.spend_percent_of_max = percent_of_max(row.spend_raw, max_spend);
        // A row with no outcomes gets no output geometry at all (absent, not 0.0): the chart rule
        // is "no scale field -> not drawn", and drawing a zero-length bar for an unobserved repo
        // would state an observation nobody made.
        if let Some(o) = row.outcomes.as_ref() {
            row.commits_percent_of_max = percent_of_max(o.commits as f64, max_commits.unwrap_or(0) as f64);
            row.prs_percent_of_max = percent_of_max(o.prs_opened as f64, max_prs.unwrap_or(0) as f64);
        }
    }
    // Each row adds the same per-session cents the headline adds. When every session carries a
    // repo the rows are a complete partition and sum to the headline exactly; with an unattributed
    // session they are a genuine subset and sum to less, the `(unattributed)` remainder that
    // `compute_attribution` states. No display-time cent moves either way.
    sort_by_spend_desc(&mut rows, |r| r.spend_raw);
    debug!(
        "aggregate::compute_by_repo: rows={} rows-with-outcomes={} max-spend={} max-commits={:?} max-prs={:?}",
        rows.len(),
        rows.iter().filter(|r| r.outcomes.is_some()).count(),
        max_spend,
        max_commits,
        max_prs
    );
    rows
}

/// One session's spend in whole cents, the unit the headline sums ([`crate::report::session_cents_total`]).
/// Unpriced sessions add zero.
fn session_cents(entry: &SessionEntry) -> i64 {
    entry.spend_usd.map_or(0, cents::to_cents)
}

/// Fold one repo's attributed sessions into its [`RepoOutcomes`], or `None` when the repo observed
/// nothing. Delegates to [`outcome::rollup`] so the row's dedupe rules cannot drift from the
/// report-wide ones they are a restriction of.
fn repo_outcomes(repo: &str, outcomes: &[&Outcomes]) -> Option<RepoOutcomes> {
    let totals = outcome::rollup(outcomes.iter().copied().map(Some));
    let row = RepoOutcomes {
        commits: totals.commits,
        prs_opened: totals.prs_opened,
        files_edited: totals.files_edited,
        lines_written: totals.lines_written,
        lines_replaced: totals.lines_replaced,
    };
    if row.is_empty() {
        trace!("aggregate::repo_outcomes: {repo} observed no outcomes; row absent");
        return None;
    }
    trace!(
        "aggregate::repo_outcomes: {repo} commits={} prs-opened={} files={} lines-written={} lines-replaced={}",
        row.commits, row.prs_opened, row.files_edited, row.lines_written, row.lines_replaced
    );
    Some(row)
}

#[derive(Default)]
struct DayAcc {
    sessions: usize,
    cents: i64,
    model_cents: BTreeMap<String, i64>,
    input: u64,
    cache_read: u64,
    cache_write: u64,
}

impl DayAcc {
    fn add(&mut self, model: &str, cents: i64, input: u64, cache_read: u64, cache_write: u64) {
        self.cents += cents;
        if cents != 0 {
            *self.model_cents.entry(model.to_string()).or_default() += cents;
        }
        self.input += input;
        self.cache_read += cache_read;
        self.cache_write += cache_write;
    }
}

/// By-day attribution on per-day buckets (design "Per-day buckets"): one zero-filled row per
/// calendar date in `[since, until]` inclusive, each carrying the cents the window's sessions
/// spent ON that day. A turn inside the window counts on its day; a turn outside it does not, and
/// its cents go to [`DayCoverage::out_of_window`] instead, so the rows plus the remainder equal the
/// headline to the cent. This retires the begin-day placement that put a multi-day session's
/// whole spend on the day it started (52% of September's spend sat on the wrong day).
///
/// A session with no buckets is placed whole on its begin day (in the window or not) and counted
/// in [`DayCoverage::unbucketed_sessions`], which the chart footnote states. Days are whole UTC
/// days, the same days the buckets are keyed by.
fn compute_by_day(report: &Report) -> (Vec<DayRow>, DayCoverage) {
    debug!("aggregate::compute_by_day: sessions={}", report.sessions.len());
    let since_date = report.since.date_naive();
    let until_date = report.until.date_naive();
    let in_window = |day: NaiveDate| day >= since_date && day <= until_date;

    let mut days: BTreeMap<NaiveDate, DayAcc> = BTreeMap::new();
    let mut date = since_date;
    while date <= until_date {
        days.insert(date, DayAcc::default());
        date += chrono::Duration::days(1);
    }

    let mut out_cents = 0_i64;
    let mut out_sessions = 0_usize;
    let mut unbucketed_sessions = 0_usize;
    let mut unbucketed_cents = 0_i64;

    for entry in report.sessions.values() {
        let begin = entry.begin.date_naive();
        let mut active: BTreeSet<NaiveDate> = BTreeSet::new();
        let mut outside = false;
        match &entry.by_day {
            Some(by_day) if !by_day.is_empty() => {
                for (day, by_model) in by_day {
                    for (model, d) in by_model {
                        if in_window(*day) {
                            let acc = days.entry(*day).or_default();
                            acc.add(
                                model,
                                d.spend_cents,
                                d.input,
                                d.cache_read,
                                d.cache_5m_write + d.cache_1h_write,
                            );
                            active.insert(*day);
                        } else {
                            out_cents += d.spend_cents;
                            outside = true;
                        }
                    }
                }
            }
            buckets => {
                // No tokens at all (an empty bucket map) or no buckets: the session sits on its
                // begin day, as every session did before buckets existed.
                if buckets.is_none() {
                    unbucketed_sessions += 1;
                    unbucketed_cents += entry.spend_usd.map_or(0, cents::to_cents);
                }
                for (model, m) in &entry.models {
                    let c = m.spend_usd.map_or(0, cents::to_cents);
                    if in_window(begin) {
                        days.entry(begin).or_default().add(
                            model,
                            c,
                            m.input,
                            m.cache_read,
                            m.cache_5m_write + m.cache_1h_write,
                        );
                    } else {
                        out_cents += c;
                        outside = true;
                    }
                }
                if in_window(begin) {
                    active.insert(begin);
                }
            }
        }
        for day in &active {
            days.entry(*day).or_default().sessions += 1;
        }
        if outside {
            out_sessions += 1;
        }
        trace!(
            "aggregate::compute_by_day: session begin={begin} active-days={} outside={outside}",
            active.len()
        );
    }

    let mut rows: Vec<DayRow> = days
        .into_iter()
        .map(|(date, acc)| {
            let spend = cents::to_dollars(acc.cents);
            DayRow {
                date: date.format("%Y-%m-%d").to_string(),
                sessions: acc.sessions,
                spend_raw: spend,
                spend: format_usd(spend),
                active: acc.sessions > 0,
                spend_percent_of_max: None,
                sessions_percent_of_max: None,
                spend_cents: acc.cents,
                cache_read_share: common::cache_read_share(acc.input, acc.cache_read, acc.cache_write, 0),
                model_cents: acc.model_cents,
            }
        })
        .collect();
    let max_spend = rows.iter().map(|r| r.spend_raw).fold(0.0_f64, f64::max);
    let max_sessions = rows.iter().map(|r| r.sessions).max().unwrap_or(0);
    for row in &mut rows {
        row.spend_percent_of_max = percent_of_max(row.spend_raw, max_spend);
        row.sessions_percent_of_max = percent_of_max(row.sessions as f64, max_sessions as f64);
    }
    let in_cents: i64 = rows.iter().map(|r| r.spend_cents).sum();
    let coverage = DayCoverage {
        in_window: format_usd(cents::to_dollars(in_cents)),
        out_of_window: format_usd(cents::to_dollars(out_cents)),
        out_of_window_sessions: out_sessions,
        unbucketed_sessions,
        unbucketed_spend: format_usd(cents::to_dollars(unbucketed_cents)),
        in_window_cents: in_cents,
        out_of_window_cents: out_cents,
        unbucketed_cents,
    };
    debug!(
        "aggregate::compute_by_day: rows={} active={} in-window-cents={in_cents} out-of-window-cents={out_cents} \
         out-of-window-sessions={out_sessions} unbucketed={unbucketed_sessions} max-spend={max_spend} \
         max-sessions={max_sessions}",
        rows.len(),
        rows.iter().filter(|r| r.active).count(),
    );
    (rows, coverage)
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
/// Every bucket is a sum of per-session integer cents ([`cents::to_cents`]), the same unit the
/// headline sums ([`crate::report::session_cents_total`]), so the rows add up to the headline
/// exactly and there is no residual row: a negative or phantom `(unattributed)` amount cannot
/// exist by construction, and the row appears only when a session with no repo is in the window.
pub fn compute_attribution(report: &Report) -> Attribution {
    debug!("aggregate::compute_attribution: sessions={}", report.sessions.len());
    let mut buckets: BTreeMap<String, (usize, i64)> = BTreeMap::new();
    let mut total_cents = 0_i64;
    let mut covered_cents = 0_i64;

    for entry in report.sessions.values() {
        let session_cents = entry.spend_usd.map_or(0, cents::to_cents);
        total_cents += session_cents;
        let key = match (&entry.repo, entry.repo_source.as_deref()) {
            (None, _) => UNATTRIBUTED_ORG.to_string(),
            (Some(_), Some(src)) => {
                covered_cents += session_cents;
                match RepoSource::from_str(src) {
                    Ok(parsed) => parsed.as_str().to_string(),
                    Err(_) => UNKNOWN_SOURCE.to_string(),
                }
            }
            (Some(_), None) => {
                covered_cents += session_cents;
                UNKNOWN_SOURCE.to_string()
            }
        };
        let bucket = buckets.entry(key).or_insert((0, 0));
        bucket.0 += 1;
        bucket.1 += session_cents;
    }
    let uncovered_cents = total_cents - covered_cents;

    let mut rows: Vec<AttributionRow> = buckets
        .into_iter()
        .map(|(source, (sessions, bucket_cents))| {
            let spend = cents::to_dollars(bucket_cents);
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

    let covered_share = if total_cents == 0 {
        "0.0%".to_string()
    } else {
        format!("{:.1}%", covered_cents as f64 / total_cents as f64 * 100.0)
    };
    debug!(
        "aggregate::compute_attribution: rows={} covered_cents={covered_cents} uncovered_cents={uncovered_cents} share={covered_share}",
        rows.len()
    );
    Attribution {
        rows,
        covered: format_usd(cents::to_dollars(covered_cents)),
        uncovered: format_usd(cents::to_dollars(uncovered_cents)),
        covered_share,
    }
}

/// Period spend set against the period's own output and calendar, binary-computed (design "Unit
/// costs, binary-computed", finding 9). Every field is a ratio of two figures the artifact already
/// states, so the prose can quote one instead of doing arithmetic the prompt forbids it.
///
/// **These are ratios, not prices.** `per-commit` is the period's WHOLE spend divided by the
/// period's distinct commits: the numerator includes every session in the window, including the
/// ones that produced no commit, so it is not "what a commit cost" and no template may frame it
/// that way. The honest reading is "this period spent $X and produced N commits"; the dishonest one
/// is a price tag, and the difference is one word.
///
/// Every field is `None` (and, via `skip_serializing_if`, ABSENT from the context) on a zero
/// denominator. No `$Inf`, no dollars-per-zero-commits.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct UnitCosts {
    /// `totals.spend-usd / totals.outcomes.commits`. `None` when the report carries no outcome
    /// rollup or observed no commit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_commit: Option<String>,
    /// `totals.spend-usd / totals.outcomes.prs-opened`. PRs OPENED, the only PR outcome the
    /// extractor counts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_pr: Option<String>,
    /// `totals.spend-usd / period.active-days`, the Phase 4 figure (rows with `active: true`), not
    /// the window's calendar length: dividing by inactive days would understate every active one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_active_day: Option<String>,
    /// `totals.spend-usd / totals.sessions`, the arithmetic MEAN. Compare it against
    /// [`Self::session_spend_p50`]: a mean far above the median is the signature of a few large
    /// sessions carrying the period.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_session: Option<String>,
    /// Median spend of a single session (nearest-rank over priced sessions). Unlike the four ratios
    /// above this is an observed session's own figure, not a quotient.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_spend_p50: Option<String>,
    /// 90th-percentile session spend, same method.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_spend_p90: Option<String>,
}

/// Divide the period's spend by a count, or `None` on a zero denominator (never `$Inf`, never a
/// dollars-per-nothing figure).
fn per_unit(spend: f64, count: u64) -> Option<String> {
    if count == 0 {
        return None;
    }
    Some(format_usd(spend / count as f64))
}

/// Nearest-rank percentile over an ASCENDING-sorted slice: index `ceil(p * n) - 1`, so p50 of an
/// even-length series is the lower of the two middle values rather than an interpolated figure no
/// session actually spent. Every value it can return is a real session's own spend.
pub(crate) fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (p * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted.get(rank - 1).copied()
}

/// Compute [`UnitCosts`] for the window. `by_day` supplies the active-day denominator so this uses
/// the SAME corrected figure `period.active-days` prints (Phase 4: rows with `active: true`, not
/// the row count), rather than a second definition of "active day".
pub fn compute_unit_costs(report: &Report, by_day: &[DayRow]) -> UnitCosts {
    let active_days = by_day.iter().filter(|r| r.active).count() as u64;
    debug!(
        "aggregate::compute_unit_costs: spend={:.2} sessions={} active-days={} outcomes={}",
        report.totals.spend_usd,
        report.totals.sessions,
        active_days,
        report.totals.outcomes.is_some()
    );
    let spend = report.totals.spend_usd;
    let (commits, prs) = match &report.totals.outcomes {
        Some(o) => (o.commits, o.prs_opened),
        None => (0, 0),
    };

    // Unpriced sessions are EXCLUDED from the distribution rather than counted as $0: a session
    // whose model is untracked has an unknown spend, and folding it in as zero would drag both
    // percentiles down with a number nobody measured.
    let mut spends: Vec<f64> = report.sessions.values().filter_map(|e| e.spend_usd).collect();
    spends.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let costs = UnitCosts {
        per_commit: per_unit(spend, commits),
        per_pr: per_unit(spend, prs),
        per_active_day: per_unit(spend, active_days),
        per_session: per_unit(spend, report.totals.sessions as u64),
        session_spend_p50: percentile(&spends, 0.50).map(format_usd),
        session_spend_p90: percentile(&spends, 0.90).map(format_usd),
    };
    debug!(
        "aggregate::compute_unit_costs: per-commit={:?} per-pr={:?} per-active-day={:?} \
         per-session={:?} p50={:?} p90={:?} priced-sessions={}",
        costs.per_commit,
        costs.per_pr,
        costs.per_active_day,
        costs.per_session,
        costs.session_spend_p50,
        costs.session_spend_p90,
        spends.len()
    );
    costs
}

/// Top-`outliers_n` sessions by spend (untracked/unpriced sessions rank as $0, ties broken by
/// short-id for determinism).
///
/// Ranked as REFERENCES first, and only the surviving `outliers_n` are materialized into
/// `OutlierRow`. Building a full row per session up front and truncating afterwards cloned each
/// session's title, repo, and whole `Outcomes` payload (commit and PR vectors, per-repo maps) for
/// every session in the window, then dropped all but ten of them.
fn compute_outliers(report: &Report, outliers_n: usize) -> Vec<OutlierRow> {
    let mut ranked: Vec<(&String, &SessionEntry)> = report.sessions.iter().collect();
    ranked.sort_by(|(a_sid, a), (b_sid, b)| {
        b.spend_usd
            .unwrap_or(0.0)
            .partial_cmp(&a.spend_usd.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
            // Tie-break on the SHORT id, not the full key: that is the ordering the old form
            // produced, because it sorted rows whose `short_id` was already truncated.
            .then_with(|| short_id(a_sid).cmp(short_id(b_sid)))
    });
    ranked.truncate(outliers_n);
    ranked
        .into_iter()
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
        .collect()
}

#[cfg(test)]
mod tests;
