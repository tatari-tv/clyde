//! The chart view model: every chart's display strings, formatted once, beside its geometry.
//!
//! Built from the [`ContextBlock`] alone, so it can carry no figure the block does not support.
//! Every label, value and tooltip here is formatted by [`crate::fmt`] or [`sections`]; geometry
//! comes from [`crate::chart`] over the block's raw twins (`spend_raw`, the cache cents). A backend
//! draws these and formats nothing: the HTML backend writes the strings through `text()`/`attr()`
//! and the geometry through `geom()`, so the licensing test sees every label as it enters the page.

use chrono::NaiveDate;
use log::debug;

use super::ContextBlock;
use super::sections;
use crate::aggregate::{self, UNATTRIBUTED_ORG};
use crate::bands::Band;
use crate::chart::{self, bars, calendar, histogram, stack, treemap, waterfall};
use crate::fmt::{format_int, format_usd, format_usd_edge};

/// Categorical palette slots a chart may assign (`--cat-1` .. `--cat-5`); [`MUTED_TONE`] is the
/// sixth, for a bucket that is not a real category (`(unattributed)`).
pub(super) const TONES: usize = 5;
pub(super) const MUTED_TONE: usize = TONES;

/// Every chart the page can draw from this block. A chart is `None` when its data is empty; the
/// backend draws that chart's empty state.
pub(super) struct Visuals {
    pub(super) orgs: Option<Pairs>,
    pub(super) repos: Option<Pairs>,
    pub(super) treemap: Option<Treemap>,
    pub(super) models: Option<Stack>,
    /// Session spend by outcome band; `None` when outcomes were not collected or nothing was spent.
    pub(super) bands: Option<Stack>,
    pub(super) waterfall: Cascade,
    pub(super) histogram: Option<Histogram>,
    pub(super) calendar: Option<Calendar>,
    /// Daily spend stacked by model, colored like [`Self::models`].
    pub(super) daily: Option<DailyStack>,
    /// The daily cache-read share line.
    pub(super) cache_share: Option<ShareChart>,
}

/// Daily spend as standing columns, one per day, split by model.
pub(super) struct DailyStack {
    /// Models in Cost Summary order, each with the tone the Cost Summary bar gives it.
    pub(super) legend: Vec<(String, usize)>,
    pub(super) columns: Vec<DailyColumn>,
    /// Max, midpoint and zero.
    pub(super) y_labels: Vec<String>,
    pub(super) x_labels: Vec<String>,
}

pub(super) struct DailyColumn {
    pub(super) tip: String,
    pub(super) x: i64,
    pub(super) w: i64,
    /// One block per legend model with spend that day, bottom first, with its tone.
    pub(super) blocks: Vec<(stack::Block, usize)>,
}

/// The daily cache-read share line and its per-day tooltips.
pub(super) struct ShareChart {
    pub(super) line: chart::ShareLine,
    pub(super) tips: Vec<String>,
    pub(super) y_labels: Vec<String>,
}

/// Paired %-sessions / %-spend bars, one row per org or repo, in spend order.
pub(super) struct Pairs {
    pub(super) rows: Vec<PairRow>,
}

