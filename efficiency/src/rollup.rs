//! Daily/weekly efficiency rollups: bucket sessions by their last-active LOCAL date
//! ([`crate::collect::CollectedSession::last_active`], the documented substitute for a per-record
//! timestamp), union each bucket's aggregate [`RawCounters`], and `finalize` ONCE per bucket --
//! the same Aggregation invariant `fold` enforces per session, applied one level up. Mirrors
//! `cost`'s `Daily`/`Weekly` rollup shape (design "API Design": "aggregate rollups (mirror cost)").

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};
use log::debug;

use crate::collect::CollectedSession;
use crate::metrics::{EfficiencySignals, RawCounters, finalize};

/// One day's (or week's) efficiency rollup: how many sessions touched that period, and their
/// combined signals -- recomputed from the UNIONED raw counters, never a field-sum/average of the
/// per-session derived metrics.
#[derive(Debug, Clone, PartialEq)]
pub struct PeriodEfficiency {
    /// `YYYY-MM-DD` for a daily bucket, the Sunday-of-week `YYYY-MM-DD` for a weekly bucket.
    pub period: String,
    pub session_count: usize,
    pub aggregate: EfficiencySignals,
}

/// Bucket `sessions` by local calendar date into the `[start, end]` window (inclusive), newest
/// first -- mirrors `cost::compute_summaries`'s day grouping.
pub fn daily(sessions: &[CollectedSession], start: NaiveDate, end: NaiveDate) -> Vec<PeriodEfficiency> {
    debug!("rollup::daily: sessions={} start={start} end={end}", sessions.len());
    let mut buckets: BTreeMap<NaiveDate, (RawCounters, usize)> = BTreeMap::new();
    for s in sessions {
        let date = s.last_active.date_naive();
        if date < start || date > end {
            continue;
        }
        let bucket = buckets.entry(date).or_default();
        bucket.0.merge(&s.efficiency.aggregate.raw);
        bucket.1 += 1;
    }
    finalize_buckets(buckets)
}

/// Bucket `sessions` into Sunday-Saturday weeks within `[start, end]`, newest first -- mirrors
/// `cost::dispatch`'s `Weekly` grouping (`days_since_sunday`).
pub fn weekly(sessions: &[CollectedSession], start: NaiveDate, end: NaiveDate) -> Vec<PeriodEfficiency> {
    debug!("rollup::weekly: sessions={} start={start} end={end}", sessions.len());
    let mut buckets: BTreeMap<NaiveDate, (RawCounters, usize)> = BTreeMap::new();
    for s in sessions {
        let date = s.last_active.date_naive();
        if date < start || date > end {
            continue;
        }
        let days_since_sunday = date.weekday().num_days_from_sunday() as i64;
        let week_sunday = date - chrono::Duration::days(days_since_sunday);
        let bucket = buckets.entry(week_sunday).or_default();
        bucket.0.merge(&s.efficiency.aggregate.raw);
        bucket.1 += 1;
    }
    finalize_buckets(buckets)
}

fn finalize_buckets(buckets: BTreeMap<NaiveDate, (RawCounters, usize)>) -> Vec<PeriodEfficiency> {
    buckets
        .into_iter()
        .rev()
        .map(|(period, (raw, session_count))| PeriodEfficiency {
            period: period.to_string(),
            session_count,
            aggregate: finalize(raw),
        })
        .collect()
}

#[cfg(test)]
mod tests;
