//! The single shared Claude-Code session-file scanner (Phase 5, cost-accuracy-verification).
//!
//! `cost` and `report` used to carry sibling copies of this discovery logic that drifted:
//! `report`'s was typed (parent/subagent grouping) and fail-loud (a UUID-v4 guard that `bail!`s on
//! a malformed dir); `cost`'s was the weaker, unguarded copy that additionally carried file mtime +
//! size for its date prefilter and cache hash. This module unifies them into ONE scanner both
//! crates consume, so the divergence class cannot recur.
//!
//! The unified [`SessionFile`] carries the UNION of what both crates need:
//! - `group_id` + `kind` -- `report`'s parent/subagent grouping;
//! - `mtime` + `size` -- `cost`'s [`filter_by_date_range`] date prefilter and cache-invalidation
//!   hash.
//!
//! Both `mtime` and `size` are read from the SAME `fs::metadata` call the empty-file check already
//! makes, so there is no extra stat per file.

use chrono::NaiveDate;
use eyre::{Result, bail};
use log::{debug, info, warn};
use regex::Regex;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

/// Whether a discovered file is a top-level parent session JSONL or one of its subagent JSONLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionFileKind {
    Parent,
    Subagent,
}

/// One discovered session JSONL file. The union of both consuming crates' needs (see the module
/// doc): `group_id`/`kind` drive `report`'s parent+subagent grouping; `mtime`/`size` drive `cost`'s
/// mtime date prefilter and cache-invalidation hash.
#[derive(Debug, Clone)]
pub struct SessionFile {
    pub path: PathBuf,
    /// The parent session's UUID stem. A parent file and its `subagents/*.jsonl` share this id, so
    /// subagent spend folds into the parent session's total.
    pub group_id: String,
    pub kind: SessionFileKind,
    pub mtime: SystemTime,
    pub size: u64,
}

const UUID_V4_PATTERN: &str = r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$";

fn uuid_v4_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(UUID_V4_PATTERN).expect("UUID-v4 pattern is a valid regex"))
}

/// Discover every session JSONL under the Claude projects directory: each top-level `*.jsonl` in
/// every project dir (a parent session) plus every `<session-uuid>/subagents/*.jsonl` (subagents
/// carrying the parent's session id).
///
/// Fail-loud (harvested from `report`): a top-level JSONL whose stem is not a UUID-v4, or a session
/// directory (one containing `subagents/`) whose name is not a UUID-v4, triggers [`bail!`] rather
/// than being misclassified. Real Claude Code session files are always UUID-v4 named, so a
/// non-UUID name is a corrupt/foreign layout that must surface loudly, never silently.
///
/// The returned list is sorted by path so the insertion order into any downstream parse/dedup
/// pipeline is stable across runs (`read_dir` yields entries in filesystem-dependent order, which
/// would otherwise make `cost`'s equal-cost dedup tie-break non-deterministic).
pub fn find_session_files(projects_dir: &Path) -> Result<Vec<SessionFile>> {
    debug!("scan::find_session_files: projects_dir={}", projects_dir.display());

    if !projects_dir.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();

    for project in read_dir_or_warn(projects_dir, "projects directory")? {
        let project_path = project.path();
        if !project_path.is_dir() {
            continue;
        }

        for entry in read_dir_or_warn(&project_path, "project directory")? {
            let entry_path = entry.path();

            if entry_path.is_file() && entry_path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                let stem = entry_path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
                if !uuid_v4_regex().is_match(stem) {
                    bail!(
                        "scan: parent JSONL stem is not a UUID-v4: {} (refusing to misclassify as a parent session)",
                        entry_path.display()
                    );
                }
                if let Some(file) = make_parent(entry_path.clone(), stem) {
                    files.push(file);
                }
                continue;
            }

            if entry_path.is_dir() {
                let stem = entry_path.file_name().and_then(|s| s.to_str()).unwrap_or_default();
                let subagents_dir = entry_path.join("subagents");
                if !subagents_dir.is_dir() {
                    continue;
                }
                if !uuid_v4_regex().is_match(stem) {
                    bail!(
                        "scan: parent session directory is not a UUID-v4: {} (refusing to misclassify subagents)",
                        entry_path.display()
                    );
                }
                for sub in read_dir_or_warn(&subagents_dir, "subagents directory")? {
                    let sub_path = sub.path();
                    if !sub_path.is_file() || sub_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                        continue;
                    }
                    if let Some(file) = make_subagent(sub_path, stem) {
                        files.push(file);
                    }
                }
            }
        }
    }

    // Sort by path so the insertion order into the parse/dedup pipeline is stable across runs.
    files.sort_by(|a, b| a.path.cmp(&b.path));

    info!("scan::find_session_files: discovered {} files", files.len());
    Ok(files)
}

fn make_parent(path: PathBuf, stem: &str) -> Option<SessionFile> {
    let (mtime, size) = file_stat(&path)?;
    if size == 0 {
        return None;
    }
    Some(SessionFile {
        path,
        group_id: stem.to_string(),
        kind: SessionFileKind::Parent,
        mtime,
        size,
    })
}

