# Design Document: excise-api-key followups

**Author:** Scott Idler
**Date:** 2026-07-30
**Status:** Draft
**Review Passes Completed:** 5/5

## Summary

`v0.18.0` shipped the api-key excision (#77, https://github.com/tatari-tv/clyde/pull/77) and left six
diagnosed followups, recorded in `docs/design/2026-07-30-excise-api-key-followups-handoff.md`. This doc
executes all six. One phase per handoff item, nothing deferred, item number == phase number. Two of the
phases land in `scottidler/claude`, not here.

## Problem Statement

### Background

The excision was a transport swap: `clyde` stopped reading an Anthropic API key and started driving the
`claude` CLI. It shipped green, live, and verified. The handoff brief is the residue: five things found
along the way that were out of that PR's scope, plus one defect it shipped. Every item there carries the
evidence that established it. This doc does not re-derive any of it; it decides and plans.

### Problem

The six items have no owner and no plan. Left alone:

- a live systemd unit on desk.lan states a falsehood about a credential, and bootstrap cannot self-heal it
- 8 sessions are permanently unenrichable, and the cause is a hypothesis nobody has tested
- 373 em-dashes in `.rs` files contradict a standing rule, with no lint to stop the drift
- 83 `klod` references sit in the tree with the migration's retirement gated on a fact Scott has now waived
- the acceptance-criteria defect class that bit five times in #77's own history has no structural fix
- `common/src/llm/cli/tests.rs` is 1,322 of an allowed 1,500 lines, so the next feature there breaks CI

### Goals

- Close all six handoff items. Scott: "all of them. put them in separate phases. I dont want to defer any of them."
- Fix item 1 in a way bootstrap can self-heal, so the class cannot recur on another host.
- Make the em-dash rule and the tree agree, and add the lint that keeps them agreeing.
- Delete the `klod` migration without stranding a host, now that `ltl-7007` and `mini` are waived.

### Non-Goals

- **Excluded: enriching the 8 failing sessions is not the deliverable.** Phase 2 ships a robustness
  change and a repeatable diagnosis. Whether all 8 then succeed is a measurement, not a requirement:
  a session whose content genuinely defeats the prompt is allowed to stay retired.
- **Excluded: rewriting em-dashes in `docs/**`.** Design docs are point-in-time records. Rewriting
  shipped prose to satisfy a new convention falsifies the record.
- **Excluded: rewriting `cost/patricks-debug-output.txt`.** Captured third-party output. Editing a
  capture makes it not a capture.
- **Excluded: AC6's actual close.** A teammate with no key must run the runbook. Phase 6 owns sending
  and tracking that ask; it cannot own someone else's re-run.
- **Excluded: widening the existing `_variable` lint's scope.** It greps `*/src/` and therefore misses
  `*/tests/` and `*/build.rs`, the same hole Phase 3's em-dash lint avoids. Found while drafting, nobody
  asked for it, and it is a separate change. Recorded here so it is not lost.
- **Parked, revisit when a second host is in play:** per-host bootstrap state reporting. Waived for now
  because Scott waived `ltl-7007` and `mini`.

## Proposed Solution

### Overview

Seven phases, `0..6`. Phase 0 is a zero-code spike because item 2's entire plan rests on an untested
hypothesis. Phases 1..6 map one-to-one onto handoff items 1..6.

| phase | handoff item | repo | model |
|---|---|---|---|
| 0 | 2 (prerequisite) | tatari-tv/clyde | opus |
| 1 | 1, `rewrite_unit` orphaned comment | tatari-tv/clyde | opus |
| 2 | 2, enrich JSON robustness | tatari-tv/clyde | opus |
| 3 | 3, em-dashes | both | sonnet |
| 4 | 4, retire the klod migration | tatari-tv/clyde | sonnet |
| 5 | 5, the acceptance-criteria gate | scottidler/claude | opus |
| 6 | 6, smaller items | tatari-tv/clyde | sonnet |

**Sequencing is not free ordering.** Phase 0 gates Phase 2. Phase 1 must precede Phase 4, because Phase 4
deletes `rewrite_unit` and Phase 1 is what moves the repair off it. **Phase 3's code half runs last of the
clyde phases**, after 1, 2, 4 and 6: it touches 79 files, and sweeping files that Phases 1 and 4 are about
to delete or rewrite is churn that also makes every earlier diff harder to read. Phase 5 and Phase 3's
rule amendment are in the other repo and can go any time, but ship first (see Rollout).

### Architecture

Three of the seven touch real design surface. The rest are mechanical.

**Phase 1 inverts how a drifted unit is repaired.** Today `rewrite_unit` (`clyde/src/bootstrap.rs:1131`)
line-edits an existing unit toward a correct shape: `.replace("klod", "clyde")`, drop `EnvironmentFile=`,
inject `Environment=PATH=` before `ExecStart=`. Editing toward a target is what produced the defect: the
directive went, its explanatory comment stayed. After this phase there is exactly one unit body in the
codebase, `clyde_service_body(claude_path_env)`, and both the fresh-install path and the repair path
write it. Nothing parses comments, so no comment-parsing heuristic can be wrong.

**Phase 2 adds a payload boundary the transport does not currently draw.** `complete_with_usage`
(`common/src/llm/cli.rs:147`) sends exactly `` ```text\n<payload>\n``` `` on stdin, and enrich passes
`prompt: ""`, so argv is `claude -p "" --model ... --system-prompt <SYSTEM_PROMPT> --tools ""`. There is
no instruction after the payload. A session body that opens with `You are a per-PR maintenance agent for
the babysit-prs skill` therefore arrives as the last imperative the model read. The `prompt` slot already
exists and is already empty for enrich, so the remedy needs no signature change; Phase 0 decides whether
that slot lands where it has to.

**Phase 0's answer, 2026-07-30: it does not, and the remedy works anyway.** Measured three ways
(marker probe, swapped-marker control, verbatim echo): `claude -p <text>` is **prepended** to the stdin
payload inside ONE user turn, so the `prompt` slot is a *pre*-payload position on the same side as the
system prompt. A reassertion there was nevertheless measured to recover valid enrich JSON on both
clusters (496 -> 175 and 230 -> 215 output tokens), as were the two post-payload positions. So Phase 2
keeps the `prompt`-slot mechanism and the no-signature-change constraint, and `ENRICH_REASSERT` is
documented as a pre-payload framing directive rather than a post-payload reassertion. Full probe record:
`docs/design/2026-07-30-excise-api-key-followups-implementation-notes.md`, Phase 0.

**Phase 3 puts the rule, the tree, and CI in agreement.** Scott's call, 2026-07-30: amend and kill. The
amendment makes `rules/safety.md` explicit that code comments are in scope (they were implicit, and 373
occurrences say the tree read the silence as an exemption). The kill is one mechanical pass. The lint is a
grep in the `.otto.yml` `lint` task, cloned from the `_variable` deny already there, because a convention
with no lint re-accumulates.

### Data Model

No schema change, and no data mutation. The 8 rows sit at `attempts=6` against a default cap of 5, so a
normal sweep skips them forever. They do not need an `attempts` reset: one post-fix sweep with
`--max-attempts 9` reaches them, and a row that enriches successfully stops being attempt-gated. Writing
SQL against `sessions.db` to un-retire them is the wrong tool, because the fix has to work through the
same path a future sweep uses.

The 8 rows, from `~/.local/share/clyde/sessions.db`, all at `attempts=6` with
`last_error = the \`claude\` CLI reply was not the expected JSON`:

```
8447c97c-310e-4277-a4c2-f00e6e20ec28   0009353f-29b9-4a69-9603-bf7ec73ae214
949d3e15-18f7-48d3-a2ab-d554397f8da9   b4d85bec-fda6-4052-9b1d-6c19a4e4553a
9a45e4bd-dd0e-4d4e-828c-28bf924f00dc   89afdb23-729e-4fb8-975b-a5a285021aeb
b13b9473-6e94-43c8-9ce1-cd99fa0be1af   c0058131-7257-4f89-a8aa-97a8ac1436c8
```

### API Design

**Phase 1.** One new private function; two existing call sites converge on it.

```rust
/// The canonical clyde enrich service body. The one body in this codebase: `install_clyde_timer`
/// writes it for a fresh install and `refresh_clyde_unit` writes it to repair a TRIGGERED drift. Not
/// a reconciler: a unit that drifts in a way no trigger names is left alone. A repair that line-edits
/// an existing unit toward this shape is what stranded a credential comment after Phase 5 of the
/// excision stripped its `EnvironmentFile=` directive; writing one body cannot.
fn clyde_service_body(claude_path_env: Option<&str>) -> String

/// True when the unit text still refers to a credential clyde no longer reads. Widens
/// `refresh_clyde_unit`'s trigger so a unit whose directive is already gone but whose comment survives
/// is still repaired.
fn mentions_retired_credential(text: &str) -> bool
```

`rewrite_unit` keeps its `klod -> clyde` and `sessions enrich -> session enrich` duties for the
legacy-rename path only, where it rewrites a unit whose body is not yet ours to own.

**Phase 2.** No new signatures. `ClaudeCli::enrich` stops passing `""` and passes a reassertion constant:

```rust
/// Frames the payload as data because the payload is untrusted prose that can itself open with an
/// imperative (`You are a ...`). The system prompt is not enough: 8 sessions returned 498 and 665
/// output tokens against a 138.7-token healthy mean, in the payload's OWN output schema.
const ENRICH_REASSERT: &str = "...";
```

Exact wording is Phase 0's output, not this doc's guess. **Amended after Phase 0:** the comment says
"frames", not "restated after the payload". `-p` text is prepended to the stdin payload, so this
constant occupies a pre-payload position; the original wording asserted a placement Phase 0 measured to
be false.

### Implementation Plan

#### Phase 0: Prove or kill the prompt-injection hypothesis
**Model:** opus

Zero code. Item 2's whole plan rests on two unproven facts, and both are cheap to establish.

- Run the handoff's repro on one known-failing session, against a copy, in `$TMPDIR` rather than the
  handoff's literal `/tmp` (sandbox rule):
  ```
  cp ~/.local/share/clyde/sessions.db "$TMPDIR/diag.db"
  clyde --log-level debug session enrich --db "$TMPDIR/diag.db" --max-attempts 9 9a45e4bd
  rg 'complete_with_usage|not the expected JSON' ~/.local/share/clyde/logs/clyde.log | /usr/bin/tail -4
  ```
  Capture the reply text, not just the failure. The handoff established the envelope is valid and
  `stop_reason` is `end_turn`; what nobody has seen is what the model actually wrote.
- Determine where `claude -p <text>` text lands relative to the stdin payload: before it, after it, or
  as a separate turn. Probe directly, with the real argv shape (`--output-format json --tools ""
  --system-prompt ...`), not a bare-flag invocation.
- With that known, hand-probe one remedy against a payload that opens with `You are a per-PR maintenance
  agent for the babysit-prs skill`: does a post-payload reassertion recover JSON where the current shape
  does not.
- Record all three verbatim in `docs/design/2026-07-30-excise-api-key-followups-implementation-notes.md`.

**Success criteria:**
- the notes state, with the captured reply, whether the failing reply obeys an instruction embedded in
  the session payload; a null result is a valid answer and redirects Phase 2 to output-format
  constraint instead of payload framing
- the notes state where `-p` text lands relative to stdin, with the probe's argv and output
- nothing was written to `~/.local/share/clyde/sessions.db`: `sha256sum` it before and after the probes
  and record both digests, identical

#### Phase 1: Converge the enrich unit on one body
**Model:** opus

- Factor `install_clyde_timer`'s `svc_body` literal out as `clyde_service_body(claude_path_env)`.
- Add to it the two directives the live desk.lan unit has and the template lacks:
  `Documentation=https://github.com/tatari-tv/clyde`, and the comment
  `# Default sweep: dormant (>=7d idle), work-scoped only, incremental.` above `ExecStart=`. This is why
  the repair loses nothing worth keeping: the one comment that had to survive is now canonical.
- Add `mentions_retired_credential(text)`. Match on `EnvironmentFile`, `enrich.env`, `Anthropic`, and
  `api key` (case-insensitive), scoped to `#` comment lines and `EnvironmentFile=` directives.
- Widen `refresh_clyde_unit`'s trigger to
  `has_stale_subcommand || has_environment_file || mentions_retired_credential`, and change its write
  from `rewrite_unit(&text, ...)` to `clyde_service_body(...)`. `backup()` already runs first and is
  unchanged, so `.clyde.bak` holds any operator customization the converge discards.
- `doctor`: report a `clyde-enrich.service` that `mentions_retired_credential` as unhealthy, remedy
  `clyde bootstrap`.
- **Document that `.clyde.bak` cannot be restored wholesale.** `backup()` (`bootstrap.rs:376-383`) copies
  the pre-repair unit, credential comment included, so restoring it verbatim re-arms
  `mentions_retired_credential` and the next `bootstrap` discards the customization again. The recovery
  instruction is: strip the credential comment from the backup, then re-apply customizations. On desk.lan
  the discarded set is empty (see Alternatives), so this is a cost for a host we have not met.
- Tests: a fixture whose credential comment survived its directive is repaired to the canonical body; a
  fixture already canonical is a no-op (`refresh_clyde_unit` returns `false`); the canonical body
  contains `Default sweep` and contains no `EnvironmentFile`; `doctor` flags the drifted fixture and
  passes the canonical one.
- Break-it check: delete `mentions_retired_credential` from the trigger and the repair test must fail.

**Success criteria:**
- `cargo test -p clyde bootstrap` and `cargo test -p clyde doctor` pass, with the four new tests named
- `grep -c EnvironmentFile ~/.config/systemd/user/clyde-enrich.service` returns 0 and
  `grep -c Anthropic` returns 0, after one `clyde bootstrap` on desk.lan
- a second `clyde bootstrap` reports `0 steps`, proving the repair is idempotent and not a rewrite loop

#### Phase 2: Make enrich survive an instruction-shaped payload
**Model:** opus

Built on Phase 0's recorded verdict, with two branches decided in advance so the phase cannot stall.

- **If Phase 0 confirms the payload wins:** add `ENRICH_REASSERT` (`sessions/src/llm.rs`) with Phase 0's
  proven wording, and pass it in the `prompt` slot of `complete_with_usage` in place of `""`.
- **If Phase 0 kills that hypothesis:** the remedy is a single stricter re-prompt on parse failure, the
  second of the handoff's three candidates. `ClaudeCli::enrich` retries once with a terse instruction and
  the same payload, and only then errors. One retry, not a loop: the failure is deterministic per the
  handoff's month of logs, so a second identical call is worthless and a third is flailing. Do not
  reach for a prefill or a schema flag unless Phase 0 confirmed one exists on this `claude` version.
- Either way: tighten the parse failure to carry a diagnosis. On `no JSON object found` the operator
  today learns only that parsing failed and has to re-run at debug to see why. **The enrichment happens
  at the call site, not inside `parse_enrich_json`.** That function takes `&str` (`sessions/src/llm.rs:218`)
  and has no access to `tokens_out`, which is destructured in `ClaudeCli::enrich` at `llm.rs:150-154`. So
  `parse_enrich_json` keeps its signature and returns its existing error; `enrich` adds `tokens_out` and a
  bounded preview via `.with_context(...)` where both are in scope. Preview only, per the logging rule.
- **Tests do not use a fake transport.** There is no seam for one: `Transport` declares only `complete`
  (`common/src/llm.rs:23`), `complete_with_usage` is an inherent method on `CliTransport`
  (`common/src/llm/cli.rs:141`), and `ClaudeCli` holds a concrete `CliTransport`, not a generic
  (`sessions/src/llm.rs:88-90`). Building one would mean widening the trait and making `ClaudeCli`
  generic, which is a real architectural change nobody asked for and which contradicts this phase's
  "no new signatures". Test the two pure pieces instead:
  - `parse_enrich_json` directly, since it is a pure function over `&str`: prose-with-no-JSON errors,
    JSON after an imperative preamble parses.
  - the argv, through the existing `CliTransport` `build_spawn` tests in `common/src/llm/cli/tests.rs`:
    per branch, either the reassertion is present for `Kind::Enrich` and absent for `Kind::Slot`, or the
    retry fires exactly once and the second failure propagates.
- Re-run the 8 with `--max-attempts 9`, no SQL, and record how many recover.

**Success criteria:**
- `cargo test -p sessions llm` and `cargo test -p common cli` pass, with the three new tests named
- the implementation notes record the before/after count for the 8 named sessions and each survivor's
  `tokens_out`; a survivor is not required, an explanation is
- a healthy enrich still averages under 200 output tokens, proving the remedy did not make the model
  chattier. Sample: the `tokens_out` of every row the post-fix `--max-attempts 9` sweep enriches
  successfully, excluding the 8 named rows, minimum 20 rows; compare against the 138.7 baseline

#### Phase 3: Amend the em-dash rule, kill all 373, add the lint
**Model:** sonnet

Two repos. `scottidler/claude` first, so the rule states the scope before the tree conforms to it.
**Numbered 3 to match handoff item 3, but executed last of the clyde phases** (see Sequencing). The rule
amendment half is not ordered and ships with Phase 5.

- `scottidler/claude`: amend `HOME/repos/.claude/rules/safety.md` to name the scope explicitly, code
  comments and string literals included, and to name the enforcing lint. The rule listed external
  systems and left code implicit; 373 occurrences are what implicit bought.
- `tatari-tv/clyde`: remove all 373 em-dashes from `.rs` files. Replacements per `rules/voice.md`:
  `--`, a colon, parens, or split the sentence. No blanket substitution: pick per site.
- One site needs care, not replacement: `report/src/render/slots/tests.rs:531` asserts
  `!prompt.contains(C)` where `C` is a literal em-dash char. Rewrite it as `'\u{2014}'` so the assertion
  survives and the tree carries no literal em-dash. This doc uses the escape for the same reason: it
  must pass the lint it specifies.
- Fix the two `README.md` files in the same pass. Leave `docs/**` and `cost/patricks-debug-output.txt`
  alone, per Non-Goals.
- `.otto.yml` `lint` task: add a deny in the shape of the `_variable` grep already there, but scoped with
  `rg`, not `grep -r ... */src/`:

  ```
  if rg -n --type rust -g '!target' '\x{2014}' .; then
    echo "❌ Found em dash in Rust source."
    exit 1
  fi
  ```

  **The scope matters and `*/src/` is wrong.** 17 of the 373 live in `*/tests/` integration files
  (`clyde/tests/{collect,export,search,serve}.rs`, `sessions/tests/export.rs`), which `*/src/` never
  walks. A lint narrower than the criterion is a hole that lets the tree drift back where CI cannot see
  it. Using `rg` also makes the lint and AC2 the same command, so they cannot diverge. `\x{2014}` keeps
  `.otto.yml` itself em-dash-free.

**Success criteria:**
- `rg -c '\x{2014}' --type rust -g '!target' .` returns nothing, and `otto lint` exits 0
- reintroducing one em-dash into `sessions/tests/export.rs` (a path outside `*/src/`) makes `otto lint`
  exit non-zero, proving the scope fix and not just the grep
- `cargo test -p report slots` passes, proving the `'\u{2014}'` rewrite kept the assertion's meaning

#### Phase 4: Retire the klod migration
**Model:** sonnet

Unblocked by Scott, 2026-07-30: `ltl-7007` and `mini` do not gate this. desk.lan is verified clean
(`~/.config/klod` and `~/.local/share/klod` both absent, re-confirmed while drafting).

**Retire the migration, keep a tripwire.** Scott waived the two hosts, not the loud failure. Deleting
`doctor`'s klod detection along with the migration makes a dirty host report **exit 0, fully healthy**:
`Report::healthy` (`doctor.rs:78-86`) is a conjunction of `!is_legacy()` checks plus
`legacy_state.is_empty()`, and `Target::Absent.is_legacy()` is `false` (`doctor.rs:33,38`). Delete the
klod dir checks and `legacy_state` is empty; delete the `Target::Legacy("klod")` arm and `timer_state`
falls through to `Absent`; `healthy()` returns true and `run()` returns `0` (`doctor.rs:22`). Meanwhile
`bootstrap` on that host takes the `!has_legacy` path, finds no clyde unit, has `install_timer == false`,
and reports `0 steps`. Dead klod timer, both tools green. That violates fail-loudly and it makes the old
version of this phase's own doctor criterion unfalsifiable.

So this phase deletes the ~80 refs of *machinery that migrates* and keeps a handful of *detection*:

- Delete the migration from `clyde/src/bootstrap.rs`: the two `migrate_dir` steps for the klod data and
  config dirs, `repoint_systemd`'s `has_legacy` branch, and `repoint_wants_symlink` (`bootstrap.rs:1096`;
  the `Paths` helper is `legacy_wants_link`, the function is `repoint_wants_symlink`, do not conflate).
