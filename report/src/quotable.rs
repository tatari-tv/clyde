//! Quotable facts: what a rendered artifact is allowed to say numerically.
//!
//! Before this module the foreign-number guard whitelisted every numeric token anywhere in the
//! serialized context block (design "Guard weakness (10)"). At ~940KB that block is mostly session
//! ids, ISO timestamps and commit shas, so effectively every 1-to-3-digit integer was pre-approved:
//! the guard reliably caught a fabricated dollar figure and let a fabricated "14 hours of
//! engineering time" straight through.
//!
//! The fix is to stop treating the serialized block as the whitelist and derive THREE sets from its
//! leaves instead:
//!
//! - [`QuotableFacts::figures`] -- the numeric tokens the prose may state as figures: display
//!   dollars, `tokens-human`, percents, counts, dates. This is the only set the prose guard accepts
//!   a number from.
//! - [`QuotableFacts::identifiers`] -- whole strings the prose may CITE verbatim: `short-id`,
//!   `begin`/`end`, commit shas, PR refs, and the free-text `title`/`summary`/`tags` a citation
//!   quotes. Their digits are exempt only inside a verbatim occurrence (see [`QuotableFacts::mask`]),
//!   so citing session `a1b2c3d4` never adds `1`, `2`, `3` and `4` to the prose whitelist.
//! - [`QuotableFacts::geometry`] -- chart coordinates (Phase 11's `viewBox`/`points`, plus the
//!   `-percent-of-max` bar widths). Kept SEPARATE from the prose whitelist on purpose: a single
//!   `points` string would otherwise inject dozens of small integers into it and quietly undo the
//!   narrowing.
//!
//! Deliberately NOT seeded with a blanket `0..=100` small-integer exemption: that would whitelist
//! `14` and let the planted "14 hours" through, which is the exact case this module exists to catch.
//!
//! The trade this makes is explicit: a false positive here is a HARD render failure, so one
//! silent-acceptance risk is exchanged for one loud-rejection risk. That is why the identifier set
//! exists at all (an untitled session cited by `short-id` and a prose PR reference are the two most
//! likely false positives) and why the corpus behind it is more than one fixture.

use eyre::{Context, Result};
use log::{debug, trace};
use regex::Regex;
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::OnceLock;

/// Longest identifier kept for verbatim masking. An enrich `summary` is the only leaf that can run
/// long; past this it is not a citation anyone types back, and keeping it would cost a scan per
/// render for a match that cannot happen.
const MAX_IDENTIFIER_BYTES: usize = 4096;

/// Chars of an RFC3339 timestamp that form its calendar date (`2026-07-01T09:14:22Z` -> `2026-07-01`).
const DATE_PREFIX_CHARS: usize = 10;

/// Chars a citation abbreviates a commit sha to.
const SHORT_SHA_CHARS: usize = 7;

/// Chars of a date that are its year.
const YEAR_CHARS: usize = 4;

/// Leaf keys whose value is an IDENTIFIER: citable verbatim, never decomposed into free figures.
/// `number` is `prs[].number` (the only `number` key in the block); `title`/`summary`/`tags` are
/// free text authored by the enrich pass, so a number inside one is evidence of nothing and must not
/// license the same number in a headline.
const IDENTIFIER_KEYS: &[&str] = &[
    "short-id",
    "begin",
    "end",
    "feed-version",
    "commits",
    "number",
    "url",
    "repository",
    "title",
    "summary",
    "tags",
];

/// Leaf keys whose value is chart GEOMETRY only, never prose. Phase 11 fills these in; the seam is
/// wired now so the chart fields it adds land in the geometry set instead of widening the prose
/// whitelist the day they appear.
const GEOMETRY_KEYS: &[&str] = &["viewbox", "points"];

/// Suffix of the bar-chart proportion keys (`spend-percent-of-max`, `commits-percent-of-max`,
/// `prs-percent-of-max`, `sessions-percent-of-max`). Both a quotable percent and legitimate bar
/// geometry, so these land in BOTH sets; matched by suffix so a later row type's proportion is
/// covered the day it is added.
const PERCENT_OF_MAX_SUFFIX: &str = "-percent-of-max";

