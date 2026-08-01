# Implementation notes: repo attribution and the routing gate

Companion to `docs/design/2026-07-31-attribution-and-routing.md`. Append-only, one section per
phase. A later decision that overrides an earlier one gets a NEW entry saying so; nothing here is
rewritten.

Every "Deviations" entry carries the command output that justified it. The design doc's own standing
rule applies to this file too: the command you ran is the only claim you own.

## Phase 1: Make the harness able to see a sequence

### Design decisions

- **The checkout matrix lives in `common`, behind a `testkit` cargo feature**
  (`common/src/checkout.rs`, `common/Cargo.toml`). The design says "one shared fixture" that Phases
  1, 3, 6 and 7 all assert against, and those phases span four crates (`common`, `session`,
  `sessions`, `efficiency`) plus `clyde`'s integration tests. A `#[cfg(test)]` module in any one
  crate is unreachable from the others, so the fixture had to be a real (feature-gated) module.
  `tempfile` was already a normal dependency of `common`, so the feature adds no dependency, only the
  module. Consumers take it as a DEV-dependency
  (`common = { path = "../common", features = ["testkit"] }`), so it never reaches a release binary.
- **Row assertions are split by altitude** (`clyde/tests/matrix.rs`). Rule-level rows call
  `common::repo` directly; anything that needs the catalog or the routing gate drives the SHIPPED
  binary through `session reindex` / `session enrich --dry-run`. Both halves use the same fixture, so
  a new shape is still added in one place.
- **`Sandbox` sets `projects-dir` ONLY in `clyde.yml` and leaves the platform default empty**
  (`clyde/tests/matrix.rs`). That asymmetry is what makes AC7 bite per command path: a command that
  reads config finds the seeded session, a command that jumps to `~/.claude/projects` finds zero.
  A grep count cannot distinguish those.
- **`attribution()` reads `repo_source` from SQLite, not from `session export`.** `ExportRecord`
  carries `repo` but not `repo_source` (`sessions/src/export.rs:133`), and provenance is precisely
  what the matrix is about: "resolved" and "resolved by rule 1 rather than by a rule-4 guess" are
  different claims. Read-only, and only after the writing command has exited.
- **`Config::configured_projects_dir()` was added** (`common/src/config.rs`) because
  `Config::projects_dir()` folds the config value and the platform default into one `PathBuf`, so a
  caller needing config STRICTLY between a flag and the default cannot tell "operator set this" from
  "nobody set anything". Collapsing those two is how `cmd_reindex` came to skip config while
  `mcp serve` honored it.
- **The container-root fixture commits in the CHILD, never at the root**
  (`common/src/checkout.rs`). `git commit` at a container root fails with
  `fatal: this operation must be run in a work tree`, which is the very condition that makes row 6 a
  matrix row. Found by the fixture failing to build.

### Deviations

- **None from the phase's specified work.** Two additions the phase did not name are recorded under
  Tradeoffs and Open questions rather than as deviations, because neither changes what Phase 1 was
  asked to deliver.

### Tradeoffs

- **`common/src/checkout/tests.rs` pins raw `git` behavior, which the design did not ask for.** The
  alternative was to assert git's behavior only indirectly, through clyde's rules. Pinning it
  directly means a git upgrade that changes `--git-common-dir`'s answer for a plain bare repo fails
  CI next to the measurement, rather than three phases downstream as an unexplained attribution
  regression. Ten assertions, all OBSERVED before they were written.
- **A containment-decline shape was added that the design does not list.** Measured 2026-07-31: every
  in-tree cwd resolves to an ANCESTOR by construction, because git's discovery walks UP, so the
  containment check cannot be made to bite by any directory shape alone. The only environment-free
  shape that triggers it is a `.git` FILE pointing at a bare repo in a SIBLING tree
  (`Matrix::outside_root`). This matters for Phase 6, which is told to add
  `detect_declines_a_repo_found_above_the_cwd`; see the open question below.
- **`no_phase_one_assertion_for_the_known_broken_rows` asserts today's WRONG answers.** Asserting a
  defect reads badly in isolation, but the alternative is silence, and silence is what let the
  register's rows go unnoticed. Each assertion names the phase that must break it, so the fix has
  something to fail against.

### Open questions

- **Row 8's label and the test name Phase 6 prescribes for it describe different behaviors.** The
  matrix calls row 8 "cwd inside `.bare/refs`", Phase 6 names its test
  `detect_declines_a_repo_found_above_the_cwd`. Measured against git 2.53.0:

  ```
  $ git -C <container>/.bare/refs rev-parse --git-common-dir
  <container>/.bare
  ```

  so the computed root is `<container>`, the cwd IS under it, containment PASSES, and the row
  resolves to `tatari-tv/airflow-dags`. That is the correct answer (the session was inside the
  container), but it is not a decline. Phase 6 will implement BOTH: row 8 asserting the measured
  resolve, and `detect_declines_a_repo_found_above_the_cwd` against `Matrix::outside_root`, which
  genuinely fails containment. Flagged here rather than silently picking one.

### Measured

**Problem 1 reproduced in the harness, before Phase 2 fixes it** (the phase's stated criterion).
Session seeded at `Matrix::no_origin`, decisions read from `session enrich --dry-run`:

```
before: ("2222...2222", "personal", would_send=false)
$ git remote add origin git@github.com:tatari-tv/side-project.git
after:  ("2222...2222", "work",     would_send=true)
```

**The repro requires the PROVISIONAL path, and the first attempt did not.** Driving the sequence
through explicit `clyde session reindex` produced `before: personal/false` and then `after: None`:
the row had dropped out of the candidate set entirely. The reason is the limit the design already
states: an explicit reindex runs `reindex_efficiency`, which writes `outcome_json`, so
`ScopeEvidence::present` is true, the decision is SETTLED at `scope_version = 2`, and
`enrich_candidates` (`sessions/src/db/enrich.rs:273-275`) excludes it. Driving it through
`session enrich` alone refreshes via `lazy_reindex`, which never runs `reindex_efficiency`, so
`outcome_json` stays NULL, `scope_version` stays NULL, and the row is re-offered and re-decided.

This is a confirmation of the design's "What limits it" paragraph rather than a correction to it,
and it CONSTRAINS Phase 2's sequence test: a test that reindexes explicitly between the two
classifications cannot see Problem 1 at all, and would pass against unfixed code. Phase 2's
`scope_never_upgrades_personal_to_work_on_a_later_probe` must drive the provisional path.

**AC7 baseline, re-executed on this branch**: all three command paths now read a `projects-dir` set
only in `clyde.yml`, asserted per path by `matrix_reindex_reads_a_projects_dir_set_only_in_config`,
`matrix_enrich_reads_a_projects_dir_set_only_in_config`, and
`matrix_mcp_serve_reads_a_projects_dir_set_only_in_config`.
