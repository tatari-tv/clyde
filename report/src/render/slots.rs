//! The prose layer: a small, bounded LLM call per document section.
//!
//! Each slot gets its own subprocess, its own curated brief, and its own fact ALLOWLIST. It returns
//! digit-free markdown prose in which every figure is a `{{fact:key}}` placeholder. Rust validates,
//! then interpolates.
//!
//! Nothing here can fail a render. A slot that violates its contract gets ONE retry with the
//! violation named, and then ships EMPTY with a WARN. That is the whole point of the inversion: the
//! artifact is already complete before any of this runs, so the worst case is a thinner document,
//! never a discarded one.

use comrak::nodes::{AstNode, NodeValue};
use comrak::{Arena, Options, parse_document};
use log::{debug, info, warn};
use serde::Serialize;

use super::document::SlotProse;
use super::facts::FactRegistry;
use crate::summarize::{Job, Kind, Transport};

/// The hard contract every slot is held to. Verified against a live `claude -p` call before any of
/// this was built (design Phase 0): the model returns conforming, digit-free placeholder prose.
const SLOT_SYSTEM_PROMPT: &str = "You write one prose slot of a Claude Code usage report. You do \
     not author the report; a deterministic renderer owns every table, every chart, and every \
     number. HARD CONTRACT, non-negotiable: (1) Output markdown PARAGRAPH PROSE only -- no \
     headings, no tables, no lists, no blockquotes, no code fences, no raw HTML. (2) Your output \
     MUST NOT contain any numeric character: no digits, no roman numerals, no fractions, no \
     superscripts. Not \"3\", not \"24/7\". (3) Every quantity appears ONLY as a placeholder of the \
     form {{fact:key}}, with key taken VERBATIM from the allowlist you are given. Never invent a \
     key, alter a key, or use a key that is not on your allowlist. (4) Do not write the VALUE of a \
     fact; write its placeholder. The renderer substitutes values. (5) Do not quantify in words to \
     dodge rule 2: no \"nearly tripled\", no \"a third of\". (6) No em dashes. (7) Emit the prose \
     and nothing else: no preamble, no explanation, no fences around the answer.";

/// Which prose slot. Each carries its prompt and its fact allowlist; the document layer owns the
/// section heading it lands under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Slot {
    ExecutiveSummary,
    WhatThisFunded,
    UsageProfile,
    Closing,
    Tradeoffs,
}

/// Every slot that is not conditional on a flag, in document order.
const UNCONDITIONAL: &[Slot] = &[
    Slot::ExecutiveSummary,
    Slot::WhatThisFunded,
    Slot::UsageProfile,
    Slot::Closing,
];

impl Slot {
    /// The key the document layer looks this slot's prose up by.
    pub(super) fn key(self) -> &'static str {
        match self {
            Slot::ExecutiveSummary => "executive-summary",
            Slot::WhatThisFunded => "what-this-funded",
            Slot::UsageProfile => "usage-profile",
            Slot::Closing => "closing",
            Slot::Tradeoffs => "tradeoffs",
        }
    }

    /// The instruction this slot is given. Compiled in, so a render needs no template on disk.
    fn prompt(self) -> &'static str {
        match self {
            Slot::ExecutiveSummary => include_str!("../../templates/slots/executive-summary.pmt"),
            Slot::WhatThisFunded => include_str!("../../templates/slots/what-this-funded.pmt"),
            Slot::UsageProfile => include_str!("../../templates/slots/usage-profile.pmt"),
            Slot::Closing => include_str!("../../templates/slots/closing.pmt"),
            Slot::Tradeoffs => include_str!("../../templates/slots/tradeoffs.pmt"),
        }
    }

    /// The ONLY fact keys this slot may cite.
    ///
    /// Per-slot rather than a single global namespace, and that is the sharpest thing the design
    /// review found: a key that resolves globally but belongs in a different sentence lets the model
    /// choose WHICH true number appears WHERE, which the digit check cannot see. Bounding each slot
    /// to a handful of keys bounds that blast radius to those keys. The brief and the validator read
    /// this same list, so they cannot drift.
    fn allowlist(self) -> &'static [&'static str] {
        match self {
            Slot::ExecutiveSummary => &[
                "period.days",
                "period.active-days",
                "totals.sessions",
                "totals.spend",
                "totals.repo-count",
                "repos.top",
                "efficiency.cache-read-share",
            ],
            Slot::WhatThisFunded => &[
                "totals.spend",
                "totals.repo-count",
                "repos.top",
                "repos.top-spend",
                "orgs.top",
                "agent-types.top",
            ],
            Slot::UsageProfile => &[
                "period.days",
                "period.active-days",
                "period.since",
                "period.until",
                "totals.sessions",
                "models.top",
            ],
            // The `late-period.*` keys carry what the retired `Forward-Looking` section was for, in
            // the only form a slot may receive it: computed figures, not session text. They are
            // ABSENT on a window under two weeks (see `facts::LATE_PERIOD_MIN_DAYS`), so this slot's
            // brief is short two lines there and the prose simply omits the trailing sentence.
            Slot::Closing => &[
                "period.days",
                "totals.sessions",
                "totals.spend",
                "repos.top",
                "late-period.days",
                "late-period.sessions",
                "late-period.spend",
                "late-period.active-days",
            ],
            Slot::Tradeoffs => &[
                "efficiency.cache-read-share",
                "efficiency.tool-error-rate",
                "efficiency.cache-1h-write-fraction",
                "efficiency.interrupts",
                "efficiency.compactions",
            ],
        }
    }
}

