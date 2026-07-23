# Implementation Notes: Session Efficiency & Behavior Signals

Running, append-only record of how the implementation diverges from or interprets
the design doc `docs/design/2026-07-22-session-efficiency-signals.md`. One section
per phase, four buckets each ("None." where empty).

## Phase 0: Signal-fixture spike

### Design decisions

- Fixtures live at the WORKSPACE root, `fixtures/efficiency/*.jsonl`, not inside
  a crate. The `efficiency` lib crate doesn't exist until Phase 1; rooting the
  fixtures one level up means Phase 1's `scaffold`-generated `efficiency/`
  directory cannot collide with them, and any future crate's tests can reach
  them via a relative `../../fixtures/efficiency/*.jsonl` `include_str!` path.
- One fixture file per signal class rather than one giant multi-signal file:
  `tool-errors.jsonl`, `interrupts.jsonl`, `compaction.jsonl`,
  `turn-duration.jsonl`, `usage.jsonl`, `clean-session.jsonl` — mirrors the
  design doc's own "Signals (full scope)" section headings, so Phase 3's
  extractor tests can name the fixture they're proving.
- `tool-errors.jsonl` deliberately carries all three cases (bash exit-code
  failure, non-Bash framework failure, healthy non-error call) in ONE file so
  the strict-subset invariant (`bash_command_failures <= tool_errors`, never
  equal, never independent) is provable from a single fixture rather than
  cross-referencing two files. Exact field paths, predicate definitions, and
  which session ids each shape was verified against are documented in
  `fixtures/efficiency/README.md` (not duplicated here — single source).
