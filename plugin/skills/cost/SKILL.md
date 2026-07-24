---
name: cost
description: Report Claude Code cost and token usage, or build a per-host usage report, via the clyde CLI - "how much did I spend today/this week", "token usage this month", "generate a usage report", "cost for this session". Use for any Claude spend/usage question. This is CLI-only; clyde's MCP server has no cost or report surface.
user-invocable: true
argument-hint: "<today | weekly | monthly | session>  (or: report collect | render | merge)"
---

# clyde:cost

Answer Claude Code spend/usage questions and build usage reports. This is
**CLI-only** - clyde's session-catalog MCP server exposes no cost or report
tools, so reach for the CLI directly.

## `clyde cost` - spend and token usage

Eight verbs (all of them):

```bash
clyde cost today        # today's total cost (this is also the default: bare `clyde cost`)
clyde cost yesterday    # yesterday's total
clyde cost session <id|current>   # cost for a specific session (id, or "current")
clyde cost daily        # daily costs across a date range
clyde cost weekly       # weekly summary (Sun-Sat weeks, clipped to Sunday)
clyde cost monthly      # monthly summary (clipped to the 1st)
clyde cost pricing      # manage / inspect model pricing config
clyde cost statusline   # install a Claude Code statusline
```

- Useful flags: `--model <MODEL>` to filter to one model, `--no-cache` to
  recompute from the JSONL instead of the cost cache, `--offline` to skip the
  network pricing refresh and use the embedded/override baseline.

## `clyde report` - per-host usage report

Three verbs (all of them):

```bash
clyde report collect    # scan session JSONL files, emit a per-host JSON usage report
clyde report render --format <markdown|pdf|html|marquee-html|marquee-markdown>   # render a collected report
clyde report merge      # merge two or more collected JSON reports into one
```

- `render` can publish straight to a marquee post (`--format marquee-html` /
  `marquee-markdown`); PDF rendering needs `pandoc`, and the marquee formats need
  the `marquee` CLI - `clyde report` reports which required tools are present.
- Typical flow: `collect` (produce the JSON) -> `render` (turn it into the
  output format you want); `merge` combines multiple collected reports first.