pub(super) struct PairRow {
    pub(super) label: String,
    pub(super) sessions_share: String,
    pub(super) sessions: String,
    pub(super) spend_share: String,
    pub(super) spend: String,
    pub(super) tip: String,
    pub(super) bars: bars::Pair,
    /// Position of this row under each sort key, `0` first.
    pub(super) rank: Ranks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Ranks {
    pub(super) spend: usize,
    pub(super) sessions: usize,
    pub(super) per_session: usize,
}

/// The org -> repo treemap.
pub(super) struct Treemap {
    pub(super) groups: Vec<TreeGroup>,
}

pub(super) struct TreeGroup {
    pub(super) label: String,
    pub(super) spend: String,
    pub(super) share: String,
    pub(super) tone: usize,
    pub(super) rect: treemap::Rect,
    /// The group's tiles; one tile covering the group when it has no repo items.
    pub(super) tiles: Vec<Tile>,
}

#[derive(Clone)]
pub(super) struct Tile {
    pub(super) label: String,
    pub(super) spend: String,
    pub(super) tip: String,
    pub(super) rect: treemap::Rect,
}

/// A single bar split by model spend.
pub(super) struct Stack {
    pub(super) segments: Vec<StackSegment>,
}

pub(super) struct StackSegment {
    pub(super) label: String,
    pub(super) share: String,
    pub(super) spend: String,
    pub(super) tip: String,
    pub(super) tone: usize,
    pub(super) segment: stack::Segment,
}

/// List price -> cache savings -> spend.
pub(super) enum Cascade {
    Drawn {
        list: String,
        savings: String,
        spend: String,
        steps: waterfall::Waterfall,
    },
    /// A cache-bearing model is unpriced, so the counterfactual is unknowable.
    Unpriced,
    /// Nothing to draw: no list price and no spend.
    Empty,
}

/// Sessions per cost bucket.
pub(super) struct Histogram {
    pub(super) columns: Vec<Column>,
    /// The axis edges under the columns: one more than the columns when the last bucket is closed.
    pub(super) edges: Vec<String>,
    /// Sessions with no price, which no bucket can hold.
    pub(super) unpriced: Option<String>,
}

pub(super) struct Column {
    pub(super) count: String,
    pub(super) tip: String,
    pub(super) height: i64,
    /// The column holds the median session, the 90th-percentile session, or both.
    pub(super) median: bool,
    pub(super) p90: bool,
}

/// The daily spend calendar.
pub(super) struct Calendar {
    pub(super) height: i64,
    pub(super) cells: Vec<(calendar::Cell, String)>,
}

/// Build every chart's view.
pub(super) fn build(block: &ContextBlock<'_>) -> Visuals {
    let agg = block.aggregates;
    let total_spend: f64 = agg.by_org.iter().map(|r| r.spend_raw).sum();
    let total_sessions = block.totals.sessions;
    debug!(
        "render::visuals::build: orgs={} repos={} sessions={total_sessions} spend={total_spend}",
        agg.by_org.len(),
        agg.by_repo.len()
    );
    let orgs = pairs(
        agg.by_org
            .iter()
            .map(|r| (r.org.as_str(), r.sessions, r.spend_raw, r.spend.as_str())),
        total_sessions,
        total_spend,
    );
    let repos = pairs(
        agg.by_repo
            .iter()
            .map(|r| (r.repo.as_str(), r.sessions, r.spend_raw, r.spend.as_str())),
        total_sessions,
        total_spend,
    );
    Visuals {
        orgs,
        repos,
        treemap: build_treemap(block, total_spend),
        models: build_models(block),
        bands: build_bands(block),
        waterfall: build_waterfall(block),
        histogram: build_histogram(block),
        calendar: build_calendar(block),
        daily: build_daily(block),
        cache_share: build_cache_share(block),
    }
}

/// `part` of `whole` as a whole percent: `"39%"`, `"<1%"` for a positive share that rounds to
/// zero, `"n/a"` on a zero denominator (never NaN).
pub(super) fn share(part: f64, whole: f64) -> String {
    if whole <= 0.0 {
        return "n/a".to_string();
    }
    let percent = part / whole * 100.0;
    if percent > 0.0 && percent < 0.5 {
        return "<1%".to_string();
    }
    format!("{}%", percent.round() as i64)
}

fn ratio(part: f64, whole: f64) -> f64 {
    if whole <= 0.0 { 0.0 } else { part / whole }
}

/// One row per `(label, sessions, spend, spend display)`, already in spend order.
fn pairs<'a>(
    rows: impl Iterator<Item = (&'a str, usize, f64, &'a str)>,
    total_sessions: usize,
    total_spend: f64,
) -> Option<Pairs> {
    let rows: Vec<(&str, usize, f64, &str)> = rows.collect();
    let shares: Vec<(f64, f64)> = rows
        .iter()
        .map(|(_, sessions, spend, _)| {
            (
                ratio(*sessions as f64, total_sessions as f64),
                ratio(*spend, total_spend),
            )
        })
        .collect();
    let lengths = bars::paired(&shares)?;
    let per_session: Vec<f64> = rows
        .iter()
        .map(|(_, sessions, spend, _)| ratio(*spend, *sessions as f64))
        .collect();
    let by_sessions = ranks(&rows.iter().map(|r| r.1 as f64).collect::<Vec<_>>());
    let by_per_session = ranks(&per_session);
    let out = rows
        .iter()
        .enumerate()
        .zip(lengths)
        .map(|((i, (label, sessions, spend_raw, spend)), bars)| {
            let sessions_share = share(*sessions as f64, total_sessions as f64);
            let spend_share = share(*spend_raw, total_spend);
            let sessions_text = sections::plural(*sessions, "session");
            let per = format_usd(per_session[i]);
            PairRow {
                tip: format!("{label}: {sessions_text} ({sessions_share}), {spend} ({spend_share}), {per} per session"),
                label: (*label).to_string(),
                sessions_share,
                sessions: sessions_text,
                spend_share,
                spend: (*spend).to_string(),
                bars,
                rank: Ranks {
                    spend: i,
                    sessions: by_sessions[i],
                    per_session: by_per_session[i],
                },
            }
        })
        .collect();
    Some(Pairs { rows: out })
}

