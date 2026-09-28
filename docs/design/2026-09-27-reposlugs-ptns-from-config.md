# Design Document: Work Reposlug Patterns From Config

**Author:** Scott Idler
**Date:** 2026-09-27
**Status:** Implemented
**Review Passes Completed:** 5/5 on the two-list draft (panel rounds 1-4); restructured 2026-09-27 to one `reposlugs-ptns` list per the owner, passes 2-5 re-run on the restructure; panel round 5 folded

## Summary

clyde decides work vs personal scope from a compiled-in org list, `WORK_ORGS = ["tatari-tv"]`. That list becomes one `reposlugs-ptns` key in `clyde.yml`: reposlug patterns (`<owner>/*` or `<owner>/<repo>`), with a `!` prefix marking an exclude. Built-in default `["tatari-tv/*"]` only when the key or the file is absent. Every stored scope decision records the policy it was made under, so a config change reclassifies the rows it affects instead of changing nothing.

## Problem Statement

### Background

- Scope gates whether a session body leaves the machine: only `work` sessions get enriched (sent to the LLM for summary/title), `personal` is never sent (`session/src/scope.rs:3-7`, `:128`, enrich gate `sessions/src/enrich.rs:179-180`).
- The classifier has three evidence paths, in order: cwd anchor under a configured root (`Anchors::scope_of`, `scope.rs:480`), git-origin remote slug (`scope.rs:291`), touch-set unanimity (`scope.rs:349`). The last two go through `is_work_slug` (`scope.rs:677`).
- Two of the three routing inputs are already config: `repo-roots` (`common/src/config.rs:363`) and `work-remote-hosts` (`config.rs:385`). The org list is the odd one out, a private const at `scope.rs:74`.
- The reposlug is rule 1 (`common::repo::detect`, `common/src/repo.rs:153`): the origin remote URL parsed into `<owner>/<repo>`, the same parse the operator's `reposlug` CLI does, run at index time and stored per row as `repo` with `repo_source=git-origin`.
- Prior session b83efe1c edited the const (adding `tatari-dev`, `otto-rs`) and added a hardcoded `WORK_REPOS` of `scottidler/{dotfiles,claude,keep}`. Rejected for being code, not for what it named; the diff is stashed (`48713b01`), not merged.

### Problem

- **Identity in code.** Which repos count as work is the operator's policy, and today it only changes by recompiling. Every teammate running clyde inherits one org list.
- **Config changes do not reclassify.** A cwd-anchor decision is settled: the row gets `enrich_status='skipped-personal'`, `scope_version=4`, and `enrich_candidates` only re-offers it when `scope_version` is NULL or older than `SCOPE_VERSION` (`sessions/src/db/enrich.rs:417-418`). Nothing records which policy the decision used, so widening the list, from code or from config, never touches rows already settled personal.
  - Measured on the operator's catalog: 605 target rows (below) are `skipped-personal`.
  - Same latent bug for `repo-roots`: adding a root has needed a `SCOPE_VERSION` bump to reach settled rows (`scope.rs:66-70`).

### Goals

- `reposlugs-ptns` in `clyde.yml` is the one source of work repos; `WORK_ORGS` is deleted. (Scott, this session: "hardcoding MY values into my clyde is fucking asinine"; b83efe1c 17:55: "I dont like that we are hardcoding these things. this needs to go into the XDG .config yml file")
- One list of reposlug patterns, `<owner>/*` or `<owner>/<repo>`, a `!` prefix for an exclude, exclude wins. (Scott: "maybe we make this just one list of reposlug_ptns"; "we even do that as includes and excludes"; "in the yaml it would be reposlugs-ptns")
- Built-in default `["tatari-tv/*"]` only when the key is absent or `clyde.yml` does not exist; a present key replaces it, no merge. `[]` means zero repos are work. (Scott: "it should only apply if the config value ... is not defined in the XDG .config yml file, or that file doesnt exist"; "it means ZERO additional repos are in scope as work repos")
- Operator's list: `tatari-tv/*`, `tatari-dev/*`, `otto-rs/otto`, `scottidler/dotfiles`, `scottidler/claude`, `scottidler/keep`. (`otto-rs/otto`: Scott, b83efe1c 17:50 "move otto-rs in the work orgs as well", and this session's example `"otto-rs/otto"`. `tatari-dev/*`: accepted implicitly; b83efe1c proposed "add tatari-dev (clear fix)" and Scott's reply extended it ("as well") without naming it; 1 live row, `tatari-dev/calvin-skills`. The three `scottidler` repos: b83efe1c 17:53)
- Repos are identified by the remote-derived reposlug, never by a directory name, except where an owner-wide `<owner>/*` include keeps today's path anchor. (Scott: "either use the tool or do what the tool does. it url parses the remote to figure out the reposlug")
- Changing `reposlugs-ptns` (or `repo-roots`) re-offers the enrich-eligible `skipped-personal` rows, with no code change and no version bump. The fingerprint is the whole policy, so any change re-offers all of them (2111 on desk); re-classification is local, and only rows that now classify Work make an LLM call.
- Invalid values fail loudly at load, naming the key, same as `work-remote-hosts`.

### Non-Goals

