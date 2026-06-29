//! `report merge` — combine multiple per-host `collect` JSON reports into one.
//!
//! Two schema decisions drive this module (both flagged in design review):
//!
//! 1. **Keep-both session keying.** A `collect` report keys its sessions in a
//!    `BTreeMap<String, SessionEntry>` by raw session id. Two hosts that happen to share a
//!    session id would COLLIDE — one silently overwriting the other — if merged on the raw key.
//!    We re-key every merged session to `"<host>/<session_id>"` so a same-id-different-host pair
//!    both survive. The host comes from the per-input report's own `host` field (the same value
//!    `collect` records), so the prefix is authoritative.
//! 2. **Recomputed totals.** The merged `totals` are RE-SUMMED from the merged session set, never
//!    blind-summed from each input's `totals`. Blind-summing would double-count any session that
//!    appears in more than one input; re-summing the actual entries is correct by construction.

use crate::config::{MergeConfig, Output};
use crate::report::{ModelTokens, Report, SessionEntry, Totals};
use crate::{OutputDest, RunResult};
use chrono::{DateTime, Utc};
use eyre::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Typed errors for the merge module. `report` is a workspace library, so per repo convention it
/// uses `thiserror` with a matchable enum rather than `eyre` strings — consumers (and tests) can
/// discriminate `SchemaMismatch` from `NoInputs` without parsing a `Display` string. The CLI
/// boundary (`merge::run`) maps these into the crate's `eyre` flow via `#[from]`.
#[derive(Debug, Error)]
pub enum MergeError {
    /// No input reports were given; there is nothing to merge.
    #[error("report merge: no input files given; nothing to merge")]
    NoInputs,

    /// Two input reports disagree on `schema-version`; merging incompatible shapes is refused.
    /// Both versions are named so the operator can see exactly which inputs clashed.
    #[error(
        "report merge: schema-version mismatch: input reports disagree ({expected} vs {found}); refusing to merge incompatible report shapes"
    )]
    SchemaMismatch { expected: u32, found: u32 },

    /// Reading an input report from disk failed.
    #[error("failed to read report at {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Parsing an input report's JSON failed.
    #[error("failed to parse report at {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// Read, validate, and merge the configured input reports, then emit the merged report to the
/// configured destination (`-o <file>` or stdout). Returns the merged session count and where it
/// landed. The human "wrote N" note is the caller's (`report::run`) responsibility and goes to
/// stderr so a stdout JSON stream stays clean.
pub fn run(cfg: &MergeConfig) -> Result<RunResult> {
    log::debug!("merge::run: inputs={} output={:?}", cfg.inputs.len(), cfg.output);

    // The merge core returns the typed `MergeError`; map it into the crate's eyre flow at this CLI
    // boundary so the rest of the eyre-based crate is unaffected (`?` lifts it via `eyre::Report`'s
    // `From<E: std::error::Error>`).
    let reports = read_reports(&cfg.inputs)?;
    let merged = merge_reports(reports)?;
    let count = merged.totals.sessions;

    let json = serde_json::to_string_pretty(&merged).context("failed to serialize merged report to JSON")?;
    let dest = write_output(&cfg.output, &json)?;

    log::info!("merge::run: merged {} sessions to {}", count, dest);
    Ok(RunResult {
        sessions_emitted: count,
        output: dest,
    })
}

/// Read and deserialize every input path into a [`Report`]. Zero inputs is a typed error (nothing
/// to merge); a single input is allowed (the merge is then an identity passthrough).
fn read_reports(inputs: &[PathBuf]) -> std::result::Result<Vec<Report>, MergeError> {
    log::debug!("merge::read_reports: count={}", inputs.len());
    if inputs.is_empty() {
        return Err(MergeError::NoInputs);
    }
    let mut reports = Vec::with_capacity(inputs.len());
    for path in inputs {
        let body = fs::read_to_string(path).map_err(|source| MergeError::Read {
            path: path.clone(),
            source,
        })?;
        let report: Report = serde_json::from_str(&body).map_err(|source| MergeError::Parse {
            path: path.clone(),
            source,
        })?;
        reports.push(report);
    }
    Ok(reports)
}