fn make_subagent(path: PathBuf, parent_stem: &str) -> Option<SessionFile> {
    let (mtime, size) = file_stat(&path)?;
    if size == 0 {
        return None;
    }
    Some(SessionFile {
        path,
        group_id: parent_stem.to_string(),
        kind: SessionFileKind::Subagent,
        mtime,
        size,
    })
}

/// Read `(mtime, size)` from a single `fs::metadata` call. `None` (skip the file) on a metadata
/// error, which also covers the previous empty-file skip: a file we cannot stat is not counted.
fn file_stat(path: &Path) -> Option<(SystemTime, u64)> {
    match fs::metadata(path) {
        Ok(m) => Some((m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len())),
        Err(e) => {
            warn!("scan: error reading metadata for {}: {}", path.display(), e);
            None
        }
    }
}

fn read_dir_or_warn(path: &Path, label: &str) -> Result<Vec<fs::DirEntry>> {
    let mut out = Vec::new();
    let iter = fs::read_dir(path).map_err(|e| eyre::eyre!("failed to read {} {}: {}", label, path.display(), e))?;
    for entry in iter {
        match entry {
            Ok(e) => out.push(e),
            Err(e) => warn!("scan: error reading entry under {}: {}", path.display(), e),
        }
    }
    Ok(out)
}

/// Every JSONL in one session's **explicit** layout, as [`SessionFile`]s carrying the mtime/size
/// [`filter_by_date_range`] and `cost`'s cache hash need.
///
/// Mirrors `session::parse`'s `discover_layout_files` for the `common` type, and reuses the SAME
/// [`make_parent`]/[`make_subagent`] constructors as [`find_session_files`], so the empty-file skip
/// and the single-stat rule are shared rather than reimplemented.
///
/// No UUID-v4 guard: the id comes from a catalog row that was indexed through the guarded scanner,
/// so re-validating it here would reject nothing. Sorted by path for the same determinism reason
/// [`find_session_files`] sorts (`read_dir` order is filesystem-dependent).
pub fn layout_files(session_id: &str, parent: &Path, subagents_dir: &Path) -> Vec<SessionFile> {
    debug!(
        "scan::layout_files: session_id={} parent={} subagents_dir={}",
        session_id,
        parent.display(),
        subagents_dir.display()
    );

    let mut files = Vec::new();

    if parent.is_file()
        && let Some(file) = make_parent(parent.to_path_buf(), session_id)
    {
        files.push(file);
    }

    if subagents_dir.is_dir() {
        match fs::read_dir(subagents_dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file()
                        && path.extension().and_then(|e| e.to_str()) == Some("jsonl")
                        && let Some(file) = make_subagent(path, session_id)
                    {
                        files.push(file);
                    }
                }
            }
            Err(e) => warn!(
                "scan::layout_files: failed to read subagents dir {}: {}",
                subagents_dir.display(),
                e
            ),
        }
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    debug!(
        "scan::layout_files: session_id={} resolved {} file(s)",
        session_id,
        files.len()
    );
    files
}

/// THE recoverability predicate for pricing: every readable JSONL for one session, live layout
/// first, staged layout second, EMPTY when there are no bytes anywhere.
///
/// Both the efficiency backfill and `report collect` branch on `is_empty()`, so "unrecoverable"
/// means one thing in both rather than each answering differently.
///
/// Deliberately NOT `sessions::transcript::transcript_layout_parts`: that resolver requires a
/// regular parent `.jsonl` because it answers "where is this session's BODY" for enrich/export/FTS,
/// and a body needs the parent. Pricing asks a different question -- "which files hold this
/// session's usage records" -- and a subagent file holds real ones. So a session whose parent was
/// reaped before staging still prices from its subagents here. The two questions stay two
/// functions; see the design doc's "One definition of recoverable".
pub fn pricing_files(
    session_id: &str,
    live_parent: &Path,
    live_project_dir: &Path,
    staged_dir: Option<&Path>,
) -> Vec<SessionFile> {
    debug!(
        "scan::pricing_files: session_id={} live_parent={} staged_dir={:?}",
        session_id,
        live_parent.display(),
        staged_dir
    );

    let live_subagents = live_project_dir.join(session_id).join("subagents");
    let live = layout_files(session_id, live_parent, &live_subagents);
    if !live.is_empty() {
        debug!(
            "scan::pricing_files: session_id={} resolved live ({} files)",
            session_id,
            live.len()
        );
        return live;
    }

    let Some(staged) = staged_dir else {
        warn!("scan::pricing_files: session_id={session_id} has no live bytes and no staged path");
        return Vec::new();
    };

    let staged_parent = staged.join(format!("{session_id}.jsonl"));
    let staged_subagents = staged.join("subagents");
    let files = layout_files(session_id, &staged_parent, &staged_subagents);
    if files.is_empty() {
        warn!(
            "scan::pricing_files: session_id={} has no readable bytes live or staged ({})",
            session_id,
            staged.display()
        );
    }
    files
}