/// Each value's position when sorted descending, ties kept in input order.
fn ranks(values: &[f64]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|a, b| values[*b].total_cmp(&values[*a]).then(a.cmp(b)));
    let mut out = vec![0; values.len()];
    for (position, index) in order.into_iter().enumerate() {
        out[index] = position;
    }
    out
}

fn tone(index: usize, org: &str) -> usize {
    if org == UNATTRIBUTED_ORG {
        MUTED_TONE
    } else {
        index % TONES
    }
}

fn build_treemap(block: &ContextBlock<'_>, total_spend: f64) -> Option<Treemap> {
    let agg = block.aggregates;
    let children: Vec<Vec<&aggregate::RepoRow>> = agg
        .by_org
        .iter()
        .map(|org| agg.by_repo.iter().filter(|r| r.org == org.org).collect())
        .collect();
    let input: Vec<(f64, Vec<f64>)> = agg
        .by_org
        .iter()
        .zip(&children)
        .map(|(org, repos)| (org.spend_raw, repos.iter().map(|r| r.spend_raw).collect()))
        .collect();
    let laid = treemap::treemap(&input)?;
    let groups = agg
        .by_org
        .iter()
        .zip(children)
        .zip(laid)
        .enumerate()
        .filter_map(|(i, ((org, repos), tiles))| {
            let tiles = tiles?;
            let org_share = share(org.spend_raw, total_spend);
            let mut drawn: Vec<Tile> = repos
                .iter()
                .zip(&tiles.items)
                .filter_map(|(repo, rect)| {
                    let rect = (*rect)?;
                    let short = repo.repo.split_once('/').map_or(repo.repo.as_str(), |(_, name)| name);
                    Some(Tile {
                        label: short.to_string(),
                        spend: repo.spend.clone(),
                        tip: format!("{}: {} ({})", repo.repo, repo.spend, share(repo.spend_raw, total_spend)),
                        rect,
                    })
                })
                .collect();
            if drawn.is_empty() {
                drawn.push(Tile {
                    label: org.org.clone(),
                    spend: org.spend.clone(),
                    tip: format!("{}: {} ({org_share})", org.org, org.spend),
                    rect: tiles.rect,
                });
            }
            Some(TreeGroup {
                label: org.org.clone(),
                spend: org.spend.clone(),
                share: org_share,
                tone: tone(i, &org.org),
                rect: tiles.rect,
                tiles: drawn,
            })
        })
        .collect();
    Some(Treemap { groups })
}

/// The priced models with spend, in Cost Summary order (spend descending). Index `i` is the model's
/// palette slot (`i % TONES`) on EVERY chart that colors by model, so a model is one color on the
/// page.
fn priced_models<'a>(block: &'a ContextBlock<'_>) -> Vec<&'a super::ModelRow> {
    block
        .totals
        .models
        .iter()
        .filter(|m| m.spend_usd.is_some_and(|s| s > 0.0))
        .collect()
}

fn build_models(block: &ContextBlock<'_>) -> Option<Stack> {
    let rows = priced_models(block);
    let values: Vec<f64> = rows.iter().map(|m| m.spend_usd.unwrap_or(0.0)).collect();
    let total: f64 = values.iter().sum();
    let segments = stack::stack(&values)?;
    let out = rows
        .iter()
        .zip(&values)
        .zip(segments)
        .enumerate()
        .map(|(i, ((row, value), segment))| {
            let share = share(*value, total);
            StackSegment {
                tip: format!("{}: {} ({share})", row.model, row.spend),
                label: row.model.clone(),
                share,
                spend: row.spend.clone(),
                tone: i % TONES,
                segment,
            }
        })
        .collect();
    Some(Stack { segments: out })
}

/// Palette slot per band: committed leads blue, edited orange, wrote-records green, and the band
/// that observed nothing is the muted tone, so it never reads as a category.
pub(super) fn band_tone(band: Band) -> usize {
    match band {
        Band::Committed => 0,
        Band::Edited => 4,
        Band::WroteRecords => 1,
        Band::NoObservedOutcome => MUTED_TONE,
    }
}

