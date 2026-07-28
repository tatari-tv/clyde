//! Persist a guard-rejected render (design 2026-07-27-month-over-month-deltas.md, Phase 2). A
//! rejection is a hard render failure and a discarded paid render; before this, the sentence that
//! triggered it went with it. `guarded` is the one call site both `markdown_from_context` and
//! `html_from_context` route their guards through, so a rejection on either path leaves the
//! artifact on disk and the error names where. `generate_then_route` is the OTHER half of the same
//! guarantee: the ordering `run`'s two branches share so that a rejection (from `guarded` or
//! anywhere else generation can fail) never reaches the output destination.
//!
//! Split out of `render.rs` for file-size discipline, per Phase 11's precedent of splitting
//! `chart`/`geometry` out of this same file (see `reconciliation.rs`'s doc comment).

use crate::OutputDest;
use crate::config::xdg_data_dir;
use chrono::Utc;
use eyre::{Context, Result, bail};
use log::debug;
use std::fs;
use std::path::PathBuf;

/// Run `guards` and, on rejection, best-effort persist `artifact` before the error propagates.
/// `artifact` is the FULL generated render, which is not always what `guards` scanned: html's
/// guards run over `visible_text`, but the diagnostic worth keeping on disk is the html itself.
pub(super) fn guarded(kind: &str, ext: &str, artifact: &str, guards: impl FnOnce() -> Result<()>) -> Result<()> {
    guards().map_err(|err| persist_rejected(kind, ext, artifact, err))
}

/// `generate()?` then `route(&artifact)`, named so the ordering guarantee both branches of `run`
/// rely on -- "a rejected render writes nothing to the output path" (design Phase 2, Acceptance
/// Criterion 3) -- is a function a test can call directly instead of a fact that only holds
/// because of where two lines sit relative to each other in `run`. A guard rejection (or any other
/// generation failure) returns `Err` from `generate`; `?` short-circuits before `route` is ever
/// invoked. Generic over the artifact type so both the markdown and html branches of `run` share
/// this one call site rather than duplicating the ordering.
pub(super) fn generate_then_route<T>(
    generate: impl FnOnce() -> Result<T>,
    route: impl FnOnce(&T) -> Result<OutputDest>,
) -> Result<OutputDest> {
    let artifact = generate()?;
    route(&artifact)
}

/// Never rescues a render and never masks why one failed: on a successful persist, `err` is wrapped
/// to name the path so the operator can open the sentence that triggered the rejection; on any
/// failure to persist (including an `xdg_data_dir()` that resolves to `None`), the failure is
/// logged at WARN and `err` propagates UNCHANGED. The guard stays fail-closed either way -- this
/// function can only ever add information to a rejection, never turn one into a success.
fn persist_rejected(kind: &str, ext: &str, artifact: &str, err: eyre::Report) -> eyre::Report {
    match try_persist_rejected(kind, ext, artifact) {
        Ok(path) => err.wrap_err(format!(
            "the rejected {kind} render was written to {} for inspection",
            path.display()
        )),
        Err(persist_err) => {
            log::warn!(
                "render::rejected::persist_rejected: kind={kind} could not persist the rejected \
                 render (the original rejection still propagates unchanged): {persist_err:#}"
            );
            err
        }
    }
}

/// The largest counter suffix tried when the timestamped name already exists (two rejections
/// landing inside the same wall-clock second). Generous headroom above the one collision this path
/// can plausibly see -- a single operator, one render at a time -- without looping forever on a
/// directory that is somehow never writable to a fresh name.
const MAX_REJECTED_SUFFIXES_TRIED: u32 = 1000;

/// Write `artifact` to `xdg_data_dir()/clyde/rejected/<YYYY-MM-DD>-<HHMMSS>-<kind>.<ext>`,
/// uniquified with a `-<N>` suffix if that name is already taken, and hand back the path written.
/// Every failure mode (no resolvable XDG data dir, a directory that cannot be created, a write that
/// fails, a name that never frees up) is a plain `Err`; the caller decides what that means for the
/// guard's own error, this function only ever reports whether the write happened.
fn try_persist_rejected(kind: &str, ext: &str, artifact: &str) -> Result<PathBuf> {
    let dir = xdg_data_dir()
        .ok_or_else(|| eyre::eyre!("no resolvable XDG data dir (neither $XDG_DATA_HOME nor $HOME is set)"))?
        .join("clyde")
        .join("rejected");
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;

    let stamp = Utc::now().format("%Y-%m-%d-%H%M%S");
    let mut path = dir.join(format!("{stamp}-{kind}.{ext}"));
    let mut suffix = 1u32;
    while path.exists() {
        suffix += 1;
        if suffix > MAX_REJECTED_SUFFIXES_TRIED {
            bail!(
                "could not find a free name under {} after {MAX_REJECTED_SUFFIXES_TRIED} attempts",
                dir.display()
            );
        }
        path = dir.join(format!("{stamp}-{kind}-{suffix}.{ext}"));
    }
    fs::write(&path, artifact).with_context(|| format!("failed to write {}", path.display()))?;
    debug!(
        "render::rejected::try_persist_rejected: kind={kind} wrote rejected render to {}",
        path.display()
    );
    Ok(path)
}
