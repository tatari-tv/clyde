# Handoff: vestigial work found while shipping the api-key excision

**Author:** Scott Idler
**Date:** 2026-07-30
**Status:** Handoff brief. Findings are diagnosed, do not re-derive.
**Audience:** the next agent picking up `tatari-tv/clyde`

> **HISTORICAL SNAPSHOT, superseded. Read this for the evidence, not for the remediation advice.**
> Written 2026-07-30 against `v0.18.0`. **All six items below have since been executed** by
> `docs/design/2026-07-30-excise-api-key-followups.md` (`Status: Implemented`), shipped in the release
> after `v0.18.0`. Per-item outcomes are in
> `docs/design/2026-07-30-excise-api-key-followups-implementation-notes.md`:
>
> | item | outcome |
> |---|---|
> | 1, `rewrite_unit` orphaned comment | Fixed. The repair now writes one canonical body; the live desk.lan unit was repaired. Notes, Phase 1 |
> | 2, eight enrich JSON failures | **All 8 recovered.** No row in the DB is attempt-retired. The cause was a payload-captured model, not the transport. Notes, Phases 0 and 2 |
> | 3, em-dashes | Rule amended and all occurrences removed, plus a CI lint. Notes, Phase 3 |
> | 4, retire the klod migration | Migration deleted, `doctor`'s tripwire retained and extended. Notes, Phase 4 |
> | 5, acceptance-criteria defect class | Execution gate added to `create-design-doc` / `how-to-execute-a-plan`. Notes, Phase 5 |
> | 6, smaller items | `cli/tests.rs` decomposed; dependabot alert already `fixed`; cost canary in the README. Notes, Phase 6 |
>
> Two things here are **stale as remediation guidance** and must not be acted on: item 1's proposed
> "drop the contiguous comment block" fix was REJECTED (see the design doc's Alternative 1, the
> ordering is unrecoverable), and item 2's "raise `--max-attempts`" was ruled out as retrying a
> deterministic failure. Item 4's "check `ltl-7007` and `mini` first" was waived by Scott.
>
> Still open, and moved to its own brief: the excision's AC6 enrichment percentage, which is blocked
> by a defect found after this was written. See `docs/design/2026-07-30-scope-dormancy-cost-handoff.md`.

