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

## Phase 3: Validate the remote's host

### Design decisions

- **`HostResolver` is an injected PORT** (`common/src/repo/host.rs`), generic not `dyn`, per the house
  DI rule. It exists for a reason specific to this module: the production resolver reads the invoking
  user's real `~/.ssh/config`, which a test cannot control. Without the seam, `HostPolicy`'s logic
  could only be exercised on hosts that happen to have no alias configured, which is not a test of
  alias handling at all.
- **The host gate lives in the CALLER, not the classifier.** `RoutingFacts::host_confers_work` is an
  already-resolved `Option<bool>`. Resolution spawns `ssh -G`, and `session::scope` is a pure
  function the routing gate has to be reasonable about against a fixed input; a classifier that
  shells out cannot be unit-tested that way.
- **`work_remote_hosts` rides in `EnrichOptions`, not as a fifth `enrich()` parameter.** It IS sweep
  configuration, like `max_attempts` and `token_budget`, and a `Default` resolving to
  `["github.com"]` keeps every existing caller CORRECT rather than merely compiling.
- **An EMPTY `work-remote-hosts` is rejected at load** (`common/src/config.rs`). It fails closed
  either way, but silently: an empty list confers work from nothing at all and returns every teammate
  to 0% coverage, which is far more likely a half-finished edit than an intention.
- **`Basis::HostRefused` is separate from `Basis::ProbeRefused`.** The two have DIFFERENT remedies: a
  probe refusal is cleared with `session reindex --clear-probe`, a host refusal is fixed by adding the
  host to `work-remote-hosts` or is a genuine attack. Phase 8 counts them separately for that reason.
- **`record_repo_host` DOES overwrite; `record_probe` does not.** The host is a property of the
  CURRENT remote, so a repo genuinely re-pointed must read as the new host. The probe record is a
  historical observation and erasing it is the leak.
- **An IPv6 authority is declined rather than parsed.** clyde has never seen one in a git remote, and
  a wrong answer here confers work scope, so the fail-closed direction is to refuse.

### Deviations

- **`ssh` is invoked WITHOUT `HOME`, and the design's implicit assumption that forwarding it matters
  is wrong.** Measured 2026-07-31 under `env -i` with `HOME` pointed at a temp dir holding a
  `Host github-work` block:

  ```
  $ env -i PATH=/usr/bin:/bin HOME=$FAKE ssh -G github-work | grep -E 'hostname|knownhosts'
  hostname github-work
  userknownhostsfile /home/saidler/.ssh/known_hosts /home/saidler/.ssh/known_hosts2
  ```

  The alias did NOT resolve and ssh read the REAL user's `known_hosts`: it resolves the home
  directory from the passwd database, not from `$HOME`. Forwarding `HOME` therefore buys nothing and
  would wrongly imply to a reader that the operator's config is reachable through the environment.
  The useful half of the same measurement is that alias resolution WORKS under a scrubbed
  environment with `PATH` alone, which is what production needs.

- **Matrix rows 19 and 20 were MOVED off-layout in the fixture**, from
  `<repo-root>/tatari-tv/<repo>` to `<home>/code/work/<repo>`. Under the work org the cwd anchor
  places the session as work on its own and the host gate is never consulted, so the test asserted
  nothing about Problem 2. Found by row 19 failing with `("work", true)` for a legitimate reason.
  Off-layout, the remote is the only signal, which is the teammate situation the git-origin branch
  was built for and therefore the one the host gate has to defend.

- **Row 20 asserts the alias is RECORDED verbatim, not that it resolves.** `ssh -G` reads the
  invoking user's real config, which a test must not depend on and cannot safely modify. The
  resolution logic is asserted against an injected resolver in `common::repo::host::tests`. Both
  halves are needed: without the matrix half, nothing proves the alias survives the trip from git to
  the catalog un-mangled.

- **The blast radius is SMALLER than the design states.** The doc says "every call site is updated in
  the same commit: `session/src/scope.rs`, `efficiency/src/outcome.rs`, `common/src/repo.rs`'s own
  rule 1, and `common/src/repo/tests.rs`". Measured with the command the doc itself prescribes:

  ```
  $ rg -n 'parse_slug' --type rust -g '!target'
  common/src/repo.rs:433   (rule 1, the one production call site)
  common/src/repo.rs:690   (the definition)
  common/src/repo/tests.rs  (five test call sites)
  ```

  `session/src/scope.rs` and `efficiency/src/outcome.rs` do not call `parse_slug` at all; they consume
  a slug the catalog already stored. Nothing was missed, and the phase is independently committable
  as the design intended.

- **Register item 11's stash was NOT dropped, and this is the one thing left for the owner.** The
  stash exists (`stash@{0}: On unblock-teammate-reports: scope trust-boundary doc + test`) and
  dropping it destroys work that is on no branch. The design's stated PURPOSE ("recorded so it is not
  rediscovered and applied") is met by the record itself. To finish it:
  `git stash drop stash@{0}`.

### Tradeoffs

- **`HostPolicy` short-circuits on a literal match before resolving.** That makes the overwhelmingly
  common case (`github.com`) spawn nothing, and it means an operator who lists an ALIAS in
  `work-remote-hosts` gets a literal match rather than a resolution. Both readings are correct and
  the cheap one wins.
- **The failure is memoized too.** A catalog with 2,000 sessions behind one unresolvable alias spawns
  `ssh` once, not 2,000 times. Asserted rather than assumed
  (`resolution_is_memoized_per_host_including_failures`).

### Open questions

- None.

### Measured

**Row 19 and row 27 both bite, verified by deletion:**

