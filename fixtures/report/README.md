# Render-eval fixtures

Frozen inputs and golden artifacts for `clyde report eval` (design
`docs/design/2026-07-26-report-story-fidelity.md`, Phase 13).

## Nothing here is real

`tatari-tv/clyde` is a **public** repo, so no fixture may be derived from real session data. A
redacted copy of a real window would publish real session titles and enrich summaries, and
redaction is not a sufficient control for narrative text: the titles ARE the sensitive payload, and
the eval needs them realistic.

Every org, repo, session title, summary, tag, commit sha, PR reference and persona below is
**invented** by a committed, seeded generator (`report/src/eval/synth.rs`). It reads no catalog, no
filesystem and no network. `report/src/eval/synth/tests.rs` asserts the vocabulary names nothing
real, so a future edit that pastes a live slug in here fails the build.

The real-data eval stays local:

```bash
clyde report collect --since 2026-06-01 --until 2026-06-30 -o fixtures/report/local/report.json
clyde report eval --fixture fixtures/report/local
```

`fixtures/report/local/` is gitignored.

## Layout

| file | what it is |
|---|---|
| `report.json` | the collected schema-v2 artifact (generated) |
| `eval.yml` | the per-fixture spec: required sections, required citations, judge floors, persona |
| `prior.json` | a prior-period artifact, lighting up Month over Month (medium only) |
| `analytics.json` | a synthesized Analytics cost export, lighting up Reconciliation (medium only) |
| `golden.md` | the committed markdown render |
| `golden.html` | the committed html render |

| fixture | what it exercises |
|---|---|
| `small` | one repo, no subagents, a seven-day window, all `git-origin` attribution |
| `medium` | three orgs, seven repos, subagents with a positive `(main-session)` residual, the full outcome mix, all four `repo-source` values, partial enrich coverage, `--prior`, `--reconcile` |
| `pathological` | zero outcomes, one unpriced nonzero-token model, a multi-day gap, carried-in sessions, an all-`path-guess` attribution, zero enrich coverage |

## The two layers

**Mechanical** (deterministic, offline, free) runs in `otto ci` against the committed goldens and
again inside `otto eval` against every fresh render, before the judge is paid for. It checks that
every cited repo, date and quoted phrase is in the context, that the required sections are present
and the forbidden ones absent, that Hard prohibition 2's phrase list and the em-dash are absent,
that the foreign-number guard is clean, and that every digit-bearing chart attribute is one the
binary computed.

**Judged** (paid, networked) runs only in `otto eval`. A model scores a FRESH render 0 to 3 on
citation accuracy, coverage, prohibition compliance and readability; a score below the floor in
`eval.yml` exits non-zero.

## Regenerating

```bash
cargo run -p report --bin fixtures -- fixtures/report        # report.json / prior.json / analytics.json
clyde report eval --write-goldens                             # golden.md / golden.html
otto ci                                                      # the mechanical layer, against the new goldens
```

The generator is seeded and its `generated` timestamp is frozen, so re-running it on an unchanged
generator rewrites byte-identical files: a diff is a generator change, never a clock tick.

The goldens are model-authored, so `--write-goldens` is the only way to regenerate one: a hand-run
`clyde report render` would splice in the machine's real persona (via `persona whoami`) and price
against the live feed, instead of the fixture's invented persona and the eval's pinned embedded
pricing. A render that failed its own mechanical checks is never written, so a golden is a
known-good artifact by construction.

## Pricing is pinned

The eval prices everything with `Pricing::embedded()`. A fixture priced against the live feed would
score differently on two days because the feed moved, and the next `data: refresh pricing` commit
would silently invalidate every golden. `report/src/eval/tests.rs` pins the exact per-model rates
the goldens were rendered against, with the regeneration remedy in its failure message.