/// Keys whose identifier value is an RFC3339 timestamp, whose calendar-date prefix is separately
/// citable ("the session on 2026-07-01").
const TIMESTAMP_KEYS: &[&str] = &["begin", "end", "feed-version"];

/// What a leaf value licenses the prose to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// Its numeric tokens may be stated as figures.
    Figure,
    /// The whole string may be cited verbatim; its digits are exempt only inside that citation.
    Identifier,
    /// Chart coordinates. Never a prose figure.
    Geometry,
    /// A binary-computed percent that is both a quotable figure and a legitimate bar width.
    FigureAndGeometry,
}

/// The serialized context block paired with the facts it licenses. Built together so the guard can
/// never run against a different block than the one the model was given.
#[derive(Debug)]
pub(crate) struct RenderContext {
    /// The JSON sent to the model, byte for byte.
    pub(crate) json: String,
    /// The three quotable sets derived from it.
    pub(crate) facts: QuotableFacts,
}

/// The three quotable sets. See the module docs for what each one licenses.
#[derive(Debug, Default)]
pub(crate) struct QuotableFacts {
    figures: BTreeSet<String>,
    identifiers: BTreeSet<String>,
    geometry: BTreeSet<String>,
}

impl QuotableFacts {
    /// Derive the sets from a serialized context block. Re-parses the JSON rather than walking the
    /// `ContextBlock` before serialization so the sets are taken from EXACTLY the bytes the model
    /// receives, and so a field a later phase adds is classified by its own key without touching
    /// this module.
    pub(crate) fn from_context_json(json: &str) -> Result<Self> {
        debug!("quotable::from_context_json: context_bytes={}", json.len());
        let value: Value = serde_json::from_str(json)
            .context("failed to re-parse the serialized context block for the quotable-facts pass")?;
        let mut facts = Self::default();
        facts.walk("", &value);
        debug!(
            "quotable::from_context_json: figures={} identifiers={} geometry={}",
            facts.figures.len(),
            facts.identifiers.len(),
            facts.geometry.len()
        );
        if log::log_enabled!(log::Level::Debug) {
            let raw = numeric_token_count(json);
            let distinct = all_numeric_tokens(json).len();
            let share = if raw == 0 {
                0.0
            } else {
                100.0 * facts.figures.len() as f64 / raw as f64
            };
            debug!(
                "quotable::from_context_json: narrowing: pre-change-tokens raw={raw} distinct={distinct} \
                 figures={} ({share:.1}% of raw)",
                facts.figures.len()
            );
        }
        Ok(facts)
    }

    /// Numbers in `prose` that no quotable fact licenses: a sum, a projection, an invented figure.
    /// A non-empty result is the render-invents-nothing violation the render guard fails on.
    ///
    /// A numeric token is permitted when it is in [`Self::figures`], or when EVERY byte of it falls
    /// inside a verbatim occurrence of an identifier. The whole-token rule is what keeps the
    /// identifier set from re-widening the whitelist: a cited PR `#1` masks one byte of a fabricated
    /// `14`, which leaves the token only partly masked and therefore still checked.
    pub(crate) fn foreign_figures(&self, prose: &str) -> Vec<String> {
        debug!(
            "quotable::foreign_figures: prose_bytes={} figures={} identifiers={}",
            prose.len(),
            self.figures.len(),
            self.identifiers.len()
        );
        let masked = self.mask(prose);
        let mut foreign = BTreeSet::new();
        for m in numeric_pattern().find_iter(prose) {
            let token = normalize(m.as_str());
            if self.figures.contains(&token) {
                continue;
            }
            if masked.get(m.start()..m.end()).is_some_and(|s| s.iter().all(|b| *b)) {
                trace!("quotable::foreign_figures: token={token} exempt inside a cited identifier");
                continue;
            }
            foreign.insert(token);
        }
        debug!("quotable::foreign_figures: foreign={}", foreign.len());
        foreign.into_iter().collect()
    }

    /// Count of distinct figure tokens: the narrowing measurement against the pre-change whitelist
    /// ([`all_numeric_tokens`]).
    pub(crate) fn figure_count(&self) -> usize {
        self.figures.len()
    }