- delete the `host_confers_work` branch from `session::scope` ->
  `matrix_row_19_a_non_allowlisted_host_attributes_but_confers_no_work_scope` FAILED
- treat a NULL `repo_host` as `Some(false)` (the strip-only violation) ->
  `matrix_row_27_a_pre_v13_row_with_no_recorded_host_keeps_its_work_authority` FAILED

Row 27 did NOT bite on the first attempt, and the reason is recorded because it generalizes: with the
checkout still on disk, `enrich`'s own `lazy_reindex` re-probes the cwd and re-populates `repo_host`
before the classifier runs, so the NULL never reaches the gate. Deleting the checkout first makes the
row a genuine pre-v13 shape. Any future test that nulls a column the reindex writes has the same
trap.

**AC9 at the RELEASE level** (Phase 2 plus Phase 3, which is what actually ships), `enrich --dry-run`
against a copy of the live 2,150-row desk.lan catalog, versus installed v0.22.0:

```
v0.22.0:      considered 1029  would-enrich 14  (personal,false) 1014  (work,false) 1  (work,true) 14
this branch:  considered 1029  would-enrich 14  (personal,false) 1014  (work,false) 1  (work,true) 14
rows whose (scope, would-send) CHANGED: 0
```

**Zero, and EXPLAINED, which is the criterion the design sets rather than zero for its own sake:**

```
recorded hosts after the pass:  github.com 1230 | (null) 929
hosts outside the allowlist:    none
rows with a work slug and a NULL host (the strip-only population): 351
```

Both populations the Data Model names are present and neither moved. There is no non-allowlisted host
on this catalog, so nothing is stripped; the 351 NULL-host work rows keep pre-v13 authority by design.
A machine with a non-github remote WOULD see a change, and this catalog cannot produce that number.
Recorded as a known-null result on the only catalog available, not as "no impact".

An earlier run of this comparison reported "1029 rows changed". That was a defect in the comparison
script (a JSON list compared against a Python tuple), not in the code; the aggregate counts were
identical on both sides, which is what caught it.

## Phase 4: The honesty batch (register items 4, 5, 6)

### Design decisions

- **The hand-built-row helper is replaced by TWO helpers, not one** (`session/src/scope/tests.rs`).
  `classify_at` drives the REAL rule-1 resolver over a matrix checkout and derives `repo_source` from
  whether the probe resolved, so a test cannot claim git-origin provenance for a cwd the resolver
  declined. `classify_stored` is the honest name for the remaining legitimate hand-built cases: a
  CORRUPT stored slug, and a `repo_source` that rules 2 through 4 write and rule 1 never does. Both
  are real states the classifier must survive; naming them differently is what keeps the distinction
  visible at every call site instead of buried in one helper that looks the same for both.
- **`anchor_disagrees_with_remote` returns a typed `Disagreement { anchor, remote }`**, not a bool.
  The two directions mean different things to an operator: a work anchor with a personal remote is
  usually a fork, a personal anchor with a work remote is usually a misfiled clone. A bool would make
  Phase 8's count unactionable.
- **A minimal `log::Log` capture was added to `sessions`' tests.** Items 5 and 6 are pure DISCLOSURE:
  item 6's fix produces the same `None` the swallowing `.ok()` did, so the warning IS the entire
  observable difference. Without a captured log, "deleting the `warn!` fails a test" is not
  satisfiable, and the register's exact complaint (a loud error silently discarded) would come back
  with no test to stop it. Tests share the buffer and filter by a needle unique to their own fixture.
- **`Db::set_raw_repo_source_for_test` is `#[cfg(test)]`.** It writes a `repo_source` no production
  writer can produce, which is precisely the row item 6 is about reading loudly. Gating it means it
  cannot be reached from production at all.

### Deviations

- **AC5's grep tripped on a DOC COMMENT, not on a live helper.** After the conversion the only
  remaining occurrence was a sentence explaining what had been removed and why. The comment was
  reworded to cite the file:line the helper lived at (more precise than the identifier anyway) so
  `rg -c 'with_repo' session/src/scope/tests.rs` exits 1 as the criterion requires. Recorded because
  it is the criterion being satisfied by an edit to prose rather than to code, and a reader should
  know that.
- **One matrix row was ADDED that the design does not list**: `work_remote_in_personal_dir`, a work
  remote checked out under a personal org directory. `cwd_anchor_outranks_the_remote_in_both
  _directions` is the test the design says must STAY, and converting it off hand-built rows needs a
  real checkout for each side of "both directions". The fork row covers one side; nothing covered the
  other.

### Tradeoffs

- **`git_origin_classifies_every_real_world_layout` lost a row and gained a test.** It used to cover
  four layouts including Patrick's bare `~`. Three are now driven from real fixtures; the fourth got
  its own test, `a_home_cwd_stays_personal_because_no_signal_can_place_it`, because its expectation
  is the OPPOSITE of what the old row asserted and burying that in a loop would hide the correction.
- **Item 5 changed comments only, and the code is byte-identical.** That is the whole resolution: the
  code always consulted the cwd anchor first, and the comments called the remote "authoritative",
  which reads as a general claim the code does not make. Inverting the precedence was drafted and
  withdrawn by the panel because it silently drops a personal fork of a work repo in a work
  directory. Keeping the precedence and fixing the words is what item 5 actually asked for.

### Open questions

- None.

### Measured

**Register item 4, the impossible row.** The old assertion was that a cwd of `/home/patrick` with
`repo_source = "git-origin"` classifies WORK. It is replaced by
`a_home_cwd_stays_personal_because_no_signal_can_place_it`, which asserts the true expectation in
both forms: a bare `~` (nothing can place it) and a git-tracked `$HOME` with a work remote (the
blocked-root guard refuses to attribute it, so it can never confer scope). The PR #82 body's claim
that Patrick's layout was fixed is wrong, and the test now says so.

