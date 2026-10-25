## Phase 1: Move resume code out of main.rs
### Design decisions
- Resume code lives in `clyde/src/resume.rs` (module file, not `resume/mod.rs`) with tests in `clyde/src/resume/tests.rs` - same wiring as doctor/projects/cli (`#[cfg(test)] mod tests;` at file end).
- `ResumeAction`, `plan_resume`, `cmd_resume`, `run_resume_action` widened to `pub(crate)` - main.rs dispatch calls `resume::cmd_resume` and `resume::run_resume_action`; `resolve_claude` and both `launch_resume` stay private to the module.
- `lazy_reindex` and `resolve_one_or_exit` stay in `clyde/src/main.rs` (shared with other commands), widened to `pub(crate)` so `resume.rs` can call them.
- The `base_record` fixture moved from `clyde/src/tests.rs` to `clyde/src/resume/tests.rs` with the 8 `plan_resume_*` tests, since nothing else in tests.rs used it. Unused imports left behind in tests.rs were removed.

### Deviations
- None.

### Tradeoffs
- Widening two main.rs helpers to `pub(crate)` vs moving them: moving would drag unrelated commands' code into this phase.

### Open questions
- None.

## Phase 2: Query resolution, everything but fzf
### Design decisions
- `Resolution`, `looks_like_id`, `resolution_from_id`, `resolution_from_search`, `resolve_query` live in `clyde/src/resume.rs`, module-private; `cmd_resume` is the only caller and maps each variant (One -> fetch + optional announce + `plan_resume`; NoMatch -> `✗ no session matches "<joined query>"`, exit 1; Pick -> `print_hits` + `✗ N sessions match; pass an id`, exit 1).
- `record_location` extracted in `clyde/src/main.rs` from `print_record_line` so the search listing and the resume announce line build `repo:branch` from one function; `print_hits`, `display_title`, `record_location` widened to `pub(crate)`.
- `looks_like_id` also rejects an empty arg and uppercase hex: the catalog stores lowercase UUIDs, and an empty arg would become `LIKE '%'`.
- `resolution_from_search` header precedence: truncated, then OR fallback, then `N sessions match "<query>"` (the doc lists the three but not their order; truncation is the more actionable one).
- `resolution_from_search` decides on `results.results.len()`, not `count`; the two always agree per `SearchResults`' contract, and the slice pattern avoids indexing.
- Integration stub `claude` prints `pwd -P` rather than `$PWD`: an exec'd `sh` may inherit a stale `$PWD` from clyde's environment.
- `resolve_one_or_exit` stays in `main.rs` for export/tag/enrich; resume no longer calls it.