/// The live scan, plus every staged session whose id is absent from it.
///
/// Live-then-staged precedence, the same rule `sessions::transcript::transcript_layout_parts`
/// applies per row, so a session staged while still live is counted ONCE (from the live root).
/// A staged root that does not exist yields the live scan unchanged.
///
/// Asymmetry with [`find_session_files`], deliberate: a non-UUID name in the projects tree
/// [`bail!`]s (it could be misclassified as a parent or subagent), but a non-UUID staged directory
/// only WARNs and is skipped. The staged filename is *derived* from the directory name
/// (`<dir>/<dir>.jsonl`), so a wrong name finds nothing rather than misclassifying something, and
/// bailing would let one stray directory in a clyde-owned cache brick every `clyde cost` run.
pub fn find_session_files_with_staged(projects_dir: &Path, staged_root: &Path) -> Result<Vec<SessionFile>> {
    debug!(
        "scan::find_session_files_with_staged: projects_dir={} staged_root={}",
        projects_dir.display(),
        staged_root.display()
    );

    let mut files = find_session_files(projects_dir)?;

    if !staged_root.is_dir() {
        debug!(
            "scan::find_session_files_with_staged: staged root absent, returning {} live file(s)",
            files.len()
        );
        return Ok(files);
    }

    let live_ids: BTreeSet<String> = files.iter().map(|f| f.group_id.clone()).collect();
    let mut staged_added = 0usize;

    for entry in read_dir_or_warn(staged_root, "staged root")? {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(id) = dir.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !uuid_v4_regex().is_match(id) {
            warn!(
                "scan: staged directory name is not a UUID-v4, skipping: {}",
                dir.display()
            );
            continue;
        }
        if live_ids.contains(id) {
            continue;
        }
        let parent = dir.join(format!("{id}.jsonl"));
        let subagents = dir.join("subagents");
        let resolved = layout_files(id, &parent, &subagents);
        staged_added += resolved.len();
        files.extend(resolved);
    }

    // Sort the union, matching find_session_files' stable-order contract (cost's equal-cost dedup
    // tie-break depends on it).
    files.sort_by(|a, b| a.path.cmp(&b.path));

    info!(
        "scan::find_session_files_with_staged: {} file(s) total ({} from the staged root)",
        files.len(),
        staged_added
    );
    Ok(files)
}

/// Prefilter session files by mtime as a *lower-bound optimization only*.
///
/// Counting is by entry timestamp (the counted-entry contract): a line counts iff its own
/// `timestamp` falls in the window, enforced per-entry by the consumer. This prefilter exists
/// solely to skip whole files that provably hold no in-window content, so it MUST NEVER drop a
/// file that could hold an in-window entry.
///
/// The only safe test is the lower bound `mtime_date >= start`. It is safe under the append-only
/// invariant: Claude Code only ever appends to a session JSONL, so a file's mtime is >= its newest
/// entry's timestamp. Therefore a file whose mtime falls before `start` has *every* entry before
/// `start` and cannot hold in-window content -- dropping it loses nothing.
///
/// There is deliberately NO upper bound (`mtime_date <= end`). A file touched after `end` (e.g. a
/// still-growing session queried for an earlier day) can still hold entries dated within the
/// window; a `<= end` exclusion would silently drop those in-window dollars.
pub fn filter_by_date_range(files: &[SessionFile], start: NaiveDate, end: NaiveDate) -> Vec<&SessionFile> {
    debug!(
        "scan::filter_by_date_range: start={}, end={}, input_count={}",
        start,
        end,
        files.len()
    );

    files
        .iter()
        .filter(|f| {
            let mtime: chrono::DateTime<chrono::Local> = f.mtime.into();
            let file_date = mtime.date_naive();
            // Lower bound only. Never exclude a file whose mtime is at/after `start`; the actual
            // window enforcement is the per-entry timestamp check in the consumer.
            file_date >= start
        })
        .collect()
}

/// The default Claude projects directory (`~/.claude/projects`).
pub fn default_projects_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

/// Subdirectory name under the XDG data root that owns clyde's data.
const CLYDE_DIR: &str = "clyde";

/// Subdirectory under clyde's data root holding durable transcript copies.
const STAGED_DIR: &str = "staged";

/// XDG data dir: a DELEGATION to [`crate::paths::xdg_data_dir`], this crate's own definition.
///
/// The comment this replaces explained why it could not call `session::paths::xdg_data_dir` (`common`
/// must not depend on `session`; the edge runs the other way). That reason no longer applies -- the
/// definition moved DOWN into `common`, which is where all five callers can reach it -- so the comment
/// is gone rather than left to contradict the edge that now exists.
fn xdg_data_dir() -> Option<PathBuf> {
    crate::paths::xdg_data_dir()
}

/// The default staged-transcript root (`~/.local/share/clyde/staged`).
///
/// THE definition of that path: `session::paths::staged_dir` delegates here, so the two can never
/// name different directories.
pub fn default_staged_dir() -> Option<PathBuf> {
    xdg_data_dir().map(|d| d.join(CLYDE_DIR).join(STAGED_DIR))
}

#[cfg(test)]
mod tests;