- **Delete `fn migrate_dir` itself** (`bootstrap.rs:406`). Its only two callers are the deleted steps at
  `bootstrap.rs:296,300`, and `Cargo.toml:12` sets `dead_code = "deny"`, so leaving it fails
  `cargo check` and takes AC4 down with it.
- **`doctor` keeps ALL of its klod detection. Nothing in `doctor` is deleted.** Retain the
  `~/.config/klod` and `~/.local/share/klod` checks that push onto `legacy_state` (`doctor.rs:120-125`)
  and retain `timer_state` whole (`doctor.rs:227-259`). Every line of it is detection, not migration:
  `legacy_present` covers the three `klod-enrich.*` paths, and `execstart_legacy` (`doctor.rs:245`) covers
  a `clyde`-named unit whose `ExecStart` still invokes `klod`. **Dropping `execstart_legacy` leaves a
  residual exit-0 hole**: on a host whose `klod-enrich.service` is already gone but whose
  `clyde-enrich.service` still points at `klod`, `legacy_present` is `false`, `target` falls through to
  `Target::Clyde`, and `healthy()` returns true. Keeping the function whole is simpler than carving it and
  strictly safer.
- **Do not "simplify" `symlink_metadata` to `exists()`.** `doctor.rs:235` uses
  `symlink_metadata(&legacy_link).is_ok()` deliberately: `exists()` follows the link and returns `false`
  for a DANGLING one, which is exactly the residue left by deleting unit files without disabling the
  timer. `bootstrap.rs:1015` used it for the same reason. Normalizing that line reopens the hole silently,
  which is why the dangling case gets its own test below.
