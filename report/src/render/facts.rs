//! The fact registry: every figure a prose slot is allowed to reference, as a Rust-formatted
//! display string.
//!
//! This is deliberately an ENUMERATED registry, not a walk of the serialized context block. The
//! block carries bools, ints, dates, nulls, nested structs, and -- the reason that matters most --
//! user-derived free text (session titles, tags, summaries, notes) that a slot must never receive.
//! A walk would register all of it. So every key below is written out by hand, paired with the
//! display string the document layer prints for the same value, which is what makes the two
//! CANNOT-diverge claim structural rather than aspirational: one source, two consumers.
//!
//! Only display strings are registrable. There is no `insert_int`, no `insert_bool`, and no way to
//! hand this type a raw operand -- if a caller wants a number in here it has to format it first,
//! through the same `fmt::` helpers the tables use.

use std::collections::BTreeMap;

use log::{debug, trace};

use super::ContextBlock;

/// Longest a single registered display string may be. A fact is a figure (`"$9,450.31"`,
/// `"96.0%"`, `"tatari-tv/philo"`), never a sentence: `basis.note` and the reconciliation scope
/// note are prose the DOCUMENT layer prints verbatim, and registering them would hand a slot a
/// paragraph to paraphrase. Comfortably above the longest real figure (a repo slug plus an org
/// prefix) and far below any of those sentences.
const MAX_FACT_BYTES: usize = 96;

/// Facts a slot may cite, keyed by dotted-kebab name.
///
/// `BTreeMap` rather than `HashMap` because the brief a slot receives is BUILT from this map and
/// two renders of the same report must produce byte-identical briefs (and therefore byte-identical
/// artifacts). A `HashMap`'s iteration order would reorder the allowlist between runs.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct FactRegistry {
    facts: BTreeMap<String, String>,
}

impl FactRegistry {
    /// Look up one fact. `None` for a key that was never registered, which is what makes an
    /// unknown `{{fact:...}}` placeholder a validation failure rather than silent empty text.
    pub(super) fn get(&self, key: &str) -> Option<&str> {
        self.facts.get(key).map(String::as_str)
    }

    pub(super) fn len(&self) -> usize {
        self.facts.len()
    }

    /// Every registered display string. Test-only: the document layer's licensing assertion needs
    /// the value set, and production code has no business enumerating it.
    #[cfg(test)]
    pub(super) fn values(&self) -> impl Iterator<Item = &str> {
        self.facts.values().map(String::as_str)
    }

    /// Register one display string under `key`.
    ///
    /// A duplicate key is a PROGRAMMING error, not a data condition: the keys are enumerated in
    /// this file, so two registrations of one key means two call sites disagree about what that key
    /// means. `debug_assert!` makes it a test failure (tests build in debug) while keeping a
    /// release build from panicking mid-render; the release path keeps the first value and shouts.
    fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        let value = value.into();
        trace!("FactRegistry::insert: key={key} value={value}");
        debug_assert!(
            value.len() <= MAX_FACT_BYTES,
            "fact {key} is {} bytes, over the {MAX_FACT_BYTES}-byte display-string limit: {value}",
            value.len()
        );
        if let Some(prior) = self.facts.insert(key.clone(), value.clone()) {
            debug_assert!(false, "duplicate fact key {key}: {prior:?} then {value:?}");
            log::error!("FactRegistry::insert: duplicate key={key} kept={prior:?} dropped={value:?}");
            self.facts.insert(key, prior);
        }
    }

    /// Register a display string that the view carries as `Option`, skipping `None`. An absent
    /// figure must be an ABSENT key (so a slot citing it fails validation), never an empty string
    /// or a literal "n/a" a slot could print as if it were a measurement.
    fn insert_opt(&mut self, key: impl Into<String>, value: Option<&str>) {
        if let Some(value) = value {
            self.insert(key, value);
        }
    }

    /// Register a display string unless it is the view layer's "not measured" sentinel. The
    /// efficiency view formats an absent ratio as the literal `"n/a"` (it is a required field in
    /// the context block), and `"n/a"` is not a figure a slot may cite.
    fn insert_measured(&mut self, key: impl Into<String>, value: &str) {
        if value != NOT_MEASURED {
            self.insert(key, value);
        }
    }
}