`rg -c 'with_repo' session/src/scope/tests.rs` exits 1 with no output. AC5 satisfied.

**Items 5 and 6 both bite, verified by deletion:**

- restore `.ok()` in place of the `repo_source` match ->
  `an_unreadable_repo_source_warns_instead_of_being_swallowed` FAILED
- delete the `anchor_disagrees_with_remote` block in `enrich` ->
  `a_cwd_anchor_disagreeing_with_the_remote_is_warned_and_still_decides` FAILED

The fork case still classifies WORK through the real gate in both the unit suite
(`cwd_anchor_outranks_the_remote_in_both_directions`, now over a real checkout) and the integration
suite (`matrix_row_18_the_fork_in_a_work_directory_stays_work_through_the_real_gate`).

## Phase 5: Make a non-biting test fail CI

### Design decisions

- **`cargo-mutants` is pinned to 27.1.0 and the task installs it if absent.** A toolchain refresh that
  generates different mutants would change what "zero" means without anyone deciding to. The task
  compares `cargo mutants --version` and installs `--locked` on a mismatch, so a fresh checkout is one
  command away from the same gate.
- **`otto mutants` is NOT a `before:` of `otto ci`, and that was decided by measurement, not by
  guess.** See "Measured" for the numbers.
- **Six paths, not two.** Both reviewers called the earlier draft's two-file scope too narrow, and
  they were right: the surviving mutants landed in four different files across three crates.
- **Zero unannotated survivors, and in the end zero ANNOTATIONS too.** Every survivor turned out to be
  a genuine test gap rather than an unavoidable mutant, so the tree carries no `// mutants:skip` at
  all. That is the better outcome: a skip is reviewable, but no skip is unarguable.

### Deviations

- **`Resolver::blocked_roots()` was DELETED rather than annotated.** Two of the twelve survivors were
  mutations of it, and `rg` showed it has no callers anywhere in the tree. The house rule is that dead
  code is removed, not silenced, and a `mutants:skip` on an unused accessor would have institutionalized
  exactly the thing the phase is against.
- **`--test-workspace true` was added, then REMOVED, and the reasoning for adding it was wrong.**
  I read three runs reporting 12, then 4, then 1 survivors as evidence that per-package test scoping
  made the verdict depend on scheduling. It did not: those were `grep -c MISSED` counts taken from
  runs that DIED before finishing (disk exhaustion, then a killed background job), so they were
  partial logs, not disagreeing verdicts. The first run to emit a summary line reported
  `287 = 15 missed + 144 caught + 128 unviable`, which adds up, and that is the only tally in this
  phase that can be trusted.

  The shipped task uses the DEFAULT per-package scope. That is the stricter discipline rather than a
  weaker one: each mutant is checked by its owning crate's tests, which forces the biting test to
  live with the code. The `scope_override` mutant proves the point (see Measured) and its fix was a
  unit test in `session`, not a wider flag. Dropping the flag also cuts the runtime roughly
  five-fold.

  Recorded at length because the wrong conclusion was load-bearing for a while: **a survivor count
  read from an unfinished run is not a survivor count.**
- **`cost::cache`'s tests were made hermetic, which is outside this design doc entirely.** Justified
  because Phase 5's own task is what makes the pre-existing defect reliably fire: those tests read and
  write the operator's REAL `~/.cache/clyde/cost/`, and `otto mutants` runs many copies of the suite
  at once. Shipping a CI task that makes another suite flaky is shipping a defect. Details under
  "Measured". The fix is a mutex plus `XDG_CACHE_HOME` pointed at a `TempDir`: the mutex alone fixes
  only the in-process collision, and the cross-process one needs the redirect.

### Tradeoffs

- **The gate is a separate task, so it can be skipped.** The alternative was making `otto ci` take
  ten-plus minutes on every commit, which would get the whole pipeline worked around rather than this
  one task. The Rollout Plan already requires `/review-panel` before merge; this joins it as a
  pre-merge step rather than a per-commit one.
- **Tests written to kill a mutant are labelled as such.** Each carries a `KILLS:` line naming the
  exact mutation. A reader can tell a coverage test from a behavior test, and if the mutation operator
  ever changes, the stale label is a signal rather than a mystery.

### Open questions

- None.

### Measured

**Runtime, which is what decided `ci` versus a separate task:**

```
39 mutants (one file),   per-package, --jobs 6:   44 s
287 mutants (six paths), per-package, --jobs 8:   18 min   (the complete run)
287 mutants (six paths), --test-workspace:        > 5x that, and never completed
```

Eighteen minutes is too long for `ci` on every commit, so the task stays out of it and is required
before merge instead. That is the design's stated fallback, chosen on the measurement it asked for.

**Survivors, and how they were driven to zero.** First full run: 12, all in `common/src/repo.rs`.

| survivor | closed by |
|---|---|
| `Resolver::blocked_roots` (x2) | DELETED: no callers anywhere in the tree |
| `home_dir_as_blocked -> vec![]` (x2) | `a_fresh_resolver_blocks_the_real_home_directory` |
| `ProbeOutcome::resolved_host` (x4) | `resolved_host_reports_the_host_only_for_a_resolved_probe` |
| `ProbeOutcome::as_str` (x2) | `probe_outcome_tokens_are_a_stable_contract` |
| containment `==` -> `!=` | `detect_declines_a_toplevel_that_does_not_contain_the_cwd` |
| `parse_slug` `\|\|` -> `&&` | two rows added to `parse_slug_garbage_returns_none` |

