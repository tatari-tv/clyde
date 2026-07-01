# CLAUDE.md - claude-pricing

Shared Claude pricing data + JSONL parsing + cost math for Tatari tools (`ccu`, `cr`). `[lib]`-only crate; consumers depend by git **tag**.

## Feed publishing & daily refresh automation (read before hand-editing `pricing.json`)

`data/pricing.json` is a **generated artifact kept current by automation**, not a hand-maintained file. Before editing it by hand, know the pipeline:

- **Runtime feed (what consumers actually read):** `ccu`, `cr`, and clyde's `pricing/` crate fetch the live feed at runtime from `https://tatari-tv.github.io/claude-pricing/pricing.json` (GitHub Pages). A data refresh reaches consumers within ~24h with no crate bump or re-pin. (In-flight: the default URL is being repointed to `https://tatari-tv.github.io/clyde/pricing.json`; see clyde `docs/design/2026-06-29-move-pricing-feed-publishing-to-clyde.md`.)
- **Daily refresh (`.github/workflows/refresh-pricing.yml`):** cron `17 6 * * *` runs `bin/update`, which scrapes Anthropic's `https://platform.claude.com/docs/en/about-claude/pricing.md`, regenerates `pricing.json`, and opens a `refresh-pricing` PR when data changed.
- **Publish (`.github/workflows/pages.yml`):** merging a `pricing.json` change to `main` deploys it to GitHub Pages.
- **These workflows are LIVE only in `tatari-tv/claude-pricing`.** The byte-identical copies vendored into clyde at `pricing/.github/workflows/` are **dormant**; GitHub runs workflows only from the repo root, never from a subdirectory.

### New-model launches are NOT fully hands-off

`bin/update` derives each model key by slugifying the pricing-table **row label**. When Anthropic ships a model with date-tiered introductory pricing (two rows, e.g. `Claude Sonnet 5 through August 31, 2026` and `Claude Sonnet 5 starting September 1, 2026`), the parser emits **broken keys** like `claude-sonnet-5-[through-august-31,-2026]` and `claude-sonnet-5-starting-september-1,-2026` instead of a clean `claude-sonnet-5`, and the `sonnet`/`opus`/`haiku` aliases (human-authored policy in `data/normalization.json`) are **not** auto-repointed. A launch like Sonnet 5 therefore needs a human to: pick the canonical id (`claude-sonnet-5`), choose the correct pricing tier, fold the broken rows into it, and repoint the alias. Until then the daily cron keeps regenerating garbage keys.

## ⚠️ Versioning rule: crate MAJOR == feed `schema_version` (DO NOT fuck this up)

The crate's **major version is locked to the feed `schema_version`**:

> **`vN.x.x` ⇔ `schema_version N`.** A `v2` tag means schema 2. A `v3` tag means schema 3.

- A feed **schema bump is a major crate bump**, and a major crate bump must correspond to a schema change. They move together, always.
- Minor/patch crate bumps = library changes that keep the **same** schema.
- `min_library_version` in the published feed = the crate version that ships that schema (schema-2 feed advertises `2.0.0`).
- **Never let the tag major and `schema_version` diverge.** A `v2` tag carrying schema 1 (or vice versa) is the exact confusion this rule prevents - consumers pin a tag whose number lies about the schema the library understands.
- The crate sat at `0.x` during schema 1; the convention was adopted at schema 2, so the first schema-2 release is `v2.0.0` (there is intentionally no `v1.x`).

When asked to "bump for a new schema": the new tag's major **is** the new `schema_version`. Don't pick an unrelated number (e.g. `0.3.0` for schema 2 - that desyncs them).

## Normalization policy

- `aliases` + `family_rules` (schema 2+) are **human-authored policy** in `data/normalization.json`, spliced into the published `data/pricing.json` by `bin/update`. `data/pricing.json` is a generated artifact.
- Every alias target and family `canonical` MUST be a real key in `pricing` (CI test `embedded_normalization_contract_is_valid` enforces this). A dangling canonical means a model silently fails to price.

## Build / test

- `otto ci` - full pipeline (test + clippy + fmt + whitespace).
- Tests live in `src/<mod>/tests.rs` (Rust 2018 submodule style), never inline `#[cfg(test)] mod tests` blocks.

## Release flow (PR-protected `main`)

1. Land changes via PR (squash merge - no merge commits).
2. Bump `Cargo.toml` version per the versioning rule above; set `bin/update` `MIN_LIBRARY_VERSION` to match; merge that PR.
3. Tag on `main` **after** merge (annotated, `vN.x.x`), then push the tag. Never tag pre-merge (a squash orphans it) and never delete/move a tag.
4. Re-pin `ccu`/`cr` to the new tag (minor bump each) and re-test.