/// The brief one slot receives: which slot, and the facts it may cite with their display values.
///
/// This is the whole payload. NOT the context block: feeding that in would recreate the "940KB of
/// temptation" the guard existed to police, and it carries user-derived free text (session titles,
/// summaries, tags) a slot must never see.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
struct Brief<'a> {
    slot: &'static str,
    facts: std::collections::BTreeMap<&'a str, &'a str>,
}

/// Largest a slot brief may be. A brief is a slot name plus roughly seven short display strings;
/// this is orders of magnitude of headroom over that and still four orders below the context block
/// it replaces. Exceeding it means something leaked a collection in.
pub(super) const MAX_BRIEF_BYTES: usize = 4_096;

/// How many slots a render attempts. The eval reports degradation as filled-against-attempted, and
/// deriving the denominator here keeps it from drifting from [`generate`]'s own loop.
pub(super) fn count(include_tradeoffs: bool) -> usize {
    UNCONDITIONAL.len() + usize::from(include_tradeoffs)
}

/// Generate every slot for one render.
///
/// INFALLIBLE by construction: the return value is whatever prose survived validation. A slot that
/// failed twice is simply absent from the map, and the document layer renders its section as a
/// header with no body.
pub(super) fn generate<T: Transport>(
    transport: &T,
    reg: &FactRegistry,
    model: &str,
    ceiling: u32,
    include_tradeoffs: bool,
) -> SlotProse {
    debug!(
        "slots::generate: model={model} ceiling={ceiling} include_tradeoffs={include_tradeoffs} facts={}",
        reg.len()
    );
    let mut prose = SlotProse::new();
    let wanted = UNCONDITIONAL
        .iter()
        .copied()
        .chain(include_tradeoffs.then_some(Slot::Tradeoffs));
    for slot in wanted {
        match one(transport, slot, reg, model, ceiling) {
            Some(text) => {
                prose.insert(slot.key(), text);
            }
            None => warn!(
                "slots::generate: slot={} ships EMPTY after a retry; its section renders with no body",
                slot.key()
            ),
        }
    }
    info!(
        "slots::generate: filled={} of {} attempted",
        prose.len(),
        UNCONDITIONAL.len() + usize::from(include_tradeoffs)
    );
    prose
}

/// Generate ONE slot: call, validate, and on violation retry ONCE naming the violation. `None` when
/// the second attempt also failed, or when the transport itself errored.
///
/// The retry is a deliberate, scoped exemption from the no-retry transport doctrine
/// (`summarize/cli.rs:8-13`). That doctrine exists to stop a blind re-fire of a failed 940KB paid
/// render; a slot call is a KB-scale brief under a small ceiling, the retry names the specific
/// violation rather than hoping, and the failure mode is degradation rather than rescue.
fn one<T: Transport>(transport: &T, slot: Slot, reg: &FactRegistry, model: &str, ceiling: u32) -> Option<String> {
    let brief = brief(slot, reg);
    let job = Job {
        kind: Kind::Slot,
        model,
        max_output_tokens: ceiling,
    };
    let mut instruction = slot.prompt().to_string();
    for attempt in 1..=ATTEMPTS {
        debug!(
            "slots::one: slot={} attempt={attempt}/{ATTEMPTS} brief bytes={}",
            slot.key(),
            brief.len()
        );
        let raw = match transport.complete(job, SLOT_SYSTEM_PROMPT, &instruction, &brief) {
            Ok(raw) => raw,
            // A transport error is not a contract violation, and retrying it is exactly what the
            // no-retry doctrine forbids. Degrade immediately.
            Err(e) => {
                warn!("slots::one: slot={} transport failed: {e:#}", slot.key());
                return None;
            }
        };
        match validate(&raw, slot.allowlist()) {
            Ok(text) => {
                debug!(
                    "slots::one: slot={} accepted on attempt={attempt} bytes={}",
                    slot.key(),
                    text.len()
                );
                return Some(text);
            }
            Err(violation) => {
                warn!(
                    "slots::one: slot={} attempt={attempt} rejected: {violation} preview={:?}",
                    slot.key(),
                    preview(&raw)
                );
                instruction = retry_instruction(slot, &violation);
            }
        }
    }
    None
}

