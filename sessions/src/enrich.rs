//! Enrichment sweep (Phase 2): fill `tags` and `summary` for dormant sessions via a cheap LLM
//! pass — the first clyde path that ships session content off-machine.
//!
//! The order is the gate: classify scope, **skip personal before any payload is built** (the
//! routing invariant — no personal content reaches the work account), parse the high-signal body
//! (live or from the staged copy), scrub secrets, then send. Failures are recorded with a bounded
//! `attempts` count so a bad session retries later but never forever. `--dry-run` walks the exact
//! same gate but stops before the send, reporting decisions and metrics.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use eyre::{Result, bail};
use log::{debug, info, warn};

use crate::db::{Db, EnrichSuccess};
use crate::llm::{Completer, ENRICH_MODEL, ENRICH_PROMPT_VERSION};
use crate::model::{EnrichDetail, EnrichStats, SessionRecord};
use crate::transcript::transcript_layout;

/// Default cap on per-session enrichment attempts before the selection predicate stops retrying.
pub const DEFAULT_MAX_ATTEMPTS: i64 = 5;
/// Cap on payload chars sent to the model. The Phase-1 body parse already bounds the body at 500K;
/// this is the send-side guard, head+tail when a body somehow exceeds it.
const SEND_CAP_CHARS: usize = 500_000;

/// How an enrichment pass is scoped and gated.
pub struct EnrichOptions {
    /// Only sessions idle at/before this instant (dormancy). `None` = no dormancy filter.
    pub dormant_before: Option<DateTime<Utc>>,
    /// Re-enrich every eligible session (vocabulary refresh); overrides manual-tag preservation.
    pub all: bool,
    /// Enrich exactly this (already-resolved) session id, bypassing the eligibility predicate.
    pub only: Option<String>,
    /// Walk the gate but do not send; report decisions and metrics.
    pub dry_run: bool,
    /// Dry-run only: write each redacted payload under this dir for the operator to inspect.
    pub show_payload: Option<PathBuf>,
    /// Per-session attempt cap (bounded retry).
    pub max_attempts: i64,
    /// Halt the sweep once cumulative tokens (in+out) reach this budget. `None` = unbounded.
    pub token_budget: Option<u64>,
}

impl Default for EnrichOptions {
    fn default() -> Self {
        Self {
            dormant_before: None,
            all: false,
            only: None,
            dry_run: false,
            show_payload: None,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            token_budget: None,
        }
    }
}

