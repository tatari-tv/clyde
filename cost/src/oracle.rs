#![allow(clippy::unwrap_used)]

//! Independent reconciliation oracle (Phase 4).
//!
//! A from-scratch recompute of a fixture's cost that shares NO code with clyde's production
//! scanner ([`crate::scanner::find_session_files`]) or parser (`claude_pricing::parse_jsonl_file`).
//! It walks the tree itself, parses JSONL with its own minimal serde structs, prices with
//! hand-transcribed rates, and reimplements the documented dedup contract independently. A match
//! with clyde is therefore genuine agreement between two implementations, not the same code
//! checked against itself.
//!
//! Reconciliation reports deltas at three separately-attributed levels so an omission is caught
//! *as what it is*, not blurred into a single arithmetic mismatch:
//!   - **file** — the discovered session-JSONL set (oracle's recursive walk vs the scanner). A
//!     file only one side found is a discovery omission.
//!   - **parse** — with file sets equal, the count of counted entries. A difference means a line
//!     survived one pipeline's parse/count but not the other's: a parse-drop, not a lost file.
//!   - **aggregation** — final per-session / total cost. A difference with files AND entries equal
//!     is a pure math divergence.
//!
//! The only clyde primitive the oracle deliberately shares is [`crate::dates::local_date`] — the
//! UTC-to-local calendar bucketing. That is neither discovery, parse, nor pricing; sharing it keeps
//! the day-window semantics identical so an equality assertion is not silently timezone-fragile.
//!
//! Deliberate choice (Phase 4): the injected-omission test uses a FILE-level omission — a session
//! JSONL placed where the scanner structurally will not look. It is unambiguous, needs no
//! "deliberately-wrong" oracle parser, and cleanly demonstrates the property that matters: a
//! pure-arithmetic recheck (which trusts clyde's own extracted entries) reproduces clyde's total
//! and is blind to the omission, while the level-separated oracle flags it at the file level.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use tempfile::TempDir;

use super::*;

// --- Independent JSONL model (NOT claude_pricing's parse structs) ---

#[derive(Deserialize)]
struct OracleRaw {
    #[serde(rename = "type")]
    entry_type: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    message: Option<OracleMsg>,
}

#[derive(Deserialize)]
struct OracleMsg {
    id: Option<String>,
    model: Option<String>,
    usage: Option<OracleUsage>,
}

#[derive(Deserialize)]
struct OracleUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

/// One counted entry, priced independently.
struct OracleEntry {
    session_id: String,
    timestamp: DateTime<Utc>,
    message_id: Option<String>,
    request_id: Option<String>,
    cost: f64,
}

/// Hand-transcribed per-Mtok `(input, output)` rates, copied by hand from
/// `pricing/data/pricing.json`. Independent of `claude_pricing::calculate_usd` so the oracle does
/// not reuse clyde's cost math. The Phase 4 fixtures carry no cache tokens, so input+output rates
/// fully determine cost: `cost = input*in/1e6 + output*out/1e6`. An unknown model returns `None`
/// and is skipped — matching clyde's unknown-model skip.
fn oracle_rate(model: &str) -> Option<(f64, f64)> {
    match model {
        "claude-opus-4-7" => Some((5.0, 25.0)),
        "claude-sonnet-4-6" => Some((3.0, 15.0)),
        "claude-haiku-4-5" => Some((1.0, 5.0)),
        _ => None,
    }
}

/// Recursively collect every non-empty `*.jsonl` under `dir`. Unlike the scanner, this encodes NO
/// structural assumption about where session files live (top-level vs `subagents/`), so a file the
/// scanner would structurally skip is still discovered here.
fn oracle_discover(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            oracle_discover(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl")
            && std::fs::metadata(&path).map(|m| m.len() > 0).unwrap_or(false)
        {
            out.push(path);
        }
    }
}

fn oracle_files(projects_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    oracle_discover(projects_dir, &mut files);
    files.sort();
    files
}