- **Fix `doctor`'s remedy string, which this phase falsifies.** `doctor.rs:334` prints
  `legacy targets/state remain, run \`clyde bootstrap\``. After this phase `clyde bootstrap` no longer
  migrates klod, so for the klod case that remedy is a lie. `print_report` has only `report.healthy()`, a
  bool, so the branch needs a named discriminator:
  `report.timer == Target::Legacy("klod") || report.legacy_state.iter().any(|s| s.contains("klod"))`.
  True gets "install a pre-retirement `clyde`, run `clyde bootstrap`, then upgrade"; everything else keeps
  the existing remedy, which is still correct for `ccu` and `claude-permit`.
- **Make the timer half of the tripwire legible.** `unit_name` is `Some` only when a `.service` file
  exists (`doctor.rs:238-244`), so on a host whose only residue is `klod-enrich.timer` or a dangling
  enable symlink, `doctor.rs:290-295` print nothing and the whole report for that host is one line:
  `enrich timer:  klod (legacy)`. Exit is 1 and detection is right, but the operator is never told which
  file to touch. Have the timer detection also push a `legacy_state` entry naming the path; those print
  one per line at `doctor.rs:313-315`, the channel that already works for the dir checks.
- **`clyde bootstrap` still reports `0 steps` on a dirty host, and that is chosen, not overlooked.** After
  this phase bootstrap genuinely cannot help: the remedy is to install a pre-retirement `clyde` first. One
  loud channel that tells the truth beats two, and `doctor` is that channel.
