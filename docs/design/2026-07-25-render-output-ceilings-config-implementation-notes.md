# Implementation notes: configurable `report render` output ceilings

Append-only companion to `docs/design/2026-07-25-render-output-ceilings-config.md`. One section per
phase, four buckets each, written at commit-prep time. A later decision that overrides an earlier one
gets a NEW entry; nothing above is rewritten.

## Phase 1: reshape `Job`, plumb the ceilings as config, defaults hold today's values

### Design decisions

- **The design doc rides the Phase 3 commit, not Phase 1's.** Scott's instruction was that the doc
  ride a phase commit rather than a standalone `docs(...)` commit, first or last, implementer's
  choice. Last, because the doc's `Status:` flips to `Implemented` in the same commit and that is only
  true after Phase 3; committing it at Phase 1 would ship a doc claiming `Draft` and then require a
  second touch to correct it.
- **`job_api_limits_map_to_todays_behavior` was replaced in Phase 1, not Phase 2 —
  `report/src/summarize/api/tests.rs`.** The doc assigns the replacement to Phase 2, but Phase 1
  deletes `Job::api_limits()`, so the test cannot compile through Phase 1 and the phase's own
  `otto ci` gate forces the replacement here. It is split exactly as Phase 2 specifies:
  `streaming_is_derived_from_the_kind_not_from_a_threshold` (per-arm `Kind::streams()`) and
  `the_default_ceilings_are_the_documented_pair`. Phase 2 now only moves the markdown literal in the
  second one.
- **The zero-ceiling validators share a body — `common::config::nonzero_ceiling` —
  `common/src/config.rs`.** AC-C2 requires two functions so each hardcodes its own key into
  `Error::custom`; it does not require two copies of the check. `de_markdown_max_output_tokens` and
  `de_html_max_output_tokens` are the two required entry points and each passes its key to one shared
  validator. Both keys are asserted by name, in separate tests, so a single shared validator naming
  neither cannot pass.
- **`Kind::streams()` stayed module-private rather than `pub(super)` —
  `report/src/summarize/api.rs`.** The doc says it stays api-private. Nothing outside `api.rs` needs
  it, and `api/tests.rs` is a child module so it reaches a private item without widening visibility.
- **Test helpers source the ceiling from the config consts, never a literal —
  `report/src/summarize/cli/tests.rs::job`, `report/src/summarize/api/tests.rs::default_job`.** One
  place per file resolves `Kind -> default ceiling`, so Phase 2's raise flows through every Guard 6
  case instead of being hand-edited at each site. The byte-identical tests keep their *expected body
  literal* hardcoded, which is the point of them.
- **`report/src/summarize/tests.rs` uses an inert `CEILING: u32 = 1_024`** for the tests that are not
  about the ceiling, rather than either default. Same reasoning the doc gives for Phase 2's four
  cosmetic sites: a fixture that tracks a real default by coincidence gets dragged along by the next
  change to that default.

### Deviations

- **`ApiTransport::complete`'s `info!` line formats `job.model` instead of a `model` local —
  `report/src/summarize/api.rs`.** Not one of the five debug logs F12 named (that line does not format
  the whole job, so it does not duplicate anything), but `model` stopped existing as a local when the
  argument was removed, so the line had to be rewritten either way.
- **The five debug/info logs F12 named dropped their separate `model=` as specified**, and
  `render.rs`'s two debug lines gained `max_output_tokens=` — the ceiling is now a resolved
  per-invocation value, so an operator reading a log has no other way to see which budget applied.
  That addition is not in the doc; it is the same instrumentation argument the doc makes for the
  ceiling being config.

### Tradeoffs

- **Three linked unit tests as the two-sided plumbing probe, vs. one end-to-end test.** The doc's
  Phase 1 proof is "a `clyde.yml` setting `markdown-max-output-tokens: 12345` produces
  `"max_tokens":12345` in the built api body AND a Guard 6 ceiling of 12345 on the cli path." A single
  test cannot span that: `build_body` is private to `api.rs`, Guard 6 is private to `cli.rs`, and
  `resolve_command` lives in a third module. So the probe is three tests along one chain, each with the
  same 12345 sentinel — `ceilings_come_from_clyde_yml_when_set` (yml -> resolved `RenderConfig`),
  `a_configured_ceiling_reaches_the_serialized_body` (api wire), and
  `guard_output_ceiling_enforces_the_configured_value_not_a_constant` (cli Guard 6) — plus
  `the_resolved_ceiling_reaches_the_port_per_job` for the `summarize::markdown`/`html` hop the doc
  calls out as the one a `build_body`-only test cannot see. All four proven to bite.
- **Alternative considered and rejected: a test seam in `render.rs`.** The two `render.rs` call sites
  are the one hop in that chain with no test coverage, because `render_via_opus_markdown` calls
  `resolve_transport_for`, which probes PATH and the environment. Adding a seam to cover two
  field-passing lines would be unrequested structure, so instead the fields were verified by reading
  and the risk is recorded here: a swap of `cfg.markdown_max_output_tokens` and
  `cfg.html_max_output_tokens` at `report/src/render.rs:263`/`:288` would not fail any test. The
  distinct-sentinel tests one hop above and below it make the swap the only undetected mutation.

### Open questions

- None.

## Phase 2: raise the markdown default and rebaseline every site that named the old ceiling

### Design decisions

- **Guard 6's bail names the key for the job that FAILED, via `Kind::max_output_tokens_key()` —
  `report/src/summarize.rs`, `report/src/summarize/cli.rs`.** The doc says the bail names
  `render.markdown-max-output-tokens`. Taken literally that would print the markdown key on an html
  ceiling failure, which is a remedy that does not remedy — the exact thing `cli.rs`'s module docs call
  worse than offering none, and the reason F10 was raised in the first place. So the key is per-kind.
  `each_kind_names_its_own_ceiling_key` (`report/src/summarize/tests.rs`) makes the html arm
  falsifiable, since no ceiling-failure test exercises the html job.