/// Parse one file into priced counted entries, applying the documented gates independently:
/// assistant-only, all required fields present, `<synthetic>` skipped, unknown model skipped.
fn oracle_parse_file(path: &Path) -> Vec<OracleEntry> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let raw: OracleRaw = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if raw.entry_type.as_deref() != Some("assistant") {
            continue;
        }
        let session_id = match raw.session_id {
            Some(s) => s,
            None => continue,
        };
        let timestamp = match raw.timestamp.and_then(|t| t.parse::<DateTime<Utc>>().ok()) {
            Some(t) => t,
            None => continue,
        };
        let message = match raw.message {
            Some(m) => m,
            None => continue,
        };
        let model = match message.model {
            Some(m) => m,
            None => continue,
        };
        let usage = match message.usage {
            Some(u) => u,
            None => continue,
        };
        if model == "<synthetic>" {
            continue;
        }
        let (in_rate, out_rate) = match oracle_rate(&model) {
            Some(r) => r,
            None => continue,
        };
        let input = usage.input_tokens.unwrap_or(0) as f64;
        let output = usage.output_tokens.unwrap_or(0) as f64;
        let cost = input * in_rate / 1e6 + output * out_rate / 1e6;
        out.push(OracleEntry {
            session_id,
            timestamp,
            message_id: message.id,
            request_id: raw.request_id,
            cost,
        });
    }
    out
}

/// The counted-entry contract's tie-break, reimplemented independently: `true` when `cand` should
/// replace `existing`. Higher cost wins; equal cost -> lexicographically lower session_id; equal
/// both -> earlier timestamp.
fn oracle_candidate_wins(existing: &OracleEntry, cand: &OracleEntry) -> bool {
    use std::cmp::Ordering;
    match cand.cost.total_cmp(&existing.cost) {
        Ordering::Greater => true,
        Ordering::Less => false,
        Ordering::Equal => match cand.session_id.cmp(&existing.session_id) {
            Ordering::Less => true,
            Ordering::Greater => false,
            Ordering::Equal => cand.timestamp < existing.timestamp,
        },
    }
}

struct OracleResult {
    files: Vec<PathBuf>,
    counted_entries: usize,
    total_cost: f64,
    per_session: BTreeMap<String, (f64, usize)>,
}

/// Independent recompute: discover -> parse -> window-filter -> dedup -> aggregate.
fn run_oracle(projects_dir: &Path, start: NaiveDate, end: NaiveDate) -> OracleResult {
    let files = oracle_files(projects_dir);

    let mut all: Vec<OracleEntry> = Vec::new();
    for f in &files {
        all.extend(oracle_parse_file(f));
    }

    // Window is enforced by entry timestamp (the counted-entry contract), not file mtime.
    all.retain(|e| {
        let d = crate::dates::local_date(&e.timestamp);
        d >= start && d <= end
    });

    // Dedup: entries with a message_id collapse by (message.id, requestId); entries without one
    // bypass dedup and count as-is.
    let mut deduped: HashMap<(String, Option<String>), OracleEntry> = HashMap::new();
    let mut no_mid: Vec<OracleEntry> = Vec::new();
    for e in all {
        match &e.message_id {
            Some(mid) => {
                let key = (mid.clone(), e.request_id.clone());
                let replace = match deduped.get(&key) {
                    Some(existing) => oracle_candidate_wins(existing, &e),
                    None => true,
                };
                if replace {
                    deduped.insert(key, e);
                }
            }
            None => no_mid.push(e),
        }
    }

    let mut per_session: BTreeMap<String, (f64, usize)> = BTreeMap::new();
    let mut total_cost = 0.0;
    let mut counted_entries = 0usize;
    for e in deduped.values().chain(no_mid.iter()) {
        let ent = per_session.entry(e.session_id.clone()).or_insert((0.0, 0));
        ent.0 += e.cost;
        ent.1 += 1;
        total_cost += e.cost;
        counted_entries += 1;
    }

    OracleResult {
        files,
        counted_entries,
        total_cost,
        per_session,
    }
}

struct ClydeResult {
    files: Vec<PathBuf>,
    counted_entries: usize,
    total_cost: f64,
    per_session: BTreeMap<String, (f64, usize)>,
}

