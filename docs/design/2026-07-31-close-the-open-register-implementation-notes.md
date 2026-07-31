# Implementation Notes: Close the Open Register

Running record of decisions, deviations, tradeoffs, and open questions while executing
`docs/design/2026-07-31-close-the-open-register.md`. Append-only: a later entry supersedes an earlier
one rather than rewriting it.

## Phase 1: Widen both lints and kill the fail-open (E + F)

### Design decisions

- **The widened `_variable` lint got the em-dash lint's full comment treatment, not just its status
  shape** (`.otto.yml`, `lint` task). The plan said "copy the explicit status shape verbatim from
  `:42-61`"; the copy also carries a scope rationale and the drop-guard note the plan asked for, so a
  reader of either block finds the same three facts (why the scope is the whole tree, why the status
  is checked explicitly, what to do when the pattern is wrong). Two lints that behave identically now
  read identically.
- **The em-dash lint's user-facing strings were retitled, not left alone** (`.otto.yml`, `lint`
  task). Adding `--include='*.pmt'` made `=== Deny em dash in Rust source ===`, `❌ Found em dash in
  Rust source.`, and `✅ No em dashes in Rust source` all understate what the scan covers. They now
  say "Rust source and slot templates" / "Rust source or slot template". House rule: names tell the
  truth. AC1's recorded pre-state quotes the old success line, which is a pre-state observation, not
  a post-condition.
- **`var_status` is the new variable name**, mirroring `em_status`, so the two `case` blocks are
  symmetric.

### Deviations

- None. All five plan bullets landed as specified: `.` plus `--exclude-dir=target` on the
  `_variable` scan, the explicit status `case`, `--include='*.pmt'` on the em-dash scan, the
  `report/src/render/slots/tests.rs:531` assertion left in place, and the drop-guard comment.

### Tradeoffs

- **`--include='*.pmt'` on the existing em-dash `grep` vs. a second scan for templates.** One `grep`
  with two `--include` filters keeps one status `case` and one pass over the tree; a separate scan
  would need its own status block and could drift from the first. Cost: the failure message cannot
  name which of the two file kinds matched, so it names both. `grep -rn` prints the offending path on
  the line above, so the operator is not actually left guessing.
- **Widening the `_variable` scan to `.` vs. enumerating `*/src/ */tests/ */build.rs`.** The
  enumeration is narrower and would need editing every time a crate grows a new top-level target;
  `.` plus `--exclude-dir=target` is what the em-dash lint already does and needs no maintenance.
  Measured zero new violations either way.

### Open questions

- None.

### Success criteria, executed

- `otto lint` exits **0** unchanged, printing `✅ No _variable patterns found` and `✅ No em dashes
  in Rust source or slot templates`.
- Planting `fn _plant() { let _foo = 1; }` in `clyde/tests/collect.rs` makes `otto lint` exit **1**
  with `❌ Found _variable binding pattern.`; `git checkout` of that file restores **0**. This is the
  half the old `*/src/` scope could not see.
- Planting an em dash in `report/templates/slots/closing.pmt` makes `otto lint` exit **1** with
  `❌ Found em dash in Rust source or slot template.`; restoring the file returns **0**.
- Full `otto ci` exits **0**.