- **`rewrite_unit` loses its last caller and must be deleted with it.** After Phase 1 its only remaining
  duty is the legacy rename, and that is exactly what this phase removes. Leaving it is dead code that
  still contains the line-filter that caused item 1.
- **`refresh_clyde_unit` survives and becomes the only path.** Verified: it is called from
  `repoint_systemd`'s `!has_legacy` branch (`bootstrap.rs:1016-1023`), which this phase keeps. This is the
  one way Phase 4 could silently undo Phase 1, so it gets a success criterion of its own.
- **Rename `repoint_systemd`.** Its name and its doc comment both say "repoint the enrich timer from
  `klod` to `clyde`", which after this phase is false. It becomes a dispatch over three cases: repair a
  drifted unit, install a fresh one, or do nothing. Name it for that.
- Delete the three `klod-*` path helpers from `Paths`: `legacy_unit`, `legacy_timer`, `legacy_wants_link`
  (`bootstrap.rs:104-120`). The handoff said four; there are three.
- The `Target::Legacy` variant stays regardless: it is still required for `ccu`, `claude-permit`, and
  `sessions enrich` (`doctor.rs:200,214,253`).
- **Sweep the 6 `klod` references in `bootstrap.rs` that none of the deletions reach.** None sit in a
  deleted range, and each survives as a lie unless named: `bootstrap.rs:339,344` (step-label strings at
  the call site, in the `!args.skip_systemd` block this phase keeps), `bootstrap.rs:896`
  (`check_stale_env_file` docstring), `bootstrap.rs:1017` (`// No klod state to migrate`, inside the kept
  `!has_legacy` branch), and **`bootstrap.rs:1161-1162` (`refresh_clyde_unit`'s own docstring, which
  mentions `klod` twice and names `repoint_systemd`, the function this phase renames)**.
  `bootstrap.rs:405` goes with `migrate_dir`. `doctor.rs`'s references all stay, because the code they
  describe stays.
- Delete the corresponding tests in `clyde/src/bootstrap/tests.rs` (31 refs) only. `clyde/src/doctor/tests.rs`
  (10 refs) is untouched: it covers detection this phase retains. Deleting tests is correct only for code
  that no longer exists.
- Fix the 3 genuinely stale mentions: `Cargo.toml:16` (provenance comment), `session/src/scope/tests.rs:12`
  (path fixture `tatari-tv/klod/main` -> `tatari-tv/clyde/main`), `README.md:234` (a link to a real
  historical design-doc filename, so keep the filename and reword the sentence around it).
- `README.md`: note that bootstrap no longer migrates klod state, and that a host still carrying it must
  install a pre-retirement `clyde` first.