/// Drive clyde's real pipeline over the same fixture: its scanner for the file set, and
/// `compute_summaries` for the aggregation.
fn run_clyde(projects_dir: &Path, start: NaiveDate, end: NaiveDate) -> ClydeResult {
    let files: Vec<PathBuf> = crate::scanner::find_session_files(projects_dir)
        .unwrap()
        .into_iter()
        .map(|f| f.path)
        .collect();

    let args = CostArgs {
        config: None,
        path: Some(projects_dir.to_path_buf()),
        model: None,
        no_cache: true,
        offline: false,
        command: None,
    };
    let config = Config::default();
    let pricing = Pricing::embedded();
    let (_, sessions) = compute_summaries(&args, &config, &pricing, start, end, false, None).unwrap();

    let mut per_session = BTreeMap::new();
    let mut total_cost = 0.0;
    let mut counted_entries = 0usize;
    for s in &sessions {
        per_session.insert(s.session_id.clone(), (s.cost, s.entries));
        total_cost += s.cost;
        counted_entries += s.entries;
    }

    ClydeResult {
        files,
        counted_entries,
        total_cost,
        per_session,
    }
}

/// A "pure-arithmetic recheck": re-derive the total by re-pricing the entries clyde's OWN scanner
/// and parser extract. Because it trusts clyde's extracted set, it reproduces clyde's reported
/// total exactly and is BLIND to a file the scanner never discovered — the failure mode the
/// level-separated oracle exists to catch.
fn pure_arithmetic_recheck(projects_dir: &Path, start: NaiveDate, end: NaiveDate) -> f64 {
    let files = crate::scanner::find_session_files(projects_dir).unwrap();
    let filtered = crate::scanner::filter_by_date_range(&files, start, end);
    let pricing = Pricing::embedded();

    let mut deduped: HashMap<(String, Option<String>), f64> = HashMap::new();
    let mut no_mid = 0.0;
    for f in &filtered {
        let parsed = match claude_pricing::parse_jsonl_file(&f.path) {
            Ok(p) => p,
            Err(_) => continue,
        };
        for e in parsed.entries {
            if e.model == "<synthetic>" {
                continue;
            }
            let d = crate::dates::local_date(&e.timestamp);
            if d < start || d > end {
                continue;
            }
            let cost = match pricing.calculate_usd(&e.model, &e.usage) {
                Ok(c) => c,
                Err(_) => continue,
            };
            match &e.message_id {
                Some(mid) => {
                    let entry = deduped.entry((mid.clone(), e.request_id.clone())).or_insert(cost);
                    if cost > *entry {
                        *entry = cost;
                    }
                }
                None => no_mid += cost,
            }
        }
    }
    deduped.values().sum::<f64>() + no_mid
}

/// A hand-authored expected manifest for a frozen fixture. The numbers are computed by hand (see
/// each test's arithmetic comments), NOT read from clyde — the whole point of an independent
/// oracle is that its ground truth does not originate in the code under test.
struct Manifest {
    files: usize,
    counted_entries: usize,
    total_cost: f64,
}

struct Delta {
    file: Option<String>,
    parse: Option<String>,
    aggregation: Option<String>,
}

impl Delta {
    fn is_clean(&self) -> bool {
        self.file.is_none() && self.parse.is_none() && self.aggregation.is_none()
    }
}

/// Reconcile the independent oracle against clyde at the three separately-attributed levels.
fn reconcile(oracle: &OracleResult, clyde: &ClydeResult) -> Delta {
    let oracle_set: BTreeSet<_> = oracle.files.iter().cloned().collect();
    let clyde_set: BTreeSet<_> = clyde.files.iter().cloned().collect();
    let only_oracle: Vec<_> = oracle_set.difference(&clyde_set).cloned().collect();
    let only_clyde: Vec<_> = clyde_set.difference(&oracle_set).cloned().collect();

    let file = if only_oracle.is_empty() && only_clyde.is_empty() {
        None
    } else {
        Some(format!(
            "file discovery diverges: only_oracle={only_oracle:?} only_clyde={only_clyde:?}"
        ))
    };

    // Parse level is meaningful only when both discovered the SAME files; otherwise an entry-count
    // difference is explained by the missing file(s), not a parse-drop.
    let parse = if file.is_none() && oracle.counted_entries != clyde.counted_entries {
        Some(format!(
            "counted-entry count diverges with identical file sets: oracle={} clyde={}",
            oracle.counted_entries, clyde.counted_entries
        ))
    } else {
        None
    };

    // Aggregation: final cost, at the cent, both in total and per session.
    let mut agg: Vec<String> = Vec::new();
    if (oracle.total_cost - clyde.total_cost).abs() >= 0.005 {
        agg.push(format!(
            "total cost diverges: oracle=${:.4} clyde=${:.4}",
            oracle.total_cost, clyde.total_cost
        ));
    }
    let mut keys: BTreeSet<&String> = oracle.per_session.keys().collect();
    keys.extend(clyde.per_session.keys());
    for k in keys {
        let o = oracle.per_session.get(k).copied().unwrap_or((0.0, 0));
        let c = clyde.per_session.get(k).copied().unwrap_or((0.0, 0));
        if (o.0 - c.0).abs() >= 0.005 || o.1 != c.1 {
            agg.push(format!(
                "session {k}: oracle=(${:.4},{}) clyde=(${:.4},{})",
                o.0, o.1, c.0, c.1
            ));
        }
    }
    let aggregation = if agg.is_empty() { None } else { Some(agg.join("; ")) };

    Delta {
        file,
        parse,
        aggregation,
    }
}