`v0.18.0` shipped the api-key excision ([#77](https://github.com/tatari-tv/clyde/pull/77), design doc
`docs/design/2026-07-29-excise-api-key.md`, phase-by-phase record in
`docs/design/2026-07-29-excise-api-key-implementation-notes.md`). That work is done, live, and
verified. This document is the residue: things found along the way that were deliberately NOT fixed
in that PR because they were out of its scope, plus one defect it shipped.

Nothing here is blocking. Every item below carries the evidence that established it, so the next
session can decide and act without re-investigating. Do not re-measure what is already measured.

## 1. `rewrite_unit` leaves an orphaned comment. This is a defect we shipped.

**Status:** real, live on desk.lan right now, cosmetic but the file states a falsehood about a
credential.

Phase 5 taught `rewrite_unit` (`clyde/src/bootstrap.rs`) to strip an `EnvironmentFile=` line by line
filtering. It strips the directive and leaves the comment block that explains it. The live unit at
`~/.config/systemd/user/clyde-enrich.service` currently reads:

```
[Service]
Type=oneshot
# The work Anthropic key lives here (0600), since systemd user services do not
# inherit the interactive shell environment. Never committed; desk-only.
# Default sweep: dormant (>=7d idle), work-scoped only, incremental.
Environment=PATH=...
ExecStart=%h/.cargo/bin/clyde --log-level info session enrich
```

There is no key. Nothing loads one. The unit says otherwise.

- **Blast radius is narrow.** `install_clyde_timer`'s generated template is clean, so fresh installs
  are correct. Only hosts that already carried the old unit are affected, which today means desk.lan.
- **Bootstrap will not self-heal it.** `refresh_clyde_unit`'s trigger is `has_stale_subcommand ||
  has_environment_file`, and this unit now trips neither, so re-running bootstrap is a no-op. The
  comment survives forever without an explicit fix.
- **The fix:** when line filtering drops an `EnvironmentFile=` line, also drop the contiguous `#`
  comment block immediately preceding it. Needs a test with a fixture carrying directive-plus-comment,
  and a negative case proving an unrelated comment (the `Default sweep:` line) survives.
- **The third comment line must survive.** `# Default sweep: dormant...` describes `ExecStart`, not
  the stripped directive. A naive "drop all comments above" fix is wrong.
- **Immediate remedy for desk.lan:** delete the two lines by hand. Bootstrap will not touch it again.

## 2. Eight sessions fail `enrich` with a JSON-parse error. Pre-existing, not ours.

**Status:** diagnosed, transport-independent, out of the excision's scope by explicit Non-Goal
("Excluded: changing what enrich or narrate produce. Transport swap only.").

The `--max-attempts 6` recovery sweep (Rollout step, run 2026-07-30) enriched 42 of 52 and recovered
38 of the 46 previously-retired rows. Eight failed, all with
`the "claude" CLI reply was not the expected JSON`, and all eight are now at `attempts=6`, so they are
retired again under the default cap of 5.

**The transport is not the problem, and this is established rather than assumed:**

```
CliTransport::complete_with_usage: job=Job { kind: Enrich, ... } ok result bytes=1917 tokens_in=2408 tokens_out=498
```

The envelope was valid, every guard passed, `stop_reason` was `end_turn`. `parse_enrich_json` then
found no `{...}` span to recover. `parse_enrich_json` already tolerates fences and surrounding prose,
so failing means the reply contained no JSON object at all.

**The diagnostic signal is the output-token count.** Healthy enrich calls in that same sweep averaged
**138.7** output tokens. These returned **498** and, on a repeat run of the same session, **665**. The
model is writing prose instead of the compact tags-plus-summary object, and the length varies run to
run, so it is not a fixed refusal string.

**Proof it predates the keyless work:** the log carries **269** occurrences of the api-transport-era
wording `Anthropic response was not the expected JSON`, spanning 2026-06-26 to 2026-07-30, against 10
of the new cli wording. Same failure, both transports, over a month.

**Two content clusters, which is the lead worth pulling:**

| cluster | count | payload shape |
|---|---|---|
| security reviews | 3 | cross-tenant authz vuln, settings.json allowlist vulns, supply-chain vuln |
| agent prompts | 5 | payload opens with `You are a per-PR maintenance agent for the babysit-prs skill` |

The agent-prompt cluster is the interesting hypothesis: enrich streams session content on stdin, and a
payload that opens with `You are a...` plausibly hijacks the model into obeying the embedded
instructions instead of the enrich system prompt. Phase 3 made `Kind::fence()` return `"text"` for
enrich, which labels the payload as data, but a strong imperative opener can still win. This is
untested and is the first thing to check.

**Reproduction, deterministic, costs cents, mutates nothing:**

```
cp ~/.local/share/clyde/sessions.db /tmp/diag.db
clyde --log-level debug session enrich --db /tmp/diag.db --max-attempts 9 9a45e4bd
grep 'complete_with_usage\|not the expected JSON' ~/.local/share/clyde/logs/clyde.log | tail -4
```

**Candidate approaches, none chosen:** a JSON prefill or output-format constraint; a stricter
re-prompt on parse failure; neutralizing instruction-shaped payload openers before send. This wants
its own design doc, not a patch. Raising `--max-attempts` further will not help: it is deterministic.

## 3. Em-dashes in Rust comments: the rule and the codebase disagree

**Status:** decision needed, no code change recommended until it is made.

`rules/safety.md` says never use em-dashes in comments or documentation. `~/Claude/writing/VOICE.md`
is stronger: Scott does not use dashes as aside markers, ever.

Measured on `main` after #77: **356 em-dash occurrences across 79 `.rs` files.** PR #77 added 16 of
them, so this is overwhelmingly established house style, not a regression introduced by the excision.

During the excision I started fixing the 16 newly-authored ones and reverted it. The reasoning, so it
is not re-litigated: fixing 16 of 356 makes files internally inconsistent, contradicts the standing
instruction to match surrounding code style, and churns a CI-green deliverable for no behavioral gain.

**The decision is binary and it is Scott's:**

- **Enforce it.** One mechanical tree-wide pass over all 356, plus a lint or a `whitespace`-style CI
  check so it cannot drift back. Without the lint it re-accumulates immediately, and a convention that
  lives only in a rules file is one every review has already drifted from.
- **Amend the rule.** Scope the em-dash prohibition to outward-facing prose (Slack, Jira, Confluence,
  design docs, PRs, READMEs) and exempt Rust code comments explicitly.

Half-enforcement is the one option to avoid. Whichever way it goes, the rule and the tree should agree.

## 4. Retiring the klod migration

**Status:** deliberately out of scope for #77, gated on a fact nobody has checked.

`klod` was this binary's name before the rename. It survives in 83 non-doc references:

| location | count | what it is |
|---|---|---|
| `clyde/src/bootstrap.rs` | 22 | the migration itself: data dir, config dir, systemd unit |
| `clyde/src/bootstrap/tests.rs` | 31 | its tests |
| `clyde/src/doctor.rs` | 17 | legacy-state detection, reports klod state as unhealthy |
| `clyde/src/doctor/tests.rs` | 10 | its tests |
| `Cargo.toml` | 1 | a provenance comment about the workspace-dep reconciliation |
| `session/src/scope/tests.rs` | 1 | a path fixture using `tatari-tv/klod/main` |
| `README.md` | 1 | a link to a real historical design-doc filename |

**80 of the 83 are the machinery that removes klod from a machine.** Deleting it does not finish the
rename, it abandons any host not yet migrated. Only 3 are genuinely stale.

**The blocking fact:** desk.lan is migrated (verified: `~/.config/klod/` is gone, the unit is
`clyde-enrich.service`, and `clyde bootstrap` now reports `0 steps`). `ltl-7007` and `mini` are
**unverified**. Check both for `~/.config/klod` and `~/.local/share/klod` before deleting anything.

If both are clean, this becomes a straightforward targeted change: delete the migration, delete
`doctor`'s legacy-klod detection, fix the 3 stale mentions, and drop the four `klod-*` hardcoded unit
paths from `Paths`. If either is dirty, migrate it first and then delete.

## 5. Three of seven acceptance criteria were mis-specified. That is the systemic finding.

**Status:** the criteria are amended in the design doc with reasoning recorded. The process lesson is
not yet captured anywhere.

All five phases shipped `otto ci` green before anyone noticed that AC2, AC4 and AC5 could not pass as
written. In every case the implementation was correct and the criterion was wrong:

- **AC2** specified `clyde session enrich --only <id>`. There is no `--only` flag (enrich takes a
  positional `[ID]`), and the id is the `session_id` uuid, not the `id` integer primary key. The
  criterion named an invocation that does not parse.
- **AC4** required the only `enrich.env` mention in Rust to be the stale-file warning. Proving Phase
  5's strip works requires fixtures that contain the string, so satisfying it literally meant deleting
  the tests that prove the phase.
- **AC5** required `report render` to fail non-zero with no `claude`. It exits 0 by deliberate
  pre-existing design (`report/src/render.rs::no_transport`, present on `main` before this work).

The doc already records two earlier instances of the same defect class in its own drafting history
(AC3 and AC7 both had to be corrected during review). So this is now five occurrences of one pattern:
**an acceptance criterion written from the design rather than from the running system.**

**The cheap structural fix:** before a design doc is called ready-to-build, execute every acceptance
criterion's literal command against the CURRENT code and record what it returns. Criteria that name a
flag, column, or exit code should be typed into a shell once. AC2's defect would have surfaced in
seconds. This is a candidate addition to `/create-design-doc`'s passes or the ready-to-build gate in
`/how-to-execute-a-plan`, and it is worth writing up rather than remembering.

## 6. Smaller items

- **`common/src/llm/cli/tests.rs` is 1,322 lines** against the 1,500 limit. Phase 3's notes predicted
  Phase 4 would shrink it by removing the `--llm api` escape-hatch cases; the task instead required
  those tests be rewritten against the new remedy string, so it grew. Decompose before the next
  feature lands tests there. `rules/dealing-with-large-files.md` covers the technique.
- **A high-severity dependabot alert on `main`** was reported by GitHub during the #77 push
  (`https://github.com/tatari-tv/clyde/security/dependabot/1`). Unrelated to this work and
  pre-existing. A `gh api repos/tatari-tv/clyde/dependabot/alerts` query returned nothing, possibly a
  token-scope issue, so read it in the web UI.
- **AC6 is the last open criterion of the excision.** A teammate with no key must run the published
  runbook end to end and clear the 50% enrichment floor. The runbook is updated and live at
  `~scott-idler/claude-usage-report-pipeline-runbook` (step 3 now runs `enrich` in the happy path and
  states plainly that no API key is needed). Keegan's re-run is the real close.
- **Cost baseline worth keeping.** The recovery sweep measured **6.3s and ~139 output tokens per
  enriched row**, and roughly **$2.05 per 100 sessions**. Finding 14 in the implementation notes
  explains why that output-token figure is a canary: if it returns to the thousands, or per-call wall
  clock returns to ~52s, then `MAX_THINKING_TOKENS` stopped being honored and enrich silently got ~3x
  dearer. Re-check after any `claude` upgrade.

## Suggested skills

- `/create-design-doc` for items 2 (enrich prompt robustness) and 5 (the acceptance-criteria gate).
  Both are behavior changes that need the full funnel, not a patch.
- `/how-to-execute-a-plan` to execute whichever of those docs gets written, delegating each phase to
  `phase-implementer` with that phase's annotated model.
- A targeted fix with no design doc for item 1 (`rewrite_unit`'s orphaned comment) and item 6's file
  decomposition. Both are contained and mechanical.
- `/review-panel` in Implementation Audit mode if the excision itself needs auditing: the design doc is
  `Status: Implemented`, so the reviewers will walk every plan bullet against the committed code.
- `rules/dealing-with-large-files.md` before splitting `cli/tests.rs`.
- `marquee:replace` if the runbook needs another revision; never WebFetch a marquee URL, it 302s to
  Okta.

## A note on where this file lives

The `handoff` skill says to write to the OS temp directory. This went into the repo instead, following
the precedent of `docs/design/2026-07-28-release-arc-handoff.md`, because the content is a durable
follow-up list rather than conversation state, and because a scratchpad path has already been
garbage-collected mid-session on this project once. If a future handoff is genuinely ephemeral, temp
is the right home.