- **The bail also keeps `--since` as a second remedy.** Raising the key is the direct fix, but a user
  who does not want a 40,000-token document should still be told the other door exists. Both are
  remedies that work on both transports, which is the criterion the file's doctrine applies.
- **40,000 is the shared over-budget fixture value —
  `report/src/summarize/cli/tests.rs`.** Three Guard 6 tests need a value above the raised markdown
  ceiling, and one of them (`guard_output_ceiling_allows_the_same_output_for_the_larger_job`)
  additionally needs it strictly BELOW the html ceiling or its premise dies. 40,000 satisfies both, so
  the three sites agree rather than drifting apart.
- **`guard_output_ceiling_accepts_exactly_the_ceiling` now reads the const rather than a literal.** It
  is a boundary test; a hardcoded boundary silently stops being the boundary the next time the default
  moves. This is the same coupling F16 objected to for the cosmetic sites, but inverted: here tracking
  the default is the point, so it tracks it by reference.
- **The prior doc's AC3 is marked REBASELINED with the retirement stated in place**, rather than
  edited to look like it always said this. Its Phase 3 success criterion and its "byte-identical when
  selected" goal line both carry a note that they were met as written and later superseded. Design docs
  are point-in-time; the retirement is a fact about what changed, not an embarrassment to paper over.

### Deviations

- **Four cosmetic sites got `1_024`, as F16 specified, but expressed as a named
  `INERT_CEILING` const in `api/tests.rs` rather than a bare literal at three call sites.** Same value,
  same decoupling; the name says why the number is arbitrary so nobody "helpfully" re-syncs it to the
  default later. The fourth site (`cli/tests.rs`'s truncation fixture) is a bare `1_024` with a comment,
  because it is one site in a different file.
- **Both READMEs gained a sentence of prose beyond the two YAML rows.** The doc's rebaseline table asks
  for the `render:` blocks; a new key with no explanation of when to touch it is a config surface
  documented by name only. `report/README.md`'s sentence also records the transport asymmetry (api sets
  the ceiling on the wire, cli checks after the fact and has already paid), because that is what makes
  "raise the key rather than re-run" the right advice on the cli path.

### Tradeoffs (Phase 2)

- **AC-C4's grep is at zero, and two things deliberately keep it reachable.** The new const's doc
  comment says "the pre-config ceiling" instead of spelling the old number, and cites the measurement as
  `16,117`, which the pattern does not match. A Phase 1 doc comment of mine also had to be reworded: it
  read `BITES: ... with { let _ = job; 16_000 }` and now reads "with any literal". Anyone who rewrites
  either comment to name the old value re-breaks the AC — which is the point of recording it here.
- **The `(max_tokens, stream)` tuple assertion was not restored in any form.** Phase 1 split it into
  two tests and Phase 2 only moved the markdown literal. Reuniting them would repackage the two-signals-
  one-value shape the doc rejected.

### Open questions

- None.

## Phase 3: live verification and the AC1 flip

### Design decisions

- **Verified against the branch binary at `target/release/clyde`, not an installed one.** The
  finalization gate forbids `cargo install` without approval, and the installed `clyde` is `v0.13.3` off
  `main` -- i.e. the 16,000 ceiling. Verifying with it would have proven nothing about this work. The
  binary under test reports `v0.13.3-9-g15c4b2d`.
- **The transport log line lives in `report.log`, not `clyde.log`.** `report` installs its own file
  logger (`report/src/lib.rs:120`, `<xdg-data>/clyde/logs/report.log`) because `env_logger` can only be
  initialized once per process and clyde deliberately skips logger setup for the absorbed
  `report`/`cost`/`permit` arms (`clyde/src/main.rs:98-108`). Recorded because looking for a render's log
  line in `clyde.log` finds nothing and reads exactly like a missing log line.

### Deviations

- None.

### Tradeoffs

- None. The phase is a single measured run, as the doc specifies; the argument against looping it is the
  per-render cost in "Performance and cost".

### Open questions

- None.

### What the run actually showed

Command, verbatim, against the 2026-07 month (`report collect --since 2026-07-01`, 1,328 sessions,
5.5MB of facts):

```
env -u ANTHROPIC_API_KEY ./target/release/clyde --log-level info report render \
  -i <tmp>/real.json --format markdown -o <tmp>/ac1.md
```

| criterion | result |
|---|---|
| exit code | **0** |
| artifact size (> 5,000 bytes) | **14,846** |
| `^## ` headers (>= 3) | **9** |
| contains `Generated offline via` | **no** |
| log names `selected=Cli (requested=Auto)` | **yes** |

The log also carried `job=Job { kind: Markdown, model: "claude-opus-4-8", max_output_tokens: 32000 }`
and `payload bytes=519124`. That is the reshaped `Job` carrying a config-resolved ceiling across a real
transport -- the one hop no unit test covers, since `render.rs`'s call sites probe PATH.

**The caveat, stated rather than buried.** That run's output landed under even the OLD 16,000 ceiling,
so it demonstrates the path works end to end but does NOT independently re-prove the failure this design
fixes. Model output length is not deterministic, and the run used `claude` 2.1.220 rather than the
2.1.219 of the original measurement. The necessity of the raise rests on the earlier measured
16,117-token render recorded in the prior doc's Open Questions, not on this run. What this run proves is
the part that was actually at risk: the reshape moved no behavior, a config value reaches a live
transport, and the largest real month renders keyless at exit 0.
