//! Quotable facts: what a rendered artifact is allowed to say numerically.
//!
//! Before this module the foreign-number guard whitelisted every numeric token anywhere in the
//! serialized context block (design "Guard weakness (10)"). At ~940KB that block is mostly session
//! ids, ISO timestamps and commit shas, so effectively every 1-to-3-digit integer was pre-approved:
//! the guard reliably caught a fabricated dollar figure and let a fabricated "14 hours of
//! engineering time" straight through.
//!
//! The fix is to stop treating the serialized block as the whitelist and derive FOUR sets from its
//! leaves instead:
//!
//! - [`QuotableFacts::figures`] -- the numeric tokens the prose may state as figures: display
//!   dollars, `tokens-human`, percents, counts, dates. This is the only set the prose guard accepts
//!   a number from.
//! - [`QuotableFacts::identifiers`] -- whole strings the prose may CITE verbatim: `short-id`,
//!   `begin`/`end`, commit shas, PR refs, and the free-text `title`/`summary`/`tags` a citation
//!   quotes. Their digits are exempt only inside a verbatim occurrence (see [`QuotableFacts::mask`]),
//!   so citing session `a1b2c3d4` never adds `1`, `2`, `3` and `4` to the prose whitelist.
//! - `QuotableFacts::cited` -- numeric tokens lexed from identifier text that the prose may repeat
//!   WITHOUT reproducing the whole identifier: structured tokens (versions, decimals, dates) and
//!   bare integers of 3+ digits. Exists because the prompt instructs the model to cite
//!   titles/summaries/commit text -- a paraphrase task -- and the verbatim mask alone rejected
//!   every paraphrase of a true, sourced number. Bare 1-2 digit integers are excluded so the
//!   planted-"14 hours" catch survives; see the field docs for the full trade.
//! - [`QuotableFacts::geometry`] -- chart coordinates (Phase 11's `viewBox`/`points`, plus the
//!   `-percent-of-max` bar widths), each stored as its WHOLE value rather than tokenized. Kept
//!   SEPARATE from the prose whitelist on purpose: a single `points` string would otherwise inject
//!   dozens of small integers into it and quietly undo the narrowing. Whole values, because the
//!   geometry rule (`geometry::reject_foreign_geometry`) is "this attribute value is one the binary
//!   computed, byte for byte" -- a token-level set would accept a fabricated `cx="120"` the moment
//!   `120` happened to be one point's y coordinate.
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
/// license the same number in a headline. `notes` is free text for the same reason: the M2 window
/// sentence carries `M2`, `v2` and `v1`, and classifying it as a figure would hand the prose a bare
/// `1` and `2` -- quotable verbatim, never decomposed.
///
/// `repo` AND `repository` are both here, and both must stay. They are the same concept reached by
/// two keys -- `RepoRow.repo` / `SessionEntry.repo` for the attribution slug, `prs[].repository` for
/// the PR's own -- and `eval::mechanical`'s `Ground::walk` already treats them as one
/// (`matches!(key, "repo" | "repository")`). Listing only `repository` fell through to
/// `Class::Figure` for every `repo`, and `add_figure_tokens` decomposes a slug into its digit runs:
/// a repo named `org/service14` licensed a bare `14` as an unconditional prose figure ANYWHERE in
/// the artifact. That is this module's own motivating bug ("14 hours of engineering time")
/// reproduced through a repo name.
const IDENTIFIER_KEYS: &[&str] = &[
    "short-id",
    "begin",
    "end",
    "feed-version",
    "commits",
    "number",
    "url",
    "repo",
    "repository",
    "title",
    "summary",
    "tags",
    "notes",
];

/// Leaf keys whose value is chart GEOMETRY only, never prose: the `viewbox` and `points` strings
/// `chart::LineChart` precomputes (Phase 11). Their digits never reach the prose whitelist, and the
/// whole value is what the HTML geometry allowlist matches attributes against.
const GEOMETRY_KEYS: &[&str] = &["viewbox", "points"];

/// Suffix of the bar-chart proportion keys (`spend-percent-of-max`, `commits-percent-of-max`,
/// `prs-percent-of-max`, `sessions-percent-of-max`). Both a quotable percent and legitimate bar
/// geometry, so these land in BOTH sets; matched by suffix so a later row type's proportion is
/// covered the day it is added.
const PERCENT_OF_MAX_SUFFIX: &str = "-percent-of-max";

