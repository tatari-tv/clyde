# CLI Shakedown Report: clyde v0.23.0

Run 2026-08-01 against a freshly installed `~/.cargo/bin/clyde` (v0.23.0, tag `v0.23.0`, commit
`21c3a32`). Every command in the **Verified working**, **Findings** and **Edge cases** sections was
executed for real; **Pipeline recipes** is suggested usage for the reader, not an execution log, and
is not counted in the 24. Mutating commands ran against copies of the
live catalog under a scratch `XDG_DATA_HOME`; the live catalog at
`~/.local/share/clyde/sessions.db` was never written by this sweep. No real `clyde session enrich`
was run: that ships transcripts off-machine and the maintainer ran it separately.

## Summary

| Metric | Count |
|---|---|
| Commands exercised | 24 |
| Passed | 21 |
| Findings | 3 (1 major, 1 medium, 1 cosmetic) |
| Edge cases tested | 9 |
| Skipped (mutating, deliberate) | 1 (`session enrich` without `--dry-run`) |

## Findings

> **All three findings are RESOLVED.** Each carries a resolution line below. The plan, the reasoning,
> and the measured evidence are in
> [`docs/design/2026-08-01-shakedown-v0.23.0-fixes.md`](design/2026-08-01-shakedown-v0.23.0-fixes.md).
> The findings themselves are NOT rewritten: this is a point-in-time record of what was observed
> against v0.23.0, and it stays that way.

### F1. MAJOR. `session scope --set work` does not re-offer a `skipped-personal` session

The override is honored **by the gate** but the row never reaches the gate on a normal run, so the
documented operator remedy silently no-ops on exactly the population it exists for.

Mechanism:

- `Db::set_scope_override` (`sessions/src/db/routing.rs:158`) writes `scope_override`,
  `scope_override_reason`, `scope_override_by` and nothing else. It does not clear `enrich_status`
  and does not touch `scope_version`.
- `Db::enrich_candidates` (`sessions/src/db/enrich.rs:345`) excludes rows matching
  `enrich_status = 'skipped-personal' AND scope_version >= SCOPE_VERSION`. Any session previously
  seen as personal carries exactly that state, and `SCOPE_VERSION` is 3.

Observed:

```
# session 15d16fad, rules said personal, forced to work
$ clyde session scope --session 15d16fad... --set work --reason "shakedown"
✓ 15d16fad scope override set to work by saidler@desk

$ clyde session enrich --dry-run --dormant-after 1h     # normal run
  15d16fad -> ABSENT from the candidate set

$ clyde session enrich --dry-run --all --dormant-after 1h
  15d16fad -> scope=work would-send=True would-enrich    # gate honors it
```

The inverse direction is unaffected: forcing `personal` works on a normal run, because the row stays
a candidate and the gate then skips it. Only the direction an operator actually needs is broken.

Workaround today is `--all`, which also re-enriches everything and overwrites manual tags. Fix is for
`--set` to clear `enrich_status` (or otherwise re-offer the row) so the override takes effect on the
next ordinary sweep.

**RESOLVED** -- `docs/design/2026-08-01-shakedown-v0.23.0-fixes.md`, Phase 2. Both override writes now
NULL `scope_version`, the mechanism `Db::record_enrich_skip` already documents for exactly this, so
`--set` and `--clear` take effect on the next ORDINARY `enrich` with no `--all` and no manual-tag
loss. `--clear` was in the same blockage in mirror image and was fixed with it, conditionally, so a
no-op clear cannot become a hidden re-offer. Clearing `enrich_status` instead was considered and
rejected: it destroys the record of WHY the row was skipped. Phase 4 carried the fix past the export
boundary, where it would otherwise have died.

### F2. MEDIUM. `doctor`'s `host-refused` count overstates its operational meaning

`Db::routing_summary` counts every row whose recorded host is absent from `work-remote-hosts`. But
the cwd anchor decides **before** the git-origin branch consults the host, so a session sitting in
`~/repos/<work-org>/<repo>` is work regardless of its remote and its host is never read.