The first COMPLETE run then reported 15, none of them in `common/src/repo.rs` (those were fixed) and
most in files the earlier partial runs never reached:

| survivor | closed by |
|---|---|
| `anchor_disagrees_with_remote`: `delete !`, `!=` -> `==` | `anchor_disagreement_is_reported_only_when_an_anchored_cwd_conflicts` |
| `record_enrich_skip`: `>` -> `>=` | `record_enrich_skip_reports_whether_it_actually_changed_anything` |
| `record_enrich_failure`: `>` -> `>=` | `record_enrich_failure_reports_whether_the_session_exists` |
| `classify_with_evidence`: `==` -> `!=` on the override | `an_operator_override_decides_in_both_directions` |
| `derive_repository`: `\|\|` -> `&&` | two rows added to `derive_repository_only_from_exact_github_pull_shape` |
| `head_tail`: `cap / 2` and `cap - head_n`, four mutants | `head_tail_splits_the_cap_into_two_halves_that_add_up` |
| token budget: `+` -> `-`/`*`, `>=` -> `<`, three mutants | `the_token_budget_halts_the_sweep_once_the_total_reaches_it` |
| `stats.skipped_empty` / `stats.redactions` accumulators | `the_sweep_counters_accumulate_across_sessions` |
| tag preservation: `\|\|` -> `&&` | `manual_tags_survive_an_ordinary_sweep_but_not_an_explicit_one` |
| `migrate_v7`/`v8`/`v9` version gates, three mutants | `the_v7_reset_fires_only_on_the_v6_hop`, `the_v8_and_v9_resets_refuse_a_version_outside_their_hop` |
| `extract`: `line_no += 1` -> `*= 1` | `an_unparseable_record_is_warned_with_its_real_line_number` |

**Two of those were worth having entirely apart from the mutant.** The token-budget guard is what
stops an unattended timer run spending without limit and had NO test at all. The migration version
gates had tests only for the hop each gate ADMITS, so nothing ever observed one refusing: inverting
`from_version != 6` would spare the upgrade that needs the reset and wipe the annotation on every
other, discarding work a previous reindex paid to compute.

**`line_no` was the one real `mutants:skip` candidate, and writing the test instead paid off
immediately.** It feeds only log messages, so an annotation would have been defensible. The test
failed on its first run for a reason I had not predicted: `extract` runs a substring PRESCREEN
(`outcome.rs:285`) before the JSON parser, so a malformed line with no `tool_use`/`gitOperation`
marker is skipped before any warning can fire, and the capture came back empty. The warning path had
no coverage at all, and a skip would have hidden that rather than recorded it.

**The containment mutant needed a shape nobody had.** `!(toplevel == cwd || cwd.starts_with(&toplevel))`
only differs from the mutant when the toplevel is neither the cwd nor an ancestor of it, and every
ordinary shape is contained by construction because git's discovery walks UP. `core.worktree` is the
reproducible way there, measured 2026-07-31:

```
$ git -C <proj> config --local core.worktree <elsewhere>
$ git -C <proj> rev-parse --show-toplevel
<elsewhere>          # not the cwd, and not an ancestor of it
```

**The override mutant is the phase's own lesson, in miniature.** `sessions` already asserted the
override end to end through the real gate, and that assertion could not close a mutant in `session`,
because a mutant is only checked by the tests that run for it. The biting test has to live in the
crate that owns the code.

**The final verdict, and how it is composed.** Two complete runs, because the harness kept killing
the 18-minute one and a partial run proves nothing:

```
gate3, all six paths:      287 mutants: 4 missed, 155 caught, 128 unviable
                           (all four survivors in sessions/src/enrich.rs)
gate5, the two sessions files after closing those four:
                            78 mutants: 0 missed,  53 caught,  25 unviable
```

The composition is sound rather than convenient: between the two runs I added TESTS only, and no
production code changed. Adding a test can kill a mutant; it cannot resurrect one. So gate3's clean
verdict on `common/src/repo.rs`, `session/src/scope.rs`, `efficiency/src/outcome.rs` and
`sessions/src/db/migrate.rs` still holds, and gate5 re-tests exactly the file the four survivors were
in (plus its sibling, since the shared `Fake` helper was extended).

**ZERO `// mutants:skip` in the tree.** Every survivor was a genuine test gap, so none was annotated.

**The last four survivors are the phase's sharpest lesson: a test can look right and still not
distinguish the mutant.** All three of my first attempts passed, and all three failed to bite:

| survivor | why the first attempt could not kill it |
|---|---|
| `tokens_in + tokens_out` -> `*` | the Fake reported 10 in / 5 out, and 15 and 50 BOTH clear any budget a test would pick. A zero `tokens_out` separates them: `10 + 0 = 10` halts, `10 * 0 = 0` never halts at all |
| tag preservation `\|\|` -> `&&` | I asserted the MANUAL-tag case, where the original and the mutant agree (both preserve). The distinguishing case is enrich-OWNED tags: the original refreshes them, the mutant preserves them forever and the vocabulary never updates |
| `skipped_empty += 1` (x2) | there are TWO such sites; my test reached the empty-BODY one. The survivor is the no-staged-copy branch, which `enrich_candidates` excludes and which is only reachable through `--only` |

The tag test then failed on its own PRECONDITION, which is worth recording separately: the row was
not re-offered, because `parsed_record` pins `modified` and the candidate predicate needs
`modified > enriched_modified`. Both the fixture and the assertion have to be right before a test
bites, and only the mutation run tells you when one of them is not.

