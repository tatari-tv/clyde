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
use log::{debug, trace, warn};
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
    /// Sessions pulled in whole by the M2 session-level window but whose `begin` predates `since`
    /// (design "by-day, corrected"). Excluded from every `by_day` row; still counted in `totals`
    /// and `by_repo`, so this is the stated gap between `by_day`'s sum and `totals.spend-usd`.
    pub carried_in: CarriedIn,
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

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct DayRow {
    pub date: String,
    /// `0` on an inactive (zero-fill) day.
    pub sessions: usize,
    #[serde(skip)]
    pub spend_raw: f64,
    pub spend: String,
    /// `false` on a zero-fill row: this calendar date fell inside `[since, until]` but no session
    /// began on it. The prompt may cite these to name a multi-day gap (design finding 3) instead of
    /// inferring one from a missing date -- there is no longer a missing date to infer from.
    pub active: bool,
    /// See [`OrgRow::spend_percent_of_max`], scaled against the max daily spend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spend_percent_of_max: Option<f64>,
    /// Same formula as `spend-percent-of-max`, scaled against the max daily session count instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sessions_percent_of_max: Option<f64>,
}

/// Sessions whose `begin` predates `since`, pulled in whole by the M2 session-level window (design
/// "by-day, corrected"). They get their OWN row instead of being clamped onto the `since` date:
/// they stay in `totals` and `by-repo` (real spend in the window) and are excluded from every
/// `by-day` date row, so a reader can see exactly what the by-day series does not account for.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct CarriedIn {
    pub sessions: usize,
    pub tokens_human: String,
    pub spend: String,
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
    let (by_day, carried_in) = compute_by_day(report);
    let charts = chart::compute_charts(&by_day);
    let outliers = compute_outliers(report, outliers_n);
    let cache = compute_cache_stats(report, pricing);
    debug!(
        "aggregate::compute: by-org={} by-repo={} by-day={} carried-in-sessions={} outliers={} \
         cache-read-share={} counterfactual={} spend-chart={} sessions-chart={}",
        by_org.len(),
        by_repo.len(),
        by_day.len(),
        carried_in.sessions,
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
        carried_in,
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
    // ALWAYS a complete partition, unlike `by-repo`: a session with no repo lands in this table's
    // own `(unattributed)` org bucket rather than being dropped, so the rows always account for the
    // whole headline and must always sum to it.
    reconcile_displayed_spend(
        "by-org",
        &mut rows,
        report.totals.spend_usd,
        |r| r.spend_raw,
        |r, s| {
            r.spend = s;
        },
    );
    debug!("aggregate::compute_by_org: rows={} max-spend={}", rows.len(), max_spend);
    rows
}

#[derive(Default)]
struct RepoAcc<'a> {
    sessions: usize,
    tokens: u64,
    spend: f64,
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
        acc.spend += entry.spend_usd.unwrap_or(0.0);
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
                spend_raw: acc.spend,
                spend: format_usd(acc.spend),
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
    sort_by_spend_desc(&mut rows, |r| r.spend_raw);
    // `by-repo` is a COMPLETE partition of the headline exactly when every session carries a repo:
    // the rows then account for the whole window and the artifact presents them that way, so the
    // displayed column must sum to the displayed total. With even one unattributed session the
    // table is a genuine subset and must NOT be forced to sum -- adding a cent to a repo row would
    // attribute money no session's own price supports, which is what `compute_attribution`'s
    // `(unattributed)` bucket exists to avoid.
    //
    // Every fixture in the corpus is fully attributed, and each one shipped a table that disagreed
    // with its own headline: `$64.86` against `$64.85`, `$49.47` against `$49.48`, `$671.26` against
    // `$671.28`.
    if !report.sessions.values().any(|e| e.repo.is_none()) {
        reconcile_displayed_spend(
            "by-repo",
            &mut rows,
            report.totals.spend_usd,
            |r| r.spend_raw,
            |r, s| {
                r.spend = s;
            },
        );
    }
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

