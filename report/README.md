# report

Scans Claude Code session JSONL files and emits a per-host JSON report, plus a synthesized
markdown writeup. A member crate of the [`clyde`](../README.md) umbrella workspace.

- Umbrella: `clyde report <collect|render>`
- Compat shim: `cr ...` (behavior-exact with the pre-merge `claude-report` tool)

Library API: `report::{ReportArgs, ReportCli, run}`. See the top-level README and
`docs/design/2026-06-24-clyde-umbrella-cli.md` for the umbrella architecture.
