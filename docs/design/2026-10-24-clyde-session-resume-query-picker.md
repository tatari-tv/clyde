# Design Document: `clyde session resume <query...>`, search fallback + fzf picker

**Author:** Scott Idler
**Date:** 2026-10-24
**Status:** Implemented
**Review Passes Completed:** 5/5, plus review-panel round 1

## Summary

`clyde session resume` only takes a session id today. The everyday loop is two commands: `clyde session search <words>`, read the id off the output, then `clyde session resume <id>`. This doc folds the first step into the second. If the args are not an id, `resume` runs the same search `session search` runs. One clean hit resumes it. Several hits open an fzf picker (fzf: the fuzzy-finder TUI already installed). No TTY + several hits prints the list and exits non-zero.

## Problem Statement

### Background

- `clyde session resume <id>` shipped in `docs/design/2026-06-29-clyde-session-resume-fork-exec.md`: resolve the id, `chdir` to the recorded cwd, exec `claude --resume <id>`.
- `ResumeArgs.id: String` (`clyde/src/cli.rs:199`) takes exactly one positional.
- `cmd_resume` (`clyde/src/main.rs:554`) -> `resolve_one_or_exit` (`main.rs:890`) -> `Db::resolve_id` (`sessions/src/db.rs:755`) -> `plan_resume` (`main.rs:529`).
- Zero matches and an ambiguous prefix both print `✗` and exit 1. Nothing searches.

### Problem

Scott's actual loop (2026-10-24):

```
❯ clyde session search https://marquee.internal.tatari.dev/p/~scott-idler/marquee-security-audit-run-1/
○ b8024af2-61ac-4746-98c2-6e0f106459e7  marquee:main [rust security-audit web-service]
    2026-10-23 3182 Marquee report publishing
❯ clyde session resume b8024af2-61ac-4746-98c2-6e0f106459e7
```

- The search output already names the session. Copying the id across is pure ceremony.
- Passing the words straight to `resume` is a parse error today (`error: unexpected argument 'security' found`, exit 2).
- The same URL query now returns **3** hits, not 1 (Measurements). "Several hits" is the common case, not an edge.

### Goals

- `clyde session resume <args...>`: an id resumes as today; anything else is a search query. (Scott)
- Exactly one hit -> resume it, no prompt. (Scott)
- More than one hit -> fzf picker with enough per-row info to choose. (Scott)
- An id-looking arg is an id only if it resolves to exactly one session; otherwise it is a search query. (Scott, 2026-10-24)

### Non-Goals

- **Excluded:** new `resume` flags for search tuning (`--limit`, `--sort`, `--include-archived`). Nobody asked; `session search` still has them.
- **Excluded:** a picker for `session tag`, `export --id`, `enrich`. Resume only.
- **Excluded:** an embedded picker crate (`skim`). Shell out to fzf (agreed 2026-10-24).
- **Excluded:** MCP `session_open` behavior. It calls `db.resolve_id` itself (`sessions/src/mcp.rs:150`) and is untouched.
- **Excluded:** changing `cap_search_response`. The 60,000-char cap exists for MCP and JSON consumers; resume reads its `truncated` flag instead (see Search path).
- **Parked:** escaping `%`/`_` in `Db::resolve_id`'s `LIKE` (`db.rs:755`). The id-shape gate keeps both characters off the resume path. Revisit if another caller feeds it free text; the fix would ride into four MCP tools.

## Proposed Solution

### Overview

```
resume <args...>
  │  lazy_reindex (unless --no-reindex), as today
  │
  ├─ looks_like_id(args)? ── yes ─> Db::resolve_id
  │                                   ├─ exactly 1 -> resume (announce unless the arg IS the full id)
  │                                   └─ 0 or 2+   -> not an id: fall through to search
  │
  └─ search (args joined, archived excluded, default limit, relevance)
        ├─ 0                                  -> ✗ no session matches, exit 1
        ├─ 1, AND, not truncated              -> "→ resuming ..." on stderr, resume
        ├─ 1, OR fallback (or truncated)      -> picker (never auto-exec)
        └─ 2+                                 -> picker
                                                   ├─ stdin not a TTY -> print list, exit 1
                                                   ├─ no fzf          -> ✗ fzf not found, exit 1
                                                   ├─ Esc/Ctrl-C      -> ✗ cancelled, exit 130
                                                   └─ pick            -> resume
```

