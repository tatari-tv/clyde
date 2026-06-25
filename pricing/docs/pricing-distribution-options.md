# claude-pricing: Distribution Options

How does the `claude-pricing` Rust library get fresh model pricing into the
hands of the binaries that depend on it (`ccu`, `cr`, future tools), without
the maintainer manually rebuilding every time Anthropic ships a new model or
adjusts a number?

This doc lays out the option space, the tradeoffs, and a recommended path.
The intent is to share it for feedback before any code is written.

## Background

Anthropic publishes pricing as markdown:
<https://platform.claude.com/docs/en/about-claude/pricing.md>

`ccu` (claude-cost-usage) and `cr` (claude-report) each currently:

1. Embed `data/pricing.json` directly into the binary via `include_str!`.
2. Provide a `bin/update` script that fetches the pricing markdown, runs a
   deterministic awk parser over it, and rewrites `data/pricing.json` plus a
   SHA-256 of the page (for staleness detection).
3. Expose `--check` (compares embedded SHA to live page SHA) and `--show`.

The parser is deterministic, no LLM in the loop. That part is settled and
should not change. (See thread:
<https://tatari.slack.com/archives/C01FXF7P3ST/p1773257748451699>.)

The remaining pain:

- Anthropic ships a new pricing row, the maintainer runs `bin/update`,
  bumps, rebuilds, pushes the tag, runs `cargo install`.
- That cycle multiplies across every consuming tool. N tools x every
  pricing change = N rebuilds.
- Users who installed a tool months ago keep getting stale numbers until
  they reinstall.

The library being built (`claude-pricing`) is the right place to fix this.
The question is the strategy.

## The axes

There are three orthogonal choices:

1. **Where does pricing live at runtime?** Baked into the binary, on disk
   as a user-controlled override, fetched fresh, or some combination.
2. **What triggers a refresh?** A human running `bin/update`, a cargo
   version bump, a runtime TTL, or a CI cron.
3. **What is the source of truth?** Anthropic's `pricing.md` (we parse it),
   LiteLLM's JSON file (third-party, crowdsourced), or a Tatari-published
   feed that we host and control.

## Five options

### A. Status quo: baked in, manual `bin/update`

The library ships `pricing.json` baked in. Bumping the library version is
how new pricing reaches downstream tools. Consumers `cargo update` and
rebuild to pick it up.

- **Pro:** zero runtime cost, hermetic, fully offline, no surprise breakage.
- **Con:** the pain we are trying to fix. The maintainer is the bottleneck;
  staleness window is "however often the maintainer remembers to look."
  Installed binaries do not self-heal.

### B. CI cron + auto-cut release

Same as A, but a GitHub Action in `claude-pricing` runs `bin/update` on a
daily cron. If `pricing.json` changed, the action opens a PR (or commits
directly and auto-tags). The maintainer reviews and merges; consumers still
`cargo update` to pick up.

- **Pro:** automates the boring half of A while keeping the hermetic
  property. The maintainer stays in the loop for unexpected page-format
  changes (a failed parse fails the action, surfaces a notification).
- **Con:** consumers still need to bump and reinstall. Does not help
  binaries already installed in the field.

### C. User-config override

The library reads `~/.config/claude-pricing/pricing.json` if present, falls
back to embedded otherwise. Add a one-shot CLI (`claude-pricing-update`, or
`ccu pricing --update`, or `cr pricing --update`) that fetches, parses,
and writes the override file.

- **Pro:** users can fix staleness without reinstalling. A single update
  file fixes every consuming tool on the box at once.
- **Con:** users still need to remember to run the update command. Same
  procrastination problem as A, just relocated from maintainer to user.

### D. Runtime fetch with TTL cache

The library checks `~/.cache/claude-pricing/pricing.json`. If the cache is
older than N days (or missing), it fetches the live pricing page, parses
it, and writes the cache. On network failure it falls back to the embedded
default.

- **Pro:** self-healing. Users get fresh pricing without thinking about it.
  Works for binaries already installed in the field.
- **Con:** introduces a network call into tools that previously had none.
  For `ccu` that means pulling in an HTTP client just for this; `cr`
  already has `ureq`. Air-gapped environments need an opt-out. The
  brittleness now lives in the library: if Anthropic restructures the page
  on a Tuesday, every cron-driven Tatari tool errors on Wednesday.

### E. Tatari-hosted JSON feed

The `claude-pricing` repo runs a daily GitHub Action. The action runs
`bin/update` against Anthropic's `pricing.md`. On change it opens a PR with
the new `pricing.json`. After merge, GitHub Pages publishes
`https://tatari-tv.github.io/claude-pricing/pricing.json`. The library's
runtime fetcher (when enabled) hits **that** stable URL, not Anthropic.

- **Pro:** decouples "raw fragile source" (Anthropic's markdown) from
  "stable contract our binaries consume" (our JSON). The brittle parse
  runs **once**, in CI, with humans in the review loop. Clients fetch a
  stable schema we control. If Anthropic's page format breaks, the PR
  fails; no client in the field breaks.
- **Con:** one more thing to maintain (a workflow plus a Pages deploy).
  Adds an HTTP dependency to `ccu` if we want runtime fetch from inside
  that tool.

## Recommendation

Combine **B + C + E**, build them into the library as layers, and let each
consumer choose how aggressive it wants to be.

```
Layer 1 (always on):  Embedded JSON           - works offline, day-zero
Layer 2 (opt in):     User config override    - manual override / pinning
Layer 3 (opt in):     Runtime fetch + TTL     - falls back to L2, then L1
```

The library exposes three constructors:

```rust
// Layer 1 only: the embedded baseline
let pricing = claude_pricing::Pricing::default();

// Layer 2 -> Layer 1: respect user override, fall back to embedded
let pricing = claude_pricing::Pricing::with_user_override(app_name)?;

// Layer 3 -> 2 -> 1: TTL'd runtime fetch, layered fallback
let pricing = claude_pricing::Pricing::auto(app_name)?;
```

The publishing side, which lives in the `claude-pricing` repo itself:

- `bin/update` (deterministic awk parser, already exists in `ccu`)
- A GitHub Action on a daily cron that runs `bin/update` and opens a PR on
  diff. Maintainer reviews; merge auto-tags a release.
- GitHub Pages deploys `pricing.json` from `main` to a stable URL.
- The library's Layer 3 fetcher hits that URL, never Anthropic's directly.

### Per-consumer guidance

- **`ccu`** is hot-path, local-first, currently no network. Start it on
  Layer 1; optionally Layer 2 via `ccu pricing --update`. Do not pull in
  an HTTP client just for Layer 3 unless the pain returns.
- **`cr`** already speaks HTTP. Layer 3 is cheap. Use `Pricing::auto`.

This answers the "what IS the answer for a tool you want others to use,
that you know the pricing will change" question:

> The brittle parse runs once, in CI, on PR review, in one repo.
> Everything downstream consumes a stable contract.

## Worth challenging

A few questions worth pushing on before committing:

- **Should the library do HTTP at all?** If the answer is "no, keep it a
  pure data + math library," drop D/E from the core crate and put the
  runtime-fetch logic in a sibling crate (`claude-pricing-fetch`). Keeps
  the core dep-light. `ccu` consumes the core only; `cr` (or any future
  network-friendly tool) opts into the fetcher crate.
- **LiteLLM as the upstream feed instead of Anthropic's page.** It is
  already JSON, so the parse is trivial. But you trade a brittle parser
  for a third-party-controlled schema that occasionally lags Anthropic
  releases or names models in non-canonical ways. Likely not worth it.
- **GitHub Pages feed is overkill?** If the daily-PR + auto-release flow
  in option B is good enough, do not stand up a Pages site. Try B first;
  escalate to E only if installed-binary staleness keeps biting users.

### Suggested phasing

1. Ship the library with Layer 1 (embedded JSON, normalize, calculate).
   This is what `ccu` and `cr` need today, deduped into one place.
2. Add option B (CI cron + auto-PR + tag) in the library repo. This
   removes most of the maintainer pain with the smallest amount of new
   infrastructure.
3. Add Layer 2 / option C at the same time as B; it is essentially free
   once the embedded path exists.
4. Hold Layer 3 / option E in reserve. Add it only if users complain that
   "I installed three months ago and my dollars-per-day is wrong."
