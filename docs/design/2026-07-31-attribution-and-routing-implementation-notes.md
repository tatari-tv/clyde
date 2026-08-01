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

## Phase 2: Close the retro-flip and the permanent lockout

### Design decisions

- **`ProbeOutcome` is returned by `detect_with_blocked_roots`; `detect` and `Resolver::detect` keep
  their slug-only `Option<String>`** (`common/src/repo.rs`). Callers that only ever wanted the
  attribution are unchanged; only the routing gate reads the typed outcome, through the new
  memoized `Resolver::probe`.
- **`GitRun` (three outcomes) replaces `run_git`'s `Option<String>`** (`common/src/repo.rs`). The
  arms depend on telling `rc=1` (git answered, no key: EVIDENCE) from any other failure (git did not
  answer: not evidence), and an `Option` erases exactly that.
- **`ORIGIN_ARGS` is a named const** shared by `read_origin` and the test that pins the read's scope.
  Spelled inline, a test could keep passing after someone dropped `--local` from the real code.
- **`Decision` gained `settled`, and `Basis::reads_stored_evidence` was removed**
  (`session/src/scope.rs`). The caller used to re-derive settledness as
  `!basis.reads_stored_evidence() || evidence.present`, which cannot express v3: a git-origin
  decision reads no stored evidence at all, yet a git-origin PERSONAL one must stay revisable.
- **`Basis` gained `Override` and `ProbeRefused`.** `ProbeRefused` is its own variant rather than a
  `GitOrigin` personal because Phase 8 must count them separately: "the remote says personal" and
  "clyde refused to trust the remote" are different operator problems.
- **`RoutingFacts` is a struct with a `Default`**, not four more positional parameters. A caller with
  none of it writes `&RoutingFacts::default()`, which reads as "no override, no recorded negative, no
  stored evidence" rather than as three bare `None`s counted out against the signature.
- **`Db::record_probe` enforces `is_conclusive_negative` itself** rather than trusting callers
  (`sessions/src/db/routing.rs`). The guard is one line and the failure mode it prevents is a
  permanent lockout, so it belongs at the write, not at each call site.
- **`record_probe` never overwrites an existing stamp** (`WHERE repo_probe IS NULL`). The FIRST
  failed observation is the evidence, and an unconditional UPDATE on a column touched for every
  session on every reindex pass would fire the v5 revision trigger forever.
- **`sessions/src/db/routing.rs` is its own file**, not an addition to `db/repo.rs`. Those columns
  answer "may this leave the machine"; `repo`/`repo_source` answer "what repo was this in". The
  design's central argument is that conflating the two is the defect.
- **`EvidenceRow` is a named struct** in `db/enrich.rs`. The four-`Option<String>` tuple clippy
  flagged is also a correctness hazard: any two could be swapped at the destructuring site and still
  compile, silently feeding the probe stamp to the host check.

### Deviations

- **`ProbeOutcome::Resolved` carries `slug` only, not `{ slug, host }` as the design's enum shows.**
  The host cannot be populated without Phase 3's `parse_slug -> RemoteSlug` change, and a field that
  is always `None` is precisely the lie the typed arms exist to stop telling. Phase 3 widens it. The
  v13 `repo_host` COLUMN still lands here, as specified.
- **The strip-only pre-v13 `repo_host` rule moved from Phase 2 to Phase 3.** Its input does not exist
  until Phase 3 populates `repo_host`, so implementing it here would be dead code, which
  `dead_code = "deny"` rejects outright. Splitting one decision across two commits is also worse for
  review than landing the rule beside the data it reads.
- **The register item 5 disagreement `warn!` was NOT added here.** It is Phase 4's work; it was
  drafted in this phase by mistake and reverted to keep the phase scoped.
- **`no_work_tree` distinguishes `NotARepo` from `Indeterminate` by a FILESYSTEM check, not by git's
  exit code.** The design's arm table has no row that separates "not a repository" from "a repository
  git could not read", and measurement shows the exit code cannot:

  ```
  plain directory            rev-parse --show-toplevel  rc=128  fatal: not a git repository
  unreadable .git/config     rev-parse --show-toplevel  rc=128  warning: unable to access '.git/config'
  .git file -> vanished dir  rev-parse --show-toplevel  rc=128  fatal: not a git repository: /nonexistent/gone
  ```

  All three are rc=128 from both probes. `has_git_marker` walks `cwd.ancestors()` for a `.git` entry,
  which answers the exact question the arm claims ("not a work tree AND NOT A GIT DIR"). It is
  deliberately more generous than git's own discovery, and every disagreement goes the safe way: a
  marker git would not have used yields `Indeterminate`, which records nothing. Matrix rows 23 and 29
  exist because of this; both stamped `not-a-repo` before the fix.

- **A cwd with a git dir but NO WORK TREE is `Indeterminate` in this phase, not `NotARepo`.** Not in
  the design, and it is a cross-phase safety requirement: that shape is exactly what Phase 6 teaches
  rule 1 to resolve (a bare-repo container root, a plain bare mirror). Stamping it conclusively NOW
  would refuse work scope for every such session even after Phase 6 made it resolvable, because a
  stamp is never cleared by a later success.

### Tradeoffs

- **AC11's deletion pairings are wrong for two of three rows, and the tests were built to bite at the
  layer each defense actually owns instead.** See "Measured" below for the full matrix. Briefly: with
  `env_clear` in place, deleting `--local` cannot break a test routed through the module, because the
  child never sees a `GIT_CONFIG_*` variable or `HOME`. So `--local` is pinned against `ORIGIN_ARGS`
  directly (the scope property it owns), and the primitive change is pinned by an `insteadOf` rule in
  the repo's OWN config, where no environment scrub can reach. This is the same class of error the
  panel's finding 33 caught once already; AC11 needs the same correction.