/// One call, then one retry. Not a knob: a second retry would be the blind re-fire the doctrine
/// forbids, and the degradation path is already correct.
const ATTEMPTS: u32 = 2;

/// How much of a rejected reply the WARN carries. Enough to diagnose what the model did, short
/// enough that a log line stays a log line.
const PREVIEW_CHARS: usize = 200;

fn preview(text: &str) -> String {
    text.chars().take(PREVIEW_CHARS).collect()
}

/// The retry instruction: the original prompt plus the named violation and the rule it broke.
///
/// Naming the specific violation is what separates this from a blind re-fire. A model told "your
/// output contained the digit 4; write that figure as a placeholder instead" has something to act
/// on; one told "try again" does not.
fn retry_instruction(slot: Slot, violation: &Violation) -> String {
    format!(
        "{}\n\nYOUR PREVIOUS ATTEMPT WAS REJECTED: {violation}\n\nWrite it again, honoring the \
         contract exactly. Every figure is a {{{{fact:key}}}} placeholder drawn verbatim from your \
         allowlist; the prose itself carries no numeric character at all.",
        slot.prompt()
    )
}

/// Build one slot's brief: its name and its allowlisted facts with their display values.
///
/// A key on the allowlist that the registry does not carry (an absent ratio, a missing prior
/// period) is simply omitted, so the model is never shown a fact it cannot cite -- and the
/// validator, reading the same allowlist, would reject it if the model cited it anyway.
fn brief(slot: Slot, reg: &FactRegistry) -> String {
    let facts = slot
        .allowlist()
        .iter()
        .filter_map(|key| reg.get(key).map(|value| (*key, value)))
        .collect();
    let brief = Brief {
        slot: slot.key(),
        facts,
    };
    let json = serde_json::to_string(&brief).unwrap_or_else(|e| {
        // A BTreeMap of &str cannot fail to serialize; if it somehow did, an empty brief degrades
        // this slot rather than failing the render.
        warn!("slots::brief: slot={} failed to serialize: {e}", slot.key());
        String::new()
    });
    debug_assert!(
        json.len() <= MAX_BRIEF_BYTES,
        "slot {} brief is {} bytes, over the {MAX_BRIEF_BYTES}-byte cap",
        slot.key(),
        json.len()
    );
    json
}

/// Why a slot reply was rejected. Carries enough to name the violation back to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Violation {
    /// The prose carried a numeric character outside a placeholder.
    Numeric(char),
    /// Stray `{{` or `}}` remained after allowlisted placeholders were removed: an unknown key, a
    /// malformed placeholder, or loose braces.
    StrayBraces,
    /// A placeholder named a key that is not on this slot's allowlist.
    OffAllowlist(String),
    /// A markdown node other than paragraph prose.
    Structure(String),
    /// An em dash, which rule 6 of the contract forbids.
    EmDash,
    /// Nothing usable came back.
    Empty,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Violation::Numeric(ch) => write!(
                f,
                "the prose contained the numeric character {ch:?}. Numbers may appear ONLY inside a \
                 {{{{fact:key}}}} placeholder"
            ),
            Violation::StrayBraces => write!(
                f,
                "the prose contained stray {{{{ or }}}} braces: a placeholder was malformed, or named \
                 a key outside the allowlist"
            ),
            Violation::OffAllowlist(key) => write!(
                f,
                "the prose cited the fact key {key:?}, which is NOT on this slot's allowlist. Use \
                 only the keys you were given"
            ),
            Violation::Structure(node) => write!(
                f,
                "the prose contained a {node} node. Output paragraph prose only: no headings, \
                 tables, lists, blockquotes, code, or raw HTML"
            ),
            Violation::EmDash => write!(
                f,
                "the prose contained an em dash. Use \"--\", a colon, parentheses, or split the \
                 sentence"
            ),
            Violation::Empty => write!(f, "the reply was empty"),
        }
    }
}