/// The stacked outcome bar: one segment per band, in band order, sized by spend cents. Zero-width
/// segments are kept so segment `i` is always band `i`.
fn build_bands(block: &ContextBlock<'_>) -> Option<Stack> {
    let table = block.bands.bands.as_ref()?;
    let values: Vec<f64> = table.rows.iter().map(|r| r.tally.cents as f64).collect();
    let segments = stack::stack(&values)?;
    let out = table
        .rows
        .iter()
        .zip(segments)
        .map(|(row, segment)| StackSegment {
            tip: format!("{}: {} ({}, {} sessions)", row.name, row.spend, row.share, row.sessions),
            label: row.name.to_string(),
            share: row.share.clone(),
            spend: row.spend.clone(),
            tone: band_tone(row.band),
            segment,
        })
        .collect();
    Some(Stack { segments: out })
}

fn build_waterfall(block: &ContextBlock<'_>) -> Cascade {
    let cache = &block.aggregates.cache;
    let (Some(list_cents), Some(savings_cents), Some(list), Some(savings)) = (
        cache.list_price_cents,
        cache.cache_savings_cents,
        cache.list_price_equivalent.as_ref(),
        cache.cache_savings.as_ref(),
    ) else {
        return Cascade::Unpriced;
    };
    match waterfall::waterfall(list_cents, savings_cents, cache.spend_cents) {
        Some(steps) => Cascade::Drawn {
            list: list.clone(),
            savings: savings.clone(),
            spend: block.totals.spend.clone(),
            steps,
        },
        None => Cascade::Empty,
    }
}

fn build_histogram(block: &ContextBlock<'_>) -> Option<Histogram> {
    let mut priced: Vec<f64> = block.sessions.iter().filter_map(|s| s.spend_raw).collect();
    priced.sort_by(f64::total_cmp);
    let cents: Vec<i64> = priced.iter().map(|v| (v * 100.0).round() as i64).collect();
    let laid = histogram::histogram(&cents)?;
    let bucket = |p: f64| aggregate::percentile(&priced, p).map(|v| histogram::bucket_of((v * 100.0).round() as i64));
    let median = bucket(0.50);
    let p90 = bucket(0.90);
    let columns = laid
        .counts
        .iter()
        .zip(&laid.heights)
        .enumerate()
        .map(|(offset, (count, height))| {
            let index = laid.first + offset;
            let (low, high) = histogram::bounds(index);
            let range = match high {
                Some(high) => format!("{} to {}", format_usd_edge(low), format_usd_edge(high)),
                None => format!("{} and up", format_usd_edge(low)),
            };
            Column {
                count: format_int(*count as u64),
                tip: format!("{range}: {}", sections::plural(*count, "session")),
                height: *height,
                median: median == Some(index),
                p90: p90 == Some(index),
            }
        })
        .collect();
    let last = laid.first + laid.counts.len() - 1;
    let mut edges: Vec<String> = (laid.first..=last)
        .map(|i| format_usd_edge(histogram::bounds(i).0))
        .collect();
    if let Some(high) = histogram::bounds(last).1 {
        edges.push(format_usd_edge(high));
    }
    let unpriced = block.sessions.len() - priced.len();
    Some(Histogram {
        columns,
        edges,
        unpriced: (unpriced > 0).then(|| {
            format!(
                "{} with no price {} not shown.",
                sections::plural(unpriced, "session"),
                if unpriced == 1 { "is" } else { "are" }
            )
        }),
    })
}

fn build_daily(block: &ContextBlock<'_>) -> Option<DailyStack> {
    let rows = &block.aggregates.by_day;
    let legend: Vec<(String, usize)> = priced_models(block)
        .into_iter()
        .enumerate()
        .filter(|(_, m)| rows.iter().any(|r| r.model_cents.get(&m.model).is_some_and(|c| *c > 0)))
        .map(|(i, m)| (m.model.clone(), i % TONES))
        .collect();
    let values: Vec<Vec<f64>> = rows
        .iter()
        .map(|r| {
            legend
                .iter()
                .map(|(model, _)| r.model_cents.get(model).copied().unwrap_or(0) as f64)
                .collect()
        })
        .collect();
    let laid = stack::columns(&values)?;
    let max_cents = rows.iter().map(|r| r.spend_cents).max().unwrap_or(0);
    let columns = rows
        .iter()
        .zip(laid)
        .map(|(row, col)| {
            let parts: Vec<String> = legend
                .iter()
                .filter_map(|(model, _)| {
                    let c = row.model_cents.get(model).copied().unwrap_or(0);
                    (c > 0).then(|| format!("{model} {}", format_usd(crate::cents::to_dollars(c))))
                })
                .collect();
            let tip = if parts.is_empty() {
                format!("{}: {}", row.date, row.spend)
            } else {
                format!("{}: {} ({})", row.date, row.spend, parts.join(", "))
            };
            let blocks = col
                .blocks
                .into_iter()
                .zip(&legend)
                .filter(|(b, _)| b.h > 0)
                .map(|(b, (_, tone))| (b, *tone))
                .collect();
            DailyColumn {
                tip,
                x: col.x,
                w: col.w,
                blocks,
            }
        })
        .collect();
    let dates: Vec<&str> = rows.iter().map(|r| r.date.as_str()).collect();
    Some(DailyStack {
        legend,
        columns,
        y_labels: chart::line::y_labels(crate::cents::to_dollars(max_cents), &format_usd),
        x_labels: chart::line::x_labels(&dates),
    })
}

