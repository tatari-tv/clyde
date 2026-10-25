//! fzf picker over a resume query's search candidates.
//!
//! Precedent: `scottidler/git-tools` `worktree/src/pick.rs`. fzf draws its UI on `/dev/tty` and
//! reads keys there; only its stdout (the chosen line) is captured. The stdin TTY gate lives in
//! `cmd_resume`, which owns the no-TTY branch; this module assumes a human is at the terminal.

use std::io::{Seek, Write};
use std::path::{Component, Path};
use std::process::{Command, Stdio};

use eyre::{Context, Result};
use report::fmt::short_id;
use sessions::{SearchResults, SessionRecord};

use crate::{display_title, record_location};

/// fzf's exit status for Esc / Ctrl-C (man fzf, EXIT STATUS).
const FZF_EXIT_INTERRUPTED: i32 = 130;

/// User-level fzf defaults that can change what a keypress-free run returns. `--select-1` in
/// either one makes fzf return a one-row list with no keypress, which would turn the OR-fallback
/// picker into an auto-exec, so both are removed from the child's environment.
const FZF_SCRUBBED_ENV: [&str; 2] = ["FZF_DEFAULT_OPTS", "FZF_DEFAULT_OPTS_FILE"];

/// What the user did in the picker.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PickOutcome {
    /// The full session id of the chosen row, checked against the candidates.
    Picked(String),
    /// Esc / Ctrl-C (fzf exit 130).
    Cancelled,
}

/// One picker row: a search hit's record and its match snippet.
struct PickItem<'a> {
    record: &'a SessionRecord,
    snippet: &'a str,
}

/// fzf input line: `<full-id>\t<short-id>  <date>  <repo:branch>  <n-msgs>  <title>  [tags]`.
/// fzf shows fields 2.. and prints the whole line on selection; field 1 is the id. Tabs and line
/// breaks in the display part become spaces so one candidate is always exactly one line with one
/// leading id field. No ANSI color.
fn fzf_line(item: &PickItem) -> String {
    let rec = item.record;
    let tags = if rec.tags.is_empty() {
        String::new()
    } else {
        format!("  [{}]", rec.tags.join(" "))
    };
    let display = format!(
        "{}  {}  {}  {}  {}{}",
        short_id(&rec.session_id),
        rec.modified.format("%Y-%m-%d"),
        record_location(rec),
        rec.n_msgs,
        display_title(rec),
        tags
    );
    format!("{}\t{}", single_line(&rec.session_id), single_line(&display))
}

fn single_line(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

/// Preview pane body for one candidate: header facts, then the summary (else the first prompt),
/// then the search snippet when the hit has one.
fn preview_text(item: &PickItem) -> String {
    let rec = item.record;
    let stamp = |t: chrono::DateTime<chrono::Utc>| t.format("%Y-%m-%d %H:%M UTC").to_string();
    let tags = if rec.tags.is_empty() { "-".to_string() } else { rec.tags.join(" ") };
    let body = rec
        .summary
        .as_deref()
        .or(rec.first_prompt.as_deref())
        .unwrap_or("(no summary or first prompt)");
    let mut text = format!(
        "{}\n{}\ncwd: {}\ncreated: {}\nmodified: {}\nmsgs: {}\ntags: {}\n\n{}\n",
        rec.title.as_deref().unwrap_or("(untitled)"),
        record_location(rec),
        rec.cwd.as_deref().unwrap_or("-"),
        rec.created.map(stamp).unwrap_or_else(|| "-".to_string()),
        stamp(rec.modified),
        rec.n_msgs,
        tags,
        body
    );
    if !item.snippet.is_empty() {
        text.push_str(&format!("\nmatch: {}\n", item.snippet));
    }
    text
}

/// The id from fzf's printed line: field 1, trailing newline trimmed. Rejects empty output, a line
/// with no tab, and an id that is not one of the `candidates` this picker supplied.
fn parse_selection(stdout: &str, candidates: &[String]) -> Result<String> {
    let line = stdout.trim_end_matches(['\n', '\r']);
    if line.is_empty() {
        eyre::bail!("fzf exited 0 but printed no selection");
    }
    let (id, _) = line
        .split_once('\t')
        .ok_or_else(|| eyre::eyre!("fzf printed a line with no tab-delimited id: {line:?}"))?;
    if !candidates.iter().any(|c| c == id) {
        eyre::bail!(
            "fzf printed id {id:?}, which is not one of the {} candidates",
            candidates.len()
        );
    }
    Ok(id.to_string())
}

/// `cat '<dir>'/{1}`: fzf substitutes `{1}` as a single-quoted string, so the only other text that
/// reaches `sh -c` is the tempdir path. A path holding `'` would break out of its quotes: refuse.
fn preview_command(dir: &Path) -> Result<String> {
    let dir = dir
        .to_str()
        .ok_or_else(|| eyre::eyre!("preview dir {} is not valid UTF-8", dir.display()))?;
    if dir.contains('\'') {
        eyre::bail!("preview dir {dir:?} contains a single quote; refusing to build the fzf preview command");
    }
    Ok(format!("cat '{dir}'/{{1}}"))
}

fn fzf_args<'a>(header: &'a str, preview: &'a str) -> [&'a str; 15] {
    [
        "--delimiter",
        "\t",
        "--with-nth",
        "2..",
        "--no-multi",
        "--layout",
        "reverse",
        "--header",
        header,
        "--with-shell",
        "sh -c",
        "--preview",
        preview,
        "--preview-window",
        "down,50%,wrap",
    ]
}