**The gate exhausted the disk before it exhausted the mutants, and the failure mode is a FALSE
GREEN.** With `--jobs 12` the first full `--test-workspace` run died partway through:

```
ERROR Worker thread failed: failed to overwrite "/tmp/cargo-mutants-clyde-ORZTho.tmp/sessions/src/enrich.rs"
Caused by: No space left on device (os error 28)
OTTO EXIT=1
```

cargo-mutants gives every job its own copy of the workspace AND its own `target/`, and `/tmp` on this
host is a 16 GB tmpfs (`df`: `tmpfs 16G ... /tmp`) while `/` has 338 GB free. The dangerous part is
what the log said up to that point: **zero survivors**. Anyone reading the survivor count rather than
the exit code would have called that a pass on a run that tested a fraction of the mutants.

AC6 is specified correctly for exactly this reason (`otto mutants; echo $?` returns `0`), and the
task now avoids the trap rather than relying on the reader: `TMPDIR` is redirected to a disk-backed
`target/mutants-scratch` (overridable with `TMPDIR_MUTANTS`) and `--jobs` drops from 12 to 4. Four is
chosen for the DISK, not the CPU; cargo-mutants' own startup warning independently says jobs above 8
is probably too high.

**Redirecting `TMPDIR` then broke the BASELINE, and that exposed a real fragility in the code, not
just in the task.** With the scratch under `$PWD/target/mutants-scratch`:

```
test repo::tests::detect_is_conclusively_not_a_repo_for_a_plain_directory ... FAILED
ERROR cargo test failed in an unmutated tree, so no mutants were tested
```

`has_git_marker` (Phase 2) walks LEXICAL ancestors, and `TMPDIR` is what decides where
`TempDir::new()` puts a "plain directory". Under `$PWD/target/...` there is a `.git` above it (this
repo), so a plain temp dir correctly reads `Indeterminate` rather than `NotARepo`. `$HOME/.cache`
would have failed identically: measured, `/home/saidler/.git` exists, because the maintainer's home
IS a dotfiles repo.

Two fixes, because there are two problems:

- the task uses `/var/tmp/clyde-mutants` (disk-backed, and verified to have no `.git` above it),
  overridable with `TMPDIR_MUTANTS`
- the TEST now states its precondition and fails with an explanatory message when the environment
  violates it, rather than asserting the wrong arm and leaving the next person to work out why a
  green suite went red when only an environment variable changed

**The auto-set timeout manufactures false findings under parallelism.** cargo-mutants derives the
per-mutant test timeout from the unmutated baseline, measured on an idle machine, and then runs the
mutants N-way parallel. Observed on a chunk whose baseline auto-set 20s:

```
INFO Auto-set test timeout to 20s
TIMEOUT  common/src/repo.rs:304:9: replace ProbeOutcome::as_str -> &'static str with "xyzzy"
TIMEOUT  common/src/repo.rs:335:8: delete ! in detect_with_blocked_roots
...  21 TIMEOUT out of 22 results
```

Replacing a `&'static str` with `"xyzzy"` cannot hang, so those are load artifacts rather than
findings. The workspace suite alone takes ~16s idle, so a 20s budget under four concurrent jobs is
not a budget at all. The task now passes `--timeout 300`, roughly 15x the idle suite.

That matters beyond convenience: a gate that manufactures noise is a gate everyone learns to ignore,
which is the same failure the register describes for a test that does not bite.

**The `cost` flake, root-caused.** Recorded in Phase 2 as observed-once and unreproduced; Phase 5
reproduced it on demand and the mechanism is now certain. Running `otto mutants` and `otto ci`
concurrently:

```
[test] test cache::tests::test_save_and_load_cached_day ... FAILED
[test] test cache::tests::test_prune_cache_removes_old_entries ... FAILED
```

`cost::cache::cache_dir()` resolves to the real `~/.cache/clyde/cost/` for every process, and
`prune_cache` deletes files a sibling test is about to read. cargo-mutants runs dozens of copies of the
suite at once, all pointed at that one directory. After the fix, `cargo test -p cost cache::` passed
five consecutive runs with 56 concurrent cargo-mutants processes hammering the same path.

## Phase 6: Rule 1 resolves at a bare-repo container root

### Design decisions

- **The root is computed once, from either source, and the containment and blocked checks run after
  it** (`common/src/repo.rs`). The design writes the fallback as a second copy of both checks inside
  the `None` arm; folding them means the two branches cannot drift, and it is why the mirrored
  containment check the design asks for is simply the same check.
- **`contains` and `same_path` canonicalize both sides.** The blocked-root comparison needs it too,
  not just containment: `$HOME` can itself be a symlink, and a blocked root that fails to match
  because of one is a silently disabled guard.
- **Canonicalization falls back to the given path** when it fails (a deleted cwd, a permission
  error), preserving the old lexical behavior rather than panicking.
- **`no_work_tree_root` returns `Result<PathBuf, ProbeOutcome>`**, so the "here is the root" and
  "here is the answer, stop" cases are distinguishable at the call site instead of being smuggled
  through an `Option` the caller has to reinterpret.

### Deviations

