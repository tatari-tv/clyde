# CLI Shakedown Report: clyde v0.12.1

Scope: the surfaces we just changed -- the `efficiency` capability, `session reindex` (v7 migration + recompute), and the `session_efficiency` MCP tool. This is not a full-CLI sweep; `session`, `mcp`, and the other subtrees were out of scope.

Focus: prove the named-subagent type recovery (v0.12.1, PR #53) actually lands across every efficiency surface a real consumer touches.

## Summary

| Metric | Count |
|--------|-------|
| Commands discovered (efficiency subtree) | 4 (`session`, `--worst`, `daily`, `weekly`) + MCP `session_efficiency` |
| Commands tested | 5 |
| Passed | 5 |
| Failed | 0 |
| Skipped | `--narrate` with-key (paid LLM call; fail-loud path tested instead) |
| Output shapes validated | 2 (CLI snake+`totals`, MCP/persisted kebab+`raw`) |
| Edge cases | 3 (nonexistent id, missing arg, ambiguous prefix) |
| Release assets | 4 targets + 4 checksums, published |

## The headline: named-agent-type recovery works end-to-end

Baseline before the fix: 274 subagents / $2,132 in the `(unknown)` bucket. After `clyde session reindex` on v0.12.1:

- Corpus: `(unknown)` subagents **274 -> 28** (the 28 are hash-only agentIds with no embedded name -- unknown by design). `phase-implementer` corrected **$1,977 -> $3,469**; `general-purpose` 222 -> 307; `review-panel` 165 -> 188.
- Live spot-check on `38a690c7` (mcp-io-rs): its three named agents `aclyde-port` / `amarquee-port` / `apersona-port` all resolve to `general-purpose` with correct costs ($35.88 / $58.31 / $22.44), on **every** surface:

| Surface | Types recovered? | Cost correct? |
|---|---|---|
| `efficiency session --by-subagent --json` | yes | yes ($226.22 aggregate) |
| `efficiency --worst` | yes | yes |
| `session_efficiency` MCP tool | yes | yes |
| catalog (`efficiency_json`) | yes | yes |

Debug log confirms the mechanism: `fold: subagents=3 spawn-types=3 unknown-agent-types=0 aggregate-cost-usd=226.22`, files=4 (parent + 3 sidecars grouped).

## Command results

- `efficiency session <id>` -- aggregate for one session. Recomputes live from transcripts (parent + sidecars). Correct.
- `efficiency session <id> --by-subagent` -- adds the per-subagent breakdown array. Correct; recovered types present.
- `efficiency --worst 5 --json` -- ranks by ascending `cache-read-share` (crs=0.0 first). Correct, valid JSON.
- `efficiency daily -d 3 --json` -- per-day aggregates, valid JSON array.
- `efficiency weekly -w 2 --json` -- per-week aggregates, valid JSON array.
- `session_efficiency` MCP tool -- `state: "computed"`, kebab-case blob read from the catalog annotation. Matches persisted verbatim.

## Output format matrix

| Command | table (TTY) | `--json` | shape |
|---|---|---|---|
| `session` | default | yes | snake_case, `aggregate.totals.*` |
| `--worst` | default | yes | snake_case, `aggregate.totals.*` |
| `daily` / `weekly` | default | yes | snake_case, `aggregate.totals.*` |
| MCP `session_efficiency` | n/a | always | kebab-case, `aggregate.raw.*` |

Table (TTY) rendering was not exercised -- this harness pipes stdout, so JSON is the TTY-detect default. Validated the JSON paths instead.

## Findings

### 1. CLI `--json` and MCP/persisted JSON use different key shapes -- FIXED

> Closed by the follow-up `fix(efficiency): unify json output to kebab case`: the CLI now serializes the same kebab-case domain types as the MCP/persisted/export surfaces. The description below is the finding as originally reported.


Same data, two shapes:
- CLI `--json`: **snake_case**, cost at `aggregate.totals.cost_usd`, a `totals` group.
- MCP / persisted `efficiency_json` / export: **kebab-case**, cost at `aggregate.raw.cost-usd`, a `raw` group.

This is a real consumer footgun -- it tripped me mid-shakedown: `jq '.aggregate.raw.cost_usd'` against CLI `--json` returns `null` (wrong group AND wrong case), which reads as "the command is broken" when the data is fine. A script that moves between `clyde efficiency --json` and the MCP tool / export blob will hit this.

Already flagged to Scott as undecided (unify vs leave). Recommendation: unify on one shape (kebab + `raw`, matching the persisted/export contract), or document the divergence in `efficiency --help`. Severity: suggestion.

### 2. `weekly -w 2` returned 3 buckets (minor)

Asked for 2 weeks, got 3 period rows -- likely the partial current week plus 2 full. Cosmetic; worth confirming the off-by-one is intentional. Severity: cosmetic.

## Edge cases

- Nonexistent id (`zzzzzzzz-dead-beef`): clean `No session found matching '...'`. No crash.
- Missing required `<ID>`: clap usage error naming the missing arg.
- Ambiguous prefix (`a`): lists all matching session ids for disambiguation.
- `--narrate` without `ANTHROPIC_API_KEY`: fails loud -- `ANTHROPIC_API_KEY not set; enrichment needs the work Anthropic key on this host`. No network attempted. The with-key path was live-verified in the prior handoff and not re-run here (paid call).

(Exit codes were measured through a `| head` pipe, so `$?` reflected `head`, not clyde -- behaviors above are by observed output, not exit status.)

## v7 migration + reindex

- On first open with v0.12.1 the v7 migration NULLed the efficiency annotation (populated rows 1564 -> 0), trigger-suppressed (export cursor untouched).
- `clyde session reindex` recomputed all candidates: `{scanned: 1592, efficiency: {candidates: 1592, computed: 1592, written: 1592}}`. One-time full recompute, as designed.

## Release validation

- Tag `v0.12.1`: annotated (`git cat-file -t` = `tag`), points to `5d489d4` == `origin/main`.
- GH release `v0.12.1`: published (not draft). Assets: linux-amd64, linux-arm64, macos-arm64, macos-x86_64 -- each `.tar.gz` + `.sha256`. All four targets present.
- Local `~/.cargo/bin/clyde` reports `v0.12.1`, matching the tag. (Did not download+run a remote asset; the locally-built binary was exercised throughout.)

## Verdict

The v0.12.1 change is solid on every efficiency surface: named-agent types recover, costs are correct and consistent CLI/MCP/catalog, the v7 recompute landed cleanly, and the release is fully published. The one standing observation (the CLI-vs-persisted JSON shape divergence) is now closed by the follow-up `fix(efficiency): unify json output to kebab case`.