- **Globs beyond the two forms** (`tatari-tv/philo-*`, `*/x`, `?`, `[...]`). Rejected at load with the key named. No new crate (clyde has no glob dependency today; `Cargo.lock`). Revisit: the first time an operator needs a mid-segment pattern; the precedent then is globset as in `github-setup-rs/src/merge.rs:44`.
- **Fuzzy matching** (leatherman's exact -> ignorecase -> prefix -> contains, `leatherman/leatherman/fuzzy.py:36-41`; `github-setup-rs/src/fuzzy.rs:21`). Excluded: this list gates what leaves the machine, and a prefix/contains match fails open (`tatari` would match `tatari-evil/x`).
- **Unanchored cwds** (`/tmp`, `~/tmp`, `~`): 203 personal rows in 2026-09-01..27. Parked, separate design doc. Measured: 0 of them pass touch-set unanimity under either policy (files in scratchpads/`/tmp` are unattributed, `scope.rs:180-185`). Revisit: next doc, scoped to the totality/scratchpad gap.
- **`<root>/<flat-clone>/src` settles Personal** before the remote is consulted. Pre-existing, identical today (panel round 2 D2).
- **ssh-config redirects of `github.com`.** Hypothetical; not how any operator repo is set up. Excluded. (Scott)
- **Un-sending on narrowing.** Removing or excluding a repo cannot recall already-enriched (`ok`) rows; the re-offer predicate only touches `skipped-personal`.
- **`cost/src/statusline.rs:11` `DEFAULT_NAME = "scottidler"`.** Audited: a statusline flavor name for an embedded generic powerline script, no identity content.
- **`clyde doctor` exits 0 when `clyde.yml` fails to parse** (prints `attribution: could not read the catalog: failed to load clyde config`, returns 0). Observed while writing this doc. Parked, separate targeted fix.
- **Recomputing stored scope on rows enrich cannot reach.** `enrich_candidates` excludes `archived=1 AND staged_path IS NULL` (`sessions/src/db/enrich.rs:395-398`: no body left to send), so those keep their stored scope. Measured: 5 of the 605 target rows.
- **Export envelope `scope-version`** carrying the policy. Parked: per-record `scope` already reflects the reclassification; revisit if a consumer caches scopes across exports.

## Proposed Solution

### Overview

1. New `reposlugs-ptns` config key: a list of include/exclude reposlug patterns, validated at load.
2. `Anchors` becomes `ScopePolicy { roots, ptns }`, built once after `Config::load()`, the only thing the classifier reads work repos from.
3. Schema v14 adds `sessions.scope_policy TEXT` (the canonical policy a decision was made under) and `sessions.repo_host_slug TEXT` (host/slug pairing). Enrich re-offers `skipped-personal` rows whose stored policy differs from the current one.

### Architecture

```
clyde.yml ──Config::load()──> Config { repo_roots, reposlugs_ptns, work_remote_hosts }
                                   │
                   ScopePolicy::new(roots, ptns)   (main.rs export/enrich, doctor.rs)
                                   │
         classify_with_evidence ── scope_of / is_work_slug / anchor_disagrees_with_remote
                                   │
      enrich: writes scope, scope_version, scope_policy = policy.fingerprint()
      candidates: re-offer skipped-personal WHERE scope_version < V OR scope_policy IS NOT fingerprint
```

- The classifier stays config-free (`scope.rs:427-437`): it takes a `&ScopePolicy`, never calls `Config::load()`.
- `ScopePolicy`, not `Anchors`: once it carries patterns, "anchors" no longer names what it is.
- `EnrichOptions::default()` / `ExportContext` default: empty `ScopePolicy` (no roots, no patterns) -> nothing is ever Work. Fail closed.

### Data Model

**Config** (`common/src/config.rs`), operator's file:

```yaml
# clyde.yml
reposlugs-ptns:
  - tatari-tv/*
  - tatari-dev/*
  - otto-rs/otto
  - scottidler/dotfiles
  - scottidler/claude
  - scottidler/keep
  # - "!tatari-tv/some-personal-repo"   # exclude: quote it, `!` starts a YAML tag
```

- `reposlugs_ptns: Vec<ReposlugPtn>`, `#[serde(default = "default_reposlugs_ptns", deserialize_with = "de_reposlugs_ptns")]`, the `work_remote_hosts` shape (`config.rs:384-395`; same shape as `eratosthenes/src/cfg/config.rs:45,67,89`).
- `pub const DEFAULT_REPOSLUGS_PTNS: &[&str] = &["tatari-tv/*"];` in `common/src/config.rs`, the one place the literal is spelled. `Config::default()` (missing file, `config.rs:464-466`) uses `default_reposlugs_ptns()` too, so absent key and absent file resolve the same.
- `ReposlugPtn` (`common/src/repo/ptn.rs`, new):
  ```rust
  pub struct ReposlugPtn { exclude: bool, owner: String, repo: RepoPart }
  pub enum RepoPart { Any, Exact(String) }   // `*` | `<repo>`
  ```
- `de_reposlugs_ptns`, each entry:
  - optional leading `!` -> exclude
  - then exactly `<owner>/<repo-part>`: one `/`, both halves non-empty, no whitespace anywhere
  - `<owner>` literal, never `*`; `<repo-part>` is `*` or a literal with no `*`, `?`, `[`
  - lowercased on load; GitHub owner and repo names are case-insensitive
  - else error naming `reposlugs-ptns` and the entry
  - an empty entry errors with a hint: "an unquoted `!owner/repo` is a YAML tag and parses as an empty string; quote it". Measured: serde_yaml reads `- !tatari-tv/x` as `""` (the existing `work-remote-hosts` check reports it as an empty entry)
  - `[]` accepted: zero repos are work
  - duplicates accepted and deduped in `ScopePolicy::new`. Every distinct exclude is KEPT, including one with no matching include: `!scottidler/private` is exactly what vetoes a `tatari-tv/*` fork whose remote is `scottidler/private` (panel round 5 M1)
- **Match rule:** `matches(slug) = any include matches AND no exclude matches`. Order does not matter; exclude wins.

**Schema v14** (`sessions/src/db.rs`, own migration step per `db.rs:59-64`):

- `ALTER TABLE sessions ADD COLUMN scope_policy TEXT` (NULL for existing rows).
- `ALTER TABLE sessions ADD COLUMN repo_host_slug TEXT` (NULL for existing rows): the rule-1 slug the stored `repo_host` was observed with (host/slug pairing, below).
- `ScopePolicy::fingerprint()`: `serde_json` of `{"ptns": [...], "roots": [...]}`, each array sorted and deduped, ptns in normalized text form (`!` kept, lowercased). JSON escaping means no two distinct policies collide, which a comma-join does not guarantee. Plain text, no hash: readable in `sqlite3`, stable across Rust versions (`DefaultHasher` is not). `serde_json` is already a `session` dependency (`session/Cargo.toml:19`).
- Normalization (lowercase, sort, dedup) happens in `ScopePolicy::new`, so a directly-constructed policy (tests, `EnrichOptions::default()`) fingerprints the same as one built from config.
- Fingerprints are host-local: root spellings are expanded per host (`config.rs:570`). The catalog is per-host (`~/.local/share/clyde`), so this is inert.
- Written at every site that writes `scope_version`: `set_enrichment` (`db/enrich.rs:296`), `record_enrich_skip` (`:321-346`), `record_enrich_failure` (`:356-373`). The `IS NOT` no-change guard in `record_enrich_skip` extends to `scope_policy`, so an unchanged re-run doesn't fire the revision trigger.
- Re-offer predicate (`db/enrich.rs:406-418`) is restructured so the freshness clause applies only to non-`skipped-personal` rows:
  ```sql
  AND ( (s.enrich_status = 'skipped-personal'
         AND (s.scope_version IS NULL OR s.scope_version < ?3 OR s.scope_policy IS NOT ?4))
     OR ((s.enrich_status IS NULL OR s.enrich_status != 'skipped-personal')
         AND (s.enriched_at IS NULL OR s.modified > s.enriched_modified
              OR s.prompt_version IS NULL OR s.prompt_version < ?2)) )
  ```
  - Why: `record_enrich_skip` never clears `enriched_at`. A row enriched `ok`, later re-run under a narrower policy and recorded `skipped-personal`, keeps `enriched_at`; under today's two-`AND` shape a later widening would never re-offer it. The comment at `db/enrich.rs:404-408` claiming `enriched_at IS NULL` for every `skipped-personal` row is false on that path and gets corrected. Live today: 0 target rows have `enriched_at` set.
  - For every row with `enriched_at IS NULL`, identical to today except the intended `scope_policy IS NOT ?4` term (panel round 2: 5,760-case SQL truth table).
- NULL on existing rows means the first run after upgrade re-offers every enrich-eligible `skipped-personal` row once; settled rows re-settle with the fingerprint and drop out. Provisional rows (`settled: false`) stay eligible every pass by design, as today.
- The fingerprint covers classifier INPUTS, not materialized evidence. `work-remote-hosts` also filters what enters `repos_touched` (`efficiency/src/outcome.rs:239-241`, `detect_trusted`); a hosts change reaches touch-set rows only when `repos_touched` is recomputed. Stated boundary, same as today.

### API Design

```rust
// session/src/scope.rs
pub struct ScopePolicy { roots: Vec<PathBuf>, ptns: Vec<ReposlugPtn> }
impl ScopePolicy {
    pub fn new(roots: &[PathBuf], ptns: &[ReposlugPtn]) -> Self;
    pub fn matches(&self, slug: &str) -> bool;          // any include AND no exclude, lowercase compare
    pub fn owner_rule(&self, owner: &str) -> OwnerRule;
    pub fn fingerprint(&self) -> String;
    pub fn scope_of(&self, cwd: &Path, facts: &RoutingFacts<'_>) -> Anchor; // was Anchors::scope_of(cwd, probe) -> Option<Scope>; takes the facts so it sees the stored slug and repo_host_slug
}
pub enum OwnerRule { Wide, Named, Unlisted }   // has an `<owner>/*` include | only `<owner>/<repo>` includes | no include (excludes never make an owner Wide or Named)
pub enum Anchor {
    Settled(Scope),   // today's Some(scope): settled:true (scope.rs:233-244)
    Excluded,         // an exclude hit on a Wide owner: Personal, settled:false, TERMINAL (no git-origin, no touch set)
    Deferred,         // Named owner: git-origin arm only, with `matches(slug)` AND slug owner == cwd owner; else Personal settled:false; NEVER the touch-set branch
    Unanchored,       // today's None: git-origin, then touch set, as today
}
fn is_work_slug(policy: &ScopePolicy, slug: &str) -> bool;  // policy.matches(slug) plus today's shape guards (scope.rs:677)

// common/src/config.rs
impl Config { pub fn reposlugs_ptns(&self) -> &[ReposlugPtn]; }
```

- **Cwd anchor**, `<root>/<owner>/<dir>/...` (today `scope.rs:480-490`), by `owner_rule(owner)`:
  - `Wide` -> Work, settled, as a work org is today, unless an exclude matches `<owner>/<dir>` or the stored git-origin slug; an exclude hit -> `Anchor::Excluded`: Personal, `settled: false`, terminal, so no later branch (touch set included) can turn it Work. Excludes are checked against the path slug `<owner>/<dir>` (read through the same `common::repo::under_deepest_root` walk `org_slot` uses, `scope.rs:492-505`, so the two cannot disagree on which component is `<dir>`), the stored `repo`, and `repo_host_slug` when present, because a rank-0 `repo` is never replaced (`sessions/src/db/repo.rs:84-88`) and the current remote may differ (panel round 5 M3, C1). Keeps today's precedence where a fork in a work dir is Work (`cwd_anchor_outranks_the_remote_in_both_directions`, `scope/tests.rs:716-732`: `<repo-root>/tatari-tv/clyde-fork` with remote `scottidler/clyde-fork`). Bare `<root>/<owner>` keeps today's probe rule (`bare_work_org_is_an_org_dir`, `scope.rs:534`) only when no `!<owner>/*` exclude exists; with one, it is `Excluded` (today's `scope.rs:483` would otherwise anchor Work).
  - `Named` -> `Deferred`: Work ONLY when `repo_source=git-origin`, `matches(slug)`, AND the slug's owner equals the cwd owner (panel round 5 M2: otherwise the synthetic `<repo-root>/scottidler/philo` with remote `tatari-tv/philo` matches `tatari-tv/*` and goes Work), and only through the git-origin arm's host and probe refusals (`scope.rs:298-323`); settled, like any git-origin Work. Anything else (no remote, an unmatched slug, a slug of another owner, touch-set evidence) is Personal, `settled: false`. A sibling worktree, a `.git` or renamed container, or the canonical `<root>/scottidler/claude` all classify by the remote. A `second-brain` session that edited one `scottidler/claude` file (live row 251) stays Personal. (panel round 3 M1, option (i))
  - `Unlisted` -> Personal, settled, as today.
  - Excludes on a `Wide` owner match `<owner>/<dir>` as well as the remote slug. That is the one place a directory name counts, and only in the fail-closed direction (it can only take Work away).
- **Why owner-wide keeps the path anchor and named repos do not:** `tatari-tv/*` reproduces today's behavior exactly (no regression for teammates on the default, forks in work dirs stay Work). A named repo is identified by its reposlug, and the directory name is not that identity.
- **Git-origin and touch set** (unanchored cwds): `matches(slug)` for the git-origin slug; every `repos_touched` key must `matches` for touch-set Work. Excludes apply on both paths.
- Measured: every target row carries the matching slug: all 369 under the three `scottidler` repos (`scottidler/claude` 296, `dotfiles` 61, `keep` 12; counts drift up as sessions are added) and all 239 under `~/repos/otto-rs/` (`otto-rs/otto`), each `repo_source=git-origin`.
- **Host trust** in the deferred arm is the git-origin arm's, unchanged: an allowlisted host confers, a non-allowlisted one refuses, a NULL host (rows indexed before v13) does NOT refuse (`scope.rs:601-605`, the strip-only rule). Measured: 10 listed-slug rows under `~/repos/scottidler/` have a NULL host and classify Work on that inherited trust.
- **Host/slug pairing** (panel round 4 M2): `record_repo_host` overwrites the host every index pass (`sessions/src/db/routing.rs:92-99`), while `upsert_repo` keeps a rank-0 git-origin slug forever (`sessions/src/db/repo.rs:84-88`). A checkout re-pointed to another remote ends up with the OLD slug and the NEW host, and the git-origin arm grants Work on that mismatched pair. Pre-existing (0 live rows), but this doc widens the reach, so it closes here: `record_repo_host(session_id, host, slug)` writes `repo_host_slug` from the same probe (`outcome.resolved_slug()`, `sessions/src/index.rs:147-150`), and its no-change guard becomes `repo_host IS NOT ?2 OR repo_host_slug IS NOT ?3` (today `repo_host IS NOT ?2`, `db/routing.rs:95`, which would never write on a re-point between two `github.com` repos). The column flows through `EvidenceRow`, the single and batch evidence reads, and export's own projection (`sessions/src/db/query.rs:36`, `:298`, `:392`). And `host_confers_work` is `Some(false)` when `repo_host_slug` is present and differs from `repo` (`sessions/src/routing.rs:130`). Pre-v14 rows (NULL `repo_host_slug`) keep today's behavior until the next index pass. The pairing only ever REMOVES authority, so it obeys the strip-only rule (`scope.rs:607-612`).
- **Disagreement disclosure** (`anchor_disagrees_with_remote`, `scope.rs:637`; `warn!` at `sessions/src/enrich.rs:154-165`, doctor count at `sessions/src/db/routing.rs:526-531`): for `Deferred`, the anchor's answer for disclosure is Work when `matches(slug)` and the slug owner equals the cwd owner, else Personal; `Excluded` answers Personal. A listed sibling worktree reports no disagreement; the synthetic fixture `work_remote_in_personal_dir` (`common/src/checkout.rs:317-320`) reports personal-vs-work exactly as today.
- **Cost of deferral:** every session under a `Named` owner that is not matched becomes provisional personal, re-classified each enrich pass. Measured on desk: 743 `skipped-personal` rows (793 total) under unlisted `~/repos/scottidler/*` dirs. No LLM call, and the `IS NOT` guard means no write when unchanged, same as today's unanchored rows.
- Phase 2 changes classifier LOGIC (owner rules, deferral, excludes), so it bumps `SCOPE_VERSION` 4 -> 5 per the module's own rule (`scope.rs:52-54`). Policy changes after that ride the fingerprint.
- `clyde doctor` prints `reposlugs-ptns: otto-rs/otto, scottidler/claude, ...` (normalized, sorted; `none` for `[]`) in the paths block beside the always-printed `config:` line (`clyde/src/doctor.rs:715`), not in `print_attribution`: `attribution()` returns before loading config when no catalog exists (`doctor.rs:126-129`).

### Implementation Plan

No Phase 0: the design rests on no unproven environmental assumption. The behavioral claim (re-offered target rows classify Work) follows from the rules above and is asserted by the live-catalog acceptance criterion.

#### Phase 1: `reposlugs-ptns` config key
**Model:** sonnet
- `ReposlugPtn` parse, `reposlugs_ptns` field, `de_reposlugs_ptns`, `DEFAULT_REPOSLUGS_PTNS`, accessor, unit tests for each rejection
- Not yet read by the classifier
- **Success criteria:**
  - `[" tatari-tv/*"]`, `["tatari-tv"]`, `["a/b/c"]`, `["/b"]`, `["*/x"]`, `["tatari-tv/philo-*"]`, `["!"]` each fail `config::load_from` with `reposlugs-ptns` in the message
  - `["Otto-RS/Otto", "!Tatari-TV/X"]` loads as include `otto-rs/otto`, exclude `tatari-tv/x`; `[]` loads as empty
  - a `clyde.yml` without the key, and no `clyde.yml` at all, both yield `reposlugs_ptns() == ["tatari-tv/*"]`
  - an unquoted `- !tatari-tv/x` fails with `reposlugs-ptns` and the quote-it hint

#### Phase 2: `ScopePolicy` replaces `Anchors` and `WORK_ORGS`
**Model:** opus
- Rename `Anchors` -> `ScopePolicy` with `ptns`; `Anchor`, `OwnerRule`; `scope_of`, `is_work_slug`, `anchor_disagrees_with_remote` (`scope.rs:637-644`) read it; `SCOPE_VERSION` 4 -> 5
- `Anchors` has 50 references in 16 files; production sites beyond the three constructors: `session/src/lib.rs`, `sessions/src/routing.rs:18,107`, `sessions/src/db/routing.rs:19,457,488`, `sessions/src/export.rs`, `sessions/src/db/query.rs`, `sessions/src/enrich.rs`, comment in `common/src/repo.rs:937`
- Delete `WORK_ORGS`; update the three production construction sites (`clyde/src/main.rs:436`, `:950`, `clyde/src/doctor.rs:144`) and the `EnrichOptions` / `ExportContext` defaults
- Update every test construction site (~41: `session/src/scope/tests.rs`, `sessions/src/enrich/tests.rs`, `sessions/src/db/query/tests.rs`, `sessions/src/db/routing/tests.rs`, `sessions/tests/export.rs`, `efficiency/src/persist/tests.rs`) to pass patterns explicitly; `clyde/tests/matrix.rs:230-242` and `clyde/tests/serve.rs:25` write `reposlugs-ptns` into their `clyde.yml`
- Doc comments stop citing `WORK_ORGS`: `common/src/checkout.rs:32`, `session/src/scope.rs:664`, `session/src/scope/tests.rs:473`, `:497` (the last is a BITES note naming the lookup to restore; rewrite it against `ScopePolicy::matches`)
- One commit: the `Anchors` -> `ScopePolicy` rename and `new()` signature change break every construction site at once, tests included
- **Success criteria:**
  - `rg -n '\bWORK_ORGS\b' -g '*.rs' .` returns zero lines
  - test, ptns `["otto-rs/*"]`: `<root>/otto-rs/x` Work settled; `<root>/tatari-tv/x` Personal settled
  - test, ptns `["tatari-tv/*", "!tatari-tv/secret"]`: `<root>/tatari-tv/x` Work; `<root>/tatari-tv/secret` Personal `settled: false`; `<root>/tatari-tv/x` with remote `tatari-tv/secret` Personal; the fork fixture `<repo-root>/tatari-tv/clyde-fork` (remote `scottidler/clyde-fork`) still Work
  - test, ptns `["scottidler/claude"]`, every Work basis `GitOrigin`, settled: `<root>/scottidler/claude/x` with remote `scottidler/claude` Work; `<root>/scottidler/claude/.claude/worktrees/f2` with remote `scottidler/claude` Work; sibling `<root>/scottidler/claude-feature` with remote `scottidler/claude` Work; `claude.git` and renamed (`claude-main`) containers with remote `scottidler/claude` Work; `<root>/scottidler/claude` with remote `scottidler/claude-fork` Personal `settled: false`; `<root>/scottidler/claude` with no remote Personal `settled: false`; `<root>/milwaukie-youth-football/x` Personal settled
  - test, cwd `<root>/scottidler/second-brain` with no remote, ptns `["scottidler/claude"]`, under each edit evidence (none; empty `repos_touched`; `{scottidler/claude: 1}`; `{scottidler/claude: 1, scottidler/eratosthenes: 1}`): every case Personal, `settled: false`, basis not `TouchSet`
  - test, ptns `["tatari-tv/*", "scottidler/dotfiles"]`: unanchored `/tmp/x` with remote `scottidler/dotfiles` Work; unanchored with touch set `{scottidler/dotfiles: 2, tatari-tv/x: 1}` (all edits accounted) Work; `{scottidler/dotfiles: 1, scottidler/eratosthenes: 1}` not Work; with `!scottidler/dotfiles` added, both Personal
  - test: the synthetic fixture `work_remote_in_personal_dir` (`<repo-root>/scottidler/philo`, remote `tatari-tv/philo`; built by the test harness, not on disk), ptns `["tatari-tv/*", "scottidler/claude"]`: Personal `settled: false`; `anchor_disagrees_with_remote` reports personal-vs-work; for the listed sibling worktree above it returns `None`
  - test, at the deferred shape (`<root>/scottidler/claude-feature`, slug `scottidler/claude`), not the flat layout of the existing guard tests (`scope/tests.rs:1042`): untrusted host -> Personal `HostRefused`; unresolved ssh alias -> refused; recorded negative probe and unreadable probe -> Personal `ProbeRefused`; NULL host -> Work (inherited trust, stated)
  - test: mixed case matches on all paths: ptn `Scottidler/Claude` vs remote `scottidler/CLAUDE`; cwd `<root>/Otto-RS/x` under `otto-rs/*`; `repos_touched` key `Otto-RS/otto`. Compare per extracted segment; never lowercase whole paths or re-key the touch-set map
  - test: `SCOPE_VERSION == 5`; `EnrichOptions::default()` never yields Work for `<root>/tatari-tv/x`
  - test, `Excluded` is terminal: ptns `["tatari-tv/*", "!tatari-tv/secret"]`, cwd `<root>/tatari-tv/secret` with no remote and touch set `{tatari-tv/x: 1}` (all accounted) is Personal `settled: false`, basis not `TouchSet`; `["tatari-tv/*", "!tatari-tv/*"]` makes bare `<root>/tatari-tv` Personal
  - test: a named include and an exclude for the same slug (`["scottidler/claude", "!scottidler/claude"]`) is Personal; a Wide and a Named entry for one owner (`["tatari-tv/*", "tatari-tv/x"]`) is Wide; a mixed-case exclude `!Tatari-TV/Secret` excludes `tatari-tv/secret`; a stranded exclude (`["tatari-tv/*", "!scottidler/private"]`) survives normalization and vetoes `<root>/tatari-tv/fork` with remote `scottidler/private`
  - test: touch-set Work is checked over every original `repos_touched` key (excludes included), not a lowercased or merged map

#### Phase 3: Schema v14 and re-offer
**Model:** opus
- `scope_policy` and `repo_host_slug` columns; migration step plus `migrate::snapshot_before_v14` registered in `Db::init` beside v10-v13 (`sessions/src/db.rs:303-310`)
- `repo_host_slug` write in `record_repo_host` and the mismatch refusal in `classify_row`
- Fingerprint writes at the three sites, predicate restructure, `IS NOT` guard; correct the `enriched_at` comment at `db/enrich.rs:404-408`
- **Success criteria:**
  - test: changing ptns (including adding only an exclude) re-offers `skipped-personal` rows whose stored `scope_policy` differs; `ok` rows are not re-offered by a policy change (freshness still applies to them, as today); a reordered, case-varied, duplicated list produces the same fingerprint
  - test, against a SETTLED-personal fixture: a second enrich pass under the same policy writes 0 rows (revision counter unchanged); a settled v13 row (NULL `scope_policy`) is re-offered exactly once, then settles; an interrupted sweep leaves unvisited rows eligible and the next pass finishes them
  - test: the `ok` -> narrowed `skipped-personal` (with `enriched_at` set) -> widened sequence re-offers the row; a previously-enriched row now provisional personal is re-offered each pass and writes 0 rows on the second
  - test, an end-to-end re-point: index a checkout with remote `scottidler/claude`, change its origin to `scottidler/second-brain` (same `github.com` host), re-index; `repo_host_slug` is now `scottidler/second-brain`, the row is Personal, basis `HostRefused`, and enrich sends 0 bodies for it; export carries the refused scope
  - test: the same row with `repo_host_slug=scottidler/claude` is Work
  - test: down/up, driven twice (5 -> 4 -> 5 -> 4 -> 5): a row re-settled by the old writer with `scope_version=4` and the current fingerprint is re-offered under `SCOPE_VERSION` 5 each time, then settles; `v13 -> v14` open writes `snapshot_before_v14`

#### Phase 4: Doctor line and docs
**Model:** sonnet
- `clyde doctor` `reposlugs-ptns:` line
- `README.md:111-138` config section: fix stale `repo-root` -> `repo-roots`, add `work-remote-hosts` and `reposlugs-ptns` (the two forms, `!` exclude and the YAML quoting it needs, default, `[]`)
- Ship ONE annotated `clyde.yml.example` at repo root (precedent: `slack-cli/slack.yml.example`) with every supported key
- `docs/session-export-contract.md:104,277-285`: scope wording says configured reposlug patterns, not `repos/tatari-tv`
- **Success criteria:**
  - every supported YAML key (not the `repo-root` tombstone, `config.rs:340-348`) appears in both `README.md` and `clyde.yml.example`, and `clyde.yml.example` loads through `config::load_from` in a test
  - `clyde doctor` output contains a `reposlugs-ptns:` line; with `[]` it prints `reposlugs-ptns: none`

## Acceptance Criteria

- [ ] `rg -n '\bWORK_ORGS\b' -g '*.rs' .` returns zero lines.
  - Observed on main (`23a82fe`): 7 lines. Code: `session/src/scope.rs:74`, `:480`, `:677`. Doc comments: `common/src/checkout.rs:32`, `session/src/scope.rs:664`, `session/src/scope/tests.rs:473`, `:497`.
- [ ] `clyde.yml` containing `reposlugs-ptns: ["tatari-tv/*"]` loads; `clyde session enrich --dry-run` exits 0.
  - Observed on main (installed `clyde v0.25.6`): with no `clyde.yml`, exit 0. With the key: exit 1, `unknown field \`reposlugs-ptns\`, expected one of \`date-tz\`, ..., \`work-remote-hosts\``.
- [ ] `clyde.yml` containing `reposlugs-ptns: ["*/x"]` makes `clyde session enrich --dry-run` exit non-zero with `reposlugs-ptns` in stderr, for the pattern reason.
  - Observed on main: exit 1, but for the unknown-field reason above. Cannot pass meaningfully until Phase 1.
- [ ] With no `clyde.yml` and no catalog (`XDG_CONFIG_HOME` and `--db` pointed at empty scratch dirs), `clyde doctor` prints `reposlugs-ptns: tatari-tv/*`.
  - Observed on main: 0 lines containing `reposlugs-ptns`. Cannot pass until Phase 4.
- [ ] With the operator's list (Data Model) and one `clyde session enrich` run, the enrich-eligible target population returns 0. Owner-wide entries select by cwd; named repos select by the stored rule-1 slug (dormancy as `dormancy_at()` = `coalesce(activity_at, modified)`, exclusion as `enrich_candidates`): `sqlite3 ~/.local/share/clyde/sessions.db "select count(*) from sessions where ((cwd like '/home/saidler/repos/tatari-dev/%') or (cwd like '/home/saidler/repos/otto-rs/%' and repo_source='git-origin' and repo='otto-rs/otto') or (cwd like '/home/saidler/repos/scottidler/%' and repo_source='git-origin' and repo in ('scottidler/dotfiles','scottidler/claude','scottidler/keep'))) and enrich_status='skipped-personal' and not (archived=1 and staged_path is null) and coalesce(activity_at, modified) < strftime('%Y-%m-%dT%H:%M:%S','now','-7 day');"`
  - Observed on main (at writing; drifts up as rows age past the 7-day window): 565. Preconditions checked live: every target row has `attempts=0` (below `max_attempts`, `db/enrich.rs:396`) and none carries a `scope_override`. The 743 provisional rows under unlisted `scottidler/*` dirs are deliberately outside this population.
- [ ] Same population and run: `select count(*) ... and enrich_status='ok'` (the query above with `enrich_status='ok'` in place of the `skipped-personal`, archive and dormancy terms) is at least the pre-run eligible count, proving the rows enriched, not merely left `skipped-personal`.
  - Observed on main: 0 `ok` rows in the population (605 `skipped-personal`, 4 NULL status).
- [ ] `sqlite3 ~/.local/share/clyde/sessions.db "select count(*) from pragma_table_info('sessions') where name in ('scope_policy','repo_host_slug');"` returns 2, and `pragma user_version` returns 14.
  - Observed on main: 0 and 13.

## Resolved Decisions

- 2026-09-27: one `reposlugs-ptns` list replaces `work-orgs` + `work-repos`; `!` prefix is an exclude, exclude wins. Supersedes the earlier "two one-way lists, personal -> work only" ruling. (Scott)
- 2026-09-27: absent key or missing `clyde.yml` -> built-in `["tatari-tv/*"]`; a present key replaces it; `[]` = zero work repos. (Scott)
- 2026-09-27: named repos are identified by the stored rule-1 reposlug, never a directory name; owner-wide `<owner>/*` keeps today's path anchor so the default reproduces current behavior. (Scott on reposlug; author on the owner-wide split)
- 2026-09-27: exclude syntax is a `!` prefix in the one list, not separate `include`/`exclude` keys. Precedent: github-setup-rs topics (`src/actions.rs:311`). Keeps "one list". (author)
- 2026-09-27: only two pattern forms, `<owner>/*` and `<owner>/<repo>`; no fuzzy prefix/contains (fails open on a send gate), no mid-segment globs (no glob crate today). (author)
- 2026-09-27: panel round 4 M2 closed by fixing: host/slug pairing via `repo_host_slug`. (author)
- 2026-09-27: panel round 3 M1 closed with option (i), narrow defer: a `Named` owner reaches Work only via a matched git-origin slug, never via touch set or another owner's remote. (author)
- 2026-09-27: re-offer on a POLICY change is keyed on a stored per-row policy string, not a `SCOPE_VERSION` bump; Phase 2's logic change bumps 4 -> 5 once. (author; both reviewers agreed, round 1)
- 2026-09-27: fingerprint covers `repo-roots` as well as the patterns; `work-remote-hosts` excluded because host refusals are already `settled: false` (`scope.rs:298-303`). (author; both reviewers agreed, round 1)
- 2026-09-27: matching is case-insensitive per segment. (author; both reviewers agreed, round 1)
- 2026-09-27: re-offer covers enrich-eligible rows only. (author, panel round 1 M2)
- 2026-09-27: per-row TEXT fingerprint over meta-table/hash; Phase 2 as one commit. (architect + staff engineer, round 1)
- 2026-09-27: panel round 4 dropped the architect's whole-owner AC rewrite: the 743 provisional rows never reach 0 by design. (panel synthesizer; recorded here)

## Alternatives Considered

### Widen `WORK_ORGS` in code (b83efe1c)
- **Why not chosen:** operator identity compiled into a shared tool; every teammate inherits it; next change needs another release

### Two lists, `work-orgs` + `work-repos` (panel rounds 1-4 draft)
- **Why not chosen:** two keys for one concept; `[]` meant different things on each; no excludes. Replaced by the owner.

### Separate `include:` / `exclude:` keys
- **Why not chosen:** two lists again; `!` in one list is the in-house precedent and keeps the policy readable top to bottom.

### Meta-table "last policy" + bulk NULL of `scope_version`
- **Why not chosen:** same selectivity as the per-row column; per-row wins on provenance: each row records the policy it was decided under.

### Hash fingerprint
- **Why not chosen:** unreadable in `sqlite3`; `DefaultHasher` unstable across Rust versions; a stable hash needs a new crate.

## Technical Considerations

### Dependencies
- No new crates. No other repo consumes `WORK_ORGS`; blast radius is this repo plus each operator's `clyde.yml`.

### Security
- Adding a pattern sends matching sessions' bodies to the LLM on the next enrich. Fail-closed stays absolute: empty policy -> nothing is Work except a Work `scope_override`; malformed config -> refuse to load; exclude wins over include.
- Narrowing cannot un-send (Non-Goals).
- `scope_override` rows are unaffected: an override outranks classification and `set_scope_override` already NULLs `scope_version` (`sessions/src/db/routing.rs:186-212`).

### Testing Strategy
- Every classifier test names its patterns explicitly (no implicit `tatari-tv`).
- Break-it check per phase: restore the const lookup / drop the exclude check / drop the predicate clause and confirm the phase's tests fail.

### Rollout Plan
- Ship order: this repo -> operator writes `~/.config/clyde/clyde.yml` on desk and lappy (none exists on desk today) -> `clyde session enrich` -> `clyde doctor`.
- Rollback: a v13 binary opens a v14 DB fine (`migrate` returns early at `version >= SCHEMA_VERSION`, `sessions/src/db/migrate.rs:82-85`) but rejects a `clyde.yml` containing `reposlugs-ptns` (`deny_unknown_fields`, `common/src/config.rs:317`). Remove the key, then downgrade. Removing it also drops every exclude: a v13 `enrich` then applies the built-in `tatari-tv` rule and sends sessions an exclude was holding back. Stop enrich (timer) before a downgrade.
- Down then up: rows the v13 binary re-settles carry `scope_version=4`; the `SCOPE_VERSION` 5 bump re-offers them on re-upgrade.
- Teammates: no change on upgrade; the default reproduces today's `tatari-tv` behavior. Anyone with other work repos adds `reposlugs-ptns`.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| First post-upgrade enrich re-offers every `skipped-personal` row (NULL fingerprint; 2111 on desk) | High | Low | Rows that re-settle personal make no LLM call; one pass, then the fingerprint matches |
| Export-cursor churn: the `sessions_updated_at_update` trigger (`sessions/src/db.rs:193-199`) bumps `export_meta.revision` on any row update, so the upgrade and every policy change make `session export --cursor` consumers re-fetch ~2111 rows | High | Low | Accepted; stated so nobody chases it as a bug |
| Unquoted `!` entry in YAML parses as a tag, read as `""` | Med | Low | Load fails loudly on the empty entry with a quote-it hint; README and example show the quoting |
| Typo'd pattern silently matches nothing | Med | Med | `doctor` prints the normalized list; malformed forms rejected at load |
| Wrong pattern sends bodies off-machine | Low | High | Explicit opt-in per pattern; excludes win; doctor shows the list; one-way limitation documented |

## Open Questions

None.

## References

- `session/src/scope.rs`, `common/src/config.rs`, `sessions/src/db/enrich.rs`
- Prior art: `leatherman/leatherman/fuzzy.py` (include/exclude), `github-setup-rs/src/fuzzy.rs`, `src/merge.rs:44`, `src/actions.rs:311`; `gx/local/src/repo.rs:431`, `slam/src/main.rs:37` (reposlug filters)
- Session b83efe1c (the rejected hardcoded widening)
