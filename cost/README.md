# cost

Reads Claude Code JSONL session logs and computes cost summaries (today, daily, weekly, monthly);
also installs the Claude Code statusline. A member crate of the [`clyde`](../README.md) umbrella
workspace.

- Umbrella: `clyde cost <today|yesterday|daily|weekly|monthly|session|statusline|pricing>`

Library API: `cost::{CostArgs, run}`. See the top-level README and
`docs/design/2026-06-24-clyde-umbrella-cli.md` for the umbrella architecture.