/// Combine the deserialized reports into one. Asserts a uniform `schema-version`, re-keys sessions
/// to `"<host>/<session_id>"` (keep-both), recomputes totals from the merged set, and widens the
/// `since`/`until` window to the min/max across inputs. `host` becomes a multi-host marker.
///
/// **Single-input passthrough.** With exactly one input the merge is a TRUE identity: the input
/// report is returned UNCHANGED — original (bare, un-re-keyed) session keys, original `generated`
/// timestamp, original `host`/`since`/`until`/`totals`. So a 1-input merge round-trips byte-for-
/// byte. Re-keying, re-summing, and a fresh `generated` only make sense when actually combining
/// two or more reports (the keep-both collision handling exists for that case alone).
fn merge_reports(reports: Vec<Report>) -> std::result::Result<Report, MergeError> {
    log::debug!("merge::merge_reports: inputs={}", reports.len());

    // Single-input identity passthrough: hand back the lone input verbatim. `<[_; 1]>::try_into`
    // moves the single element out without any index/unwrap, and falls through (returning the Vec)
    // for any other length.
    let reports = match <[Report; 1]>::try_from(reports) {
        Ok([only]) => return Ok(only),
        Err(reports) => reports,
    };

    let schema_version = assert_uniform_schema(&reports)?;

    let mut sessions: BTreeMap<String, SessionEntry> = BTreeMap::new();
    let mut hosts: BTreeSet<String> = BTreeSet::new();
    let mut since: Option<DateTime<Utc>> = None;
    let mut until: Option<DateTime<Utc>> = None;

    for report in reports {
        hosts.insert(report.host.clone());
        since = Some(match since {
            Some(cur) => cur.min(report.since),
            None => report.since,
        });
        until = Some(match until {
            Some(cur) => cur.max(report.until),
            None => report.until,
        });
        for (sid, entry) in report.sessions {
            // Keep-both: re-key by host so same-id-different-host sessions both survive.
            let key = format!("{}/{}", report.host, sid);
            sessions.insert(key, entry);
        }
    }

    let totals = recompute_totals(&sessions);
    let host = multi_host_marker(&hosts);

    // since/until are always Some here: read_reports guarantees >= 1 input and the loop above sets
    // both on the first iteration.
    let since = since.unwrap_or_else(Utc::now);
    let until = until.unwrap_or_else(Utc::now);

    Ok(Report {
        schema_version,
        generated: Utc::now(),
        host,
        since,
        until,
        totals,
        sessions,
    })
}

/// All inputs must share the same `schema-version`; merging incompatible shapes is a typed error
/// naming both versions. Returns the common version on success.
fn assert_uniform_schema(reports: &[Report]) -> std::result::Result<u32, MergeError> {
    // `first()` avoids an index panic; `merge_reports` only reaches here with >= 2 inputs, but a
    // missing first element degenerates to "no common version" rather than panicking.
    let Some(first) = reports.first().map(|r| r.schema_version) else {
        return Err(MergeError::NoInputs);
    };
    for report in &reports[1..] {
        if report.schema_version != first {
            return Err(MergeError::SchemaMismatch {
                expected: first,
                found: report.schema_version,
            });
        }
    }
    Ok(first)
}

/// Recompute `totals` by RE-SUMMING the merged session set (never blind-summing each input's
/// `totals`, which double-counts overlap). Per-model token counts are summed; per-model and
/// session-level spend is summed from the entries' own priced `spend-usd` fields (no re-pricing —
/// each input was priced at collect time and we trust those figures).
fn recompute_totals(sessions: &BTreeMap<String, SessionEntry>) -> Totals {
    log::debug!("merge::recompute_totals: sessions={}", sessions.len());

    let mut models: BTreeMap<String, ModelTokens> = BTreeMap::new();
    let mut untracked: BTreeSet<String> = BTreeSet::new();
    let mut total_spend = 0.0_f64;

    for entry in sessions.values() {
        for name in &entry.untracked_models {
            untracked.insert(name.clone());
        }
        for (model, mt) in &entry.models {
            let acc = models.entry(model.clone()).or_default();
            acc.input += mt.input;
            acc.output += mt.output;
            acc.cache_5m_write += mt.cache_5m_write;
            acc.cache_1h_write += mt.cache_1h_write;
            acc.cache_read += mt.cache_read;
            acc.total += mt.total;
            if let Some(spend) = mt.spend_usd {
                let acc_spend = acc.spend_usd.get_or_insert(0.0);
                *acc_spend = round_cents(*acc_spend + spend);
                total_spend += spend;
            }
        }
    }

    Totals {
        sessions: sessions.len(),
        spend_usd: round_cents(total_spend),
        untracked_models: untracked.into_iter().collect(),
        models,
    }
}

/// Multi-host marker for the merged report's `host` field. A single distinct host (e.g. a 1-input
/// identity merge, or several reports from the same host) keeps that bare host name; multiple
/// distinct hosts are joined `a+b+c` so the provenance is visible in the output.
fn multi_host_marker(hosts: &BTreeSet<String>) -> String {
    hosts.iter().cloned().collect::<Vec<_>>().join("+")
}

fn round_cents(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Emit the merged JSON to the configured destination: `-o <file>` writes atomically (temp +
/// rename in the target dir), omitting `-o` streams the JSON to stdout so `... | jq` works.
fn write_output(output: &Output, json: &str) -> Result<OutputDest> {
    match output {
        Output::File(path) => {
            write_file_atomic(path, json)?;
            Ok(OutputDest::File(path.clone()))
        }
        Output::Stdout => {
            let mut out = std::io::stdout().lock();
            out.write_all(json.as_bytes())
                .context("failed to write merged report JSON to stdout")?;
            out.write_all(b"\n")
                .context("failed to write trailing newline to stdout")?;
            Ok(OutputDest::Stdout)
        }
    }
}

/// Write `json` to `path` atomically: a temp file in the target's own directory, flushed, then
/// renamed over the destination (matches [`crate::report::write_json`]'s durability contract).
fn write_file_atomic(path: &Path, json: &str) -> Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(dir).with_context(|| format!("failed to create output dir {}", dir.display()))?;

    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("failed to create temp file in {}", dir.display()))?;
    tmp.write_all(json.as_bytes())
        .context("failed to write merged JSON to temp file")?;
    tmp.flush().context("failed to flush temp file")?;
    tmp.persist(path)
        .with_context(|| format!("failed to atomically rename temp file to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests;