- **The `common == cwd` branch consults `--is-bare-repository`, which the design's snippet does not.**
  This is an amendment to the amendment, and it closes a hole the phase would otherwise have
  INTRODUCED rather than one it exposes. Measured against git 2.53.0:

  | cwd | `--git-common-dir` | `--is-bare-repository` | correct root |
  |---|---|---|---|
  | plain bare repo | `.` | true | `<cwd>` |
  | cwd inside a NON-bare `.git` | `.` | **false** | **`<cwd>`'s PARENT** |

  The design maps `common == cwd` to `root = cwd` unconditionally. For a cwd inside a normal repo's
  `.git` that roots at `<repo>/.git`, and the blocked check compares the root, so a repo at `$HOME`
  probed from `$HOME/.git` would compute `$HOME/.git`, MISS the guard, and attribute the dotfiles
  repo. Today that path is unreachable (the old code declined the moment `--show-toplevel` failed),
  so the fallback is what would have opened it. `detect_blocks_a_cwd_inside_a_blocked_repos_git_dir`
  is the test, and deleting the `--is-bare-repository` branch is what makes it fail.

- **Row 8 asserts a RESOLVE, not the decline its prescribed test name implies.** Flagged in Phase 1's
  open question and settled here by measurement: `git -C <container>/.bare/refs rev-parse
  --git-common-dir` returns `<container>/.bare`, so the root is `<container>`, the cwd is under it,
  and containment passes. That is the correct answer. Both behaviors are asserted rather than one
  being picked: `detect_resolves_from_inside_the_bare_dir` for the measured row-8 shape, and
  `detect_declines_a_repo_found_outside_the_cwd` (a `.git` pointer into a SIBLING tree) for the
  genuine containment rejection the phase's named test wanted.

### Tradeoffs

- **`has_git_marker` stays more generous than git's own discovery.** It ignores mount points and
  `GIT_CEILING_DIRECTORIES`, so it can report a marker git would not have used. Every disagreement
  yields `Indeterminate`, which records nothing: under-recording costs one re-probe, over-recording
  is a permanent refusal of work scope.

### Open questions

- None. Phase 1's row-8 question is closed above.

### Measured

**Every change bites, verified by deletion:**

| deletion | fails |
|---|---|
| the `--git-common-dir` fallback | all five no-work-tree rows |
| the `--is-bare-repository` branch | `detect_blocks_a_cwd_inside_a_blocked_repos_git_dir` |
| canonicalized containment | `detect_resolves_a_symlinked_cwd` |

**`clyde session reindex --reresolve-repo` on a copy of the live 2,150-row catalog, this branch
versus installed v0.22.0 running the SAME repair:**

```
                v0.22.0    this branch
git-origin         1330           1431
known-path          243            254
files-touched       196            148
path-guess           84             20
(unresolved)        309            309

rows git-origin here but not under v0.22.0:  101
regressions (the reverse):                     0
```

**101 sessions, across exactly 7 cwds, and ALL SEVEN are bare-repo container roots.** Every one:
`--show-toplevel` fails, `--git-common-dir` names a `.bare`, `is-bare-repository` is true.

```
/home/saidler/repos/scottidler/second-brain          86
/home/saidler/repos/scottidler/slack-dashboard        6
/home/saidler/repos/scottidler/git-tools/clone        4
/home/saidler/repos/tatari-tv/drata-cli               2
/home/saidler/repos/tatari-tv/slack-cli               1
/home/saidler/repos/scottidler/second-brain/voice     1
/home/saidler/repos/scottidler/ralph-wiggum-loop      1
```

**This corrects two claims in the design, and the correction is in the design's favour.**

1. The design counts THREE bare containers on desk.lan (`okta/okta-cli-client`, `nvidia/skillspector`,
   `qdrant/qdrant`) and concludes Problem 4 is invisible here. Those three exist and are bare, but
   they carry 0, 1 and 1 sessions between them. The seven that DO carry sessions are different
   directories the count missed.
2. "**Problem 4 is invisible on desk.lan**" and "desk.lan cannot validate Problem 4 end to end" are
   both too strong. What is true is the narrower claim the design also makes: rule 4 MASKED it. No
   session moved from unresolved to `git-origin` (the phase's stated criterion, confirmed at exactly
   0), because a container under `<repo-root>/<org>/<repo>` gets a path-guess. What was lost was not
   coverage but PROVENANCE: 101 sessions were attributed by the lowest-confidence rule instead of by
   their remote, and two of the seven containers are `tatari-tv` work repos.

So the phase's success criterion holds as written (0 from unresolved, a genuine null result) while
the framing around it understated the defect. desk.lan CAN validate Problem 4; it just cannot
validate it in the population the criterion looked at.

**`docs/design/2026-07-26-report-story-fidelity.md` corrected.** Its four-row table claimed rule 1 was
"already layout-agnostic, verified 2026-07-26"; every row was a cwd inside a work tree. The fifth row
(the container ROOT, `fatal: this operation must be run in a work tree`, rc=128) is added, with
Keegan's cost and a pointer to this design.

## Phase 7: Rule 3 stops reading the layout

### Design decisions

- **`SharedResolver` is a new `Sync` sibling of `Resolver`** (`common/src/repo.rs`), not a change to
  it. `efficiency::collect` prices sessions through rayon's `par_iter`, so the resolver it hands rule
  3 must be shared by reference across threads and a `&mut Resolver` cannot be. The design does not
  mention this; it says "memoized through `Resolver`'s existing per-path cache", which is not
  reachable from a parallel iterator.
- **The lock is held around the map only, never across the `git` spawn.** Two threads racing the same
  uncached directory both probe and both insert the same answer: one wasted spawn, no wrong result.
  Holding it across the subprocess would serialize the entire parallel pass, which is the house rule
  about not holding a lock across blocking work.
- **`build_session` takes `Option<&SharedResolver>` where it took `Option<&Path>`**, keeping the
  original shape: the parameter is BOTH the outcome switch and rule 3's input, so asking for outcomes
  without the means to bucket them stays inexpressible. `repo_root` left that call path entirely;
  rule 4 still uses it, but rule 4 lives in `common::repo` and never came through here.
