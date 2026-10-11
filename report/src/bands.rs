//! Outcome bands and the unobserved-and-flagged spend rollup.
//!
//! Every session lands in exactly ONE band, first match wins, decided from its observed
//! [`Outcomes`]. The names say what was observed, not what happened: files written through a Bash
//! heredoc or commits made by a subprocess are invisible to `outcome::extract`, so
//! `no-observed-outcome` is not "no result" ([`NO_OBSERVED_CAVEAT`] says so on the page).
//!
//! Spend is summed in integer cents ([`crate::cents`]), so the bands add up exactly to the sum of
//! the sessions' own cents. A share whose denominator is zero is `n/a`, never NaN.
//!
//! This module also owns the other two unobserved-and-flagged rollups: a heavy model on a short
//! session (both thresholds are `render.waste.*` config, never constants here) and the count and
//! spend per [`EfficiencyFlag`] kind. The view it builds is serialized into the context block, so
//! its display strings are licensed like every other figure on the page.

use log::debug;
use serde::Serialize;

use crate::cents::{to_cents, to_dollars};
use crate::fmt::{format_int, format_usd};
use crate::outcome::Outcomes;
use crate::report::{Report, SessionEntry};
use efficiency::EfficiencyFlag;

/// What the money produced, as far as the extractor could see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Band {
    Committed,
    Edited,
    WroteRecords,
    NoObservedOutcome,
}

/// Every band, in display order.
pub(crate) const BANDS: [Band; 4] = [
    Band::Committed,
    Band::Edited,
    Band::WroteRecords,
    Band::NoObservedOutcome,
];

impl Band {
    /// The band's name as it prints.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Band::Committed => "committed",
            Band::Edited => "edited",
            Band::WroteRecords => "wrote-records",
            Band::NoObservedOutcome => "no-observed-outcome",
        }
    }

    /// The rule that puts a session in this band, in words.
    pub(crate) fn rule(self) -> &'static str {
        match self {
            Band::Committed => "at least one commit or opened pull request",
            Band::Edited => "files edited, no commit or pull request",
            Band::WroteRecords => "a Confluence, Jira or Slack write, no file edit",
            Band::NoObservedOutcome => "none of the above",
        }
    }
}

/// The band a session belongs to. First match wins; a session with no outcome blob at all has
/// observed nothing.
pub(crate) fn classify(outcomes: Option<&Outcomes>) -> Band {
    let Some(o) = outcomes else {
        return Band::NoObservedOutcome;
    };
    if !o.commits.is_empty() || !o.prs.is_empty() {
        Band::Committed
    } else if o.files_edited > 0 {
        Band::Edited
    } else if o.confluence_writes + o.jira_writes + o.slack_messages > 0 {
        Band::WroteRecords
    } else {
        Band::NoObservedOutcome
    }
}

/// A session count and the cents they spent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Tally {
    pub(crate) sessions: usize,
    pub(crate) cents: i64,
}

impl Tally {
    fn add(&mut self, cents: i64) {
        self.sessions += 1;
        self.cents += cents;
    }
}

/// A session's spend in cents; an unpriced session spends none.
pub(crate) fn session_cents(entry: &SessionEntry) -> i64 {
    entry.spend_usd.map_or(0, to_cents)
}

/// One tally per band, in [`BANDS`] order.
pub(crate) fn roll_up<'a>(sessions: impl Iterator<Item = &'a SessionEntry>) -> [Tally; 4] {
    let mut tallies = [Tally::default(); 4];
    for entry in sessions {
        let band = classify(entry.outcomes.as_ref());
        if let Some(index) = BANDS.iter().position(|b| *b == band)
            && let Some(tally) = tallies.get_mut(index)
        {
            tally.add(session_cents(entry));
        }
    }
    tallies
}

/// Sessions that used a heavy model and were short. A model is heavy when its name contains any
/// `heavy_models` entry, ignoring case; a session is short at `max_turns` turns or fewer.
pub(crate) fn heavy_on_short<'a>(
    sessions: impl Iterator<Item = &'a SessionEntry>,
    heavy_models: &[&str],
    max_turns: u64,
) -> Tally {
    let heavy: Vec<String> = heavy_models.iter().map(|m| m.to_lowercase()).collect();
    let mut tally = Tally::default();
    for entry in sessions {
        let uses_heavy = entry
            .models
            .keys()
            .any(|model| heavy.iter().any(|h| model.to_lowercase().contains(h.as_str())));
        if uses_heavy && entry.efficiency.aggregate.raw.turns <= max_turns {
            tally.add(session_cents(entry));
        }
    }
    tally
}

/// The flag kinds, in display order, with the label each prints under.
const FLAG_KINDS: [(&str, &str); 3] = [
    ("low-cache-read-share", "Low cache-read share"),
    ("high-tool-error-rate", "High tool-error rate"),
    ("auto-compaction", "Auto-compaction"),
];

