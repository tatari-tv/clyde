# Implementation Notes: Month over Month deltas, computed

Design doc: `docs/design/2026-07-27-month-over-month-deltas.md`

## Phase 0: Prove the rejected tokens are comparison figures

### Design decisions

- Established the per-render `--prior` configuration from the exact shell commands in the shipit
  subagent's transcript
  (`~/.claude/projects/-home-saidler-repos-tatari-tv-clyde/6dbe3155-f40f-46d3-9b1c-52a760d99246/subagents/agent-ashipit-d399c59e432f168f.jsonl`),
  not from this doc's own summary table, because the log never records `--prior` (confirmed by
  reading `render.rs:41` and `lib.rs:150`: the field isn't in the INFO line, and the two `debug!`
  lines that would carry it never fired because all nine renders ran at the default `Info` filter).
  Ten literal `clyde report render -i ...` commands exist in the window; the first is the
  already-discarded warm-up, and the remaining nine map to the nine `render::run` log lines one for
  one by count and chronological order.
- Classified each of the five prose-guard rejections from its token and its configuration, not its
  excerpt, per the phase's own instruction, and confirmed why: `excerpt()` (`render.rs:474-490`)
  scans the WHOLE document for the first occurrence of the literal substring, not the flagged
  sentence, and the `prose` it's called on (`render.rs:315,359-360`) is the entire markdown body or
  HTML visible text.

### Deviations

- The design doc's own Problem Statement table (5 no-`--prior` / 4 with-`--prior`, 40% / 75% /
  100% rejected) does not match the per-render `--prior` configuration recovered from the exact
  shell commands (4 no-`--prior` / 5 with-`--prior`, and the with-`--prior` rejection rate is 80%,
  not 75%). Corrected in a new Resolved Decision rather than silently used; see the doc for the full
  per-render table.
- Render #1 in this doc's nine is not a rejection of the prose/claim guard this design is about at
  all. It is a separate, correctly-firing reconcile-identity guard, rejecting a deliberately
  corrupted `--reconcile` export before the render reached the LLM. Recorded as its own class rather
  than folded into "no-`--prior` rejections."

### Tradeoffs

- None. Zero code this phase; the only tradeoff was which evidence source to trust for the
  `--prior` configuration (chosen: the exact commands actually run, over inferring from this doc's
  own summary table, per the phase's explicit instruction not to treat the doc's input as
  independent confirmation of itself).

### Open questions

- None from this phase. The open question the STOP creates is for Scott and the team lead: whether
  to revise this design's root-cause section against the corrected evidence (round-number/threshold
  invention in the current-period narrative, not a missing subtraction) before Phase 1 proceeds, or
  to treat the `--prior`-specific fix as still worth shipping on its own independent merits (the
  template contradiction in Resolved Decisions and the `excerpt`/persistence fixes in Phases 1-2 are
  unaffected by this finding) while opening a new doc for the round-number/threshold defect.

## STOP: Phase 0 result

**The phase's own success criteria required at least two of the three (turned out to be four)
`--prior` rejections to be Month over Month comparison figures. Zero of the four are.** One is
confirmed by content ("also running above 100 sessions," a threshold about the current period,
explicitly named in this doc's own Problem Statement). Two share its exact token. One shares its
exact token and shape with a no-`--prior` rejection where a comparison fabrication is mechanically
impossible. Per the phase's STOP condition: this plan's root cause does not hold against the
evidence as it stands. Phases 1 through 6 were not started.