### Deviations
- `Resolution::Pick.header` is only read by a `debug!` line in the no-TTY branch this phase; Phase 3's picker is its real consumer. The doc's no-TTY output does not print the header, and `#![deny(dead_code)]` needs the field read.
- The no-TTY stderr line pluralizes: `✗ 1 session matches; pass an id` for a lone OR-fallback or truncated hit, `✗ N sessions match; pass an id` otherwise (doc wrote only the plural form).
- Acceptance criterion 2 (`resume qqxzqqxznomatch` -> no match) no longer holds on the live catalog: the design session (`98bec2e1`) and this implementer session both contain the literal word, so the live search now finds it (`count: 2` at check time; `count: 1` on the first run, which resumed `98bec2e1` and exec'd the real claude, which exited 1 on no stdin). Behavior is correct per the design; the probe term is self-defeating once indexed. Verified the no-match path instead with a runtime-generated term never written to any transcript.

### Tradeoffs
- Announce line uses a plain `→` (no color) vs colored like `✗`: keeps the integration assertion on the literal `→ resuming` independent of whether `colored` emits ANSI on a piped stderr.
- Stub `claude` written per test fixture vs one shared stub: per-fixture keeps tests isolated; the fork/exec ETXTBSY window is not hit because clyde execs the stub long after the write fd is closed.

### Open questions
- Acceptance criterion 2 needs a new probe term that cannot self-index (e.g. generated at check time), since any fixed literal in the doc lands in the catalog the moment a session reads the doc.

## Orchestrator: acceptance criterion 2 amended (after Phase 2)

### Design decisions
- None.

### Deviations
- AC2 in the design doc now generates its zero-hit term at check time instead of naming `qqxzqqxznomatch`. Doc defect, not a code change: the fixed literal became searchable once sessions read the doc. Evidence: `clyde session search qqxzqqxznomatch --no-reindex` -> `count: 2` (sessions `98bec2e1`, `e1c89b0b`). The code's no-match path is unchanged and was shown with a runtime term: `✗ no session matches "qqxqbaabeeaeffddaeca"`, exit=1.

### Tradeoffs
- None.

### Open questions
- None.

## Phase 3: fzf picker
### Design decisions
- `clyde/src/resume/pick.rs` holds `PickOutcome`, `PickItem<'a>` (borrows the hit's record and snippet), `fzf_line`, `preview_text`, `parse_selection`, `preview_command`, `write_preview`, `fzf_args`, `run_picker`, `pick`. Tests in `clyde/src/resume/pick/tests.rs`.
- `cmd_resume` (`clyde/src/resume.rs`) maps `Picked(id)` to the normal `db.get` + `plan_resume` path with no announce line (the user just chose the row), `Cancelled` to `ResumeAction::Cancelled`, and any picker `Err` (fzf missing included) to a `✗ <cause chain>` line + exit 1, matching how `run_resume_action` renders a `Launch` failure.
- `run_resume_action` prints `✗ cancelled` and exits `EXIT_CANCELLED` (130).
- `write_preview` refuses any session id that is not one plain path component (or holds a tab), so a malformed id can never write outside the tempdir or split fzf's field 1. The catalog holds 0 non-UUID ids today, so this never fires on real data.
- `preview_command` is split out so the `'`-in-dir refusal is unit-testable without controlling `$TMPDIR`.
- fzf's stderr is piped and captured for the non-0/130 error message; its UI uses `/dev/tty` (fzf default; `--no-tty-default` is the opt-in to use stderr), so this does not affect drawing.
- fzf exit 1 (no match, Enter on an empty list) and death by signal land in the generic `Err` branch, naming the code (or "a signal") and fzf's stderr.
- Stub-fzf tests serialize on a module `ENV_LOCK`: the env-scrub test sets `FZF_DEFAULT_OPTS=--select-1` and `FZF_DEFAULT_OPTS_FILE` on the test process (edition 2024 `unsafe`), then removes them, so the "neither var reached fzf" assertion actually bites. Proven by disabling the `env_remove` loop: the test fails with `FZF_DEFAULT_OPTS reached fzf`.
- Bite proofs run this phase: removing the scrub fails `run_picker_scrubs_user_fzf_defaults_from_the_child_env`; disabling the candidate check fails `parse_selection_rejects_id_not_among_candidates` and `run_picker_rejects_an_id_it_did_not_supply`; leaking the TempDir (`keep()` instead of `close()`) fails all 5 stub tests that assert the dir is gone.

### Deviations
- TTY gate lives in `cmd_resume`, not in `pick` (doc: `pick` = tty gate + which + run_picker). `cmd_resume` owns the no-TTY branch (`print_hits` + exit 1), so it must know the gate result; putting the check in `pick` would need a third `PickOutcome` variant that is not a pick outcome. Same effect, correct seam.
- Rows reach fzf through an unlinked `tempfile::tempfile()` used as its stdin, not a pipe, so the doc's "writing rows fails (fzf exited early) -> kill and wait the child, then Err" step no longer exists. Reason: `main` restores SIGPIPE to `SIG_DFL` (`reset_sigpipe`), so writing to a pipe fzf already closed (e.g. fzf exiting 2 on a bad option before reading stdin) would kill clyde silently with no message. A file cannot raise SIGPIPE; fzf's early exit is reported through the exit-code branch instead.
- The fzf-not-found message pluralizes (`1 session` / `N sessions`), matching Phase 2's no-TTY line, and appends `which`'s own error in parens.
- fzf args are passed as one 15-element array from `fzf_args` rather than chained `.arg` calls (rustfmt rendered the chain unreadably); the arg list and order are exactly the doc's.

### Tradeoffs
- `PickItem` borrows from `SearchResults` vs owning clones: no copy of records for a short-lived picker.
- Per-test stub scripts written to fresh tempdirs vs one shared stub: isolation per test; the ETXTBSY window (another thread forking while the script's write fd is open) is the same accepted micro-race as Phase 2's stub `claude`.
- Picked rows do not print the `→ resuming` announce: the user selected the row on screen, so restating it before claude takes the terminal is noise. The doc's one-hit message section covers query/prefix landings only.

### Open questions
- None.

## Phase 4: Surfaces
### Design decisions
- README "Resuming sessions" documents shipped behavior from the Phase 2 and 3 notes: no announce line on a picked row, no-TTY prints the SearchResults JSON then `✗ N sessions match; pass an id` (exit 1), Esc exits 130 with `✗ cancelled`, missing fzf fails loudly, truncation and OR-fallback header, env scrub of the fzf defaults.
- The SKILL.md resume section keeps the "NOT a read path" framing and adds the query form with an explicit agent prohibition, stating the two failure modes (several hits exit 1 with no TTY, one hit execs interactive claude).

### Deviations
- None.

### Tradeoffs
- Documented the `resume <query...>` form inside the existing sections rather than adding new headings: both files already had one resume section each.

### Open questions
- None.

## Implementation audit fold-in (review-panel round 1, run xXpoh1pf)

### Design decisions
- `clyde/src/cli.rs` `Resume` help: several hits, a lone any-term hit, or a truncated result open the fzf picker; the list-and-exit-1 path applies only with no TTY on stdin. The old text described the Phase 2 state.
- `README.md` "Resuming sessions": a lone any-term or truncated-list hit goes to the picker (so it can need fzf), and the no-TTY path prints what `session search` prints (JSON only when stdout is piped). The Phase 4 note's claim that the README matched shipped behavior was wrong on these two points.

### Deviations
- Supersedes the Phase 2 entry that `looks_like_id` rejects uppercase hex. It now accepts hex digits of either case (`clyde/src/resume.rs:looks_like_id`), and `resolution_from_id` compares with `eq_ignore_ascii_case`. Reason: `Db::resolve_id`'s `LIKE` ignores ASCII case, so uppercase prefixes worked on v0.27.2, and `session tag`/`export --id`/`enrich` still accept them. Rejecting them was an undisclosed user-visible regression (panel: 19 of 20 sampled uppercase prefixes became "no session matches"). Tests: `looks_like_id` accepts `B8024AF2`; new `resolution_from_id_uppercase_full_id_is_silent`.

### Tradeoffs
- Accept uppercase vs keep rejecting and document it: accepting restores v0.27.2 behavior and sibling parity in one line.

### Open questions
- None.

## PR mutants gate fold-in (PR #124, `mutants / report`)

### Design decisions
- `clyde/tests/resume.rs:resume_help_lists_claude_and_fzf_under_required_tools` catches deletion of the `["session", "resume"]` help arm in `clyde/src/main.rs:parse_cli`.
- `clyde/src/resume/pick.rs:fzf_missing_message` extracted from `pick` so the singular/plural branch has a unit test.

### Deviations
- `#[mutants::skip]` on the `#[cfg(not(unix))]` `launch_resume` in `clyde/src/resume.rs`, via `cargo add mutants` (0.0.4, attribute-only crate). cargo-mutants runs on unix, where that body is compiled out, so its mutant can never be caught. First skip annotation in the tree; `.otto.yml`'s mutants task names an annotated skip as the remedy for an uncatchable survivor.

### Tradeoffs
- Attribute skip vs a `.cargo/mutants.toml` `exclude_re`: the regex would have to match a line number or also exclude the unix `launch_resume`, which is caught today.

### Open questions
- None.

Local diff-scoped run (`otto mutants --diff`, PR diff incl. this fix): 59 mutants, 21 caught, 38 unviable, 0 missed.
