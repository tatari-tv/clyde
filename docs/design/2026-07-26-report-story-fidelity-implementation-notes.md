## Phase 0: Spike the real outcome vocabulary and size the attribution recovery

### Design decisions
- Measured the rule-3 ceiling against the EXACT tool-call shape `efficiency::outcome::union` already
  extracts (`Edit`/`Write` only, `input.file_path`, confirmed by a non-error `tool_result`) rather
  than a looser scan of every edit-shaped tool -- `efficiency/src/outcome.rs:318-329` (`classify_tool`)
  is the ground truth for what Phase 3 will actually build `repos_touched` from, so the ceiling had
  to be measured against that exact filter, not an approximation.
- Reproduced the doc's own 562 / 283 / 279 session-and-dollar figures live before measuring anything
  new, to confirm the window (`--since 2026-06-26 --until 2026-07-25`) and the `$HOME`-or-temp-dir
  split are exactly reproducible against today's catalog, not drifted since the doc was authored.

### Deviations
- None. Zero code changed, per the phase's own constraint.

### Tradeoffs
- A naive scan (also counting `MultiEdit`/`NotebookEdit`, and not filtering on `tool_result`
  confirmation) gives 76 unique-argmax sessions instead of 73, and 83 touched-at-least-one instead
  of 80. Reported only the code-matching numbers in Resolved Decisions since those are what Phase 3
  will actually produce; the wider scan would overstate the ceiling Phase 3 is held to.

### Open questions
None.