/// What the efficiency view prints (via `workload::fmt_ratio_pct`) for a ratio the window never
/// measured. A required context-block field cannot simply be absent, so it carries this sentinel --
/// and a sentinel is not a figure a slot may cite.
const NOT_MEASURED: &str = "n/a";

/// Build the registry from the same view structs the document layer renders.
///
/// Every key is deliberate. When adding one, ask whether a slot citing it in the WRONG sentence
/// would still be true -- per-slot allowlists bound that blast radius, but a key nothing needs is
/// a key that only widens it.
pub(super) fn build(block: &ContextBlock<'_>) -> FactRegistry {
    let mut reg = FactRegistry::default();

    reg.insert("period.since", &block.period.since);
    reg.insert("period.until", &block.period.until);
    reg.insert("period.days", block.period.days.to_string());
    reg.insert("period.active-days", block.period.active_days.to_string());

    reg.insert("totals.sessions", crate::fmt::format_int(block.totals.sessions as u64));
    reg.insert("totals.repo-count", block.totals.repo_count.to_string());
    reg.insert("totals.spend", &block.totals.spend);
    reg.insert("totals.tokens-human", &block.totals.tokens_human);

    let cache = &block.aggregates.cache;
    reg.insert_measured("cache.cache-read-share", &cache.cache_read_share);
    reg.insert("cache.input-tokens-human", &cache.input_tokens_human);
    reg.insert("cache.cache-read-tokens-human", &cache.cache_read_tokens_human);
    reg.insert_opt("cache.list-price-equivalent", cache.list_price_equivalent.as_deref());
    reg.insert_opt("cache.cache-savings", cache.cache_savings.as_deref());

    let eff = &block.efficiency;
    reg.insert_measured("efficiency.cache-read-share", &eff.cache_read_share);
    reg.insert_measured("efficiency.tool-error-rate", &eff.tool_error_rate);
    reg.insert_measured("efficiency.cache-1h-write-fraction", &eff.cache_1h_write_fraction);
    reg.insert("efficiency.interrupts", crate::fmt::format_int(eff.interrupts));
    reg.insert("efficiency.compactions", crate::fmt::format_int(eff.compactions));

    let carried = &block.aggregates.carried_in;
    reg.insert("carried-in.sessions", carried.sessions.to_string());
    reg.insert("carried-in.tokens-human", &carried.tokens_human);
    reg.insert("carried-in.spend", &carried.spend);

    let unit = &block.unit_costs;
    reg.insert_opt("unit-costs.per-commit", unit.per_commit.as_deref());
    reg.insert_opt("unit-costs.per-pr", unit.per_pr.as_deref());
    reg.insert_opt("unit-costs.per-active-day", unit.per_active_day.as_deref());
    reg.insert_opt("unit-costs.per-session", unit.per_session.as_deref());
    reg.insert_opt("unit-costs.session-spend-p50", unit.session_spend_p50.as_deref());
    reg.insert_opt("unit-costs.session-spend-p90", unit.session_spend_p90.as_deref());

    reg.insert("attribution.covered", &block.attribution.covered);
    reg.insert("attribution.uncovered", &block.attribution.uncovered);
    reg.insert("attribution.covered-share", &block.attribution.covered_share);

    if let Some(outcomes) = &block.outcomes {
        let t = &outcomes.totals;
        let mut count = |key: &str, v: Option<u64>| {
            if let Some(v) = v {
                reg.insert(format!("outcomes.{key}"), crate::fmt::format_int(v));
            }
        };
        count("sessions-with-commits", t.sessions_with_commits);
        count("commits", t.commits);
        count("prs-opened", t.prs_opened);
        count("confluence-writes", t.confluence_writes);
        count("jira-writes", t.jira_writes);
        count("slack-messages", t.slack_messages);
        count("files-edited", t.files_edited);
        count("lines-written", t.lines_written);
        count("lines-replaced", t.lines_replaced);
    }

    if let Some(recon) = &block.reconciliation {
        reg.insert("reconciliation.billed", &recon.billed);
        reg.insert("reconciliation.modeled", &recon.modeled);
        reg.insert("reconciliation.unseen-account-spend", &recon.delta);
        reg.insert("reconciliation.operator", &recon.operator);
        reg.insert("reconciliation.window", &recon.window);
    }

    for row in &block.aggregates.by_repo {
        let id = slug(&row.repo);
        reg.insert(format!("by-repo.{id}.spend"), &row.spend);
        reg.insert(format!("by-repo.{id}.sessions"), row.sessions.to_string());
        reg.insert(format!("by-repo.{id}.tokens-human"), &row.tokens_human);
    }
    for row in &block.aggregates.by_org {
        let id = slug(&row.org);
        reg.insert(format!("by-org.{id}.spend"), &row.spend);
        reg.insert(format!("by-org.{id}.sessions"), row.sessions.to_string());
        reg.insert(format!("by-org.{id}.repos"), row.repos.to_string());
    }
    for row in &block.totals.models {
        let id = slug(&row.model);
        reg.insert(format!("by-model.{id}.spend"), &row.spend);
        reg.insert(format!("by-model.{id}.tokens-human"), &row.tokens_human);
        reg.insert(format!("by-model.{id}.sessions-using"), row.sessions_using.to_string());
    }
    for row in &block.efficiency.agent_type_costs {
        let id = slug(&row.name);
        reg.insert(format!("by-agent-type.{id}.spend"), &row.spend);
        reg.insert(format!("by-agent-type.{id}.tokens-human"), &row.tokens_human);
    }

    // Curated derived keys. A slot writing "most of it in X" needs the NAME of the top row, and
    // making it derive that from an array would mean handing it the array. These are the only
    // derived facts, and each one is the first element of a list the binary already pre-sorted.
    if let Some(top) = block.aggregates.by_repo.first() {
        reg.insert("repos.top", &top.repo);
        reg.insert("repos.top-spend", &top.spend);
    }
    if let Some(top) = block.aggregates.by_org.first() {
        reg.insert("orgs.top", &top.org);
    }
    if let Some(top) = block.totals.models.first() {
        reg.insert("models.top", &top.model);
        reg.insert("models.top-spend", &top.spend);
    }
    if let Some(top) = block.efficiency.agent_type_costs.first() {
        reg.insert("agent-types.top", &top.name);
        reg.insert("agent-types.top-spend", &top.spend);
    }

    if let Some(prior) = &block.prior {
        reg.insert("prior.since", &prior.since);
        reg.insert("prior.until", &prior.until);
        reg.insert("prior.days", prior.days.to_string());
        reg.insert("prior.spend", &prior.totals.spend);
        reg.insert("prior.sessions", crate::fmt::format_int(prior.totals.sessions as u64));
        reg.insert("prior.tokens-human", &prior.totals.tokens_human);
    }

    debug!("facts::build: registered={}", reg.len());
    reg
}

/// Fold an arbitrary display name (a repo slug, a model name, an agent type) into a key segment.
///
/// The design names the `/` -> `-` case (`tatari-tv/philo` -> `tatari-tv-philo`); this generalizes
/// it, because the same segment position also carries model names and agent types like
/// `(main-session)`. Every character outside `[a-z0-9]` becomes `-`, runs collapse, and the ends
/// are trimmed -- so a key is always matchable by the `[a-z0-9.-]+` pattern the placeholder
/// grammar accepts. Two names that fold to the same segment collide, and a collision is a
/// `debug_assert` failure in [`FactRegistry::insert`] rather than a silently overwritten fact.
fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.extend(ch.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests;