**Success criteria:**
- every surviving `klod` reference in Rust is inside `doctor`'s tripwire, and `rg -c klod README.md`
  returns >= 1 (the historical design-doc filename, plus the retirement note this phase adds). `rg -n klod --type rust --type toml
  -g '!target' .` must show zero hits in `bootstrap.rs`, `Cargo.toml`, and `session/src/scope/tests.rs`
- `otto ci` exits 0, and `rg -c 'fn rewrite_unit|fn migrate_dir' clyde/src/bootstrap.rs` returns nothing
- Phase 1's repair still works after the deletion: the Phase 1 test that repairs a drifted unit still
  passes, and hand-planting a credential comment in `~/.config/systemd/user/clyde-enrich.service` then
  running `clyde bootstrap` still removes it
- **the tripwire fails loudly on all five residue states.** Two existing tests are RETAINED, not written:
  `legacy_klod_dirs_are_unhealthy` (`doctor/tests.rs:127`) already plants both klod dirs and asserts
  `!report.healthy()`, and `clyde_service_with_klod_execstart_is_legacy` (`doctor/tests.rs:195`) already
  covers the klod-`ExecStart` path. **One test is genuinely new: the dangling
  `timers.target.wants/klod-enrich.timer` symlink, which has no test today** (`rg -n symlink
  clyde/src/doctor/tests.rs` returns nothing). Break-it check for the new one: change `symlink_metadata`
  to `exists()` and it must fail. This replaces the old criterion, which was unfalsifiable: with the
  detection deleted, "reports no legacy state" was true no matter how dirty the host was

#### Phase 5: Gate acceptance criteria on the running system
**Model:** opus

`scottidler/claude`. Five occurrences of one pattern in #77's history: an acceptance criterion written
from the design rather than from the running system. AC2 named `--only`, a flag that does not exist. AC4
required deleting the tests that prove its own phase. AC5 required a non-zero exit that
`report/src/render.rs::no_transport` returns 0 by deliberate design. AC3 and AC7 were caught in review.

- `HOME/.claude/skills/create-design-doc/SKILL.md`: add the execution gate to the ready-to-build
  criteria. Every acceptance criterion that names a flag, column, path, or exit code gets its literal
  command run against current `main` before the doc is called ready, and the observed output recorded in
  the doc next to the criterion. A criterion whose command cannot run yet says so and names why.
- Same file: extend the Key Rules bullet on acceptance criteria to state the failure mode in one line,
  so the reason survives the next edit.
- `HOME/.claude/skills/how-to-execute-a-plan/SKILL.md`: step 0.5 already verifies criteria after
  implementation. Add a back-reference so a criterion arriving unexecuted is treated as a doc defect,
  not as work to do.
- This doc dogfoods the gate by hand, since the gate does not exist yet: every acceptance criterion below
  was typed into a shell against `main` while drafting and its observed output recorded. It caught two
  defects in this doc's own first draft, which is the argument for the gate. The em-dash lint was scoped
  `*/src/` and would have missed 17 sites; the `Paths` klod helper count was 4 and is 3.

**Success criteria:**
- `rg -c 'literal command' ~/repos/scottidler/claude/HOME/.claude/skills/create-design-doc/SKILL.md`
  returns non-zero, and the gate names where the output is recorded
- the Acceptance Criteria section of THIS doc carries an observed-output line per criterion
- `general:skill-reviewer` on the edited skill returns no critical finding

#### Phase 6: The smaller items
**Model:** sonnet

Four unrelated items, **one commit for the phase**, matching every other phase and Rollout. Settled here
rather than left to the executor, because commit structure is the owner's call and taste.md specifies one
otto-ci-green commit per phase.

- **Decompose `common/src/llm/cli/tests.rs`** (1,322 of 1,500). Follow
  `refs/dealing-with-large-files.md`: markers via Edit, split with `head`/`/usr/bin/tail`, never `sed`
  line ranges. The file already carries section banners that are the module boundaries:
  `cli/tests/mod.rs` keeps the five helpers (`transport`, `job`, `envelope_json`, `real_model_usage`,
  `good_envelope`) and declares `argv`, `env`, `envelope`, `guards`, `usage`, `fatal`. The helpers need
  `pub(super)` and each submodule opens with `use super::*;`, because they are currently private items in
  a single file and the split turns every use into a cross-module one. `cli.rs`'s existing
  `#[cfg(test)] mod tests;` declaration is unchanged: it resolves to the directory.
- **Dependabot alert is already closed.** Re-queried while drafting, with the work persona the handoff's
  attempt lacked: alert #1, `quinn-proto`, high, `state: fixed`. `quinn-proto` is absent from
  `Cargo.lock` entirely. No code, no ignore-file. Recorded here so nobody re-investigates.
- **The excision's own AC6** (not this doc's AC6). Send Keegan the ask: run
  `~scott-idler/claude-usage-report-pipeline-runbook` end to end with no API key, report the enrichment
  percentage. Track it to an answer. His re-run is the close, and it is not ours to perform; sending and
  tracking the ask is, which is why AC7 covers it.
- **Cost baseline canary into the README.** 6.3s and ~139 output tokens per enriched row, ~$2.05 per 100
  sessions. If output tokens return to the thousands or per-call wall clock to ~52s, `MAX_THINKING_TOKENS`
  stopped being honored. This belongs in living docs, not a design doc, because it must be re-checked
  after every `claude` upgrade.

**Success criteria:**
- `otto bloat` exits 0 and no file under `common/src/llm/cli/tests/` exceeds 700 lines
- `cargo test -p common` runs the same 62 tests it does today, all passing
- `README.md` carries the enrich cost baseline with the two canary thresholds named, and
  `gh api repos/tatari-tv/clyde/dependabot/alerts --jq '[.[]|select(.state=="open")]|length'` returns 0

## Acceptance Criteria

Per Phase 5's gate, each was executed against `main` while drafting and the observed output recorded.

- [ ] **AC1.** `clyde bootstrap` on desk.lan leaves `~/.config/systemd/user/clyde-enrich.service` with
      zero matches for `EnvironmentFile` and zero for `Anthropic`, and a second run reports `0 steps`.
      *Observed on `main`:* the live unit currently carries 0 `EnvironmentFile` and 3 comment lines, 1
      of which matches `Anthropic`. Criterion is currently FALSE, which is the defect.
- [ ] **AC2.** `rg -o '\x{2014}' --type rust -g '!target' . | wc -l` returns 0 and `otto lint` exits 0.
      *Observed on `main`:* returns **373**. The command is `rg -o … | wc -l`, not `rg -c`, deliberately:
      `rg -c` prints per-file **matching-line** counts, which sum to **356** over 79 files, and 356 is
      also the occurrence count under `*/src/`. Those two 356s collide by coincidence and the collision
      already fooled one reviewer into calling this observation false. Occurrences are the quantity that
      matters, so the criterion names the command that emits them. `otto lint` ran and exited 0 (`✅ No
      trailing whitespace`, `✅ No _variable patterns`) because no em-dash deny exists yet; both halves
      must hold, so the lint is load-bearing. The 17 occurrences outside `*/src/` are why Phase 3's lint
      uses `rg` rather than the existing `grep -r … */src/` shape.