/// Validate one slot reply against its allowlist, returning the accepted prose.
///
/// Order matters and follows the design: strip the allowlisted placeholder spans, then hold the
/// REMAINDER to the digit and brace rules, then hold the whole reply to the structural rule.
fn validate(raw: &str, allow: &[&str]) -> Result<String, Violation> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(Violation::Empty);
    }

    let remainder = strip_allowlisted(text, allow)?;

    // (a) No numeric character ANYWHERE outside a placeholder. `\p{N}`, not `[0-9]`: a roman
    // numeral, a fraction glyph, and a superscript digit are all figures a reader reads as data.
    if let Some(ch) = remainder.chars().find(|c| c.is_numeric()) {
        return Err(Violation::Numeric(ch));
    }
    // (b) No stray braces: an unknown key, a malformed placeholder, or loose braces.
    if remainder.contains("{{") || remainder.contains("}}") {
        return Err(Violation::StrayBraces);
    }
    // (c) No em dash. Rule 6 of the contract called itself non-negotiable while nothing enforced
    // it, so a reply carrying one shipped unchecked -- and an em dash in a generated report is
    // exactly the house-style violation that is hardest to notice after the fact. Checked on the
    // REMAINDER for symmetry with the rules above; a registry value cannot carry one, since every
    // fact is a Rust-formatted figure.
    if remainder.contains('\u{2014}') {
        return Err(Violation::EmDash);
    }
    // (d) Paragraph prose only, per the parser marquee renders with.
    structure_ok(text)?;

    Ok(text.to_string())
}

/// Re-run the structural checks on prose that has ALREADY had its placeholders substituted.
///
/// Belt and braces, and deliberately so. The registry admits only Rust-formatted display strings, so
/// no interpolated value CAN carry a heading, a pipe, or a brace -- and this enforces it anyway,
/// because "the registry cannot contain structure" is an invariant maintained by code that could
/// change, while this is a check on the bytes about to be written.
pub(super) fn verify_interpolated(text: &str) -> Result<(), String> {
    if text.contains("{{") || text.contains("}}") {
        return Err(Violation::StrayBraces.to_string());
    }
    structure_ok(text).map_err(|v| v.to_string())
}

/// Remove every `{{fact:key}}` span whose key is on the allowlist, and fail on one whose key is not.
///
/// `split_once` throughout: slot prose is arbitrary UTF-8, and a byte-offset slice panics the moment
/// a multibyte character straddles the boundary.
fn strip_allowlisted(text: &str, allow: &[&str]) -> Result<String, Violation> {
    const OPEN: &str = "{{fact:";
    const CLOSE: &str = "}}";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let Some((before, after)) = rest.split_once(OPEN) else {
            out.push_str(rest);
            return Ok(out);
        };
        out.push_str(before);
        let Some((key, tail)) = after.split_once(CLOSE) else {
            // Unterminated: leave the braces in so the brace check sees them.
            out.push_str(OPEN);
            out.push_str(after);
            return Ok(out);
        };
        if !allow.contains(&key) {
            return Err(Violation::OffAllowlist(key.to_string()));
        }
        rest = tail;
    }
}

/// Assert the reply parses as paragraph prose and nothing else.
///
/// Parsed with comrak under the SAME extensions marquee's markdown lane enables, so validation and
/// eventual rendering share one grammar. This is why a leading `#` check is not enough: a setext
/// heading needs no `#`, a table needs only pipes and a dashed line, and a raw HTML block needs only
/// a tag at the start of a line. The node allowlist is deliberately strict -- anything not named
/// here is a violation, so a future comrak node type fails closed rather than sailing through.
fn structure_ok(text: &str) -> Result<(), Violation> {
    let arena = Arena::new();
    let mut options = Options::default();
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.tasklist = true;
    options.extension.autolink = true;
    let root = parse_document(&arena, text, &options);
    check_node(root)
}

/// Recursive half of [`structure_ok`].
fn check_node<'a>(node: &'a AstNode<'a>) -> Result<(), Violation> {
    let value = node.data.borrow().value.clone();
    let allowed = matches!(
        value,
        NodeValue::Document
            | NodeValue::Paragraph
            | NodeValue::Text(_)
            | NodeValue::SoftBreak
            | NodeValue::Emph
            | NodeValue::Strong
            | NodeValue::Strikethrough
            | NodeValue::Code(_)
            | NodeValue::Link(_)
            | NodeValue::Escaped
    );
    if !allowed {
        return Err(Violation::Structure(node_name(&value)));
    }
    for child in node.children() {
        check_node(child)?;
    }
    Ok(())
}

/// A short, reader-facing name for a rejected node, for the WARN and the retry prompt. The `Debug`
/// rendering of a comrak node carries its whole payload; a model told "a Heading node" acts on that
/// better than one shown a struct dump.
fn node_name(value: &NodeValue) -> String {
    let debug = format!("{value:?}");
    debug.split(['(', ' ', '{']).next().unwrap_or("unknown").to_lowercase()
}

#[cfg(test)]
mod tests;