/// Keys whose identifier value is an RFC3339 timestamp, whose calendar-date prefix is separately
/// citable ("the session on 2026-07-01").
const TIMESTAMP_KEYS: &[&str] = &["begin", "end", "feed-version"];

/// The identifier keys whose value is FREE TEXT the prompt instructs the model to cite -- the only
/// leaves whose numeric tokens feed [`QuotableFacts::cited`]. Deliberately NOT every identifier:
/// a sha, short-id, or URL is random characters, and lexing those would license every 3+ digit run
/// in every hex id as a standalone prose figure (`8f14e45f...` licensing a bare `167`), which is
/// the pre-change whitelist creeping back in through identifiers nobody cites as prose. `notes` is
/// also excluded: it is binary-authored methodology text the prompt orders quoted verbatim WHOLE
/// (never paraphrased), so a count inside a note stays quotable only inside its sentence.
const FREE_TEXT_KEYS: &[&str] = &["title", "summary", "tags"];

/// The one key whose class depends on the value's JSON shape, not on the key alone.
///
/// `commits` is overloaded across the context block: `sessions[].outcomes.commits` is an ARRAY OF
/// SHA STRINGS (identifiers, cited verbatim and abbreviated to a short prefix), while
/// `outcomes.totals.commits` and `by-repo[].outcomes.commits` are bare NUMBER counts (figures the
/// prose states outright). Classifying on the key alone routed the count through `add_identifier`,
/// which licensed its digits by verbatim substring instead of as the figure it is -- and put a
/// two-digit count through the short-sha branch on the way.
const SHAPE_DEPENDENT_KEY: &str = "commits";

/// The JSON shape a leaf value arrived as. Only [`SHAPE_DEPENDENT_KEY`] consults it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// A JSON string.
    Text,
    /// A JSON number, stringified for tokenizing.
    Number,
}

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

/// The quotable sets. See the module docs for what each one licenses.
#[derive(Debug, Default)]
pub(crate) struct QuotableFacts {
    figures: BTreeSet<String>,
    identifiers: BTreeSet<String>,
    geometry: BTreeSet<String>,
    /// Numeric tokens lexed from FREE-TEXT identifier leaves ([`FREE_TEXT_KEYS`]: title, summary,
    /// tags) that the prose may repeat WITHOUT reproducing the whole identifier: structured
    /// tokens (`0.5.4`, `0.85`, dates) and bare integers of three or more digits (`500`, `2026`).
    /// The prompt tells the model to CITE session titles, summaries and
    /// commit text -- a paraphrase task -- while the identifier mask only exempts a byte-for-byte
    /// reproduction of the whole string, so "shipped as v0.5.0" against a summary reading "bump to
    /// v0.5.0" was rejected as fabrication: a true, sourced fact treated identically to an invented
    /// one, at the cost of a full render each time. This set licenses what the prompt demands.
    ///
    /// Bare ONE- and TWO-digit integers are deliberately excluded: they are the high-collision class
    /// (counts, day numbers), and licensing them from free text would re-legalize the planted
    /// "14 hours of engineering time" the moment any summary carried a bare `14` -- this module's
    /// motivating catch. Those stay quotable only inside a verbatim occurrence of their identifier.
    cited: BTreeSet<String>,
}

