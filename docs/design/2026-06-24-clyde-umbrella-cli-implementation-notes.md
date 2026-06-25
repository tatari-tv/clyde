# Implementation Notes: clyde umbrella CLI

Running record of how the implementation diverges from or interprets
`docs/design/2026-06-24-clyde-umbrella-cli.md`. Append-only.

## Phase 0: Rename klod to clyde

### Design decisions
- Renamed the XDG namespace constant `KLOD_DIR` -> `CLYDE_DIR` (`session/src/paths.rs`)
  rather than keeping the symbol name and only changing its value, so the symbol matches
  the value and greps stay honest.
- Left the literal on-disk path `/home/saidler/repos/tatari-tv/klod/main` in
  `session/src/scope/tests.rs` unchanged. That test only exercises org-based work/personal
  classification (any `tatari-tv/*` path is Work); the string reflects the real local
  checkout dir, which is still named `klod`. Renaming the GitHub repo (and thus the local
  dir) is a separate ops step out of scope for the code rename.
- Renamed the `argv[0]` literals in `clyde/src/cli/tests.rs` from `"klod"` to `"clyde"` for
  accuracy even though clap ignores the program name during `try_parse_from`.

### Deviations
- None. The rename is exactly the Phase 0 scope: member dir, bin name, crate name,
  `default-members`, XDG path constants, doc comments, crate descriptions, README, and the
  `CARGO_BIN_EXE_*` reference in the serve integration test.

### Tradeoffs
- Used a scoped `perl -i` bulk replace for the doc-comment/description references in the
  `session`/`sessions` library crates and the README (all unambiguous `\bklod\b` -> `clyde`),
  vs. per-line Edits. The whole-word boundary plus an explicit exclude of `scope/tests.rs`
  kept the real on-disk path intact. The load-bearing source edits (paths.rs constants, cli.rs
  name, main.rs log filename) were done as explicit Edits, not the bulk pass.

### Open questions
- None.

## Phase 1: Subtree-merge the four repos

### Design decisions
- Subtree-added from the local clones (`/home/saidler/repos/tatari-tv/<repo>` `main`) rather
  than the GitHub remotes; the local checkouts were clean and on `main`, and a local fetch is
  faster and offline. Full history (no `--squash`), so each merge commit carries the original
  lineage as its second parent (verified: `git log <add-commit>^2` reaches the pre-merge HEAD;
  total workspace history grew to 222 commits).
- Added all four (`report`, `cost`, `permit`, `pricing`) to `[workspace] members` in this phase
  per the design. This leaves the workspace intentionally non-building (git-pinned pricing dep,
  unreconciled dep versions, two `[[bin]]` packages with no lib) until Phase 2 — consistent with
  the design's "import-only; no clean build expected yet" and the PR-B grouping (Phases 1-3 land
  as one green unit).

### Deviations
- None.

### Tradeoffs
- Left the imported nested `Cargo.lock`, `.otto.yml`, `install.sh`, `clippy.toml`, and
  `rustfmt.toml` files in place for now. They are redundant under a single workspace but removing
  them is Phase 2 (deps/lints reconciliation) and Phase 6 (CI/docs) work; deleting them in the
  import commit would muddy the "import-only" boundary.

### Open questions
- None.