"resume" in every leaf is the existing `plan_resume` -> `run_resume_action`, unchanged. A missing cwd or reaped transcript still fails there with today's messages.

### Id detection

Scott's rule (2026-10-24): "anything that looks like an id. search and see if resolves to 1 item, if so, you know you have an id. if not" it's a query.

- `looks_like_id(query: &[String]) -> Option<&str>`: exactly one arg, every char `[0-9a-f-]`. No length gate.
- That arg goes to `Db::resolve_id`. Exactly one match -> it is an id, resume it.
- Zero or 2+ matches -> it is not an id. The arg goes to search like any other word.
- Anything that fails `looks_like_id` (a URL, multiple words, `fix-auth`) skips `resolve_id` and goes straight to search. That also keeps `%` and `_` away from the unescaped `LIKE`.
- Known trade-off: a hex-only word can match the start of a real id (Measurements: `beef` and `deed` each match one live session) and resume it. The one-hit announce line names it before claude starts.

**Announce on the id path:** a prefix match prints the same `→ resuming` line as a search hit. Only an arg that equals the resolved full id stays silent (today's behavior).

### Search path

- Call: `db.search(&query.join(" "), None, false, SortBy::Relevance)`. Same defaults as `session search`.
- `Db::search` reruns AND as OR when AND finds nothing and sets `fallback = Some(Fallback::Or)` (`db.rs:810`). A one-hit OR result matched only some words.
- `cap_search_response` (`db.rs:1268`) drops trailing hits past 60,000 chars and sets `truncated`. Broad words truncate today (Measurements: `review` -> 20 of 50, `truncated: true`).
- **Auto-exec only when `count == 1 && fallback.is_none() && !truncated`.** Everything else with >= 1 hit goes to the picker.
- Picker header on truncation: `showing the first N matches (truncated); refine the query`.
- Stale index: if `lazy_reindex` fails it warns and queries stored data (`main.rs:1089`). A single hit from stored data still auto-execs. That matches `session search`, and `plan_resume` still checks the transcript exists before launching.

### Picker

Copy the in-house precedent: `scottidler/git-tools` `worktree/src/pick.rs`.

- **Gate:** `std::io::stdin().is_terminal()`. fzf draws on `/dev/tty` and reads keys there; only its stdout is captured (precedent comment, `pick.rs:6-10`).
- **Not a TTY:** print the candidates with the existing renderers, then stderr `✗ N sessions match; pass an id` and exit 1.
  - `print_hits` (`main.rs:1132`) on the full `SearchResults`, so piped output is exactly the JSON `session search` emits.
- **fzf lookup:** `which::which("fzf")` (already a `clyde` dep, as `resolve_claude` uses at `main.rs:631`). Missing -> `✗ fzf not found on PATH; needed to pick among N sessions`, exit 1. Checked only on the picker path; the one-hit path never needs fzf.
- **No timeout** on the fzf child. It is human-paced, so `common::proc::run_bounded`'s 120s cap and null stdin do not apply. Named here so the exemption is deliberate.
- **Environment scrub:** remove `FZF_DEFAULT_OPTS` and `FZF_DEFAULT_OPTS_FILE` from the child's env. The panel proved `FZF_DEFAULT_OPTS=--select-1` makes fzf return a one-row list with no keypress, which would turn the OR-fallback picker into an auto-exec.
- **Shell pin:** `--with-shell 'sh -c'` so the preview command does not depend on `$SHELL` (zsh, fish).

**Row** (fzf input, one line per candidate, tab-delimited):

```
<full-id>\t<short-id>  <date>  <repo:branch>  <n-msgs>  <title>  [tags]
```

- fzf args: `--delimiter '\t' --with-nth 2.. --no-multi --layout reverse --header <header> --with-shell 'sh -c' --preview <cmd> --preview-window down,50%,wrap`.
- `repo:branch` built exactly as `print_record_line` does (`main.rs:1213`): cwd basename, `HEAD` -> empty branch, drop the `:` when empty. Siblings render identically.
- Title via `display_title` (`main.rs:1185`), char-safe truncation.
- Tabs, `\n`, `\r` in title and tags are replaced with spaces so one candidate is always one line.
- No ANSI color in rows.
- Header: `<N> sessions match "<query>"`, `no session matched all terms; showing any-term matches` on OR fallback, or the truncation line above.

**Selection:** fzf prints the whole chosen line. `parse_selection(stdout, candidates)` takes field 1, trims the newline, rejects empty or tab-less output, and rejects an id that is not one of the candidates it supplied.

**Outcome type:**

```rust
enum PickOutcome {
    Picked(String),
    Cancelled,
}
```

- fzf exit 0 -> `Picked`. Exit 130 (man fzf: Esc/Ctrl-C) -> `Cancelled`. Any other code -> `Err` naming fzf's code and its stderr.
- `main` maps every `Err` to exit 1 (`main.rs:70-77`), so cancel cannot be an `Err`. `cmd_resume` turns `Cancelled` into a new `ResumeAction::Cancelled`; `run_resume_action` prints `✗ cancelled` and exits 130, the same way it already exits non-zero for `Reaped`.

**Preview:**

- One file per candidate in a `tempfile::TempDir`, named by full session id.
- Content: title; `repo:branch`; cwd; created/modified; n-msgs; tags; blank line; `summary` (else `first-prompt`); blank line; `match: <snippet>` when the hit has one.
- fzf arg: `--preview "cat '<dir>'/{1}"`. fzf substitutes `{1}` as a single-quoted string (man fzf). A `<dir>` containing `'` -> loud error before launching fzf.
- `tempfile` moves from `[dev-dependencies]` to `[dependencies]` in `clyde/Cargo.toml` via `cargo add`.

**Lifecycle and failure contract:**

- `pick` owns the `TempDir` and the fzf child. Nothing outlives `pick`, so the later `exec` (which never runs `Drop`) cannot leak either.
- spawn fails -> `Err` naming fzf; the `TempDir` drops normally.
- writing rows fails (fzf exited early) -> kill and wait the child, then `Err`.
- wait fails -> `Err`.
- On `Picked` and `Cancelled`, `pick` calls `TempDir::close()` before returning. A close error is a `warn!`, not a failure: litter in `$TMPDIR` must not block a resume or mask the user's cancel.
- An `Err` from any step above wins over a close error.

### One-hit message

stderr, before the exec: `→ resuming <short-id>  <repo:branch>  <title>`. It names the session a query or prefix landed on before `claude` takes the screen.

### stdout not a TTY

- Out of scope to change. Today `resume <id> | cat` execs claude with a piped stdout, and claude decides what to do with it. The one-hit path keeps that behavior.
- The picker gate is stdin only; stdout is irrelevant to fzf, which writes the UI to `/dev/tty`.
- Covered by the shakedown's "piped" case.

### Data Model

No schema change. New types, in `clyde/src/resume.rs`:

```rust
/// What a resume query resolved to, before any picking.
enum Resolution {
    /// Resume this id directly. `announce` prints the one-hit stderr line.
    One { id: String, announce: bool },
    /// Search candidates for the picker; the full `SearchResults` is kept so no-TTY prints it as-is.
    Pick { results: SearchResults, header: String },
    NoMatch,
}
```

`PickItem { record, snippet }` rows are derived from the `SearchResults` inside `pick`.

### API Design

```rust
// clyde/src/cli.rs
pub struct ResumeArgs {
    /// Session id, unique id prefix, or search terms.
    #[arg(required = true, num_args = 1..)]
    pub query: Vec<String>,
    #[arg(long)]
    pub no_reindex: bool,
    #[arg(last = true)]
    pub extra: Vec<String>,
}

// clyde/src/resume.rs
fn looks_like_id(query: &[String]) -> Option<&str>;
fn resolution_from_id(matches: &[String], arg: &str) -> Option<Resolution>;        // pure; Some only for exactly one match
fn resolution_from_search(results: SearchResults, query: &str) -> Resolution;     // pure
fn resolve_query(db: &Db, query: &[String]) -> Result<Resolution>;                // thin, db

// clyde/src/resume/pick.rs
fn fzf_line(item: &PickItem) -> String;                                  // pure
fn preview_text(item: &PickItem) -> String;                              // pure
fn parse_selection(stdout: &str, candidates: &[String]) -> Result<String>;  // pure
fn run_picker(fzf: &Path, items: &[PickItem], header: &str) -> Result<PickOutcome>;  // no tty gate; testable with a stub fzf
fn pick(results: &SearchResults, header: &str) -> Result<PickOutcome>;   // tty gate + which + run_picker
```

`cmd_resume` keeps its shape: it returns a `ResumeAction` and `main` drops the `Db` before `run_resume_action` (`main.rs:281-292`).

### Implementation Plan

Phases are cut so each one leaves every new function called. `clyde` is a bin-only crate with `#![deny(dead_code)]` (`main.rs:3`), so a function with no caller fails `otto ci`.

#### Phase 0: Prove clap and fzf assumptions
**Model:** sonnet
- Done while authoring; zero product code. Results:
- clap: positional `Vec<String>` (`required = true, num_args = 1..`) beside `#[arg(last = true)] extra` parses as needed (scratch crate, clap 4):

  ```
  ["3bc0a20d"] -> query=["3bc0a20d"] extra=[]
  ["marquee", "security", "--", "--model", "opus"] -> query=["marquee", "security"] extra=["--model", "opus"]
  ["--no-reindex", "foo", "bar", "--", "--model", "opus"] -> query=["foo", "bar"] no_reindex=true extra=["--model", "opus"]
  ["foo", "--no-reindex", "bar"] -> query=["foo", "bar"] no_reindex=true
  [] -> ERR one or more required arguments were not provided
  ["--", "--model", "opus"] -> ERR one or more required arguments were not provided
  ```

- fzf: `/usr/bin/fzf` 0.67.0. Exit codes, `{1}` single-quoting, `--with-shell`, and `FZF_DEFAULT_OPTS_FILE` confirmed from `man fzf`.
- Panel probe: `FZF_DEFAULT_OPTS=--select-1` with one row exits 0 with no keypress (`/home/saidler/.cache/review-panel/runs/A7j99Tyn/fzf-probe.txt`). Hence the env scrub.
- **Success criteria:** both above; nothing to build.

#### Phase 1: Move resume code out of main.rs
**Model:** sonnet
- `main.rs` is 1399 lines; `otto bloat` caps files at 1500 (`.otto.yml:7`).
- Move `ResumeAction`, `plan_resume`, `cmd_resume`, `run_resume_action`, `resolve_claude`, both `launch_resume` into `clyde/src/resume.rs`; the 8 `plan_resume_*` tests (`clyde/src/tests.rs:48-190`) into `clyde/src/resume/tests.rs`.
- Zero behavior change.
- **Success criteria:** `otto ci` green; `cargo test -p clyde plan_resume` runs 8 tests; `rg -c '^fn plan_resume' clyde/src/main.rs` finds nothing.

#### Phase 2: Query resolution, everything but fzf
**Model:** opus
- `ResumeArgs.id` -> `query: Vec<String>`. Update the `Resume` doc comment (`cli.rs:101-110`) and the three cli tests (`clyde/src/cli/tests.rs:159-200`); add `resume_parses_multi_word_query_with_extra`.
- `looks_like_id`, `Resolution`, `resolution_from_id`, `resolution_from_search`, `resolve_query`, all wired into `cmd_resume`.
- In this phase `Resolution::Pick` always takes the no-TTY branch: print the list, exit 1. That is the final no-TTY behavior, so nothing here is a stopgap; Phase 3 adds the TTY branch.
- Unit tests (pure): `looks_like_id` true for a full UUID, an 8-char prefix and `beef`; false for `marquee`, a URL, two args, `fix-auth`. `resolution_from_search`: 0 -> `NoMatch`; 1 AND -> `One { announce: true }`; 1 OR -> `Pick`; 1 truncated -> `Pick`; 3 -> `Pick`. `resolution_from_id`: one match equal to the arg -> `One { announce: false }`; one prefix match -> `One { announce: true }`; 0 or 2 matches -> `None` (falls to search).
- Integration tests, new `clyde/tests/resume.rs`, seeded like `clyde/tests/search.rs` (`--db` + `session reindex --projects-dir`), seeded cwds are real tempdirs so `plan_resume` reaches `Launch`, stub `claude` on `PATH` that prints `$PWD` and its args:
  - one unique-word hit: stub prints `--resume <id>`; stderr has `→ resuming`.
  - two hits, stdin not a TTY: exit 1; stdout parses as `SearchResults` JSON with `count == 2`; stub not invoked.
  - OR-fallback single hit, stdin not a TTY: exit 1; stub not invoked.
  - zero hits: exit 1; stderr `no session matches`.
  - full id: stub invoked; no `→ resuming` line.
  - id-shaped word with zero id matches but one search hit: resumes via search.
  - ambiguous id prefix (2 seeded ids sharing it): goes to search, not to `resolve_id`'s candidates.
- **Success criteria:** `cargo test -p clyde -- looks_like_id resolution_from` passes with every `Resolution` variant covered; `cargo test -p clyde --test resume` passes all seven cases.

#### Phase 3: fzf picker
**Model:** opus
- `clyde/src/resume/pick.rs` per the Picker section, called from `cmd_resume` when stdin is a TTY. `PickOutcome`, `ResumeAction::Cancelled`, env scrub, `--with-shell`, tempdir lifecycle.
- `tempfile` to `[dependencies]` via `cargo add`.
- Add `fzf` to the `["session", "resume"]` REQUIRED TOOLS list (`main.rs:102`), purpose `session resume: pick among several matches`.
- Unit tests (pure): `fzf_line` on a title with `\t`, `\n` and multibyte chars yields exactly one line with one leading id field; `parse_selection` takes field 1, trims, rejects `""`, a tab-less line, and an id not in the candidates; `preview_text` falls back to `first-prompt` when `summary` is `None`.
- Stub-fzf tests on `run_picker` (no TTY needed, since the gate lives in `pick`): a stub that echoes the second row -> `Picked(<that id>)`; a stub that exits 130 -> `Cancelled`; a stub that prints a foreign id -> `Err`; a stub that records its env -> neither `FZF_DEFAULT_OPTS` nor `FZF_DEFAULT_OPTS_FILE` present; after every case the tempdir path no longer exists.
- **Success criteria:** those tests pass; `clyde session resume --help` lists `fzf` under REQUIRED TOOLS.

#### Phase 4: Surfaces
**Model:** sonnet
- `README.md:288-303` "Resuming sessions": query form, picker, no-TTY behavior, fzf requirement.
- `README.md:41` external-tools list: `clyde session resume` (claude, fzf).
- `plugin/skills/sessions/SKILL.md:48-62`: resume now takes search terms. An agent never calls `resume` with free text: with several hits it exits 1 (no TTY), and with one it execs an interactive claude.
- **Success criteria:** `rg -c 'resume <query' README.md plugin/skills/sessions/SKILL.md` reports >= 1 line in each file; `rg -n fzf README.md` hits the tools list and the resume section.

## Acceptance Criteria

Each was run on 2026-10-24 against the installed `clyde v0.27.2` (tag `10b0503`; `origin/main` `d74fe4e` adds only two pricing-data commits).

- [ ] `clyde session resume marquee security audit --no-reindex < /dev/null` exits 1 and prints the matching sessions.
  - Observed on main: `error: unexpected argument 'security' found`, exit 2.
- [ ] With a term generated at check time, `t=qq$(head -c8 /dev/urandom | od -An -tx1 | tr -d ' \n'); clyde session resume "$t" --no-reindex` prints `✗ no session matches "<t>"` and exits 1.
  - Observed on main: `✗ no session matches "qqxzqqxznomatch"`, `exit=1` (via the id resolver), with the original fixed literal.
  - Amended 2026-10-24 during execution: a fixed literal written in this doc gets indexed as soon as any session reads the doc. After Phase 2, `clyde session search qqxzqqxznomatch --no-reindex` returned `count: 2` (the authoring session `98bec2e1` and the executing session `e1c89b0b`). A one-hit state would exec claude on that session. The `qq` prefix keeps the term off the id path.
  - Observed on Phase 2 build (`c7066a1`): `✗ no session matches "qqxqbaabeeaeffddaeca"`, exit=1.
- [ ] `clyde session resume --help` lists `fzf` under REQUIRED TOOLS.
  - Observed on main: only `✅ claude  2.1.310  (session resume: exec \`claude --resume <id>\`)`.
- [ ] `cargo test -p clyde --test resume` passes, covering one hit, several hits with no TTY, an OR-fallback single hit that never launches claude, zero hits, and a full id.
  - Observed on main: no `clyde/tests/resume.rs` (`ls clyde/tests`: collect, doctor, export, matrix, search, serve). Cannot run until Phase 2.
- [ ] `wc -l clyde/src/main.rs` stays under 1500 and `otto bloat` passes.
  - Observed on main: 1399.

## Measurements (2026-10-24, live catalog, `clyde v0.27.2`)

- Catalog: 5235 sessions, 3246 not archived, **0** non-UUID `session_id`s. The hex+dash gate misses nothing.
- Hex-word prefix collisions against live ids:

  | word | sessions whose id starts with it |
  |------|-----|
  | `add` | 2 |
  | `beef` | 1 |
  | `deed` | 1 |
  | `babe` `bead` `cafe` `dead` `decade` `face` `facade` `fade` | 0 |

- 111 of 5235 ids start with four `a-f` letters.
- Motivating query: `clyde session search https://marquee.internal.tatari.dev/p/~scott-idler/marquee-security-audit-run-1/ --no-reindex` -> `count: 3, fallback: null` (`e0fd2451` 38 msgs, `d18ded3d` 311, `b8024af2` 3240).
- Truncation: `clyde session search review --no-reindex` -> `count: 20, truncated: true`. Panel also measured `marquee` 38 and `clyde` 31, both truncated.
- `zzzz` is not a safe zero-hit term: `clyde session search zzzz --no-reindex` -> `count: 2`.

## Resolved Decisions

- 2026-10-24, Scott: strict id detection; anything else is search.
- 2026-10-24, Scott: `id` becomes `Vec<String>`.
- 2026-10-24, Scott: no length gate. An id-looking arg is an id only when `resolve_id` returns exactly one session; 0 or 2+ -> search. This supersedes the earlier "ambiguous id prefix goes to the picker" and the `ID_MIN_LEN` 4-vs-8 question.
- 2026-10-24, Scott: shell out to fzf; missing fzf fails loudly.
- 2026-10-24, Scott: no TTY + several hits -> print the list, exit non-zero.
- 2026-10-24, Scott: archived excluded, same as `session search`.
- 2026-10-24, Scott: one hit prints a stderr line naming the pick.
- 2026-10-24, author + both panel seats: an OR-fallback single hit goes to the picker, never auto-exec.
- 2026-10-24, author + panel: a truncated result never auto-execs.
- 2026-10-24, author + panel: TTY gate is stdin only, matching the `worktree` precedent; stdout handling for the exec'd claude is unchanged from today.
- 2026-10-24, author + panel: Esc/Ctrl-C exits 130 through a typed `PickOutcome::Cancelled`, not an `Err`.
- 2026-10-24, author + panel: scrub `FZF_DEFAULT_OPTS`/`FZF_DEFAULT_OPTS_FILE` and check the pick against the candidates.
- 2026-10-24, author + panel: Phase 2 merged into resolution work; phases recut for `#![deny(dead_code)]`.
- 2026-10-24, author: a stale-index single hit may auto-exec, same as `session search` querying stored data.
- 2026-10-24, author: the id path announces a prefix match; only an exact full id is silent.
- 2026-10-24, author: the preview pane stays. Staff called it unrequested; it is how the picker meets Scott's "enough information to make that decision". Panel synthesis demoted the finding on the same grounds.

## Alternatives Considered

### Alternative 1: `skim` crate (embedded fzf)
- **Description:** in-process picker, no external binary.
- **Pros:** no runtime dep.
- **Cons:** heavy dep tree; fzf is already installed and in-house precedent shells out.
- **Why not chosen:** Scott, 2026-10-24.

### Alternative 2: Preview via `clyde session export --id`
- **Description:** `--preview 'clyde session export --id {1} --no-reindex | jq ...'`.
- **Pros:** no tempdir.
- **Cons:** a config load and a JSON blob per cursor move; adds `jq` as a hard dep.
- **Why not chosen:** a tempdir of plain files is cheaper and has no extra deps.

### Alternative 3: Preview text in a hidden fzf field
- **Description:** pack the preview into field N, `--preview 'printf %b {N}'`.
- **Cons:** escaping newlines and quotes through fzf and the shell is fragile.
- **Why not chosen:** fragile.

### Alternative 4: New `clyde session show <id>` for previews
- **Why not chosen:** unrequested surface.

### Alternative 5: Bypass `cap_search_response` for resume
- **Description:** fetch the full 50 hits uncapped for the picker.
- **Why not chosen:** changes `Db::search`'s contract for one caller. Reading `truncated` and telling the user to refine is enough.

## Technical Considerations

### Dependencies
- Runtime: `fzf` on PATH, only on the picker path. `tempfile` promoted to a normal dep.
- No cross-repo blast radius in code. `tatari-skills` pins `plugin/` by tag, so the SKILL.md change reaches agents only after the pin bump (operator step after the tag).

### Performance
- Id path: one `resolve_id` query, unchanged. Query path: one `search`, the same work `session search` already does.

### Security
- `extra` is never logged (existing rule, `main.rs:556`). The query is logged at DEBUG; it is the user's own search text.
- The preview command runs through `sh -c` (pinned). Only the tempdir path (rejected if it holds `'`) and fzf's own single-quoted `{1}` reach it.

### Testing Strategy
- Pure functions unit-tested (Phases 2, 3).
- Binary-level integration with a stub `claude` (Phase 2).
- `run_picker` with a stub `fzf` (Phase 3). The fzf UI itself is covered by the shakedown.

### Rollout Plan
- One PR, phase-per-commit. Bump rides the PR. After merge: tag, `cargo install`, `tatari-skills` pin bump.
- Shakedown: 0 hits, 1 hit, many hits (pick + Esc), id prefix, ambiguous prefix, OR-fallback single hit, truncated query, piped stdout.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| A hex-only word resolves as an id prefix | Low (`beef`, `deed` today) | Wrong session opens | Accepted by Scott; the id path announces every prefix match before claude starts |
| User fzf defaults auto-select | Low today (unset on desk) | Partial match auto-execs | Env scrub; candidate check in `parse_selection` |
| Agent calls `resume` with free text | Low | Agent blocks or execs claude | No-TTY path exits 1; SKILL.md forbids it |
| Tempdir leaked on exec | Low by construction | `$TMPDIR` litter | `pick` owns and closes the tempdir; stub tests assert it is gone |
| Truncated list hides the wanted session | Med on broad words | User doesn't see it | Header says truncated and to refine |

## Open Questions

None.

## Review Log

- Pass 1, draft: shape from the design-research brief, re-anchored to `origin/main` `d74fe4e` (the local checkout sat on a diverged WIP branch).
- Pass 2, correctness: fixed the version note (v0.27.2 is `10b0503`); fixed `cargo test` multi-filter syntax (filters go after `--`); confirmed 8 `plan_resume_*` tests on main.
- Pass 3, clarity: replaced an unfalsifiable Phase 6 check with an `rg -c` count; split the no-TTY renderer by path.
- Pass 4, edge cases: id-candidate path has no `SearchResults`; integration seeds need real cwds; an unresumable pick still fails in `plan_resume`.
- Pass 5, excellence: tempdir ownership inside `pick`.
- Panel round 1 (`/home/saidler/.cache/review-panel/runs/A7j99Tyn/synthesis.md`): 4 must-fix, 5 cheap-win, all folded in and each re-verified by the author against `d74fe4e` and the live catalog.
  - fzf `--select-1` hole -> env scrub + candidate check.
  - `zzzz` criterion wrong (2 live hits) -> `qqxzqqxznomatch`, measured 0.
  - `#![deny(dead_code)]` (`main.rs:3`, no `lib.rs`) -> phases recut to 0-4, each leaving new code called.
  - truncation -> auto-exec requires `!truncated`.
  - `Pick` now carries the full `SearchResults` so no-TTY prints it as-is.
  - exit 130 via `PickOutcome::Cancelled` (`main.rs:70-77` maps every `Err` to 1).
  - fzf child failure contract and `--with-shell 'sh -c'` named.
  - stale-index singleton decided; picker-path tests added with a stub fzf.
  - Scott, after round 1: no length gate; exactly one `resolve_id` match = id, else search. Dropped `ID_MIN_LEN` and the id-candidate picker.
  - Deferred by panel and not adopted: rollout ACs for installed version and skills pin (that is the ship step, not this doc's criteria); clap 4.6.7 vs 4.6.1 (Phase 2 parser tests cover it).

## References

- `docs/design/2026-06-29-clyde-session-resume-fork-exec.md` (the resume verb this extends)
- `scottidler/git-tools` `worktree/src/pick.rs` (fzf picker precedent)
- `man fzf`: EXIT STATUS, placeholder quoting, `--with-shell`, environment variables