- Verification script `fixtures/efficiency/bin/verify-fixtures.sh` (throwaway
  `jq`, per the phase's own success criteria) asserts every path in the README
  resolves and the subset invariant holds; it is deliberately NOT wired into
  `otto ci` — it is a one-off spike artifact Phase 3's real Rust tests will
  supersede, per the phase-implementer's "never fake or stub" guidance
  applied to test tooling: don't manufacture permanent CI machinery for a
  phase whose own success criteria call it a spike.
- Everything is redacted or synthesized from a *verified real record shape*:
  no raw prompt, diff, file content, or command output survives into a
  fixture; only field names/nesting, booleans, fixed framework marker
  strings (the interrupt text markers), and non-sensitive numeric values
  (token counts, durations) are real.

### Deviations

- **`toolUseResult.interrupted:true` does not occur anywhere in the sampled
  corpus.** `fixtures/efficiency/interrupts.jsonl`'s structured-interrupt
  record is SYNTHESIZED (real object shape, `interrupted` field hand-flipped
  to `true`), not harvested verbatim, because a full scan of all 2,883
  session files / 39,358 occurrences of the `interrupted` key found zero
  `true` values. Same effect, correct seam: the shape is real, only the value
  is invented. Phase 3 should treat this predicate as untested-against-a-real-
  positive until a genuine interrupted-Bash-call transcript surfaces.
- **`compactMetadata.trigger:"manual"` does not occur anywhere in the sampled
  corpus.** Every compaction observed live was `"auto"`. The `manual` record
  in `fixtures/efficiency/compaction.jsonl` is SYNTHESIZED (real
  `compactMetadata` shape, `trigger` hand-set to `"manual"`) — same
  same-effect-correct-seam reasoning as above.
- **`bash_command_failures`'s text pattern lives in the top-level
  `toolUseResult` field, not `message.content[].content`.** The design doc
  says "the result text matches the `Error: Exit code N` shape" without
  naming which field; live data shows the `message.content[]` tool_result
  block's own `content` string is `"Exit code N\n..."` (no `Error:` prefix),
  while the sibling top-level `toolUseResult` field collapses to the string
  `"Error: Exit code N\n..."` ONLY on a Bash failure (it is the
  `{stdout,stderr,interrupted,isImage,noOutputExpected}` object on success).
  Fixtures and the README lock the predicate onto `toolUseResult` (the field
  that actually carries the literal `"Error: Exit code N"` text) — Phase 3
  should implement against this field, not `message.content[].content`.

### Tradeoffs

- Redacted/reconstructed fixtures over raw-copied live files: raw copies
  would be the most "verbatim harvested," but would ship real prompts, file
  paths, and command output (some referencing internal Tatari repos/infra)
  into a public-shaped git history. Chose structurally-faithful redaction
  (real schema, placeholder content) over verbatim copies — matches the
  task's explicit redaction requirement and the org's "never commit secrets"
  policy; the tradeoff is fixtures are hand-assembled JSON rather than a
  straight `cp`, so a subtle schema quirk not seen in the *specific* records I
  sampled could still be missed. Mitigated by keeping the field-by-field
  provenance trail in the README so later phases can re-verify against fresh
  live samples if a metric looks wrong.
- One `jq` script covering all six fixtures over six small standalone
  scripts: less "throwaway per fixture," but a single script means the
  subset-invariant check (which spans only `tool-errors.jsonl`) sits next to
  the per-fixture path checks instead of being orphaned in its own file.

### Open questions

None. Phase 0 has no design decisions requiring Scott's confirmation — it is
a data-gathering spike with no production code or API surface.

## Phase 1: Scaffold `efficiency` lib crate + umbrella wiring

### Design decisions

- New workspace member `efficiency` created as a plain clyde-native lib crate
  (`efficiency/Cargo.toml`, `efficiency/src/lib.rs`, `efficiency/src/cli.rs`),
  copying the `pub mod cli; pub fn run(args, common::Globals) -> eyre::Result<i32>`
  shape from `report`/`cost`/`permit` exactly, since the design doc's
  Phase 5+ dispatch (`dispatch_tool(efficiency::run(args, globals), debug)`)
  requires that signature.
- No `build.rs` and no `[[bin]]`. `report`/`cost`/`permit` carry a `build.rs`
  (`GIT_DESCRIBE` for their own now-retired standalone `ccu`/`cr`/
  `claude-permit` shims), but `efficiency` never had one — it matches
  `session`/`sessions`/`common`, the genuinely clyde-native crates in this
  workspace, none of which carry `build.rs`.
- `efficiency::run` does **not** install its own logger (no `log_file_path`/
  `setup_logging` duplicated from the sibling crates). `clyde/src/main.rs`
  already initializes one shared logger for every clyde-native arm
  (`setup_logging(&level, &log_path)`) before the `Command` match runs;
  `report`/`cost`/`permit` are the ONLY arms excluded from that
  (`matches!(cli.command, Command::Report(_) | Command::Cost(_) |
  Command::Permit(_))`, `clyde/src/main.rs:102`) because they are absorbed
  legacy shims that must stay behavior-exact with their pre-merge standalone
  logging. `efficiency` has no such legacy behavior to preserve, so it is
  deliberately **not** added to that skip-list — it reuses clyde's logger
  like `Bootstrap`/`Doctor`/the `Sessions` subtree do. This is documented
  inline at both call sites (`efficiency/src/lib.rs`, `clyde/src/main.rs`).
- `EfficiencyArgs` (`efficiency/src/cli.rs`) is an empty `#[derive(Args)]`
  struct for this phase — no subcommands or flags. The design doc's
  `session <id>` / `daily` / `weekly` / `--worst` / `--json` surface is
  explicitly Phase 5 (Output surfaces), not this phase.
- `Command::Efficiency(efficiency::EfficiencyArgs)` added to the `Command`
  enum in `clyde/src/cli.rs` (grouped with the other tool-shaped variants,
  right after `Permit`), and the dispatch arm
  `Command::Efficiency(args) => dispatch_tool(efficiency::run(args, globals), debug)`
  added to `clyde/src/main.rs`'s `run()` match (right after the `Permit` arm).
  Workspace `Cargo.toml:2` members list and `clyde/Cargo.toml` gained the new
  path dependency, both hand-edited to match the existing alphabetical/lint
  style (no external crate versions to fetch via `cargo add` — the only new
  deps are `clap`/`common`/`eyre`/`log`, all already pinned at the workspace
  level or path-local).

### Deviations

- The design doc's own Architecture section (line ~90) names the wiring
  check as "the `Report|Cost|Permit` special-case at `clyde/src/main.rs:102`
  (help_target)" as if it were one thing. It is actually two distinct
  mechanisms at different locations: (1) the own-logging skip-list at
  `clyde/src/main.rs:102` (a `matches!` on `cli.command`), and (2) the
  REQUIRED-TOOLS `--help` special-case (`cli::help_target`, defined in
  `clyde/src/cli.rs:290-321`, invoked in `clyde/src/main.rs:51-74`). Both
  were checked explicitly: (1) resolved as "do not add" per the design
  decision above; (2) does not need an entry either, since `efficiency` in
  Phase 1 shells out to no external binary and has no subcommand-specific
  `--help` block to attach. Same effect as the doc intended (both special
  cases audited), correct seam identified for each.

### Tradeoffs

- Reusing clyde's shared logger (vs. giving `efficiency` its own
  `log_file_path`/`setup_logging` copy like the three absorbed tools) trades
  away symmetry with `report`/`cost`/`permit`'s file layout for avoiding a
  needless duplicate `env_logger::Builder::init()` call, which would panic
  ("attempted to set a logger after the logging system was already
  initialized") the moment `efficiency` was NOT added to the main.rs
  skip-list. Since there is no legacy-shim reason to own a separate logger,
  simpler and correct beats sibling-mirroring here.

### Open questions

None. Phase 1 is a pure scaffold with no design ambiguity: `clyde efficiency`
runs and exits 0, `otto ci` is green, and every choice above follows directly
from "match the sibling crates' idioms" plus "`efficiency` is clyde-native,
not absorbed."