- **ONE resolver for the whole pass**, created in `collect_layouts` rather than per session. Sessions
  overlap heavily on directories, so a per-session memo would re-probe the same checkout once per
  session instead of once per catalog.

### Deviations

- **The memo collapses per REPOSITORY, not per directory.** A successful probe seeds every ancestor up
  to and including the first carrying a `.git` marker. Those are all inside the same repository by
  construction of the walk, so rule 1 gives every one the identical answer, and stopping at the marker
  means a submodule or nested checkout never inherits its parent's slug. This is the design's own
  stated remedy ("if a full reindex regresses meaningfully the cache key moves to the git common
  dir") reached more cheaply: the `.git` marker already identifies the repository, so no extra `git`
  call is needed to find it. Measured below.

- **Rule 3 now requires the edited file's parent directory to still EXIST.** The path parse it
  replaces worked on strings and would bucket a checkout deleted years ago. The trade is deliberate
  and it is this branch's thesis applied consistently: a slug parsed out of a vanished path is a
  GUESS, and rule 4 is the rule permitted to guess (and is marked `path-guess` wherever it is
  rendered). Rule 3 claims to report what a session actually edited. Asserted explicitly by
  `union_repos_touched_needs_the_parent_to_still_exist` so it is a stated cost rather than a
  discovery.

### Tradeoffs

- **The old `union_repos_touched_is_empty_off_the_configured_root` was INVERTED and renamed**, per the
  design, so the assumption it encoded cannot quietly return. It asserted that a checkout outside
  `repo-root` buckets to nothing; that WAS Problem 5.
- **The `union` tests moved onto real checkouts.** They used to pass paths like
  `/repos/tatari-tv/clyde/src/lib.rs` that never existed on disk, which is precisely the register's
  complaint in miniature: they asserted a directory CONVENTION rather than a fact about a repository.
  The cases that genuinely do not care about buckets (commits, PRs, MCP counts) use a resolver with
  nothing behind it, named `no_slugs()` so the intent is visible.

### Open questions

- None.

### Measured

**A 96-second "regression" that did not exist, and the correction matters more than the number.**
The first comparison was `~/.cargo/bin/clyde` (12.30 s) against `./target/debug/clyde` (106.11 s) and
I read it as an 8.6x regression from this phase. It is a RELEASE binary against a DEBUG one. Two
subsequent investigations chased it: a subprocess count (which showed only ~116 extra successful
spawns, the rest being PATH-search failures) and a bisect across the phase commits, which finally
gave it away by reporting Phase 1 alone at 103.67 s. Phase 1 changes no probing at all.

Rebuilt in release, three runs each on fresh copies of the live 2,150-row catalog:

```
v0.22.0 (installed release):   11.87 s / 10.47 s / 10.32 s
this branch (release):         10.43 s / 10.48 s / 10.47 s
```

No regression. The branch is marginally faster and noticeably more consistent.

> **CORRECTED 2026-08-01 by the implementation audit.** The 8.6x claim above is correctly retracted,
> but "no regression / marginally faster" does NOT reproduce. Re-measured release-to-release on the
> same host, both binaries against fresh copies of the same `sessions.db.pre-v13.bak` (2,165 rows),
> branch copies pre-migrated so the timed section is the reindex and not the migration:
>
> ```
> interleaved A/B, 5 runs each, cold (fresh v13, no repo_host populated):
>   v0.22.0 (installed):  11.99 / 12.02 / 12.08 / 12.09 / 12.19    median 12.08
>   this branch:          14.32 / 14.41 / 14.52 / 14.67 / 14.70    median 14.52
>
> steady state, same copy reindexed repeatedly (passes 2-4):
>   v0.22.0 (installed):  10.46 / 10.50 / 10.57
>   this branch:          12.05 / 12.42 / 12.44
> ```
>
> The installed-binary numbers reproduce the row above almost exactly (10.46-10.57 against
> 10.32-10.47), which validates the harness; the divergence is entirely in the branch measurement.
> **There is a consistent steady-state cost of roughly 1.8 s, about 17%.** The distributions do not
> overlap and it reproduced across two independent harnesses.
>
> The mechanism is NOT established. The likely candidate is the added per-cwd origin probe and host
> recording, but that was not measured and is not asserted here. Sizing the cost, deciding whether
> 17% on a background reindex is acceptable, and finding the mechanism are open.

**The repository-wide collapse still earns its place, measured the same way:**

```
per-directory memo only:       13.21 s / 14.99 s
repository-wide collapse:      10.43 s / 10.48 s
```

So the collapse is worth roughly 3-4 s of a 10 s reindex, which is real but a fraction of what the
debug numbers implied. It stays, and this note records that it was originally justified by an
artifact.

Rule 3's probing costs almost nothing on its own: with `repos_touched` short-circuited to empty, the
same debug binary ran 108.66 s against 111.93 s with it enabled. That measurement is in debug units
and therefore only a ratio, but the ratio is the point: rule 3 is ~3% of the pass, not the cause of
anything.

**The lesson, since it cost three investigations: never compare a `cargo build` binary against a
`cargo install` one.** The house rule about measuring rather than guessing does not help if the two
things measured are not comparable.

## Phase 8: Disclose, and update the runbook

### Design decisions

- **`clyde doctor` gained a catalog, which it never had.** `run()` now takes the db path, and the
  attribution section is READ-ONLY and deliberately does NOT feed `Report::healthy()`. A session
  refused by the routing gate is the gate WORKING, and a diagnostic that exits non-zero for correct
  behavior is one people stop running.
- **Config and catalog are best-effort.** `doctor` exists to say what is wrong, so an unreadable
  config or a missing catalog prints a line naming which one failed rather than aborting the whole
  report.
- **Every routing count carries its own REMEDY on the same line.** Six counts rather than one because
  they have six different fixes; a count on its own is not actionable at 3am.
- **`common::config::config_file_path` was made public** so `doctor` can name WHICH file it loaded. A
  diagnostic that prints settings without naming their source leaves the reader guessing between a
  forgotten config and the built-in defaults, which is the confusion register item 8 came from.
- **The rule-4-inert line is a `note:`, not a failure.** Plenty of hosts have no `<org>/<repo>` layout,
  but rule 4 silently never firing looks identical to rule 4 being broken.

### Deviations

- **`doctor` RE-PROBES to count `Blocked` / `OutsideRoot` / `Indeterminate`, rather than reading them
  from the catalog.** It has to: those three outcomes record NOTHING by design, which is the property
  that stops a transient failure becoming a lockout, so the catalog cannot distinguish them. A live
  probe is legitimate here in a way it is not at the gate, because `doctor` asks about the machine as
  it is NOW rather than about when a session ran. Memoized per repository, so it is a handful of
  `git` calls.

- **The runbook was NOT published.** It is a marquee post, so updating it is outward-facing and is
  the owner's to send. The full diff is drafted at
  `docs/design/2026-07-31-runbook-update-draft.md`, to publish with `marquee:replace` once the branch
  is released.

- **Register item 11's stash was NOT dropped**, as recorded under Phase 3. The register itself says
  "if the stash is gone, nothing is lost", so the risk is low, but destroying work that exists on no
  branch is the owner's call: `git stash drop stash@{0}`.

### Open questions

- None.

### Measured

**Two wrong `indeterminate` predicates, both caught by running the thing rather than reasoning about
it.** The design asks for a counter so "a host where every probe is indeterminate" is visible. Getting
it right took three attempts on the live catalog:

```
counting every non-git-origin row:            734   (counts everything rules 2-4 resolved)
+ an on-disk filter:                          399   (counts BLOCKED roots)
live re-probe, split by actual outcome:   0 / 0 / 21 (blocked / outside-root / indeterminate)
```

The 399 was the instructive one: this maintainer's `$HOME` is itself a git repo, so every session
directly under it resolves to a blocked root, correctly, and a predicate that cannot see the
difference reports the gate working as if it were broken.

**And the 21 turned out to be a real defect in `has_git_marker`, added back in Phase 2.** Git reports
`fatal: not a git repository` for `/home/saidler/repos`, yet `/home/saidler/.git` exists. It is a
plain directory containing only `info/` (the `info/exclude` global-ignore trick), so it is not a
repository and git is right. `has_git_marker` tested only for `.git` EXISTENCE, so it saw a marker,
downgraded 21 conclusive `NotARepo` answers to `Indeterminate`, and made `doctor` tell the operator to
go check `safe.directory` for a problem that did not exist.

A `.git` directory now has to carry `HEAD` to count; a `.git` FILE is a gitdir pointer and always
counts. After the fix:

```
blocked 0 | outside-root 0 | indeterminate 0
probe-refused 26 -> 370
```

**The 370 is the number to check before believing this is safe, and it was checked.** Those rows are
cwds that genuinely are not repositories, so a conclusive negative is the correct record, and it will
refuse a later git-origin work slug for exactly the reason Problem 1 exists. Re-running the routing
decision against installed v0.22.0 on the live catalog:

```
this branch: considered 1044  would-enrich 26  (personal,false) 1017  (work,false) 1  (work,true) 26
rows whose (scope, would-send) CHANGED vs v0.22.0: 0
```

Zero. `would-enrich` rises from 14 to 26 because Phase 6 newly attributes the container-root sessions,
which is the coverage win rather than a routing change.

**AC8, executed:**

```
$ clyde doctor | rg -i 'repo-root'
  repo-root:     /home/saidler/repos
$ echo $?
0
```

One line, an absolute path, exit 0, and piping is safe because `clyde doctor` (`clyde/src/doctor.rs`)
has no TTY branch. That is the distinction round 2 conflated: `clyde session doctor`'s `print_doctor`
DOES switch to JSON when piped, and it is a different command.

**The mutation gate still passes on the file Phases 6 to 8 changed most:** `common/src/repo.rs`, 100
mutants, 35 caught, 65 unviable, 0 missed.

### Out of scope, closed

The `cost::cache` flake first reported in Phase 2 and root-caused in Phase 5 came back once more at
the very end, in a DIFFERENT test (`cost::tests::resolve_stale_feed_offline_reads_the_sidecar_via_the
_public_wrapper`), and the second failure showed the Phase 5 fix was only half of one.

`cost/src/cache.rs` and `cost/src/tests.rs` each held their OWN module-level mutex, and both redirect
`XDG_CACHE_HOME`. Two mutexes guarding one global serialize nothing against each other, which is
exactly what `common/src/lib.rs` already documents:

> ONE process-wide lock for every test in this crate that reads or mutates the process environment.
> Deliberately crate-level rather than per-module.

Consolidated onto a single `cost::ENV_LOCK`, following that existing pattern rather than inventing a
third one. Five consecutive `cargo test -p cost` runs clean afterwards.

Worth stating plainly, because it is the same lesson twice: the Phase 5 fix addressed the symptom I
had measured (a shared cache DIRECTORY) and missed the general form (a shared PROCESS ENVIRONMENT
with per-module locks). The house rule already had the general form written down.