    /// A byte mask over `prose`, true wherever a verbatim identifier occurrence covers the byte.
    /// One `match_indices` pass per identifier (linear in the prose per identifier, never quadratic
    /// in the prose).
    fn mask(&self, prose: &str) -> Vec<bool> {
        let mut masked = vec![false; prose.len()];
        let mut occurrences = 0usize;
        for id in &self.identifiers {
            for (start, matched) in prose.match_indices(id.as_str()) {
                occurrences += 1;
                for slot in masked.iter_mut().skip(start).take(matched.len()) {
                    *slot = true;
                }
            }
        }
        debug!(
            "quotable::mask: identifiers={} occurrences={} prose_bytes={}",
            self.identifiers.len(),
            occurrences,
            prose.len()
        );
        masked
    }

    /// Walk the value tree, classifying each leaf by the key that owns it. An array element inherits
    /// its array's key (`commits: [sha, sha]`), which is what makes `commits[]` and `tags[]`
    /// classify as their parent does.
    fn walk(&mut self, key: &str, value: &Value) {
        match value {
            Value::Object(map) => {
                for (k, v) in map {
                    self.add_label_segments(k);
                    self.walk(k, v);
                }
            }
            Value::Array(items) => {
                for v in items {
                    self.walk(key, v);
                }
            }
            Value::String(s) => self.absorb(key, s),
            Value::Number(n) => self.absorb(key, &n.to_string()),
            Value::Bool(_) | Value::Null => {}
        }
    }

    /// The digit-bearing segments of a FIELD NAME (`session-spend-p90` -> `p90`,
    /// `cache-1h-write-fraction` -> `1h`) are citable label text: both prompts describe those
    /// signals in words the artifact prints back ("the 1h premium", "the p90 session"). Only mixed
    /// digit-and-letter segments qualify, so a purely numeric path element could never smuggle a
    /// bare figure in through a key.
    fn add_label_segments(&mut self, key: &str) {
        for segment in key.split('-') {
            let has_digit = segment.chars().any(|c| c.is_ascii_digit());
            let has_alpha = segment.chars().any(|c| c.is_ascii_alphabetic());
            if has_digit && has_alpha {
                self.identifiers.insert(segment.to_string());
            }
        }
    }

    /// Route one leaf value into its set(s).
    fn absorb(&mut self, key: &str, raw: &str) {
        match classify(key) {
            Class::Figure => self.add_figure_tokens(raw),
            Class::Geometry => self.add_geometry_tokens(raw),
            Class::FigureAndGeometry => {
                self.add_figure_tokens(raw);
                self.add_geometry_tokens(raw);
            }
            Class::Identifier => self.add_identifier(key, raw),
        }
    }

    fn add_figure_tokens(&mut self, raw: &str) {
        for m in numeric_pattern().find_iter(raw) {
            let token = m.as_str();
            self.figures.insert(normalize(token));
            // A date's YEAR is quotable on its own (section headers state it); its month and day
            // are not, which is the whole point of keeping the date one token.
            if date_prefix(token).is_some()
                && let Some(year) = char_prefix(token, YEAR_CHARS)
            {
                self.figures.insert(year);
            }
        }
    }

    fn add_geometry_tokens(&mut self, raw: &str) {
        for m in numeric_pattern().find_iter(raw) {
            self.geometry.insert(normalize(m.as_str()));
        }
    }

    /// Add an identifier plus the abbreviated forms a citation actually uses: a commit sha's short
    /// prefix, a timestamp's calendar date, and a PR number's `#N` form.
    fn add_identifier(&mut self, key: &str, raw: &str) {
        if raw.is_empty() {
            return;
        }
        if raw.len() > MAX_IDENTIFIER_BYTES {
            trace!(
                "quotable::add_identifier: key={key} skipped, {} bytes over the {MAX_IDENTIFIER_BYTES} cap",
                raw.len()
            );
            return;
        }
        self.identifiers.insert(raw.to_string());
        if key == "commits"
            && raw.chars().count() > SHORT_SHA_CHARS
            && let Some(short) = char_prefix(raw, SHORT_SHA_CHARS)
        {
            self.identifiers.insert(short);
        }
        if TIMESTAMP_KEYS.contains(&key)
            && let Some(date) = date_prefix(raw)
        {
            self.identifiers.insert(date);
        }
        if key == "number" {
            self.identifiers.insert(format!("#{raw}"));
        }
    }
}