- **Test-helper `git` invocations across the tree now `env_clear`.** Three tests mutate `GIT_DIR` /
  `GIT_CONFIG_*` / `HOME` process-wide, cargo runs a binary's tests concurrently, and `ENV_LOCK` only
  serializes the tests that take it. Without the scrub a leaked `GIT_DIR` broke `git init` in
  `common::checkout` and surfaced as eight unrelated fixture failures.
- **`a_git_origin_attribution_settles_the_decision_with_no_reindex` was split in two.** Its personal
  half asserted the exact behavior Problem 3 forbids. The work half kept its intent and name;
  `a_personal_git_origin_decision_is_re_offered_rather_than_excluded` asserts the new one.

### Open questions

- **A fourth forgery channel exists that the design does not name**, found while building the
  deletion proofs: an `insteadOf` rule in the repo's OWN `--local` config rewrites a personal origin
  into a work one with no environment variable and no hostile `~/.gitconfig`. Measured:

  ```
  $ git config --local 'url.git@github.com:tatari-tv/.insteadOf' 'git@github.com:scottidler/'
  $ env -i PATH=... git remote get-url origin       -> git@github.com:tatari-tv/sideproject.git
  $ env -i PATH=... git config --local --get ...    -> git@github.com:scottidler/sideproject.git
  ```

  It is CLOSED by the primitive change (`git config`, not `git remote get-url`), so nothing is
  outstanding in the code. It is worth adding to the design's vector table, because it is the only
  vector where the primitive change is the sole defense, and the table currently credits `--local`
  and `env_clear` with covering the `insteadOf` row.

### Measured

**The defense matrix, executed 2026-07-31.** Each row is one deletion; the cells name which test
fails. This replaces AC11's pairing, which measurement shows is not achievable as written.

| deletion | which test fails |
|---|---|
| `env_clear()` from `run_git` | `git_dir_in_the_environment_cannot_forge_an_attribution` |
| `--local` from `ORIGIN_ARGS` | `the_origin_primitive_reads_only_the_repos_own_config` |
| `git config` reverted to `git remote get-url` | `the_origin_primitive_does_not_apply_insteadof_rewriting`, and two more |
| forward `HOME` (with `--local` intact) | **nothing**, exactly as the panel's finding 33 predicted |
| forward `HOME` AND drop `--local` | `a_hostile_home_gitconfig_cannot_forge_an_attribution` |

Every single-defense deletion is caught by exactly one test, and no defense is redundant.

**AC1 and AC2 bite, verified by deletion:**

- delete the `facts.repo_probe` branch from the git-origin arm ->
  `scope_never_upgrades_personal_to_work_on_a_later_probe` FAILED, and
  `clearing_the_probe_record_recovers_a_refused_session` FAILED
- replace `record_probe`'s `is_conclusive_negative` guard with `false` ->
  `a_transient_git_failure_never_stamps` FAILED

**AC9, the v12 -> v13 migration on a COPY of the live desk.lan catalog (2,150 rows):**

```
before: user_version=12   personal 1043 | work 864 | (null) 243
after:  user_version=13   personal 1043 | work 864 | (null) 243
row-level scope diff: ZERO rows changed
repo_probe non-null: 0        scope_override non-null: 0      (no backfill, as designed)
all six v13 columns present; sessions.db.pre-v13.bak written
```

**The routing-decision delta on the same catalog**, `session enrich --dry-run`, installed v0.22.0
versus this branch, each against its own copy:

```
v0.22.0:      considered 1029   would-enrich 14   (personal,false) 1014  (work,false) 1  (work,true) 14
this branch:  considered 1029   would-enrich 14   (personal,false) 1014  (work,false) 1  (work,true) 14
rows whose (scope, would-send) CHANGED: 0
```

**Phase 2 in isolation changes no row's scope on the live catalog**, which is what the design claims
and what the release-level caveat (Phase 3's host gate ships with it) is measured against separately.

25 rows took a `not-a-repo` conclusive stamp on that copy and 0 took `no-origin`; none of the 25
carries a git-origin work slug, which is why no decision moved.

Also observed, and worth carrying into the PR body: `scope_version` on the live catalog is NULL on
1,113 rows and `1` on 1,037, with ZERO rows at 2. Enrich has not run since v2 landed, so the entire
catalog is re-offerable and the v3 bump changes nothing about which rows are eligible.

**Operator surfaces shaken down against the catalog copy:** `session scope --list` (empty and
populated), `--set` refused without `--reason`, `--set work` warning by name about the conclusive
negative it overrides, `--clear`, two-modes-at-once refused, `--clear-probe` refused without
`--session`, and `--clear-probe --session <id>` clearing then re-observing in the same command.

### Out of scope, observed

`cost::cache::tests::test_save_and_load_cached_day` failed ONCE during a full-workspace run and has
passed on nine subsequent runs (isolated and workspace). The `cost` cache tests all read and write
the REAL `~/.cache/clyde/cost/` directory and clean up with `remove_file`, so concurrent tests in
that binary share process-global state; that is the most likely mechanism, but I did not reproduce
it and will not claim the exact interleaving. Unrelated to this design doc, in a crate it does not
touch. Flagged for a follow-up rather than fixed here.