/// One number in the prose no quotable fact licenses, carrying the BYTE span of the exact
/// occurrence the regex matched. Before this, a rejection named only the token and the render guard
/// re-searched the whole document for a `starts_with` match to build its excerpt -- which is how
/// `500` quoted a line carrying the licensed `$1,500.08` and `100` quoted an unrelated model id. The
/// span travels with the token so the caller quotes the actual violating occurrence instead of
/// guessing at one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForeignFigure {
    pub(crate) token: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
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
            "quotable::from_context_json: figures={} identifiers={} geometry={} cited={}",
            facts.figures.len(),
            facts.identifiers.len(),
            facts.geometry.len(),
            facts.cited.len()
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
    ///
    /// One entry per rejected OCCURRENCE, in match order, not deduped by token: each carries the
    /// exact span it was found at, so a caller quoting the sentence never has to re-locate it.
    pub(crate) fn foreign_figures(&self, prose: &str) -> Vec<ForeignFigure> {
        debug!(
            "quotable::foreign_figures: prose_bytes={} figures={} identifiers={}",
            prose.len(),
            self.figures.len(),
            self.identifiers.len()
        );
        let masked = self.mask(prose);
        let mut foreign = Vec::new();
        for m in numeric_pattern().find_iter(prose) {
            let token = normalize(m.as_str());
            if self.figures.contains(&token) {
                continue;
            }
            if self.cited.contains(&token) {
                trace!("quotable::foreign_figures: token={token} licensed by citable source text");
                continue;
            }
            if masked.get(m.start()..m.end()).is_some_and(|s| s.iter().all(|b| *b)) {
                trace!("quotable::foreign_figures: token={token} exempt inside a cited identifier");
                continue;
            }
            foreign.push(ForeignFigure {
                token,
                start: m.start(),
                end: m.end(),
            });
        }
        debug!("quotable::foreign_figures: foreign={}", foreign.len());
        foreign
    }

    /// Count of distinct figure tokens: the narrowing measurement against the pre-change whitelist
    /// ([`all_numeric_tokens`]).
    pub(crate) fn figure_count(&self) -> usize {
        self.figures.len()
    }

    /// `true` when `value` is a geometry string the binary computed: a `viewBox`, a `points` list,
    /// or a bar proportion, matched WHOLE. This is the licence the HTML geometry allowlist checks
    /// every digit-bearing attribute inside a chart subtree against, so a fabricated coordinate,
    /// a reflowed `points` string, or an unanticipated attribute all fail it.
    pub(crate) fn licenses_geometry(&self, value: &str) -> bool {
        self.geometry.contains(value.trim())
    }

    /// How many distinct geometry values the context licenses. Operator-facing only (the geometry
    /// guard's DEBUG line), the way [`Self::figure_count`] is for the prose guard.
    pub(crate) fn geometry_count(&self) -> usize {
        self.geometry.len()
    }

    /// A byte mask over `prose`, true wherever a verbatim identifier occurrence covers the byte.
    /// One `match_indices` pass per identifier (linear in the prose per identifier, never quadratic
    /// in the prose).
    /// [`Self::mask`], for a guard outside this module.
    ///
    /// The claim guard needs the SAME exemption the value guard has: `summary`, `title` and `notes`
    /// are classified `Identifier` precisely so the prose may quote them verbatim, and a quoted
    /// enrich summary reading "spent 3 hours chasing the flake" is a licensed citation, not a
    /// fabricated duration. Without this the two guards disagreed about the same sentence and the
    /// claim guard hard-failed a paid render over it.
    ///
    /// Returned as the whole byte mask rather than a per-span predicate so a caller checking many
    /// spans pays the identifier scan ONCE, not once per span.
    pub(crate) fn cited_mask(&self, prose: &str) -> Vec<bool> {
        self.mask(prose)
    }

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
            Value::String(s) => self.absorb(key, s, Shape::Text),
            Value::Number(n) => self.absorb(key, &n.to_string(), Shape::Number),
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
                if let Some(ordinal) = percentile_ordinal(segment) {
                    self.identifiers.insert(ordinal);
                }
            }
        }
    }

    /// Route one leaf value into its set(s).
    fn absorb(&mut self, key: &str, raw: &str, shape: Shape) {
        match classify(key, shape) {
            Class::Figure => self.add_figure_tokens(raw),
            Class::Geometry => self.add_geometry_value(raw),
            Class::FigureAndGeometry => {
                self.add_figure_tokens(raw);
                self.add_geometry_value(raw);
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

    /// Add a geometry value WHOLE: an attribute is licensed only when its value is one of these
    /// strings byte for byte, so the value is stored exactly as the binary emitted it (trimmed of
    /// surrounding whitespace, which HTML attribute quoting can add and which changes no geometry).
    /// Never tokenized: see the module docs for why a token-level set would not fail closed.
    fn add_geometry_value(&mut self, raw: &str) {
        let value = raw.trim();
        if value.is_empty() {
            return;
        }
        trace!("quotable::add_geometry_value: bytes={}", value.len());
        self.geometry.insert(value.to_string());
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
        // Token-level licensing for the free text the prompt tells the model to cite: see the
        // `cited` field docs for the rule and the trade, and `FREE_TEXT_KEYS` for why other
        // identifiers (shas, ids, urls) never feed it.
        if FREE_TEXT_KEYS.contains(&key) {
            for m in numeric_pattern().find_iter(raw) {
                let token = normalize(m.as_str());
                if cited_token_qualifies(&token) {
                    self.cited.insert(token);
                }
            }
        }
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
fn classify(key: &str, shape: Shape) -> Class {
    if key.ends_with(PERCENT_OF_MAX_SUFFIX) {
        return Class::FigureAndGeometry;
    }
    if GEOMETRY_KEYS.contains(&key) {
        return Class::Geometry;
    }
    // See `SHAPE_DEPENDENT_KEY`: a sha string is an identifier, a count is a figure, and they share
    // a key.
    if key == SHAPE_DEPENDENT_KEY {
        return match shape {
            Shape::Text => Class::Identifier,
            Shape::Number => Class::Figure,
        };
    }
    if IDENTIFIER_KEYS.contains(&key) {
        return Class::Identifier;
    }
    Class::Figure
}

/// The English ordinal a `p<N>` percentile label is WRITTEN as: `p90` -> `90th`, `p50` -> `50th`.
/// `None` for any other segment shape.
///
/// Found by the Phase 13 render eval, on the first live render it ever measured. `session-spend-p90`
/// is a real field and "the 90th percentile" is the natural way to say it, but the label segment
/// `p90` only masks the digits INSIDE `p90` -- so a correct sentence about a real figure was
/// rejected as a fabrication, twice in a row, on both render paths. This licenses the ordinal
/// SPELLING of a label the binary itself named, and nothing else: a bare `90` anywhere in the prose
/// is still unlicensed, which is the narrowing Phase 10 exists to keep.
fn percentile_ordinal(segment: &str) -> Option<String> {
    let digits = segment.strip_prefix('p')?;
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    // 11th/12th/13th are the exceptions every naive ordinal function gets wrong.
    let suffix = match (n % 100, n % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    };
    Some(format!("{digits}{suffix}"))
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
/// A calendar date is ONE token, not three. The pre-change guard split `2026-07-14` into `2026`,
/// `07` and `14`, which is how every day-of-month in the window became a pre-approved standalone
/// integer -- the planted "14 hours" rode in on the 14th of the month. Keeping the date whole means
/// a date is quotable as a date and nothing else; [`QuotableFacts::add_figure_tokens`] adds the YEAR
/// back on its own, because a header ("Claude Code, April 2026") legitimately states it.
///
/// A DOTTED VERSION is ONE token for the same reason: `v0.5.4` used to lex as `0.5` plus a bare
/// `4`, so no licensing rule could ever make a real version from a commit message quotable without
/// also handing the prose a standalone `4`. The `v`/`V` prefix is captured INTO the token so it
/// can never strand a bare `0.5.4` next to an unmatched `v`; [`normalize`] then canonicalizes the
/// prefix away, so `v0.6.5` and `0.6.5` are one fact in either direction (measured live: models
/// write the conventional `v` form even when the source is bare).
fn numeric_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\d{4}-\d{2}-\d{2}|[vV]?\d+(?:\.\d+){2,}|\d{1,3}(?:,\d{3})+(?:\.\d+)?|\d+(?:\.\d+)?")
            .expect("numeric-token pattern is a valid regex")
    })
}

/// Whether a token lexed from citable identifier text is licensed on its own (the
/// [`QuotableFacts::cited`] set): anything EXCEPT a bare integer of one or two digits. Structured
/// tokens (a dot, a date's dashes) and 3+ digit integers carry enough shape that repeating one is
/// citation, not invention; a bare `14` carries none, and licensing it from free text is exactly
/// how the fabricated "14 hours" would come back.
fn cited_token_qualifies(token: &str) -> bool {
    let digits = token.chars().filter(char::is_ascii_digit).count();
    let structured = token.chars().any(|c| !c.is_ascii_digit());
    structured || digits >= 3
}

/// One canonical spelling per figure: `6,200` and `6200` are the same fact, so they compare equal.
///
/// A dotted version's `v`/`V` prefix is normalized away for the same reason. The 2026-07-28 rate
/// measurement on the shipped guard found 6 of 7 rejections were the SAME true fact: a summary
/// reading "bump ... from 0.6.4 to 0.6.5" (bare) against prose writing the conventional `v0.6.5`.
/// The model normalizes version typography regardless of the prompt's copy rule, and a prefix
/// carrying zero numeric information must not turn a sourced version into a fatal rejection. A
/// WRONG version still rejects; only the spelling is canonicalized, exactly like the commas.
fn normalize(token: &str) -> String {
    let stripped = token.replace(',', "");
    if let Some(rest) = stripped.strip_prefix(['v', 'V'])
        && rest.chars().all(|c| c.is_ascii_digit() || c == '.')
    {
        return rest.to_string();
    }
    stripped
}

#[cfg(test)]
mod tests;