/// Rewrite each row's DISPLAYED spend string so the column sums to `total` exactly, for a table
/// that IS a complete partition of the headline. Callers must establish that first; see
/// [`crate::cents`] for the algorithm and for when it declines to allocate.
///
/// Only the display string moves. `spend_raw` keeps its MEASURED value, deliberately, so the bar
/// geometry (`spend-percent-of-max`) stays proportional to what was actually spent and the sort
/// order cannot be perturbed by a presentation cent. The two therefore differ by at most a cent on
/// at most one row, which is why `spend_raw` is `#[serde(skip)]`: it is the raw operand, never a
/// figure the model sees, so there is no field the artifact can quote them disagreeing through.
///
/// `rows` must already be in DISPLAY order: the allocator breaks remainder ties by index, so the
/// order it receives is what makes the result reproducible.
fn reconcile_displayed_spend<T>(
    table: &str,
    rows: &mut [T],
    total: f64,
    raw: impl Fn(&T) -> f64,
    set: impl Fn(&mut T, String),
) {
    let measured: Vec<f64> = rows.iter().map(&raw).collect();
    let Some(cents) = cents::allocate(&measured, total) else {
        debug!("aggregate::reconcile_displayed_spend: {table} left as measured");
        return;
    };
    for (row, c) in rows.iter_mut().zip(cents) {
        set(row, format_usd(cents::to_dollars(c)));
    }
    debug!(
        "aggregate::reconcile_displayed_spend: {table} reconciled {} row(s) to {total:.2}",
        rows.len()
    );
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
    spend: f64,
}

/// By-day attribution, corrected (design "by-day, corrected"): one zero-filled row per calendar
/// date in `[since, until]` inclusive -- a skipped week now leaves visible `active: false` rows
/// instead of no row at all (finding 3). A session whose `begin` date predates `since` is pulled
/// in whole by the M2 session-level window; rather than clamping it onto the `since` date (which
/// used to inflate day 1 up to 4.4x, finding 2), it is folded into the returned [`CarriedIn`] and
/// excluded from every date row. A `begin` date after `until` still clamps DOWN defensively (it
/// should never happen: `begin <= modified <= until`), matching the pre-existing defensive
/// boundary guard for the upper bound.
fn compute_by_day(report: &Report) -> (Vec<DayRow>, CarriedIn) {
    debug!("aggregate::compute_by_day: sessions={}", report.sessions.len());
    let since_date = report.since.date_naive();
    let until_date = report.until.date_naive();

    let mut days: BTreeMap<NaiveDate, DayAcc> = BTreeMap::new();
    let mut date = since_date;
    while date <= until_date {
        days.insert(date, DayAcc::default());
        date += chrono::Duration::days(1);
    }

    let mut carried_sessions = 0usize;
    let mut carried_tokens = 0u64;
    let mut carried_spend = 0.0_f64;

    for entry in report.sessions.values() {
        let raw_date = entry.begin.date_naive();
        if raw_date < since_date {
            carried_sessions += 1;
            carried_tokens += entry.total_tokens();
            carried_spend += entry.spend_usd.unwrap_or(0.0);
            continue;
        }
        let date = raw_date.min(until_date);
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
            active: acc.sessions > 0,
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
    let active_days = rows.iter().filter(|r| r.active).count();
    debug!(
        "aggregate::compute_by_day: rows={} active={} carried-in-sessions={} carried-in-spend={:.2} \
         max-spend={} max-sessions={}",
        rows.len(),
        active_days,
        carried_sessions,
        carried_spend,
        max_spend,
        max_sessions
    );
    let carried_in = CarriedIn {
        sessions: carried_sessions,
        tokens_human: format_tokens_human(carried_tokens),
        spend: format_usd(carried_spend),
    };
    (rows, carried_in)
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
    // Only a POSITIVE residual is money the rows have not accounted for. A negative one says the
    // per-session spends EXCEED the headline, which the doc comment's long-context story cannot
    // produce -- it is a merged artifact, or `round_cents` on the headline against unrounded
    // per-session spends. Folding it would create (or shrink) the `(unattributed)` bucket by a
    // NEGATIVE amount, `format_usd` would render `-$X.XX` in the artifact, and the prose is licensed
    // to quote it. The rows falling a hair short of the headline is the better failure: the residual
    // is declared a pricing artifact, not attribution, so an impossible one is dropped and logged,
    // never published.
    if residual < 0.0 {
        warn!(
            "aggregate::compute_attribution: per-session spends EXCEED totals.spend-usd by {:.2}; \
             refusing to fold a negative residual into ({UNATTRIBUTED_ORG}) -- the rows will fall \
             short of the headline by that amount",
            -residual
        );
    } else if residual != 0.0 {
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
fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
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
