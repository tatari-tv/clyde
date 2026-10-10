# CLI Shakedown Report: clyde v0.25.7

- Binary: `~/.cargo/bin/clyde`, `clyde --version` -> `clyde v0.25.7`
- Source: worktree at `main` `2a4c67f` (tag `v0.25.7`, PR #96 "work reposlug patterns from config")
- Live config: `~/.config/clyde/clyde.yml` (`reposlugs-ptns`: `tatari-tv/*`, `tatari-dev/*`, `otto-rs/otto`, `scottidler/dotfiles`, `scottidler/claude`, `scottidler/keep`)
- Live catalog: `~/.local/share/clyde/sessions.db`, `user_version` 14, 3948 rows
- Date: 2026-09-27
- Phase 0 (permission setup) skipped, as instructed. No settings files were edited.

## How live state was protected

- Invalid-config and mutating cases ran with `XDG_CONFIG_HOME`, `XDG_DATA_HOME` and `XDG_CACHE_HOME` all set to scratch dirs under `$TMPDIR/shake`, and `--db` pointed at a scratch DB.
- **`clyde session enrich --dry-run` writes to the catalog (Finding D1).** For that reason every enrich dry run went against a copy of the live DB, taken with `sqlite3 -readonly ~/.local/share/clyde/sessions.db ".backup $TMPDIR/shake/live/sessions.db"`, and read the live config (read-only). The live DB was never passed to `enrich`.
- Live DB queries used `sqlite3 -readonly`. Live `session ls/search/export` calls used `--no-reindex`.
- Deviation, disclosed: `clyde session enrich --show-payload /tmp/x` (an argument-validation edge case) was run once against the live catalog without `--dry-run`. It exited 1 with `--show-payload only applies with --dry-run` before the sweep started, so nothing was sent off-machine: there is no `enrich::enrich:` sweep line in the log for that run. But `cmd_enrich` runs `lazy_reindex` before it validates arguments (Bug 7), so this one call did an incremental live reindex. The log line is `index::reindex: scanned=3386 upserted=2 skipped=3384`. That is the same catalog refresh any `clyde session ls` without `--no-reindex` does.
- Scratch-DB runs that did not override `XDG_DATA_HOME` still log to the live `~/.local/share/clyde/logs/clyde.log`. `--db` does not redirect logs.

## Summary

| Metric | Count |
|--------|-------|
| Commands discovered | 43 leaf commands (51 help pages, every one exits 0) |
| Commands tested | 34 (run beyond `--help`) |
| Commands passed | 25 |
| Commands failed (bug found) | 9: `session enrich`, `session export`, `doctor`, `cost daily`, `cost session`, `efficiency` (bare), `efficiency session`, `efficiency weekly`, `permit audit`/`suggest`/`report` (format handling) |
| Commands skipped | 9: mutating (`session stage`, `bootstrap`, `update check/install/revert`, `mcp register/unregister`, `permit log`), interactive/stdio (`mcp serve`), model calls or publishing (`report eval`, `report render --format marquee-markdown`) |
| Config validation cases | 34 (21 rejected, 13 accepted) |
| Pipelines tested | 14 |
| Edge cases tested | 30 |
| Bugs | 16 (2 medium, 14 low) plus 1 doc/UX finding (D1), 12 cosmetic and 4 suggestions |

## Focus: `reposlugs-ptns` load validation

Harness: one YAML file per case copied to `$T/cfg/clyde/clyde.yml`, then:

```bash
XDG_CONFIG_HOME=$T/cfg XDG_DATA_HOME=$T/data XDG_CACHE_HOME=$T/cache \
  clyde --db $T/data/empty.db session enrich --dry-run
```

Every rejected entry exits 1, and stderr starts with `failed to load clyde config: failed to parse config <path>: ...`. The table gives the text after that prefix.

| Case | YAML | Exit | Message |
|---|---|---|---|
| leading space | `[" tatari-tv/*"]` | 1 | `reposlugs-ptns entry " tatari-tv/*" must not contain whitespace` |
| inner space | `["tatari tv/x"]` | 1 | `... "tatari tv/x" must not contain whitespace` |
| trailing space | `["tatari-tv/x "]` | 1 | `... must not contain whitespace` |
| tab | `["tatari-tv/\tx"]` | 1 | `... must not contain whitespace` |
| no `/` | `["tatari-tv"]` | 1 | `... "tatari-tv" must be \`<owner>/<repo>\` or \`<owner>/*\`, no \`/\` found` |
| `a/b/c` | `["a/b/c"]` | 1 | `... must have exactly one \`/\`, got more than one` |
| `/b` | `["/b"]` | 1 | `... "/b" must have a non-empty owner and a non-empty repo` |
| `owner/` | `["tatari-tv/"]` | 1 | same non-empty message |
| `*/x` quoted | `["*/x"]` | 1 | `... the owner half must be a literal owner name, never a glob (\`*\`, \`?\`, \`[\`); only the repo half may be \`*\`` |
| `*/x` unquoted | `- */x` | 1 | `did not find expected alphabetic or numeric character at line 2 column 6, while scanning an alias` (YAML error, does not name the key; C8) |
| `tatari-*/x` | `["tatari-*/x"]` | 1 | owner-glob message |
| `!*/x` | `["!*/x"]` | 1 | owner-glob message |
| `tatari-tv/philo-*` | `["tatari-tv/philo-*"]` | 1 | `... the repo half must be \`*\` or a literal repo name; mid-segment globs ... are not supported` |
| `philo?` | `["tatari-tv/philo?"]` | 1 | mid-segment glob message |
| `[ab]` | `["tatari-tv/[ab]"]` | 1 | mid-segment glob message |
| `!` | `["!"]` | 1 | `reposlugs-ptns entry "!" names no owner/repo after the \`!\`` |
| unquoted `- !tatari-tv/x` | block style | 1 | `reposlugs-ptns entry is empty; an unquoted \`!owner/repo\` is a YAML tag and parses as an empty string -- quote it, e.g. "!owner/repo"` (quote-it hint present) |
| unquoted flow `[!tatari-tv/x]` | flow style | 1 | `did not find expected whitespace or line break at line 1 column 30, while scanning a tag` (no hint, no key name; C8) |
| `""` | `[""]` | 1 | the quote-it hint (fine: same cause) |
| null entry | `- ~` | 1 | `reposlugs-ptns entry "~" must be ...` (reports `~` as a literal string; C9) |
| scalar | `reposlugs-ptns: tatari-tv/*` | 1 | `reposlugs-ptns: invalid type: string "tatari-tv/*", expected a sequence at line 1 column 17` |

Accepted shapes, as `clyde doctor` shows them:

| Case | YAML | `doctor` line |
|---|---|---|
| mixed case | `["Otto-RS/Otto", "!Tatari-TV/X"]` | `reposlugs-ptns: !tatari-tv/x, otto-rs/otto` (lowercased, sorted) |
| empty list | `[]` | `reposlugs-ptns: none` |
| duplicates | `["tatari-tv/*","tatari-tv/*","TATARI-TV/*","otto-rs/otto","otto-rs/otto"]` | `reposlugs-ptns: otto-rs/otto, tatari-tv/*` (deduped) |
| quoted excludes | `- "!tatari-tv/secret"` and `- '!scottidler/private'` | `!scottidler/private, !tatari-tv/secret, tatari-tv/*` (the stranded exclude is kept, per design) |
| wide + named | `["tatari-tv/*", "tatari-tv/x"]` | `tatari-tv/*, tatari-tv/x` |
| self-exclude | `["scottidler/claude", "!scottidler/claude"]` | `!scottidler/claude, scottidler/claude` |
| key absent | `min-enrichment: 0.5` | `reposlugs-ptns: tatari-tv/*` |
| empty file | (0 bytes) | `reposlugs-ptns: tatari-tv/*` |
| file absent | no `clyde.yml` | `reposlugs-ptns: tatari-tv/*` (design AC 4 passes) |
| null value | `reposlugs-ptns:` | `reposlugs-ptns: none` (S2) |
| `!!owner/x` | `["!!tatari-tv/x"]` | `!!tatari-tv/x` (S1) |
| `..` repo | `["tatari-tv/.."]` | `tatari-tv/..` (S1) |
| non-ASCII | `["TATARİ-tv/x"]` | `tatarİ-tv/x` (`to_ascii_lowercase` leaves `İ` alone; harmless) |

- Design AC 1: `reposlugs-ptns: ["tatari-tv/*"]` makes `session enrich --dry-run` exit 0. **Pass.**
- Design AC 3: `["*/x"]` exits non-zero, with `reposlugs-ptns` in stderr, for the pattern reason. **Pass.**
- Phase 1 success criteria: every rejection named in the design fails with `reposlugs-ptns` in the message, the unquoted-`!` hint is present, mixed case normalizes, and `[]` loads as empty. **Pass.**

## Focus: `clyde doctor` `reposlugs-ptns:` line

- Live: `reposlugs-ptns: otto-rs/otto, scottidler/claude, scottidler/dotfiles, scottidler/keep, tatari-dev/*, tatari-tv/*`. Normalized and sorted, as specified.
- `[]` -> `reposlugs-ptns: none`. **Pass.**
- No file and no catalog -> `reposlugs-ptns: tatari-tv/*`. **Pass.**
- With an invalid config, doctor prints `reposlugs-ptns: (could not load config: failed to parse config <path>)` with no reason, still prints `✓ all integrations resolve to clyde`, and exits 0 (Bug 3).
- The label is one column wider than the other paths-block labels (C1).

## Focus: scope decisions from `session enrich --dry-run`

All runs below use a snapshot of the live DB and the live config.

### Live policy, current state (`$T/live/sessions.db` snapshot)

```bash
clyde --db $T/live/sessions.db session enrich --dry-run | jq 'del(.details)'
```

```json
{"considered":1279,"enriched":0,"skipped-personal":672,"skipped-empty":1,"failed":0,
 "would-enrich":606,"redactions":75,"tokens-in":0,"tokens-out":0,"dry-run":true}
```

- **Work: 607 rows** (606 `would-enrich` plus 1 `skipped-empty` with scope work). **Skipped-personal: 672.**
- On a first-ever pass (the same snapshot with `scope_policy` nulled, which is the live state just after the v14 migration), the same policy gives `considered=1924`, `would-enrich=606`, `skipped-personal=1317`, `skipped-empty=1`. Personal is higher there because settled personal rows have not yet been stamped with the fingerprint. The live log shows exactly this run already happened against the live DB, at `2026-09-28T02:37:39Z` (`considered=1924 ... skipped_personal=1317 ... would_enrich=606 dry_run=true`). That was the one live dry run made right after install. It stamped 1317 `skipped-personal` rows and 1 `skipped-empty` row, and 794 `skipped-personal` rows still have NULL `scope_policy` (Finding D1).

Breakdown by cwd (details joined to the snapshot):

| cwd bucket | would-enrich | skipped-personal | skipped-empty |
|---|---|---|---|
| `~/repos/otto-rs/*` | 236 | 0 | 0 |
| `~/repos/scottidler/claude*` | 264 | 0 | 0 |
| `~/repos/scottidler/dotfiles*` | 57 | 0 | 0 |
| `~/repos/scottidler/keep*` | 12 | 0 | 0 |
| `~/repos/tatari-dev/*` | 1 | 0 | 0 |
| `~/repos/tatari-tv/*` | 1 | 0 | 1 |
| `~/repos/scottidler/` other | 0 | 671 | 0 |
| outside `~/repos` (unanchored) | 35 | 1 | 0 |

- Design AC 5 target population (`skipped-personal`, dormant, not archived-unstaged), queried read-only on live: **570**. Dry-run work rows inside the targets: 236 + 264 + 57 + 12 + 1 = **570**. Exact match, so a real run would clear the AC.

Spot checks:

- **`~/repos/scottidler/claude`**: 292 `skipped-personal` rows live (repo `scottidler/claude`, `git-origin`), including `.claude/worktrees/f2-*` (9) and `HOME/.claude/skills/*` (4). 264 would enrich. The other 26 are not dormant yet (`coalesce(activity_at, modified) >= now-7d`), and 5 NULL-status rows are also still active. Nothing under the canonical dir, or any sibling or worktree, is personal.
- **`~/repos/otto-rs/otto`**: 239 rows, all `git-origin` `otto-rs/otto`. 236 would enrich. The 3 left out (`5eca319f`, `83363a5c`, `8d2115ec`) are `archived=1` with no staged copy, which is the documented exclusion.
- **`~/repos/tatari-dev/*`**: only 1 row in the catalog, `tatari-dev/calvin-skills` (`f45aa37d`). It would enrich.
- **`second-brain` touching `scottidler/claude`**: 2 rows (`9dfec73e`, `47ee21e5`), cwd `~/repos/scottidler/second-brain`, `repo=scottidler/claude` via `files-touched`. Personal, as the design specifies for a Named owner.
- **Unanchored cwds** (`~`, `~/tmp`, `~/.claude`, `/tmp`, `~/pentest`, `~/Documents`, `~/.config/herdr`): 35 rows go work by touch set (`scottidler/claude` 23, `scottidler/dotfiles` 12). This is the designed touch-set path, but note that sessions run from `~` or `/tmp` go off-machine because they edited a listed repo (O3).
- `/tmp/claude-1000/fabric-main` (`danielmiessler/fabric`): personal. Correct.

### Policy variants (each on a fresh snapshot copy, scratch config)

| Policy | considered | would-enrich | skipped-personal |
|---|---|---|---|
| `[]` | 1924 | 0 | 1924 |
| operator list in mixed case (`Tatari-TV/*`, `OTTO-RS/Otto`, `ScottIdler/Claude`, ...) plus `"!scottidler/claude"` | 1924 | 319 | 1604 |

- In the exclude run, no would-enrich row has repo `scottidler/claude`. Rows by repo: `otto-rs/otto` 236, `scottidler/dotfiles` 69, `scottidler/keep` 12, `tatari-dev/calvin-skills` 1, `tatari-tv/persona-cli` 1. The exclude is terminal on both the git-origin and the touch-set paths.
- Stored fingerprint: `{"ptns":["!scottidler/claude","otto-rs/otto","scottidler/claude",...,"tatari-tv/*"],"roots":["/home/saidler/repos"]}`. It is lowercased, sorted, keeps `!`, and the policy change re-offered every `skipped-personal` row (`considered` jumped from 1279 to 1924).

## Focus: `scope` in `clyde session export`

```bash
clyde session export --no-reindex > exp.json
jq '{sv:."scope-version", cursor, n:(.sessions|length), scopes:([.sessions[].scope]|group_by(.)|map({(.[0]):length})|add)}' exp.json
# {"sv":5,"cursor":597967,"n":3386,"scopes":{"personal":1848,"work":1538}}
```

- The JSON is valid. `scope` is present on every row, and its values are only `work` or `personal`. The envelope has `scope-version: 5` (Phase 2's bump).
- **Export disagrees with the current policy on 577 rows (Bug 2).** Joining the dry-run details to the export gives `work->personal` for 577 rows, `personal->personal` for 551, and `work->work` for 1. All 577 carry stored `scope='personal'`, `scope_version=4`, `scope_policy=NULL`, status `skipped-personal`. Example: `clyde session export --no-reindex --id 0023c1c2 | jq -c '{"scope-version", s: .sessions[0] | {scope, cwd}}'` gives `{"scope-version":5,"s":{"scope":"personal","cwd":"/home/saidler/repos/scottidler/claude"}}`.

## Focus: schema v14 columns (live, read-only)

```bash
sqlite3 -readonly ~/.local/share/clyde/sessions.db "pragma user_version; select count(*) from pragma_table_info('sessions') where name in ('scope_policy','repo_host_slug');"
# 14
# 2
```

- Both columns are `TEXT`. Design AC 7: **Pass.**
- `scope_policy` distribution: NULL/`ok` 1692, fingerprint/`skipped-personal` 1317, NULL/`skipped-personal` 794, NULL/NULL 144, fingerprint/`skipped-empty` 1. The 1318 stamped rows were all written by the 02:37Z dry run. No non-dry enrich has run since the v14 migration: the last `dry_run=false` sweep is at 17:41Z, and the `pre-v14.bak` is from 19:37 local, which is 02:37Z.
- `repo_host_slug` is populated on 2434 of 3948 rows.
- `sessions.db.pre-v14.bak` exists, with `-wal`/`-shm` sidecars copied, as `snapshot_before` does.

## Command Results (broader coverage)

### session

| Invocation | Exit | Result |
|---|---|---|
| `session ls --no-reindex --limit 5` | 0 | JSON array; filters `--repo tatari-dev`, `--since 2d`, `--tag rust`, `--model opus` all work |
| `session search --no-reindex --limit 3 reposlugs` | 0 | `{"count":3,"results":[{"record":...}]}`; `--sort recency` works |
| `session search --no-reindex --limit 0 claude` | 0 | `count: 0` |
| `session doctor` | 0 | `{"total":3948,"enriched":1692,"never-enriched":2255,"skipped-personal":2111,...}` (C10) |
| `session scope --list` | 0 | `no scope overrides` |
| `session export ...` | 0 | see matrix; `--cursor` from one page's envelope yields the following rows (`[523940,523953]`) |
| `session export --id 0023c1c2 --with-body` | 0 | `body` array, 28498 chars; `--max-body-bytes 200` -> empty body, `body-truncated: true` (message-boundary safe) |
| `session enrich --dry-run` | 0 | on snapshots only (above) |
| `session resume`, `session tag`, `session reindex`, `session stage` | skipped | mutating or exec; only argument-validation errors run (Edge Cases) |

### cost (`--offline`, scratch `XDG_CACHE_HOME`)

| Invocation | Exit | Result |
|---|---|---|
| `cost --offline` / `cost today --offline` | 0 | `{"today":286.39,"sessions":56}` (JSON when piped) |
| `cost today --offline --total` | 0 | `286.39` |
| `cost yesterday --offline -v` | 0 | per-session list on a TTY; plain JSON when piped (`-v` ignored in JSON) |
| `cost daily/weekly/monthly --offline` | 0 | JSON when piped; aligned tables, bars and braille chart on a TTY (`script -qc "stty cols 120; ..."`) |
| `cost session --offline 750c9593` | 0 | `Session 750c9593: $23.94 (561 entries)` |
| `cost pricing --show --offline` | 0 | table, 5-column rates |
| `cost statusline --list` | 0 | `nerdfonts`, `scottidler (default)` |

### efficiency

| Invocation | Exit | Result |
|---|---|---|
| `efficiency daily -d 2` | 0 | JSON array of 2 periods |
| `efficiency weekly -w 1` | 0 | **2** periods (Bug 5) |
| `efficiency --worst 3` | 0 | array of 3 |
| `efficiency` (bare) | 0 | empty stdout and stderr (C3) |
| `efficiency session 0023c1c2 [--by-subagent]` | 0 | `aggregate, flags, session-id[, subagents]` |
| `efficiency session 00` | 0 | `Multiple sessions match '00':` list (Bug 6) |
| `efficiency session deadbeef-nope` | 0 | `No session found matching ...` (Bug 6) |

### report

| Invocation | Exit | Result |
|---|---|---|
| `report collect --since 2026-09-20 --until 2026-09-21 -o rep.json` | 0 | 40 sessions; coverage warning (12.5% < 50%) on stderr |
| `report collect ... \| jq '.sessions\|length'` | 0 | `40` |
| `report merge rep.json rep.json` | 0 | 40 (union dedups) |
| `report render -i rep.json -o -` | 0 | markdown on stdout; `persona whoami failed; rendering anonymously` because the sandbox blocks `tatari.okta.com` (not a clyde defect) |
| `report eval`, `render --format marquee-markdown` | skipped | model calls / publishes |

### permit

| Invocation | Exit | Result |
|---|---|---|
| `permit check` | 0 | 3 PASS lines, 328236 events |
| `permit audit --format json` | 0 | array of 730 |
| `permit audit --risk dangerous` | 0 | 7 rules |
| `permit suggest [--format json]` | 0 | 169 suggestions |
| `permit report [--format json]` | 0 | object with counts |
| `permit clean --dry-run` | 0 | `Would delete 139244 events older than 90 days (dry run, no changes made).` (source checked first: `count_older_than` only) |
| `permit install` (no `--yes`) | 0 | `OK permit hook already installed` (dry-run by default) |
| `permit apply` (no flags) | 1 | `No filter specified. Use --promote, --remove, --deny, or --all.` |
| `permit log` | skipped | writes an event |

### top level and mcp

| Invocation | Exit | Result |
|---|---|---|
| `doctor` (live) | 0 | paths block, `reposlugs-ptns:` line, attribution: 3948 rows, 0 host-refused, 2 probe-refused, 0 anchor/remote |
| `mcp status` | 0 | `user -> registered`, project/desktop not (all on stderr; C4) |
| `mcp bundle --out $T/clyde.mcpb` | 0 | 25M zip: `manifest.json`, `server/clyde` |
| `bootstrap`, `update *`, `mcp register/unregister`, `mcp serve` | skipped | mutating / stdio |

## Output Format Matrix

| Command | JSON | Table | Markdown | Other |
|---|---|---|---|---|
| `session ls/search/export/doctor`, `enrich --dry-run` | always JSON; valid through `jq` | n/a | n/a | |
| `cost today/yesterday/daily/weekly/monthly` | `--json` works; also the default when piped | TTY only; aligned | n/a | `--total` plain number works; `-g` braille chart on TTY |
| `efficiency *` | default when piped; valid | TTY | n/a | |
| `report collect/merge` | valid JSON | n/a | n/a | |
| `report render` | n/a | n/a | `-o -` works | pdf/marquee not run |
| `permit audit` | valid | works | **no header row** (Bug 11) | unknown `--format yaml` silently gives a table (Bug 12) |
| `permit suggest` | valid | works | **falls back to table** (Bug 12) | |
| `permit report` | valid | works | **falls back to text** (Bug 12) | `--format bogus` gives text, exit 0 |

## Failures & Bugs

### Doc / UX finding

D1. **`session enrich --dry-run` writes skip decisions to the catalog, and its `--help` does not say so.**
   - Help text (`clyde/src/cli.rs:323`): "Preview the gate's decisions without sending anything off-machine". That is accurate about the network, but silent about local writes.
   - Evidence: `sessions/src/enrich.rs:193`, `:213` and `:232` call `db.record_enrich_skip(...)` for personal and empty rows before the `if opts.dry_run` branch at `:275`. The UPDATE at `sessions/src/db/enrich.rs:362-368` sets `scope`, `enrich_status`, `scope_version` and `scope_policy`, and fires the revision trigger. `clyde/src/main.rs:921` also runs `lazy_reindex` first.
   - Repro on a snapshot: `sqlite3 r.db "update sessions set scope_policy=null"; clyde --db r.db session enrich --dry-run >/dev/null; sqlite3 r.db "select count(scope_policy), (select revision from export_meta) from sessions"`. Result: `0|602169` before, `1318|603490` after. The run writes 1318 rows, moves the export revision by 1321, and changes what the next run considers (1924 -> 1279).
   - **Not new in this PR.** In v0.25.6 (`git show v0.25.6:sessions/src/enrich.rs`) the same three `record_enrich_skip` calls (`:181`, `:205`, `:226`) come before the `if opts.dry_run` check (`:258`). The v0.25.6 UPDATE (`sessions/src/db/enrich.rs:346-349`) writes `scope`, `enrich_status` and `scope_version`, and the help text is identical. v0.25.7 only adds `scope_policy` to what gets written.
   - Why it matters more now: a dry run under a trial `reposlugs-ptns` list stamps the trial fingerprint, can demote an `ok` row's status to `skipped-personal`, and advances every `export --cursor` consumer.
   - Fix options: document it in `--help` ("records skip decisions locally"), or make `--dry-run` write nothing.

### Medium

2. **`session export` reports a stale `scope` under the new envelope `scope-version: 5`.**
   - Repro: `clyde session export --no-reindex --id 0023c1c2 | jq -c '{"scope-version", s: .sessions[0] | {scope, cwd}}'` gives `{"scope-version":5,"s":{"scope":"personal","cwd":"/home/saidler/repos/scottidler/claude"}}`, while the dry run under the same config classifies it `work`.
   - Scale: 577 rows. Export resolves `scope_override -> stored scope -> classify(cwd)` (`sessions/src/export.rs:76-77,149-150`), and those rows' stored scope was decided at `scope_version=4` under no policy.
   - The envelope claims v5 produced them. This stays wrong until a real (non-dry) enrich re-settles each row, and any teammate who never runs enrich keeps it forever.
   - Expected: fall through to the classifier when the stored `scope_version` is below `SCOPE_VERSION` or `scope_policy` differs from the current fingerprint; or document the staleness.
3. **`clyde doctor` hides the config error and reports healthy.**
   - Repro: `printf 'reposlugs-ptns:\n  - !tatari-tv/x\n' > $T/cfg/clyde/clyde.yml; XDG_CONFIG_HOME=$T/cfg XDG_DATA_HOME=$T/data clyde --db $T/r.db doctor; echo $?`.
   - Result: exit 0, `reposlugs-ptns: (could not load config: failed to parse config <path>)` with no reason (the quote-it hint is lost), `✓ all integrations resolve to clyde`, and `attribution: could not read the catalog: failed to load clyde config`. That last line blames the catalog, which is fine.
   - Expected: print the full error chain (`{:#}`) and exit non-zero, or at least not print the check mark.

### Low

4. **`cost daily`: a day's cost depends on `--days`.**
   - Repro: `clyde cost daily --offline -d 3 | jq -c '.days[-1]'` gives 2026-09-25 = `295.36`. `-d 4` and `-d 5` give 2026-09-25 = `295.3`.
   - Cause: the window filter (`cost/src/lib.rs:331-339`) runs before the `(message.id, requestId)` dedup (`:393-414`). A key whose winning copy falls on the day just outside the window survives as its in-window duplicate, and is counted on the oldest day.
5. **`efficiency weekly -w N` returns N+1 weeks** and does not "mirror `cost weekly`".
   - Repro: `clyde efficiency weekly -w 1 | jq -c '[.[].period]'` gives `["2026-09-27","2026-09-20"]`; `clyde cost weekly --offline -w 1 | jq -c '[.weeks[].week]'` gives `["2026-09-27"]`.
   - Cause: `efficiency/src/lib.rs:176` `start = today - (weeks*7 - 1)` days, not clipped to Sunday the way `cost/src/lib.rs:807` is.
6. **Not-found and ambiguous ids exit 0.**
   - Repro: `clyde cost session --offline nope-nope; echo $?` gives `0`.
   - Repro: `clyde efficiency session deadbeef-nope; echo $?` gives `0`.
   - Repro: `clyde efficiency session 00; echo $?` gives `0`.
   - Compare `clyde session export --id nope-nope`, which exits 1.
7. **`session enrich` validates arguments after side effects.**
   - The DB is opened (created and migrated to v14) before config load, and `lazy_reindex` runs before the `--show-payload`/`--dry-run` and id/`--all` checks (`clyde/src/main.rs:295,921-928`).
   - Repro: `clyde --db $T/new.db session enrich --show-payload /tmp/x` exits 1 but reindexes. With an invalid config, `$T/new.db` is created at v14 before the config error.
8. **`permit audit --risk <invalid>` is silently ignored.**
   - Repro: `clyde permit audit --risk bogus; echo $?` lists all 730 rules and exits 0.
   - Cause: `permit/src/lib.rs:179` `risk.and_then(RiskTier::from_str_opt)`.
9. **Unknown `--format` values fall back silently** (`permit audit --format yaml`, `permit report --format bogus`, exit 0). All three commands use a `_ =>` default arm (`permit/src/cmd/audit.rs:149`, `report.rs:108`, `suggest.rs:171`).
10. **`reposlugs-ptns` owner/repo charset is not validated.** `"!!tatari-tv/x"` loads as an exclude on owner `!tatari-tv`, and `"tatari-tv/.."` loads too. Both are dead entries that can never match, and they fail silently. See S1.
11. **`permit audit --format markdown` has no header or separator row**, so it is not a markdown table. Repro: `clyde permit audit --format markdown 2>/dev/null | head -2`. Source: `permit/src/cmd/audit.rs:151-160`.
12. **`permit suggest --format markdown` and `permit report --format markdown` are advertised in `--help` but not implemented.** They print the table/text form.
13. **Rows enriched as work, later reclassified personal, keep their LLM summary and tags.** Live row `9c810474` (`/tmp`, touched `tatari-tv/tatari-skills`): status `skipped-personal`, scope `personal`, 513-char summary and tags retained, exported under `personal`. This behavior predates the PR; it is the `ok -> skipped-personal` path the design names.
14. **`efficiency` warns once per `<synthetic>` entry.** `WARN efficiency::metrics ... unpriced model \`<synthetic>\`` plus a matching `claude_pricing::feed` WARN, about 170 of each per session scan. `cost` skips `<synthetic>` silently (`cost/src/lib.rs:322`). The live log is over 58M lines.
15. **`report render -o -` stderr says `wrote 40 sessions to stdout`.** Render writes markdown, not sessions.
16. **`session export --limit 0` error prints Debug formatting:** `--limit must be between 1 and 9223372036854775807; got Some(0)`.
17. **`cost` `--no-cache` is rejected after a subcommand** (`clyde cost daily --no-cache` gives `unexpected argument`), while `--offline` works on either side.

### Cosmetic

- C1: the `doctor` label `reposlugs-ptns:` is 16 columns wide while the other labels pad to 15, so its value is misaligned.
- C2: `cost statusline --help` long text narrates internals (`clyde/src/cli.rs`, clap debug assertion history).
- C3: bare `clyde efficiency` prints nothing and exits 0. It should print help or a default rollup.
- C4: `mcp status` writes its report to stderr only, so `clyde mcp status | grep user` matches nothing.
- C5: `mcp bundle --out` has no help text.
- C6: in `report render` markdown frontmatter, `tags:` is followed by a blank line.
- C7: `cost weekly/monthly --rolling -w N` returns N+1 buckets, the oldest one partial, labelled like a full week or month (e.g. `-m 2 --rolling` gives 2026-07 `$1601.95` vs `$9906.90` clipped).
- C8: unquoted `- */x` and flow-style `[!tatari-tv/x]` fail with raw YAML scanner errors that do not name `reposlugs-ptns` or give the quote-it hint.
- C9: a `- ~` entry is reported as entry `"~"`.
- C10: `session doctor` `enriched` (1692, status `ok`) plus `never-enriched` (2255, `enriched_at IS NULL`) is 3947, not `total` 3948. Row `9c810474` has `enriched_at` set but is not `ok`.
- C11: the dry-run JSON `details` has no `cwd` or `repo`, so checking a scope decision needs a join against the DB.
- C12: `--db` does not redirect logs; scratch-DB runs log to the live `clyde.log`.

### Suggestions

- S1: validate owner/repo against GitHub's charset (`[A-Za-z0-9._-]`, no leading `!`, not `.`/`..`) so dead entries fail loudly, the same way padded hosts do in `work-remote-hosts`.
- S2: `reposlugs-ptns:` with a null value silently means zero work repos (`doctor` shows `none`). This fails closed, but it is the "I meant to fill this in" case that `de_work_remote_hosts` refuses. Consider erroring on null and requiring an explicit `[]`.
- S3: `doctor` could count rows whose `scope_policy` differs from the current fingerprint, and rows whose stored `scope_version` is below `SCOPE_VERSION`: the pending re-offer backlog.
- S4: `enrich --dry-run` details could carry `cwd`, `repo` and `basis`, so the preview reads without a DB join.

## Pipeline Recipes

```bash
# Dry-run work vs personal totals (against a DB copy; see Finding D1)
sqlite3 -readonly ~/.local/share/clyde/sessions.db ".backup /tmp/c.db"
clyde --db /tmp/c.db session enrich --dry-run | jq '.details|group_by(.status)|map({status:.[0].status, n:length})'
# [{"status":"skipped-empty","n":1},{"status":"skipped-personal","n":672},{"status":"would-enrich","n":606}]

# Export scope histogram
clyde session export --no-reindex | jq '[.sessions[].scope]|group_by(.)|map({(.[0]):length})|add'
# {"personal":1848,"work":1538}

# Export rows whose scope disagrees with the current policy
jq -r '.details[]|[."session-id",.scope]|@tsv' dry.json | sort > dry.tsv
clyde session export --no-reindex | jq -r '.sessions[]|[."session-id",.scope]|@tsv' | sort > exp.tsv
join -t$'\t' dry.tsv exp.tsv | awk -F'\t' '{print $2"->"$3}' | sort | uniq -c
#   551 personal->personal
#   577 work->personal
#     1 work->work

# Cursor pagination
c=$(clyde session export --no-reindex --limit 2 | jq -r .cursor)
clyde session export --no-reindex --cursor "$c" --limit 2 | jq -c '[.sessions[]."updated-at"]'
# [523940,523953]

# v14 columns, read-only
sqlite3 -readonly ~/.local/share/clyde/sessions.db \
  "select coalesce(substr(scope_policy,1,12),'NULL'), coalesce(enrich_status,'NULL'), count(*) from sessions group by 1,2"

# Dormancy split for the last day
clyde session export --no-reindex --since 1d --dormant-after 1h | jq -c '[.sessions[].dormant]|group_by(.)|map(length)'
# [8,71]

# Top cache-waste sessions
clyde efficiency --worst 3 | jq -c 'length'          # 3

# Report collect -> merge -> render
clyde report collect --since 2026-09-20 --until 2026-09-21 -o rep.json
clyde report merge rep.json rep.json | jq '.sessions|length'   # 40
clyde report render -i rep.json -o - | head

# Cost totals
clyde cost monthly --offline -m 2 -t                  # 13944.99
clyde cost daily --offline -d 3 | jq '.days|map(.cost)|add'

# Permit risk histogram
clyde permit audit --format json 2>/dev/null | jq -r '.[].risk' | sort | uniq -c
clyde permit suggest --format json | jq -c 'length'   # 169
```

## Edge Cases

| Input | Exit | Behavior |
|---|---|---|
| `session export --id X --limit 3` | 1 | `--id is exclusive of the bulk export filters (...)` |
| `session export --with-body --limit 1` | 1 | `--with-body/--max-body-bytes require --id` |
| `session export --id 00` | 1 | `✗ "00" is ambiguous (10 matches)` |
| `session export --id nope-nope` | 1 | `✗ no session matches "nope-nope"` |
| `session export --limit 0` | 1 | Debug-formatted message (Bug 16) |
| `session export --cursor notanumber` | 2 | clap `invalid digit found in string` |
| `session resume` (no id) | 2 | usage |
| `session resume nope-nope --no-reindex` | 1 | `✗ no session matches` |
| `session ls --since garbage` | 1 | `could not parse since 'garbage': expected a span (e.g. 7d), RFC 3339, or YYYY-MM-DD` |
| `session search` (no query) | 2 | usage |
| `session scope` (no mode) | 1 | `pass exactly one of --set, --clear or --list` |
| `session scope --set work --session X` | 1 | `--set requires --reason: an unexplained routing flip is not auditable` |
| `session enrich --show-payload /tmp/x` | 1 | correct message, but only after a reindex (Bug 7) |
| `session reindex --session abc` | 1 | `--session requires --reresolve-repo or --clear-probe` |
| `session tag` (no id) | 2 | usage |
| `report collect --since garbage` | 1 | parse error |
| `report collect --since 2026-09-21 --until 2026-09-20` | 1 | `--since (...) is after --until (...)` |
| `report merge` (no inputs) | 1 | `no input files given; nothing to merge` |
| `permit audit --risk safe --apply` | 2 | clap conflict |
| `permit audit --risk bogus` | 0 | filter ignored (Bug 8) |
| `permit apply` (no flags) | 1 | names the required flags |
| `efficiency session 00` / `deadbeef-nope` | 0 | Bug 6 |
| `cost session nope-nope` | 0 | Bug 6 |
| 21 invalid `reposlugs-ptns` shapes | 1 | all named; see table above |

## Release Validation

- Tag `v0.25.7`: exists, annotated (`git cat-file -t` gives `tag`), points to `2a4c67fc8adf489940ed4f7d0b70e5a4502ee770`, which is `HEAD`/`main`.
- Release: published 2026-09-28T02:39:28Z, not a draft, not a prerelease.
- Assets: `linux-amd64`, `linux-arm64`, `macos-arm64`, `macos-x86_64` `.tar.gz`, each with a `.sha256`. Present: all 4 targets.
- Binary test: downloaded `clyde-v0.25.7-linux-amd64.tar.gz` (host `Linux x86_64`). The sha256 `4280c45d...cdfcc0c` matches the published file. The extracted `./clyde --version` prints `clyde v0.25.7`, the same as the installed binary. Its `doctor` against a scratch config prints the expected normalized `reposlugs-ptns:` line.

## Observations

- O1: the `reposlugs-ptns` validator and its messages are good. Every design-listed rejection names the key and the entry, and the unquoted-`!` hint fires on the common block style.
- O2: the fingerprint re-offer works as designed. A policy change re-offered all 1924 eligible `skipped-personal` rows, and an unchanged policy on a settled snapshot considered 1279.
- O3: the unanchored touch-set path sends sessions run from `~`, `/tmp`, `~/pentest` and similar dirs when every edit touched listed repos (35 rows live). The design states this; worth a conscious yes because `scottidler/claude` and `dotfiles` are now listed.
- O4: `session ls`, `search` and `export` default to a lazy reindex, which writes the catalog. Read-only use needs `--no-reindex` on every call.
- O5: the design AC 5 population (570) matches the dry run's work rows inside the targets exactly, so a real `clyde session enrich` would satisfy AC 5 and AC 6. That was not run (not approved).
