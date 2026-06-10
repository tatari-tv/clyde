# CLAUDE.md - claude-pricing

Shared Claude pricing data + JSONL parsing + cost math for Tatari tools (`ccu`, `cr`). `[lib]`-only crate; consumers depend by git **tag**.

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