- [ ] **AC3.** `rg -c klod --type rust --type toml -g '!target' .` returns hits in `clyde/src/doctor.rs`
      and `clyde/src/doctor/tests.rs` ONLY, at **>= 17** and **>= 10** respectively; **exactly zero** for
      `bootstrap.rs`, `bootstrap/tests.rs`, `Cargo.toml`, and `session/src/scope/tests.rs`; and
      `rg -c klod README.md` returns **>= 1**, including the historical design-doc filename.
      Post-implementation counts get recorded in the notes.
      *Observed on `main`:* 82 across 6 Rust/TOML files (`bootstrap.rs` 22, `bootstrap/tests.rs` 31,
      `doctor.rs` 17, `doctor/tests.rs` 10, `Cargo.toml` 1, `session/src/scope/tests.rs` 1) plus
      `README.md` 1. The handoff's 83 counted the README; the Rust/TOML surface is 82.
      *Amended twice during review.* First: the original criterion demanded zero `klod` in Rust, which is
      incompatible with the detection Phase 4 retains, since it must name the klod paths to find them. A
      criterion that forces deleting the tripwire is the defect class Phase 5 exists to prevent.
      **Amended a third time, during implementation.** `rg -c klod README.md returns exactly 1`
      contradicted Phase 4's OWN bullet requiring a README note that bootstrap no longer migrates klod
      state -- a note that cannot be written without naming klod. Two bullets of the same phase
      contradicted each other again, and the count was again measured before the change it governs.
      Changed to `>= 1`. Gaming the line count to satisfy `exactly 1` was rejected: the note is the
      substantive requirement (it is what tells a stranded host what to do), and shrinking prose to fit
      a metric is how a criterion starts driving the implementation instead of checking it.
      Second, and this one is subtler: pinning it to `exactly 17` and `exactly 10` was **a criterion
      measured before the change it governs**, the mirror image of the same defect. Phase 4 adds a klod
      discriminator to `doctor`'s remedy branch, which must reference `klod`, so `doctor.rs` goes to at
      least 18; and the new dangling-symlink test raises `doctor/tests.rs`. Two bullets of the same phase
      contradicted each other. `>=` still catches the thing being guarded, which is deletion of detection,
      and functional deletion is caught by AC6's break-it check, which no count ever could.
- [ ] **AC4.** `otto ci` exits 0 with `✅ All CI checks passed!`, and `otto bloat` reports no file over
      1,500 lines with `common/src/llm/cli/tests.rs` no longer present as a single file.
      *Observed on `main`:* `common/src/llm/cli/tests.rs` is 1,322 lines, under the limit, so `otto bloat`
      passes today. The criterion is about the decomposition, not the limit.
- [ ] **AC5.** The implementation notes record, for the 8 named sessions, how many recovered and the
      `tokens_out` of each survivor, plus Phase 0's verbatim verdict on the injection hypothesis.
      *Observed on `main`:* all 8 sit at `attempts=6` with `last_error = the \`claude\` CLI reply was not
      the expected JSON`. A recovery count of 0 satisfies this criterion if the explanation is recorded;
      an unrecorded outcome does not.
- [ ] **AC6.** A dirty klod host fails loud after Phase 4, across the dir half AND the timer half:
      `legacy_klod_dirs_are_unhealthy` (`doctor/tests.rs:127`) and
      `clyde_service_with_klod_execstart_is_legacy` (`doctor/tests.rs:195`) both still pass, a NEW test
      asserts `diagnose(...).healthy() == false` for a dangling
      `timers.target.wants/klod-enrich.timer` symlink, and changing `symlink_metadata` to `exists()`
      (`doctor.rs:235`) makes that new test fail.
      *Observed on `main`:* `Report::healthy()` (`doctor.rs:78-86`) returns false today because
      `legacy_state` carries the klod dir checks. After Phase 4 without a tripwire it would return true
      and `clyde doctor` would exit 0 on that host, which is why this is an overall criterion and not a
      per-phase one.
- [ ] **AC7.** The `create-design-doc` skill carries the execution gate, and Keegan has the AC6 ask.
      `rg -c 'literal command' ~/repos/scottidler/claude/HOME/.claude/skills/create-design-doc/SKILL.md`
      returns non-zero, and the implementation notes record the date the runbook ask was sent plus its
      current state.
      *Observed on `main`:* the skill's ready-to-build gate does not mention executing criteria; the
      phrase is absent. The ask has not been sent.

## Resolved Decisions

- **2026-07-30, Scott: all six items, one phase each, nothing deferred.** Verbatim: "all of them. put
  them in separate phases. I dont want to defer any of them." This overrides the handoff's own triage,
  which routed items 1 and 6 to targeted fixes with no doc.
- **2026-07-30, Scott: `ltl-7007` and `mini` do not gate Phase 4.** Verbatim: "note I dont care about
  ltl-7007 and mini. dont think they should block us." The handoff called their state "the blocking
  fact"; it is waived. Phase 4's README note is what replaces the check.
- **2026-07-30, Scott: amend the rule AND kill the em-dashes.** Verbatim: "yes ammend and kill all
  em-dashes." Read as: the amendment makes the rule's scope explicit rather than exempting code, and the
  tree conforms. Half-enforcement was the one option the handoff said to avoid, and a lint is what
  prevents it.
- **2026-07-30, author: Phase 1 converges on a canonical body instead of dropping the orphaned comment
  block.** The handoff proposed dropping the contiguous `#` block preceding the stripped directive, and
  separately required `# Default sweep:` to survive. Those two are compatible only for one of the two
  possible original line orderings, and the ordering is unrecoverable: no `.clyde.bak` exists for that
  unit, and `git log --all -S 'Anthropic key lives here'` and `-S 'Default sweep'` both return nothing,
  so the klod-era unit was hand-authored and never in the repo. A fix whose correctness depends on an
  unrecoverable fact is not a fix. See Alternative 1.
- **2026-07-30, author: item 6's dependabot bullet needs no work.** Already `fixed`; see Phase 6.

### Review panel, 2026-07-30 (Architect / gemini-3.1-pro-preview + Staff Engineer / codex, both rc=0)

Both reviewers converged that Phase 1's canonical-converge is sound and told me not to reopen
Alternatives 1 or 2. The converge's cost on desk.lan was quantified during review and is **two comment
lines and nothing else**: `Nice=10` is already in the template (`bootstrap.rs:1269`), so it was never an
operator customization, and `Documentation=` is the only other divergence, which Phase 1 adopts.

