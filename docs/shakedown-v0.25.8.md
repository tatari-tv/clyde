# CLI Shakedown Report: clyde v0.25.8

Scope: the areas PR #97 (https://github.com/tatari-tv/clyde/pull/97) touched. MCP startup and the six MCP tools, `clyde session reindex`, and the catalog read paths that run a lazy reindex first (`session ls`, `session search`). Everything else in clyde is unchanged by this release and was not re-exercised.

## Summary

| Metric | Count |
|--------|-------|
| Commands discovered (in scope) | 11 (`mcp serve` + 6 tools, `mcp status`, `session reindex`, `session ls`, `session search`) |
| Commands tested | 11 |
| Commands passed | 11 |
| Commands failed | 0 |
| Commands skipped | 0 |
| Pipelines tested | 4 |
| Edge cases tested | 5 |

## Headline numbers

| | v0.25.7 | v0.25.8 |
|---|---|---|
| `mcp serve` time to `initialize` response | 29.7s | 0.84-0.89s (3 runs) |
| `session reindex` | 32.7s | 2.00s, 1.72s |
| `session ls` (lazy reindex included) | not measured | 0.88s |
| `session search` (lazy reindex included) | not measured | 1.16s |

Claude Code's MCP connect timeout is 30s, which v0.25.7 had crossed.

## Command Results

### MCP over stdio

All driven as JSON-RPC on stdin: `initialize`, `notifications/initialized`, then `tools/call`.

| Call | Result |
|---|---|
| `initialize` | `serverInfo {"name":"clyde","version":"0.25.8"}` |
| `tools/list` | 6 tools: `session_efficiency`, `session_grep`, `session_open`, `session_read`, `sessions_ls`, `sessions_search` |
| `sessions_search {"query":"parallel session parse","limit":3}` | `count: 3`, full records |
| `sessions_ls {"repo":"tatari-tv/clyde","limit":3}` | the live shakedown session first: `repo: tatari-tv/clyde`, `repo-source: git-origin`, `n-msgs: 440` |
| `session_open {"id":"97df47f3"}` (prefix) | `state: resumeable`, `resume-command: claude --resume 97df47f3-...` |
| `session_grep {"id":..., "query":"partition_unchanged"}` | hits, first role `assistant` |
| `session_read {"id":..., "offset":0, "limit":2}` | `state: read`, `total: 69`, role-labeled messages |
| `session_efficiency {"id":...}` | `state: not-computed` (documented: the efficiency pass has not reached this live session) |

### `clyde session reindex`

```
$ clyde session reindex | jq -c .reindex
{"scanned":3372,"upserted":0,"skipped-unchanged":3372,"backfilled":0,"archived":636}
```

Exit 0 both runs, 2.00s and 1.72s wall, 25-33 MB RSS.

### `clyde session ls` / `clyde session search`

```
$ clyde session ls --repo tatari-tv/clyde --since 1d --limit 5     # 0.88s, exit 0
$ clyde session search parallel session parse --limit 3 --sort recency   # 1.16s, exit 0
97df47f3-4052-455c-bfbb-13c489f2498f  tatari-tv/clyde    Debug broken images
3ac3d69f-6c3d-4f68-8baf-d4a1d1f931a2  tatari-tv/marquee  Terraform merge pre-flight checks
1f78f43b-c684-482b-9669-5057c4862a36  tatari-tv/clyde    Reindex parallelization and skipped unchanged sessions
```

### Skip lifecycle, end to end (isolated catalog)

One synthetic transcript under a scratch `--projects-dir`, cwd `/home/saidler/repos/tatari-tv/clyde`:

```
{"step":"new","scanned":1,"upserted":1,"skipped-unchanged":0}
{"step":"unchanged","scanned":1,"upserted":0,"skipped-unchanged":1}
{"step":"grown","scanned":1,"upserted":1,"skipped-unchanged":0}
search bravo -> count=1 repo=tatari-tv/clyde source=git-origin n-msgs=2
```

A new transcript is inserted, an untouched one is skipped without a parse, and an appended one is re-parsed (the appended text is searchable). Repo attribution resolves via `git-origin`.

## Output Format Matrix

| Command | Piped output | Valid |
|---|---|---|
| `session reindex` | JSON object | yes (`jq -e` on counts) |
| `session ls` | JSON array | yes (`array len=3`) |
| `session search` | JSON object with `results[]` | yes |
| MCP tool results | JSON text in `content[0].text` | yes (`fromjson`) |

## Failures & Bugs

None in scope.

## Pipeline Recipes

```bash
# reindex counts only
clyde session reindex | jq -c .reindex

# recent sessions for a repo: id, repo, title
clyde session search parallel session parse --limit 3 --sort recency \
  | jq -r '.results[] | "\(.record["session-id"])  \(.record.repo)  \(.record.title)"'

# time-to-initialize for the MCP server
start=$(date +%s.%N)
(cat init.json; sleep 8) | timeout 8 clyde mcp serve \
  | while IFS= read -r l; do echo "$(echo "$(date +%s.%N) - $start" | bc)s"; break; done

# is the catalog current without reindexing
clyde session ls --limit 3 --no-reindex | jq length
```

## Edge Cases

| Input | Result |
|---|---|
| `sessions_search` for a term with no hits | `count: 0`, `results: []`, not an error |
| `session_open` on a nonexistent id | JSON-RPC error `no session matches "zzzz-nonexistent"` |
| `session_grep` with a wrong argument name | `isError: true`, `failed to deserialize parameters: missing field query` |
| `session ls --since notadate` | exit 1, `could not parse since 'notadate': expected a span (e.g. 7d), RFC 3339, or YYYY-MM-DD` |
| `session reindex --projects-dir /nonexistent/projects` | exit 0, all-zero counts, a WARN in the log |

## Release Validation

- Tag `v0.25.8`: exists, annotated (`git cat-file -t` -> `tag`), points at `b530468`, the merge commit of PR #97.
- Release: published, not a draft, 8 assets: `linux-amd64`, `linux-arm64`, `macos-arm64`, `macos-x86_64` tarballs, each with a `.sha256`.
- Binary: downloaded `clyde-v0.25.8-linux-amd64.tar.gz`, `sha256sum -c` OK, `--version` -> `clyde v0.25.8`, matching the installed binary.

## Observations

- `session reindex --projects-dir` on a missing directory exits 0 with an empty result and only a WARN (`session/src/scan.rs:38-41`). That code predates this release (last touched in PR #94, https://github.com/tatari-tv/clyde/pull/94). A typo'd `--projects-dir` reads as "nothing to index" instead of an error.
- The install build prints 3 `rmcp::model::ServerInfo` deprecation warnings (`sessions/src/mcp.rs:32,487,493`). Not a failure.
- A running `clyde mcp serve` keeps the old binary loaded. Claude Code sessions need to reconnect the clyde server in `/mcp` to pick up v0.25.8.