fn write_jsonl(path: &Path, lines: &[&str]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, lines.join("\n")).unwrap();
}

fn assert_manifest(o: &OracleResult, m: &Manifest, label: &str) {
    assert_eq!(o.files.len(), m.files, "{label}: oracle file count vs manifest");
    assert_eq!(
        o.counted_entries, m.counted_entries,
        "{label}: oracle counted-entry count vs manifest"
    );
    assert!(
        (o.total_cost - m.total_cost).abs() < 1e-9,
        "{label}: oracle total cost vs manifest: expected {}, got {}",
        m.total_cost,
        o.total_cost
    );
}

// The parent session file, main-only. m1/r1 is a streaming partial + final pair (dedups to 1);
// m2/r2 is a distinct message.
//   m1 final (opus-4-7): 1000*5/1e6 + 500*25/1e6 = 0.005 + 0.0125 = 0.0175
//   m2       (sonnet-4-6): 2000*3/1e6 + 400*15/1e6 = 0.006 + 0.006  = 0.012
//   -> main-only: files=1, counted=2, total=0.0295
const PARENT_LINES: &[&str] = &[
    r#"{"type":"assistant","sessionId":"session-parent","timestamp":"2026-06-15T10:00:00Z","requestId":"r1","message":{"id":"m1","model":"claude-opus-4-7","usage":{"input_tokens":1000,"output_tokens":100}}}"#,
    r#"{"type":"assistant","sessionId":"session-parent","timestamp":"2026-06-15T10:00:05Z","requestId":"r1","message":{"id":"m1","model":"claude-opus-4-7","usage":{"input_tokens":1000,"output_tokens":500}}}"#,
    r#"{"type":"assistant","sessionId":"session-parent","timestamp":"2026-06-15T10:05:00Z","requestId":"r2","message":{"id":"m2","model":"claude-sonnet-4-6","usage":{"input_tokens":2000,"output_tokens":400}}}"#,
];

// A subagent file carrying the PARENT sessionId, so its spend folds into the parent total.
//   m3 (haiku-4-5): 1000*1/1e6 + 1000*5/1e6 = 0.001 + 0.005 = 0.006
//   -> main+subagents: files=2, counted=3, total=0.0355
const SUBAGENT_LINES: &[&str] = &[
    r#"{"type":"assistant","sessionId":"session-parent","timestamp":"2026-06-15T11:05:00Z","requestId":"r3","message":{"id":"m3","model":"claude-haiku-4-5","usage":{"input_tokens":1000,"output_tokens":1000}}}"#,
];

fn window() -> (NaiveDate, NaiveDate) {
    (
        NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
        NaiveDate::from_ymd_opt(2026, 6, 30).unwrap(),
    )
}

