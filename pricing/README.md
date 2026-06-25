# claude-pricing

Library that owns Claude pricing data, JSONL session parsing, and cost math. A member crate of the
[`clyde`](../README.md) umbrella workspace, consumed by `cost` and `report` as a path dependency
(`claude-pricing = { path = "../pricing", features = ["fetch"] }`); they pass `app_name = "clyde"`
so the user pricing override resolves to `~/.config/clyde/pricing.json`.

No binary. Public API unchanged: `claude_pricing::...`. The crate version stays locked to the feed
`schema_version` (see `CLAUDE.md`), so it does NOT inherit the workspace version line.

This crate is also the publishing point for the JSON pricing feed the tools fetch at runtime.