/// Run an enrichment pass. Generic over the [`Completer`] so tests inject a deterministic fake;
/// `completer` may be `None` only for a dry-run (no send happens). Returns the sweep tally.
pub fn enrich<C: Completer>(db: &Db, completer: Option<&C>, opts: &EnrichOptions) -> Result<EnrichStats> {
    debug!(
        "enrich::enrich: dormant_before={:?} all={} only={:?} dry_run={} max_attempts={} token_budget={:?}",
        opts.dormant_before, opts.all, opts.only, opts.dry_run, opts.max_attempts, opts.token_budget
    );
    if !opts.dry_run && completer.is_none() {
        bail!("enrich: a Completer is required for a live (non-dry-run) pass");
    }
    let now = Utc::now();

    let records = match &opts.only {
        Some(id) => match db.get(id)? {
            Some(rec) => vec![rec],
            None => {
                warn!("enrich::enrich: session {id} not found");
                Vec::new()
            }
        },
        None => db.enrich_candidates(opts.dormant_before, ENRICH_PROMPT_VERSION, opts.max_attempts, opts.all)?,
    };
    let force = opts.all || opts.only.is_some();

    let mut stats = EnrichStats {
        considered: records.len(),
        dry_run: opts.dry_run,
        ..Default::default()
    };

    for rec in &records {
        let scope = session::classify(rec.cwd.as_deref().map(std::path::Path::new));

        // --- Routing gate: personal content never leaves the machine. ---
        if !scope.is_work() {
            db.record_enrich_skip(&rec.session_id, scope.as_str(), "skipped-personal")?;
            stats.skipped_personal += 1;
            stats
                .details
                .push(detail(rec, scope.as_str(), false, None, None, "skipped-personal"));
            continue;
        }

        // --- Parse the high-signal body (live or from the staged copy). ---
        let Some((parent, subagents_dir)) = transcript_layout(rec) else {
            warn!(
                "enrich::enrich: {} archived with no staged copy; skipping",
                rec.session_id
            );
            db.record_enrich_skip(&rec.session_id, scope.as_str(), "skipped-empty")?;
            stats.skipped_empty += 1;
            stats
                .details
                .push(detail(rec, scope.as_str(), false, None, None, "skipped-empty"));
            continue;
        };
        let body = session::parse::parse_one(&rec.session_id, &parent, &subagents_dir).map(|p| p.body);
        let body = match body {
            Some(b) if !b.trim().is_empty() => b,
            _ => {
                db.record_enrich_skip(&rec.session_id, scope.as_str(), "skipped-empty")?;
                stats.skipped_empty += 1;
                stats
                    .details
                    .push(detail(rec, scope.as_str(), false, None, None, "skipped-empty"));
                continue;
            }
        };

        // --- Cap, then scrub secrets — the chokepoint every off-machine payload passes. ---
        let (capped, truncated) = head_tail(&body, SEND_CAP_CHARS);
        if truncated {
            warn!(
                "enrich::enrich: {} body exceeded {SEND_CAP_CHARS} chars; sent head+tail",
                rec.session_id
            );
        }
        let (redacted, redactions) = session::redact::scrub(&capped);
        let payload_bytes = redacted.len();
        stats.redactions += redactions;

        // --- Dry-run: report the decision, never send. ---
        if opts.dry_run {
            stats.would_enrich += 1;
            stats.details.push(detail(
                rec,
                scope.as_str(),
                true,
                Some(redactions),
                Some(payload_bytes),
                "would-enrich",
            ));
            if let Some(dir) = &opts.show_payload {
                write_payload_dump(dir, &rec.session_id, &redacted)?;
            }
            continue;
        }

        // --- Budget guard: halt before a send that would blow the per-run token budget. ---
        if let Some(budget) = opts.token_budget
            && stats.tokens_in + stats.tokens_out >= budget
        {
            warn!(
                "enrich::enrich: token budget {budget} reached ({} in + {} out); halting sweep early",
                stats.tokens_in, stats.tokens_out
            );
            break;
        }

        // --- Send. completer is Some here (checked above; dry-run already returned). ---
        let Some(completer) = completer else {
            bail!("enrich: live pass with no Completer (unreachable)");
        };
        match completer.enrich(&redacted) {
            Ok(out) => {
                // Preserve tags only when they are manually owned and not force-overridden;
                // enrichment-owned or absent tags are refreshed.
                let overwrite_tags = force || rec.tags.is_empty() || !db.tags_are_manual(&rec.session_id)?;
                let success = EnrichSuccess {
                    summary: &out.summary,
                    tags: overwrite_tags.then_some(out.tags.as_slice()),
                    scope: scope.as_str(),
                    enriched_modified: rec.modified,
                    enrich_model: ENRICH_MODEL,
                    prompt_version: ENRICH_PROMPT_VERSION,
                    redaction_count: redactions,
                    tokens_in: out.tokens_in,
                    tokens_out: out.tokens_out,
                };
                db.set_enrichment(&rec.session_id, &success, now)?;
                stats.enriched += 1;
                stats.tokens_in += out.tokens_in;
                stats.tokens_out += out.tokens_out;
                stats.details.push(detail(
                    rec,
                    scope.as_str(),
                    true,
                    Some(redactions),
                    Some(payload_bytes),
                    "ok",
                ));
            }
            Err(e) => {
                db.record_enrich_failure(&rec.session_id, scope.as_str(), &e.to_string())?;
                stats.failed += 1;
                stats.details.push(detail(
                    rec,
                    scope.as_str(),
                    true,
                    Some(redactions),
                    Some(payload_bytes),
                    "failed",
                ));
            }
        }
    }

    info!(
        "enrich::enrich: considered={} enriched={} skipped_personal={} skipped_empty={} failed={} would_enrich={} redactions={} tokens_in={} tokens_out={} dry_run={}",
        stats.considered,
        stats.enriched,
        stats.skipped_personal,
        stats.skipped_empty,
        stats.failed,
        stats.would_enrich,
        stats.redactions,
        stats.tokens_in,
        stats.tokens_out,
        stats.dry_run,
    );
    Ok(stats)
}

/// Truncate `s` to `cap` chars by keeping a head and a tail (char-safe), flagging when truncation
/// happened. Below the cap the text is returned unchanged.
fn head_tail(s: &str, cap: usize) -> (String, bool) {
    if s.chars().count() <= cap {
        return (s.to_string(), false);
    }
    let head_n = cap / 2;
    let tail_n = cap - head_n;
    let head: String = s.chars().take(head_n).collect();
    let tail_rev: Vec<char> = s.chars().rev().take(tail_n).collect();
    let tail: String = tail_rev.into_iter().rev().collect();
    (format!("{head}\n...[truncated]...\n{tail}"), true)
}

/// Write a redacted payload dump for `--dry-run --show-payload` (operator opt-in only).
fn write_payload_dump(dir: &std::path::Path, session_id: &str, redacted: &str) -> Result<()> {
    debug!(
        "enrich::write_payload_dump: dir={} session_id={session_id}",
        dir.display()
    );
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(format!("{session_id}.txt")), redacted)?;
    Ok(())
}

fn detail(
    rec: &SessionRecord,
    scope: &str,
    would_send: bool,
    redaction_count: Option<usize>,
    payload_bytes: Option<usize>,
    status: &str,
) -> EnrichDetail {
    EnrichDetail {
        session_id: rec.session_id.clone(),
        scope: scope.to_string(),
        would_send,
        redaction_count,
        payload_bytes,
        status: status.to_string(),
    }
}

#[cfg(test)]
mod tests;