Folded in as plan changes:

- **Phase 4 made a dirty klod host report exit-0 healthy.** Convergent, critical, verified against
  `doctor.rs:78-86,33,38,22`. Phase 4 now retains a tripwire and AC6 exists to keep it honest. The old
  Phase 4 criterion ("`clyde doctor` reports no legacy state") was unfalsifiable after its own deletions.
- **`fn migrate_dir` had to be deleted, not just its callers.** `dead_code = "deny"` (`Cargo.toml:12`)
  makes it a build break, which would have taken AC4 down.
- **12 `klod` references sat outside every deleted range**, including `refresh_clyde_unit`'s own docstring
  (`bootstrap.rs:1161-1162`), which names the function Phase 4 renames. Phase 4 now enumerates them.
- **The `tokens_out` diagnosis was impossible where the doc put it.** `parse_enrich_json` takes `&str`;
  the enrichment moved to `ClaudeCli::enrich`.
- **Phase 2's fake-transport test had no seam.** `Transport` declares only `complete`; the tests now
  target the pure function and the existing `build_spawn` argv tests instead of widening a trait.
- **AC2 recorded occurrences while naming a command that emits matching lines.** Command changed to
  `rg -o … | wc -l`. Two independent 356s (total matching lines, and occurrences under `*/src/`) collide
  by coincidence, which is exactly how a competent reviewer read the observation as false.
- **`repoint_wants_link` does not exist**; the function is `repoint_wants_symlink` (`bootstrap.rs:1096`).
- Also folded: AC6/AC7 added for Phase 5 and the Keegan ask; Phase 6 commit policy settled at one;
  measurement definitions added to Phase 0 and Phase 2; the `.clyde.bak` re-arming caveat; Phase 3's risk
  likelihood dropped to Low; `clyde_service_body`'s "SINGLE source of truth" reworded, since the converge
  is trigger-gated and not a reconciler.

Two corrections found while verifying the panel's M1 remedy myself, after the panel went idle without
answering the follow-up:

- **`doctor` loses nothing at all.** My first M1 fix said to keep a tripwire and delete the
  "repair-oriented" part of `timer_state`. Reading `doctor.rs:227-259`, there is no repair-oriented part:
  it is all detection. Dropping `execstart_legacy` (`doctor.rs:245`) would have left a second exit-0 hole
  for a host whose `klod-enrich.service` is gone but whose `clyde-enrich.service` still invokes `klod`.
  Phase 4 now touches `doctor` only to fix its remedy string.
- **`doctor`'s remedy string becomes false.** `doctor.rs:334` says to run `clyde bootstrap`, which after
  Phase 4 cannot migrate klod. Branching that message is now a Phase 4 bullet.

Second panel round, on the M1 remedy itself. It confirmed the tripwire closes M1 and that keeping
`timer_state` whole is the only version needing no new proof, since the detection surface stays
byte-identical to `main`, which detects correctly today. It also enumerated all five residue states as
covered. Three more folds came out of it:

- **AC3's exact counts were a criterion measured before the change it governs.** Phase 4's own remedy
  branch must reference `klod`, so `doctor.rs` cannot stay at 17. Changed to `>=` for the two doctor files,
  exact zero for the other four. This is the same defect class as #77's AC2/AC4/AC5, caught here in the
  doc that adds the gate against it, on the second pass over the same criterion.
- **The timer half of the tripwire is illegible and had no criterion.** A host whose only residue is
  `klod-enrich.timer` or a dangling enable symlink produces a one-line report naming no path, because
  `unit_name` is `Some` only when a `.service` exists. The timer detection now pushes a `legacy_state`
  entry, and AC6 covers the timer half.
- **The dangling-symlink case has no test today**, and it is the state most likely to be "simplified" away
  later by normalizing `symlink_metadata` to `exists()`. New test plus a break-it check.

Pushbacks, not folded, with rationale:

- **Staff: "AC2's observation is false, 373 should be 356."** Rejected. Staff ran `rg -c`, which counts
  matching lines, and compared it against an occurrence count. Occurrences are 373 and the 17 outside
  `*/src/` are correct; re-verified with `rg -o … | wc -l`. The real defect was units, not the number, and
  it is fixed above.
- **Architect: "AC5 asserts a Non-Goal; ACs must assert requirements."** Rejected. AC5 asserts that the
  notes *record* the outcome and states outright that a recovery count of 0 satisfies it. That is
  consistent with the non-goal, not in tension with it. No change.

## Alternatives Considered

### Alternative 1: drop the contiguous comment block above the stripped directive
- **Description:** the handoff's own proposal. When line filtering drops `EnvironmentFile=`, also drop
  the contiguous `#` block immediately preceding it.
- **Pros:** smallest possible diff; preserves every operator customization; no change to the fresh-install
  template.
- **Cons:** correctness depends on where `EnvironmentFile=` sat relative to
  `# Default sweep: dormant...`. If the directive sat below all three comments, the block drop eats the
  comment the handoff says must survive. If it sat between comment 2 and comment 3, the drop is correct.
  The observed post-rewrite file is identical under both orderings, so the file cannot distinguish them.
- **Why not chosen:** the ordering is unrecoverable from disk and from git history (both checked). Every
  content-matched refinement also fails: the credential comment's second line
  (`# inherit the interactive shell environment. Never committed; desk-only.`) mentions no credential
  token, so per-line matching orphans it, and first-match-to-end-of-block or last-match-to-end-of-block
  both drop all three lines.

### Alternative 2: detect only, never repair
- **Description:** leave `rewrite_unit` alone; `doctor` reports a unit mentioning a retired credential as
  unhealthy and tells the operator to fix it by hand.
- **Pros:** zero risk of discarding an operator customization; fails loudly.
- **Cons:** the handoff's actual complaint is that bootstrap cannot self-heal, and this does not change
  that. It converts a silent falsehood into a permanent nag.
- **Why not chosen:** a check that can only ever say "still broken" is not a fix. Phase 1 keeps the doctor
  check as the loud signal and adds the repair behind it.

### Alternative 3: raise `--max-attempts` for the 8 failing sessions
- **Description:** no code; sweep them with a higher cap.
- **Pros:** free.
- **Cons:** the handoff established the failure is deterministic across two transports and a month of
  logs (269 api-era occurrences, 10 cli-era). Retrying a deterministic failure is the 2-strike
  antipattern.
- **Why not chosen:** already ruled out by evidence in the handoff.

### Alternative 4: one PR for all six items
- **Description:** a single branch, single PR.
- **Pros:** one review, one merge.
- **Cons:** two of the phases are in a different repo, so a single PR is not physically possible; and a
  PR mixing a 373-site mechanical sweep with a systemd repair makes both unreviewable.
- **Why not chosen:** ruled out by the cross-repo split. See Rollout.