/// The daily cache-read share as a whole-day percent with one decimal (`"96.4%"`), the format the
/// period-wide share prints in.
fn share_percent(share: f64) -> String {
    format!("{:.1}%", share * 100.0)
}

fn build_cache_share(block: &ContextBlock<'_>) -> Option<ShareChart> {
    let rows = &block.aggregates.by_day;
    let shares: Vec<Option<f64>> = rows.iter().map(|r| r.cache_read_share).collect();
    let dates: Vec<&str> = rows.iter().map(|r| r.date.as_str()).collect();
    let line = chart::share_line(&shares, &dates)?;
    let tips = rows
        .iter()
        .map(|r| match r.cache_read_share {
            Some(share) => format!("{}: {} of input read from cache", r.date, share_percent(share)),
            None => format!("{}: no tokens", r.date),
        })
        .collect();
    Some(ShareChart {
        line,
        tips,
        y_labels: vec![share_percent(1.0), share_percent(0.5), share_percent(0.0)],
    })
}

fn build_calendar(block: &ContextBlock<'_>) -> Option<Calendar> {
    let rows = &block.aggregates.by_day;
    let first = NaiveDate::parse_from_str(&rows.first()?.date, "%Y-%m-%d").ok()?;
    let values: Vec<f64> = rows.iter().map(|r| r.spend_raw).collect();
    let laid = calendar::calendar(first, &values)?;
    let cells = laid
        .cells
        .into_iter()
        .zip(sections::day_rows(rows, true))
        .map(|(cell, (date, spend))| (cell, format!("{date}: {spend}")))
        .collect();
    Some(Calendar {
        height: laid.height,
        cells,
    })
}

/// Every display string the views carry, for the licensing test: the page may print these and
/// nothing else of its own.
#[cfg(test)]
pub(super) fn display_strings(v: &Visuals) -> Vec<String> {
    let mut out = Vec::new();
    for pairs in [&v.orgs, &v.repos].into_iter().flatten() {
        for r in &pairs.rows {
            out.extend([
                r.label.clone(),
                r.sessions_share.clone(),
                r.sessions.clone(),
                r.spend_share.clone(),
                r.spend.clone(),
                r.tip.clone(),
            ]);
        }
    }
    if let Some(t) = &v.treemap {
        for g in &t.groups {
            out.extend([g.label.clone(), g.spend.clone(), g.share.clone()]);
            for tile in &g.tiles {
                out.extend([tile.label.clone(), tile.spend.clone(), tile.tip.clone()]);
            }
        }
    }
    if let Some(s) = &v.models {
        for seg in &s.segments {
            out.extend([seg.label.clone(), seg.share.clone(), seg.spend.clone(), seg.tip.clone()]);
        }
    }
    if let Some(s) = &v.bands {
        for seg in &s.segments {
            out.extend([seg.label.clone(), seg.share.clone(), seg.spend.clone(), seg.tip.clone()]);
        }
    }
    if let Cascade::Drawn {
        list, savings, spend, ..
    } = &v.waterfall
    {
        out.extend([list.clone(), savings.clone(), spend.clone()]);
    }
    if let Some(h) = &v.histogram {
        for c in &h.columns {
            out.extend([c.count.clone(), c.tip.clone()]);
        }
        out.extend(h.edges.iter().cloned());
        out.extend(h.unpriced.iter().cloned());
    }
    if let Some(c) = &v.calendar {
        out.extend(c.cells.iter().map(|(_, tip)| tip.clone()));
    }
    if let Some(d) = &v.daily {
        out.extend(d.legend.iter().map(|(model, _)| model.clone()));
        out.extend(d.columns.iter().map(|c| c.tip.clone()));
        out.extend(d.y_labels.iter().cloned());
        out.extend(d.x_labels.iter().cloned());
    }
    if let Some(c) = &v.cache_share {
        out.extend(c.tips.iter().cloned());
        out.extend(c.y_labels.iter().cloned());
        out.extend(c.line.x_labels.iter().cloned());
    }
    out
}

#[cfg(test)]
mod tests;