/// Which set a leaf key's value belongs to.
///
/// A DENYLIST by construction: the named keys are identifiers or geometry, everything else is a
/// figure. That is deliberate and safe because the context block is string-only display values by
/// construction (design Phase 5 -- every raw operand is `#[serde(skip)]`), so an unnamed key is a
/// figure the binary already formatted. An allowlist would instead hard-fail the render every time a
/// later phase adds a field, which is the wrong failure for a guard whose false positive is fatal.
fn classify(key: &str) -> Class {
    if key.ends_with(PERCENT_OF_MAX_SUFFIX) {
        return Class::FigureAndGeometry;
    }
    if GEOMETRY_KEYS.contains(&key) {
        return Class::Geometry;
    }
    if IDENTIFIER_KEYS.contains(&key) {
        return Class::Identifier;
    }
    Class::Figure
}

/// The calendar-date prefix of an RFC3339 timestamp, or `None` when the value is not date-shaped.
fn date_prefix(raw: &str) -> Option<String> {
    let prefix = char_prefix(raw, DATE_PREFIX_CHARS)?;
    let shaped = prefix
        .chars()
        .enumerate()
        .all(|(i, c)| if i == 4 || i == 7 { c == '-' } else { c.is_ascii_digit() });
    shaped.then_some(prefix)
}

/// The first `n` chars of `raw`, or `None` when it is shorter. Char-based, per the crate's
/// no-string-slice lint.
fn char_prefix(raw: &str, n: usize) -> Option<String> {
    let prefix: String = raw.chars().take(n).collect();
    (prefix.chars().count() == n).then_some(prefix)
}

/// Every numeric token in `text` under the PRE-CHANGE tokenizer, deduped. This is the whitelist the
/// guard used before the three sets existed, kept unchanged so the narrowing is measured against
/// what actually shipped rather than against a moved goalpost.
pub(crate) fn all_numeric_tokens(text: &str) -> BTreeSet<String> {
    pre_change_pattern()
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect()
}

/// How many tokens the pre-change whitelist carried with duplicates kept: its literal size, since
/// it was a `Vec` scanned with `contains`. [`all_numeric_tokens`] is its distinct size; both are
/// reported when the narrowing is measured, because the two answer different questions.
pub(crate) fn numeric_token_count(text: &str) -> usize {
    pre_change_pattern().find_iter(text).count()
}

/// The pre-change tokenizer: bare digit runs, so `$9,450.31` was two tokens (`9` and `450.31`) and
/// a standalone `9` anywhere in the prose rode in on the thousands separator.
fn pre_change_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\d+(?:\.\d+)?").expect("pre-change numeric pattern is a valid regex"))
}

/// A numeric token, thousands separators included: `9,450.31` is ONE token, not `9` plus `450.31`.
/// Two things follow, both wanted. A figure keeps its magnitude, so quoting `$9,450.31` no longer
/// licenses a bare `9` anywhere in the prose. And a count the binary emitted as a bare integer
/// (`6200`) matches the comma-grouped form the artifact prints (`6,200`) once [`normalize`] strips
/// the separators, which the pre-change guard only ever got away with by accident.
///
/// A date (`2026-07-01` -> `2026`, `07`, `01`) and a version still decompose into separate tokens
/// that each match the facts, so legitimate dates and versions never read as fabricated.
/// A calendar date is ONE token, not three. The pre-change guard split `2026-07-14` into `2026`,
/// `07` and `14`, which is how every day-of-month in the window became a pre-approved standalone
/// integer -- the planted "14 hours" rode in on the 14th of the month. Keeping the date whole means
/// a date is quotable as a date and nothing else; [`QuotableFacts::add_figure_tokens`] adds the YEAR
/// back on its own, because a header ("Claude Code, April 2026") legitimately states it.
fn numeric_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\d{4}-\d{2}-\d{2}|\d{1,3}(?:,\d{3})+(?:\.\d+)?|\d+(?:\.\d+)?")
            .expect("numeric-token pattern is a valid regex")
    })
}

/// One canonical spelling per figure: `6,200` and `6200` are the same fact, so they compare equal.
fn normalize(token: &str) -> String {
    token.replace(',', "")
}

#[cfg(test)]
mod tests;