fn flag_kind(flag: &EfficiencyFlag) -> &'static str {
    match flag {
        EfficiencyFlag::LowCacheReadShare { .. } => "low-cache-read-share",
        EfficiencyFlag::HighToolErrorRate { .. } => "high-tool-error-rate",
        EfficiencyFlag::AutoCompaction { .. } => "auto-compaction",
    }
}

/// Count and spend per flag kind, in [`FLAG_KINDS`] order. A session carrying a kind counts once
/// under it, and once under each other kind it carries, so rows can overlap and are not a partition.
pub(crate) fn flag_rollup<'a>(sessions: impl Iterator<Item = &'a SessionEntry>) -> [Tally; 3] {
    let mut tallies = [Tally::default(); 3];
    for entry in sessions {
        for (index, (kind, _)) in FLAG_KINDS.iter().enumerate() {
            if entry.efficiency.flags.iter().any(|f| flag_kind(f) == *kind)
                && let Some(tally) = tallies.get_mut(index)
            {
                tally.add(session_cents(entry));
            }
        }
    }
    tallies
}

/// `part` of `whole` as a whole percent: `"39%"`, `"<1%"` for a positive share that rounds to zero,
/// `"n/a"` when `whole` is not positive (never NaN). Integer math, so the cents stay cents.
pub(crate) fn percent(part: i64, whole: i64) -> String {
    if whole <= 0 {
        return "n/a".to_string();
    }
    if part > 0 && part * 200 < whole {
        return "<1%".to_string();
    }
    format!("{}%", (part * 200 + whole) / (whole * 2))
}

fn usd(cents: i64) -> String {
    format_usd(to_dollars(cents))
}

/// The page's bands, heavy-on-short and flag figures, every figure already formatted.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct BandsView {
    /// `None` when outcomes were not collected: the bands are then not computable.
    pub(crate) bands: Option<BandTable>,
    pub(crate) heavy_on_short: HeavyOnShort,
    pub(crate) flags: Vec<FlagRow>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct BandTable {
    pub(crate) rows: Vec<BandRow>,
    pub(crate) total_sessions: String,
    pub(crate) total_spend: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct BandRow {
    pub(crate) band: Band,
    pub(crate) name: &'static str,
    pub(crate) rule: &'static str,
    pub(crate) sessions: String,
    pub(crate) spend: String,
    /// Share of the report's session spend.
    pub(crate) share: String,
    #[serde(skip)]
    pub(crate) tally: Tally,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct HeavyOnShort {
    /// The configured substrings, so the sentence can say what "heavy" meant.
    pub(crate) models: Vec<String>,
    pub(crate) max_turns: String,
    pub(crate) sessions: String,
    pub(crate) spend: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct FlagRow {
    pub(crate) label: &'static str,
    pub(crate) sessions: String,
    pub(crate) spend: String,
}

/// Build the view from a report and the two `render.waste.*` thresholds.
pub(crate) fn build_view(report: &Report, heavy_models: &[&str], short_session_turns: u64) -> BandsView {
    let sessions = || report.sessions.values();
    let bands = (report.outcomes_enabled == Some(true)).then(|| {
        let tallies = roll_up(sessions());
        let total = Tally {
            sessions: tallies.iter().map(|t| t.sessions).sum(),
            cents: tallies.iter().map(|t| t.cents).sum(),
        };
        let rows = BANDS
            .iter()
            .zip(tallies)
            .map(|(band, tally)| BandRow {
                band: *band,
                name: band.name(),
                rule: band.rule(),
                sessions: format_int(tally.sessions as u64),
                spend: usd(tally.cents),
                share: percent(tally.cents, total.cents),
                tally,
            })
            .collect();
        BandTable {
            rows,
            total_sessions: format_int(total.sessions as u64),
            total_spend: usd(total.cents),
        }
    });
    let heavy = heavy_on_short(sessions(), heavy_models, short_session_turns);
    let flags: Vec<FlagRow> = FLAG_KINDS
        .iter()
        .zip(flag_rollup(sessions()))
        .filter(|(_, tally)| tally.sessions > 0)
        .map(|((_, label), tally)| FlagRow {
            label,
            sessions: format_int(tally.sessions as u64),
            spend: usd(tally.cents),
        })
        .collect();
    debug!(
        "bands::build_view: bands={} heavy-on-short={} flag-kinds-hit={}",
        report.outcomes_enabled == Some(true),
        heavy.sessions,
        flags.len()
    );
    BandsView {
        bands,
        heavy_on_short: HeavyOnShort {
            models: heavy_models.iter().map(|m| (*m).to_string()).collect(),
            max_turns: format_int(short_session_turns),
            sessions: format_int(heavy.sessions as u64),
            spend: usd(heavy.cents),
        },
        flags,
    }
}

#[cfg(test)]
mod tests;