/// One preview file per candidate, named by full session id, so the id must be one plain path
/// component (the catalog stores UUIDs; anything else is refused loudly, never written elsewhere).
fn write_preview(dir: &Path, item: &PickItem) -> Result<()> {
    let id = &item.record.session_id;
    let mut parts = Path::new(id).components();
    if !matches!((parts.next(), parts.next()), (Some(Component::Normal(_)), None)) || id.contains('\t') {
        eyre::bail!("session id {id:?} is not a plain file name; cannot write its preview");
    }
    let path = dir.join(id);
    std::fs::write(&path, preview_text(item)).with_context(|| format!("writing preview {}", path.display()))
}

/// Run `fzf` over `items` with `header`. No TTY gate and no PATH lookup, so a stub `fzf` drives it in
/// tests. Owns the preview `TempDir` and the fzf child: nothing outlives this call, so the later
/// `exec` of claude (which never runs `Drop`) cannot leak either. No timeout: fzf is human-paced.
fn run_picker(fzf: &Path, items: &[PickItem], header: &str) -> Result<PickOutcome> {
    log::debug!(
        "run_picker: fzf={} items={} header={header:?}",
        fzf.display(),
        items.len()
    );
    let dir = tempfile::Builder::new()
        .prefix("clyde-resume-")
        .tempdir()
        .context("creating the picker preview tempdir")?;
    let preview = preview_command(dir.path())?;
    for item in items {
        write_preview(dir.path(), item)?;
    }
    // Rows go in through an unlinked temp file, not a pipe: `main` restores SIGPIPE's default
    // disposition, so writing to a pipe fzf already closed (e.g. it exited on a bad option) would
    // kill clyde silently instead of surfacing an error.
    let mut rows = tempfile::tempfile().context("creating the picker rows file")?;
    for item in items {
        writeln!(rows, "{}", fzf_line(item)).context("writing picker rows")?;
    }
    rows.rewind().context("rewinding the picker rows file")?;

    let mut cmd = Command::new(fzf);
    cmd.args(fzf_args(header, &preview))
        .stdin(Stdio::from(rows))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for var in FZF_SCRUBBED_ENV {
        cmd.env_remove(var);
    }
    let out = cmd
        .spawn()
        .with_context(|| format!("failed to launch fzf ({})", fzf.display()))?
        .wait_with_output()
        .context("waiting on fzf")?;

    let outcome = match out.status.code() {
        Some(0) => {
            let candidates: Vec<String> = items.iter().map(|i| i.record.session_id.clone()).collect();
            PickOutcome::Picked(parse_selection(&String::from_utf8_lossy(&out.stdout), &candidates)?)
        }
        Some(FZF_EXIT_INTERRUPTED) => PickOutcome::Cancelled,
        code => {
            let code = code.map_or_else(|| "a signal".to_string(), |c| format!("code {c}"));
            eyre::bail!(
                "fzf exited with {code}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
    };
    // Litter in $TMPDIR must not block a resume or mask the user's cancel, so a close error warns.
    let path = dir.path().to_path_buf();
    if let Err(e) = dir.close() {
        log::warn!("run_picker: failed to remove preview dir {}: {e}", path.display());
    }
    log::debug!("run_picker: outcome={outcome:?}");
    Ok(outcome)
}

/// Locate fzf on PATH and run the picker over every hit in `results`. Missing fzf is a loud error;
/// it is looked up only here, so the one-hit path never needs it.
pub(crate) fn pick(results: &SearchResults, header: &str) -> Result<PickOutcome> {
    let n = results.results.len();
    let fzf = which::which("fzf").map_err(|e| {
        let noun = if n == 1 { "session" } else { "sessions" };
        eyre::eyre!("fzf not found on PATH; needed to pick among {n} {noun} ({e})")
    })?;
    let items: Vec<PickItem> = results
        .results
        .iter()
        .map(|hit| PickItem {
            record: &hit.record,
            snippet: &hit.snippet,
        })
        .collect();
    run_picker(&fzf, &items, header)
}

#[cfg(test)]
mod tests;