## Technical Considerations

### Dependencies

- `claude` CLI on `PATH` for Phase 0's probes and Phase 2's re-run. No new crates in any phase.
- Phase 3 and Phase 5 depend on `~/repos/scottidler/claude`, a personal repo, so `gh` runs under the home
  persona there and the work persona here. The `gh()` function keys on `$PWD` and gets this right for
  both; `GH_PERSONA` is the override if a call is made from the wrong directory.
- Phase 6's AC6 bullet depends on Keegan, who is not on this critical path.

### Performance

Phase 2 is the only phase that can move a number. The reassertion adds tens of input tokens per enrich
call against a payload that runs to 500KB (`enrich::SEND_CAP_CHARS`), so input cost is unmoved. The risk
worth watching is output: a reassertion that makes the model verbose would raise the ~139-token mean,
which is why that is a Phase 2 success criterion and why Phase 6 puts the canary in the README.

### Security

- Phase 1 widens what bootstrap rewrites. `backup()` runs before every write and is unchanged, so
  `.clyde.bak` is the recovery path for a discarded customization.
- Phase 1's `mentions_retired_credential` matches on directive and comment text only, never on a value.
  No secret is read, logged, or echoed. The excision already established that clyde reads no credential
  file; this phase deletes the last text claiming otherwise.
- Phase 2 quotes a bounded preview of a model reply into an error. Session payloads are already redacted
  upstream (`redaction_count` is a durable column) and the preview is truncated by chars, matching
  `preview_truncates_by_chars_and_survives_multibyte`.

### Testing Strategy

- Every phase ships `otto ci` green with exactly one commit, except Phase 6 where four unrelated items may
  take one commit each.
- Phase 1 and Phase 3 each carry a break-it check: remove the fix, prove the new test fails. Phase 1's is
  deleting `mentions_retired_credential` from the trigger; Phase 3's is reintroducing one em-dash.
- Phase 3 is the one phase where a green CI is not sufficient evidence, because a bad replacement can be
  green and still garble a sentence. The 373 sites get read, not batch-substituted.
- Phase 4 deletes tests. That is legitimate only because they cover deleted code, and `otto ci` green plus
  `clyde bootstrap` reporting `0 steps` on desk.lan is what proves nothing live regressed.
- Phase 6's decomposition must not change the test count: 62 before, 62 after.

### Rollout Plan

Two repos, so two PRs and a forced order.

1. `scottidler/claude`: Phase 3's rule amendment and Phase 5's skill gate. One PR, home persona
   (`~/repos/scottidler/*`, so the `gh()` function picks `github-pat-home` off `$PWD`). Lands first so
   the rule states its scope before this tree conforms to it, and so the gate exists before the next
   design doc is written.
2. `tatari-tv/clyde`: Phases 0, 1, 2, 4, 6, then 3's code half. One PR, work persona, one commit per
   phase. Branch `excise-api-key-followups`, PR title `feat: excise api key followups`. The title must
   slugify to the branch or the `branch-pr-title-guard` hook denies `gh pr create`; fix a block by
   rewriting the title, never by renaming the branch.
3. Merge with `--admin`, per the precedent in #64 (https://github.com/tatari-tv/clyde/pull/64). Babysit
   to green first: CI passing and every CodeRabbit thread fixed, replied to, and resolved.
4. The version bump rides the feature PR as `bump --no-tag`, at **minor** (Phase 2 changes what enrich
   sends). Confirm the gates live before assuming the flow: `bump --gates`, or both of
   `gh api repos/tatari-tv/clyde/branches/main/protection` and
   `gh api repos/tatari-tv/clyde/rules/branches/main`. Never plain `bump` on a gated repo, and never a
   bump-only release branch.
5. Post-merge: `bump --tag-only` on updated `main`, then `git push origin <tag>` by explicit name. Never
   `git push --tags`.
6. Verify live: `cargo install --path clyde`, then `clyde --version` reporting the tagged version, then
   `clyde bootstrap` twice on desk.lan (first run repairs, second reports `0 steps`) and `clyde doctor`
   clean. Localhost green is not shipped.

**Blast radius.** `scottidler/claude` is symlinked into `~/.claude`, so Phase 3's and Phase 5's edits take
effect for every project on this machine the moment they land, not just clyde. That is the point of the
lint amendment, and it is why the rule change ships as its own reviewable PR rather than buried in a Rust
sweep.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Phase 1's converge discards an operator customization on a host we have not seen | Low | Med | `backup()` writes `.clyde.bak` before every write; bootstrap reports the action; Alternative 2's doctor check ships too, so the operator is told |
| Phase 0 kills the injection hypothesis and Phase 2 has no remedy | Med | Med | a null result is an accepted Phase 0 outcome and redirects Phase 2 to the output-format constraint; the diagnosis improvement to `parse_enrich_json` ships either way |
| Phase 3's 373 replacements garble a sentence while staying CI-green | Low | Low | only **6 of the 373** are outside comments (`doctor.rs:334`, `bootstrap.rs:209,226,244`, `report/src/report/tests.rs:871`, `slots/tests.rs:531`); the other ~367 are `//`, `///`, `//!` where a bad replacement cannot change behavior. Those 6 get individual reads; the rest are still read, not batch-substituted |
| Phase 4 strands a host still carrying klod state | Low | High | the retained `doctor` tripwire fails the host loud with a named remedy, which is the mitigation that actually reaches the operator; the README note and git history are secondary |
| Phase 4's tripwire rots because no host exercises it | Med | Low | it is covered by a unit test with a planted temp dir and a break-it check, not by a live host |
| Phase 6's decomposition drops a test silently | Low | Med | test count is a success criterion: 62 before, 62 after |
| The em-dash lint fires on a legitimate future use | Low | Low | there is no legitimate use in `.rs`; the one existing case is an assertion that em-dashes are absent, rewritten as `'\u{2014}'` |

## Open Questions

None.

## References

- `docs/design/2026-07-30-excise-api-key-followups-handoff.md`, the brief this doc executes
- `docs/design/2026-07-29-excise-api-key.md`, the shipped excision, `Status: Implemented`
- `docs/design/2026-07-29-excise-api-key-implementation-notes.md`, the phase-by-phase record and Finding 14 (the output-token canary)
- #77 (https://github.com/tatari-tv/clyde/pull/77), the excision PR
- #64 (https://github.com/tatari-tv/clyde/pull/64), the `--admin` merge precedent
- `refs/dealing-with-large-files.md`, the decomposition technique Phase 6 follows
- `rules/safety.md`, the em-dash rule Phase 3 amends
- `rules/voice.md`, the four legal em-dash replacements
- `~scott-idler/claude-usage-report-pipeline-runbook`, the AC6 runbook (read via `marquee read`, never WebFetch)
