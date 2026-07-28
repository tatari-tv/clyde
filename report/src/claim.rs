//! Claim-shaped guards over rendered prose: fabricated DURATIONS and bare MULTIPLIERS.
//!
//! [`crate::render::reject_foreign_numbers`] is VALUE-shaped -- it asks whether a figure appears in
//! the quotable-facts set. That is the right guard for a fabricated dollar or token figure, and it
//! structurally cannot reach "14 hours of engineering time" on a real window: `14` is licensed
//! several times over there (a day with 14 sessions, a repo with 14 sessions, a PR numbered 14), so
//! the value passes and the sentence is still fabricated. The fabrication is the UNIT, not the
//! number, and no value whitelist can ever catch it. Phase 10 measured exactly this and recommended
//! a claim-shaped check instead of widening the value set.
//!
//! Both templates already ban these sentences, so this enforces a stated rule rather than inventing
//! policy: Hard prohibition 2 ("no speculative quantification", with "would have required N hours of
//! senior-engineer time" listed verbatim) and the strict rule "Numbers not in the context (hours,
//! days of work, headcount equivalents) are NEVER fabricated".
//!
//! A rejection is a HARD render failure, so each pattern is drawn as narrowly as its evidence
//! allows:
//!
//! - **Duration units with no context source at all** (hours, minutes, seconds, weeks, months,
//!   years) are rejected on the unit alone. Nothing in the context block is denominated in them, so
//!   a figure carrying one is fabricated by construction. The unit must be a NOUN separated from the
//!   figure by whitespace: `1-hour` and `5-minute` are hyphenated ADJECTIVES naming a cache-write
//!   tier, which `aggregates.cache` does license, and they are not matched.
//! - **Days are licensed** (`period.days`, `period.active-days`, an `N-day` window), so a day figure
//!   is rejected only when it is framed as LABOR -- an engineer-day compound, a day count qualified
//!   as engineering time, or days "saved". "30 days" and "30 days of work across 7 repos" stay
//!   legal, because both are statements about the window, not about effort.
//! - **A bare `Nx` multiplier** is rejected outright. It is arithmetic over two figures by
//!   definition, and the binary emits no multiplier for prose to copy.

use eyre::{Result, bail};
use log::{debug, trace};
use regex::Regex;
use std::sync::OnceLock;

use crate::quotable::QuotableFacts;

/// One rejected claim: what matched, which rule it broke, and the BYTE span of the capture, so the
/// caller quotes the actual clause via `render::excerpt_at` rather than re-searching the prose for
/// the matched text (the same span-not-substring fix `QuotableFacts::ForeignFigure` carries).
/// Typed rather than a formatted string, so the caller composes the operator-facing error instead of
/// re-parsing one.
#[derive(Debug, PartialEq)]
pub(crate) struct Violation {
    pub text: String,
    pub rule: &'static str,
    pub start: usize,
    pub end: usize,
}

const DURATION_RULE: &str = "no field in the context block is denominated in that unit, so the \
                             figure was invented (Hard prohibition 2: no speculative quantification)";
const LABOR_RULE: &str = "a day count is licensed, but framing it as engineering effort is not \
                          (Hard prohibition 2: no speculative quantification)";
const MULTIPLIER_RULE: &str = "a multiplier is arithmetic over two figures and the binary emits \
                               none (Hard prohibition 1: every number is copied, never computed)";

/// Reject generated prose that made a duration or multiplier claim. `kind` names the render path
/// for the operator-facing error and WARN, mirroring `reject_foreign_numbers`. Fail closed.
///
/// `facts` supplies the same identifier exemption the VALUE guard applies: see
/// [`fabricated_claims`].
pub(crate) fn reject_fabricated_claims(kind: &str, prose: &str, facts: &QuotableFacts) -> Result<()> {
    let violations = fabricated_claims(prose, facts);
    debug!(
        "claim::reject_fabricated_claims: kind={kind} prose_chars={} violations={}",
        prose.chars().count(),
        violations.len()
    );
    if violations.is_empty() {
        return Ok(());
    }
    // The matched text WITH the sentence around it, for the same reason the foreign-number guard
    // quotes an excerpt: a bare match names the phrase and not the claim it sits inside. Grouped and
    // capped by `render::group_by_label`/`render::cite`, the same machinery the value guard uses, so
    // a claim repeated many times over one artifact does not produce one line per occurrence.
    let groups = crate::render::group_by_label(&violations, |v| v.text.as_str(), |v| (v.start, v.end), |v| v.rule);
    let cited = crate::render::cite(&groups, prose, |g, excerpt| {
        format!("{:?} in {excerpt:?} -- {}", g.label, g.extra)
    });
    log::warn!(
        "claim::reject_fabricated_claims: {kind} path REJECTED -- generated prose made \
         fabricated claim(s): {cited}"
    );
    bail!(
        "{kind} rendering made claim(s) the context block cannot support: {cited} -- refusing to emit \
         the artifact"
    );
}