#[test]
fn oracle_equals_clyde_and_manifest_main_only_and_with_subagents() {
    let (start, end) = window();

    // ----- main-only -----
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let proj = projects.join("proj");
    write_jsonl(&proj.join("session-parent.jsonl"), PARENT_LINES);

    let manifest_main = Manifest {
        files: 1,
        counted_entries: 2,
        total_cost: 0.0295,
    };
    let oracle = run_oracle(&projects, start, end);
    assert_manifest(&oracle, &manifest_main, "main-only");

    let clyde = run_clyde(&projects, start, end);
    let delta = reconcile(&oracle, &clyde);
    assert!(
        delta.is_clean(),
        "main-only: oracle must equal clyde at every level: file={:?} parse={:?} agg={:?}",
        delta.file,
        delta.parse,
        delta.aggregation
    );
    assert_eq!(oracle.counted_entries, clyde.counted_entries, "main-only: entry count");
    assert!(
        (oracle.total_cost - clyde.total_cost).abs() < 0.005,
        "main-only: cost to the cent (oracle {} vs clyde {})",
        oracle.total_cost,
        clyde.total_cost
    );

    // ----- main + subagents (fold into the ONE parent session) -----
    write_jsonl(
        &proj.join("session-parent").join("subagents").join("agent.jsonl"),
        SUBAGENT_LINES,
    );

    let manifest_sub = Manifest {
        files: 2,
        counted_entries: 3,
        total_cost: 0.0355,
    };
    let oracle2 = run_oracle(&projects, start, end);
    assert_manifest(&oracle2, &manifest_sub, "main+subagents");

    let clyde2 = run_clyde(&projects, start, end);
    let delta2 = reconcile(&oracle2, &clyde2);
    assert!(
        delta2.is_clean(),
        "main+subagents: oracle must equal clyde at every level: file={:?} parse={:?} agg={:?}",
        delta2.file,
        delta2.parse,
        delta2.aggregation
    );
    assert_eq!(
        oracle2.counted_entries, clyde2.counted_entries,
        "main+subagents: entry count"
    );
    assert!(
        (oracle2.total_cost - clyde2.total_cost).abs() < 0.005,
        "main+subagents: cost to the cent (oracle {} vs clyde {})",
        oracle2.total_cost,
        clyde2.total_cost
    );
    assert_eq!(clyde2.per_session.len(), 1, "subagent spend must fold into ONE session");
}

#[test]
fn injected_scanner_omission_is_flagged_at_file_level_and_missed_by_arithmetic() {
    let (start, end) = window();

    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects");
    let proj = projects.join("proj");
    write_jsonl(&proj.join("session-parent.jsonl"), PARENT_LINES);
    write_jsonl(
        &proj.join("session-parent").join("subagents").join("agent.jsonl"),
        SUBAGENT_LINES,
    );

    // INJECT a scanner discovery omission: a session JSONL inside the session-uuid dir but OUTSIDE
    // `subagents/`. `scanner::find_session_files` only recurses into `subagents/`, so it
    // structurally skips this file; the oracle's recursive discovery finds it.
    //   m4 (sonnet-4-6): 1000*3/1e6 + 1000*15/1e6 = 0.003 + 0.015 = 0.018
    write_jsonl(
        &proj.join("session-parent").join("stray.jsonl"),
        &[
            r#"{"type":"assistant","sessionId":"session-parent","timestamp":"2026-06-15T12:00:00Z","requestId":"r4","message":{"id":"m4","model":"claude-sonnet-4-6","usage":{"input_tokens":1000,"output_tokens":1000}}}"#,
        ],
    );

    let oracle = run_oracle(&projects, start, end);
    assert_eq!(oracle.files.len(), 3, "oracle recursively discovers the stray file");
    assert_eq!(oracle.counted_entries, 4);
    assert!(
        (oracle.total_cost - 0.0535).abs() < 1e-9,
        "oracle total includes the stray file: expected 0.0535, got {}",
        oracle.total_cost
    );

    let clyde = run_clyde(&projects, start, end);
    assert_eq!(
        clyde.files.len(),
        2,
        "clyde's scanner structurally skips the stray file"
    );
    assert_eq!(clyde.counted_entries, 3);

    let delta = reconcile(&oracle, &clyde);
    assert!(
        delta.file.is_some(),
        "the scanner omission must be flagged at the FILE level"
    );
    assert!(
        delta.parse.is_none(),
        "with divergent file sets the discrepancy must NOT be misattributed to a parse-drop"
    );

    // A pure-arithmetic recheck reproduces clyde's total (it trusts clyde's extracted entries) and
    // is blind to the omitted file — the exact miss the level-separated oracle catches.
    let recheck = pure_arithmetic_recheck(&projects, start, end);
    assert!(
        (recheck - clyde.total_cost).abs() < 1e-9,
        "the pure-arithmetic recheck must reproduce clyde's total (recheck {} vs clyde {})",
        recheck,
        clyde.total_cost
    );
    assert!(
        (oracle.total_cost - recheck).abs() > 0.01,
        "the oracle sees dollars the arithmetic recheck misses (oracle {} vs recheck {})",
        oracle.total_cost,
        recheck
    );
}