Measured on a copy of the live catalog with `work-remote-hosts: [git.example.invalid]`:

```
doctor:  host-refused  1451
reality: git-origin sessions with a tatari-tv slug whose cwd HAS the repos/<org> anchor: 772
         ... whose cwd LACKS it (the only ones the host gate can affect):                  0
         decisions actually changed by the allowlist swap:                                 0
```

So `host-refused` read 1451 while the number of decisions the host policy altered was zero. The
count is literally what its doc comment says ("rows whose recorded host is NOT in
`work-remote-hosts`"), but the operator-facing remedy line invites reading it as "rows that lost work
scope". This is a superset of the alias divergence CodeRabbit raised on PR #84; that one was
disclosed in the remedy string, this magnitude was not.

**RESOLVED** -- `docs/design/2026-08-01-shakedown-v0.23.0-fixes.md`, Phase 3, and fixed as a CLASS
rather than as this one count. While confirming this finding, `probe-refused` measured 326-vs-0 on the
live catalog with SHIPPED config, which is the same defect at larger real magnitude and needed no
planted allowlist to see. `doctor` now tallies `Basis` from the real classifier and prints decisions
and conditions as two separate groups, so a refusal count is a count of DECISIONS by construction and
cannot drift from the gate again. `host_refused` is gone (a condition with no honest reading);
`probe_refused` became `probe_recorded` under conditions, same query, a name that says what it counts.
The alias divergence is closed too: `doctor` resolves hosts through the real `SshResolver`, exactly as
the gate does.

### F3. COSMETIC. Every user-facing error prints an eyre `Location:` footer

```
$ clyde session scope --session <id> --set work
Error: --set requires --reason: an unexplained routing flip is not auditable

Location:
    clyde/src/main.rs:964:9
```

The messages themselves are excellent and specific. The source location is noise for a CLI user and
appears on every error path, not just internal ones. Global to the binary, not new in v0.23.0.

**RESOLVED** -- `docs/design/2026-08-01-shakedown-v0.23.0-fixes.md`, Phase 1. The real shape was wider
than the footer: the binary had THREE error renderings, and `dispatch_tool` had already decided which
one was right and shipped it on `report`/`cost`/`permit`/`efficiency` while `main` was never brought
along. That renderer is now extracted as `render_error` and every path goes through it, so there is
one rendering for the whole binary and `--log-level debug|trace` keeps the location capture on every
subcommand instead of four. Disabling eyre's `track-caller` feature was the drafted fix and was
withdrawn: it kills the capture under `--log-level debug` too, silently gutting the escape hatch.

## Verified working

**`clyde doctor`** -- exit 0. Reports `repo-root` as one absolute path, the four per-rule resolution
counts plus `(unresolved)`, and eight routing lines (`probe-refused`, `host-refused`, `host-unknown`,
`overrides`, `anchor/remote`, `blocked`, `outside-root`, `indeterminate`), each with its own remedy.
Pipes cleanly (no TTY branch). The `REPROBE_SAMPLE_MAX = 64` cap is in place and announces itself
when it truncates.

**`clyde session scope`** -- full lifecycle exercised against a copy:

```
$ clyde session scope --list
no scope overrides

$ clyde session scope --session 00849874 --set work --reason "cli-shakedown v0.23.0"
warning: 00849874 carries a conclusive negative probe (not-a-repo@2026-08-01T16:43:46Z).
         Forcing `work` overrides recorded evidence that this cwd had no work remote.
✓ 00849874 scope override set to work by saidler@desk

$ clyde session scope --list
00849874  work  saidler@desk  2026-08-01T17:59:20Z  cli-shakedown v0.23.0

$ clyde session scope --session 00849874 --clear
✓ cleared the scope override on 00849874
```

The conclusive-negative warning fires and names the stamp it is overriding, as designed.

**`clyde session reindex` repair flags**

```
$ clyde session reindex --clear-probe            # no --session
Error: --clear-probe requires --session <id>; there is no catalog-wide form   (exit 1)

$ clyde session reindex --clear-probe --session <id>    # exit 0; re-records on the same pass if
                                                        # the cwd still declines conclusively
$ clyde session reindex --reresolve-repo
✓ cleared repo attribution for 2184 session(s); reresolving...                (exit 0)
```

**`work-remote-hosts` config key** -- honored, and a malformed `clyde.yml` is fatal on the enrich
path rather than falling back to defaults:

```
$ printf 'work-remote-hosts: [not-a-list\n' > $XDG_CONFIG_HOME/clyde/clyde.yml
$ clyde session enrich --dry-run <id>
Error: failed to load clyde config                                            (exit 1)
```

**Report pipeline** -- `report collect` wrote 21 sessions / 79 KB and warned correctly about 0%
enrichment coverage; `report render --format markdown` wrote 7.2 KB and warned correctly about the
missing `--reconcile`. Both exit 0. Neither warning is a failure.

**Real-data reindex** (copy of the live v13 catalog, 2179 rows):

| | before | after |
|---|---|---|
| `repo_host` non-null | 1278 | 1321 (all `github.com`) |
| `repo_probe` non-null | 326 | 371 (all `not-a-repo`) |
| scope | personal 1064 / work 897 | unchanged, zero rows flipped |

14.8 s for the pass. Zero scope regressions, which is what the design predicted for this catalog.

## Edge cases

| Input | Result |
|---|---|
| `scope --set` without `--reason` | exit 1, "an unexplained routing flip is not auditable" |
| `scope --clear` without `--session` | exit 1, "--set and --clear require --session <id>" |
| `scope --set sideways` | exit 2, clap: `[possible values: work, personal]` |
| `scope --session deadbeef-0000` | exit 1, `no session matches "deadbeef-0000"` |
| `scope --session 0` (ambiguous) | exit 1, `"0" is ambiguous (10 matches)` |
| `reindex --clear-probe` without `--session` | exit 1, refuses; no catalog-wide form |
| `enrich --show-payload` without `--dry-run` | rejected by an explicit guard |
| `enrich <id> --all` together | rejected by an explicit guard |
| malformed `clyde.yml` on the enrich path | exit 1, fatal (correct: never default on a send path) |

## Pipeline recipes

```bash
# Which hosts is the catalog actually attributing from?
sqlite3 ~/.local/share/clyde/sessions.db \
  "SELECT repo_host, COUNT(*) FROM sessions WHERE repo_host IS NOT NULL GROUP BY 1 ORDER BY 2 DESC;"

# Preview the gate and count what would ship, without sending anything
clyde session enrich --dry-run --dormant-after 1h \
  | jq '{considered, would_enrich: ."would-enrich", skipped: ."skipped-personal"}'

# Every session the gate would send, by repo
clyde session enrich --dry-run --dormant-after 1h \
  | jq -r '.details[] | select(."would-send") | .["session-id"]' | wc -l

# The routing counts alone, for a health check
clyde doctor | sed -n '/routing decisions:/,$p'

# Full-month coverage for a 30-day report
clyde session enrich --dormant-after 1h
clyde report collect -o ./report.json
clyde report render -i ./report.json --format markdown -o ./out/report.md
```

## Observations

- `clyde doctor` and `clyde session doctor` are different commands with different output. The first
  is installation + attribution + routing; the second is enrichment health. The names do not hint at
  that split, and the runbook has to explain it. Worth considering a rename.
- `enrich` has no window flag. Coverage for a 30-day report is controlled entirely by
  `--dormant-after`, which reads as a safety gate rather than a scope control. `--dormant-after 1h`
  is the documented way to get month-to-date coverage; that indirection is not obvious from `--help`.
- The v0.23.0 host allowlist cannot change a decision on a catalog where every work checkout lives
  under `~/repos/<org>/<repo>`, because the cwd anchor decides first. It is insurance for the
  off-layout population, which is the population it was built for. Worth stating plainly somewhere,
  because `doctor`'s count makes it look far more active than it is (F2).