/// Every duration or multiplier claim in `prose`, in pattern order. Separated from the bail so the
/// tests read the findings directly instead of grepping an error string.
///
/// Each pattern carries the claim in capture group 1, because the leading [`OPENS_A_FIGURE`] guard
/// consumes the character before it.
///
/// A claim lying entirely INSIDE a verbatim identifier occurrence is exempt, mirroring
/// `QuotableFacts::foreign_figures`. The two guards run over the same prose and must agree about
/// what a citation is: `summary`, `title` and `notes` are classified `Identifier` precisely so the
/// prose may quote them, and both prompts instruct it to. An enrich summary reading "spent 3 hours
/// chasing the flake", quoted verbatim, is a licensed citation on the value side -- and used to be a
/// HARD render failure on this side, throwing away a paid call over the model doing as it was told.
/// The exemption is span-scoped, so a fabricated "3 hours" anywhere OUTSIDE a quoted identifier is
/// still caught.
pub(crate) fn fabricated_claims(prose: &str, facts: &QuotableFacts) -> Vec<Violation> {
    let cited = facts.cited_mask(prose);
    let mut out = Vec::new();
    for (pattern, rule) in [
        (duration_pattern(), DURATION_RULE),
        (labor_day_pattern(), LABOR_RULE),
        (multiplier_pattern(), MULTIPLIER_RULE),
    ] {
        for caps in pattern.captures_iter(prose) {
            let Some(claim) = caps.get(1) else {
                continue;
            };
            if cited
                .get(claim.start()..claim.end())
                .is_some_and(|span| !span.is_empty() && span.iter().all(|b| *b))
            {
                trace!(
                    "claim::fabricated_claims: {:?} exempt inside a cited identifier",
                    claim.as_str()
                );
                continue;
            }
            out.push(Violation {
                text: claim.as_str().to_string(),
                rule,
                start: claim.start(),
                end: claim.end(),
            });
        }
    }
    out
}

/// What may sit immediately before a figure for it to READ as a quantity: start of text, or any
/// character that is not part of a number or an identifier.
///
/// This is the narrowing the committed goldens forced. A plain `\b` matched the `6` in
/// `claude-sonnet-4-6 second and claude-haiku-4-5 third` -- a model name followed by an ordinal,
/// which is correct prose in a shipped artifact. Excluding a preceding `-`, `.`, or `,` means a
/// digit inside an identifier or inside a comma-grouped magnitude is never the start of a claim.
/// `crate::regex` has no lookbehind, so the character is consumed and the claim is capture group 1.
const OPENS_A_FIGURE: &str = r"(?:^|[^\w.,-])";

/// What may separate a figure from the unit it counts: horizontal whitespace, optionally spanning
/// ONE soft line wrap.
///
/// Whitespace is required (that is what makes the unit a noun the figure counts, so the hyphenated
/// cache-tier adjectives `1-hour cache writes` / `5-minute cache writes` do not match), but a plain
/// `\s+` also crossed blank lines and swallowed whole document structures. It rejected a correct
/// HTML render whose last table cell read `$24.98` and whose NEXT SECTION HEADING was `Month over
/// Month`: seven newlines apart, matched as "24.98 months", and the artifact -- a paid call -- was
/// refused over it. A figure ending one block and a word opening another are not a claim.
///
/// One wrap is still allowed, because prose legitimately breaks a line between the number and its
/// unit; two means they are in different blocks.
/// `[^\S\r\n]` is "whitespace that is not a line break". Spelled that way rather than `[ \t]`
/// because the labor pattern compiles under the `x` (verbose) flag, which strips a literal space
/// from the pattern -- including inside a character class -- and silently left a tab-only separator
/// that no real prose matches.
const SEPARATES_A_FIGURE: &str = r"(?:[^\S\r\n]+|[^\S\r\n]*\r?\n[^\S\r\n]*)";

/// A figure followed by a duration unit the context block never carries.
fn duration_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"(?i){OPENS_A_FIGURE}(\d[\d,]*(?:\.\d+)?{SEPARATES_A_FIGURE}(?:hours?|hrs?|minutes?|mins?|seconds?|secs?|weeks?|months?|years?))\b"
        ))
        .expect("duration pattern is a valid regex")
    })
}

/// A DAY figure framed as labor. Three shapes, and only these three, because `period.days` and
/// `period.active-days` make a bare day count legitimate:
///
/// - a labor compound (`14 engineer-days`, `3 person days`);
/// - a day count qualified as engineering effort (`3 days of senior-engineer time`);
/// - days saved (`saved 14 days`, `14 days saved`).
fn labor_day_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // `(?x)` (verbose) so the four alternatives stay one per line and readable; literal
        // whitespace is ignored under it, and every space this pattern means is written `\s`.
        Regex::new(&format!(
            r"(?ix) {OPENS_A_FIGURE} (
                  \d[\d,]*(?:\.\d+)? [\s-]+ (?:engineer|engineering|developer|dev|person|human|staff|senior) [\s-]+ days?
                | \d[\d,]*(?:\.\d+)? {SEPARATES_A_FIGURE} days? \s+ of \s+ (?:senior[\s-]*)? (?:engineer\w*|developer|dev|human|manual|staff)
                | \d[\d,]*(?:\.\d+)? {SEPARATES_A_FIGURE} days? \s+ saved
                | save[ds]? \s+ \d[\d,]*(?:\.\d+)? \s+ days?
              ) \b"
        ))
        .expect("labor-day pattern is a valid regex")
    })
}

/// A bare `Nx` multiplier: `3x`, `1.5x`, `10x the prior period`. The trailing `\b` keeps a session
/// short-id (`3xf9a2b1`) out of it, and [`OPENS_A_FIGURE`] keeps a model name out.
fn multiplier_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(r"(?i){OPENS_A_FIGURE}(\d[\d,]*(?:\.\d+)?x)\b"))
            .expect("multiplier pattern is a valid regex")
    })
}

#[cfg(test)]
mod tests;
