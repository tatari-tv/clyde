# Design Document: Fix CI and Release Workflows

**Author:** Scott Idler
**Date:** 2026-03-11
**Status:** Draft
**Review Passes Completed:** 5/5

## Summary

The v0.3.0 release produced zero binary artifacts - only auto-generated source archives. GitHub Actions workflows never ran. This design doc covers: (1) diagnosing and fixing the release workflow, (2) adding a CI workflow for push/PR validation, and (3) verifying the end-to-end release pipeline produces working artifacts like otto-rs/otto does.

## Problem Statement

### Background

The previous design doc (2026-03-11-tiered-pricing-yesterday-releases.md) specified a release workflow adapted from otto-rs/otto. The workflow file and install.sh were committed, a v0.3.0 tag was created and pushed, but the GitHub release shows only 2 assets (auto-generated source archives) instead of the expected 10 (4 tarballs + 4 sha256 files + 2 source archives).

The otto-rs/otto repo has **three** workflow files:
- `ci.yml` - runs on push to main and PRs (test, clippy, fmt, build matrix)
- `release-and-publish.yml` - runs on v* tags (build 4 targets, create release, Docker)
- `test-setup-otto.yml` - tests the setup-otto GitHub Action

CCU currently has only `release.yml` and **zero** recorded workflow runs, meaning GitHub Actions has never executed for this repo.

### Problem

1. **Release workflow never fired:** v0.3.0 has zero binary artifacts. The workflow exists in the repo at the tagged commit but never ran.
2. **No CI workflow:** No push/PR validation. Otto has `ci.yml` running tests, clippy, and fmt on every push/PR. CCU has nothing.
3. **No verification step:** The previous implementation committed files but never confirmed the pipeline actually worked end-to-end.

### Goals

- Diagnose why the release workflow didn't run and fix it
- Add `ci.yml` modeled after otto's CI workflow
- Rename `release.yml` to `release-and-publish.yml` for consistency with otto
- Delete the broken v0.3.0 release/tag and re-release after fixes
- Verify end state: a GitHub release with 10 assets (4 tarballs + 4 checksums + 2 source archives)

### Non-Goals

- Docker image distribution (CCU is a local CLI tool, not a service)
- `test-setup-ccu.yml` (no setup-ccu GitHub Action exists)
- Homebrew formula or other package managers
- Changing the release workflow's build logic (it's structurally correct, adapted from a working otto template)

## Proposed Solution

### Overview

Three changes: (1) add `ci.yml`, (2) rename `release.yml` to `release-and-publish.yml`, (3) diagnose and fix the Actions trigger issue, then re-tag and verify.

### Phase 1: Add CI Workflow

Create `.github/workflows/ci.yml` adapted from otto's CI:

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

env:
  RUST_VERSION: 1.92.0
  CARGO_TERM_COLOR: always

jobs:
  test:
    name: Test
    runs-on: ubuntu-latest
    steps:
    - uses: actions/checkout@v4

    - name: Install Rust
      uses: dtolnay/rust-toolchain@master
      with:
        toolchain: ${{ env.RUST_VERSION }}
        components: rustfmt, clippy

    - name: Cache Rust dependencies
      uses: Swatinem/rust-cache@v2
      with:
        prefix-key: "v1-rust"

    - name: Run tests
      run: cargo test --verbose

    - name: Check formatting
      run: cargo fmt --check

    - name: Run clippy
      run: cargo clippy -- -D warnings

  build:
    name: Build
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        include:
          - os: ubuntu-latest
          - os: macos-14
    steps:
    - uses: actions/checkout@v4

    - name: Install Rust
      uses: dtolnay/rust-toolchain@master
      with:
        toolchain: ${{ env.RUST_VERSION }}

    - name: Cache Rust dependencies
      uses: Swatinem/rust-cache@v2
      with:
        prefix-key: "v1-rust"
        shared-key: ${{ matrix.os }}

    - name: Build
      run: cargo build --release --verbose
```

This is a direct copy of otto's `ci.yml` with the `makefile` branch removed from push triggers (CCU doesn't have that branch).

### Phase 2: Rename Release Workflow

Rename `.github/workflows/release.yml` to `.github/workflows/release-and-publish.yml` for consistency with otto. No content changes needed - the workflow logic is structurally identical to otto's (minus Docker).

**Note on GIT_DESCRIBE:** Both the CCU and otto release workflows set a `GIT_DESCRIBE` env var via shell. However, both repos' `build.rs` files run `git describe --tags --always` directly at compile time and inject the result via `cargo:rustc-env=GIT_DESCRIBE`. The workflow env var is redundant but harmless. No change needed - just documenting for clarity.

### Phase 3: Diagnose and Fix Actions Trigger

The most likely cause of zero workflow runs: **GitHub Actions may not be enabled for this repo.** The repo was created today (2026-03-11T01:55:20Z). Possible causes:

1. **Actions not enabled:** New repos sometimes need Actions explicitly enabled in Settings > Actions > General
2. **Permissions issue:** The workflow has `permissions: contents: write` but the repo-level Actions permissions may be restricted
3. **Tag timing:** If the tag was pushed via API/CLI before GitHub indexed the workflow file, the trigger may have been missed

**Fix approach:**
1. Check Actions settings in GitHub UI (Settings > Actions > General)
2. Ensure "Allow all actions and reusable workflows" is selected
3. Push the CI workflow to main - if it runs, Actions is working
4. Delete the v0.3.0 tag and release
5. Re-tag v0.3.0 (or bump to v0.3.1) and push

### Phase 4: Verify End State

After re-tagging, verify the release has all expected assets:

```
ccu-v0.3.x-linux-amd64.tar.gz
ccu-v0.3.x-linux-amd64.tar.gz.sha256
ccu-v0.3.x-linux-arm64.tar.gz
ccu-v0.3.x-linux-arm64.tar.gz.sha256
ccu-v0.3.x-macos-x86_64.tar.gz
ccu-v0.3.x-macos-x86_64.tar.gz.sha256
ccu-v0.3.x-macos-arm64.tar.gz
ccu-v0.3.x-macos-arm64.tar.gz.sha256
Source code (zip)
Source code (tar.gz)
```

Also verify `install.sh` works:
```bash
curl -fsSL https://raw.githubusercontent.com/scottidler/claude-cost-usage/main/install.sh | bash
ccu --version
```

## Alternatives Considered

### Alternative 1: Keep release.yml name as-is

- **Description:** Don't rename to match otto convention
- **Pros:** Less churn
- **Cons:** Inconsistent with the reference implementation; harder to compare/sync
- **Why not chosen:** Consistency with otto makes maintenance easier

### Alternative 2: Combine CI and release into one workflow

- **Description:** Single workflow file with conditional jobs
- **Pros:** Fewer files
- **Cons:** More complex triggers; harder to read; doesn't match otto pattern
- **Why not chosen:** Separate workflows for separate concerns is cleaner

### Alternative 3: Skip CI workflow, just fix release

- **Description:** Only fix the release trigger
- **Pros:** Smaller change
- **Cons:** No push/PR validation; every push to main is unvalidated
- **Why not chosen:** CI is table stakes. Otto has it. CCU should too.

## Technical Considerations

### Dependencies

No new Cargo dependencies. Uses same GitHub Actions as otto:
- `actions/checkout@v4`
- `dtolnay/rust-toolchain@master`
- `Swatinem/rust-cache@v2`

### Performance

CI workflow adds ~2-3 min per push/PR. Release workflow adds ~5-8 min per tag push. Both are acceptable.

### Testing Strategy

1. Push `ci.yml` to main - verify CI runs and passes (green check on commit)
2. Push a test tag (e.g., `v0.3.1`) - verify release workflow runs
3. Check release page for all 10 assets
4. Run `install.sh` on Linux to verify end-to-end download and install

### Rollout Plan

1. Add `ci.yml` (adapted from otto)
2. Rename `release.yml` to `release-and-publish.yml`
3. Push to main, verify CI workflow runs
4. If CI passes, delete v0.3.0 tag and release
5. Bump version to v0.3.1 (or re-use v0.3.0)
6. Tag and push
7. Verify release has 10 assets
8. Test `install.sh`

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Actions still not enabled after fix | Low | High | Check Settings UI manually; push CI first as canary |
| Cross-compilation fails in CI | Low | Medium | Proven pattern from otto; same Rust version and targets |
| Deleting v0.3.0 tag breaks something | Low | Low | No users yet (0 stars, 0 forks, repo created today) |
| CI flaky on macOS runners | Low | Low | Otto runs the same matrix successfully |

## Open Questions

- [ ] Is GitHub Actions enabled for this repo? Check Settings > Actions > General
- [ ] Re-use v0.3.0 or bump to v0.3.1 after fixing?

## References

- otto-rs/otto workflows: `~/repos/otto-rs/otto/.github/workflows/`
- otto v1.1.1 release (reference): 10 assets, 4 tarballs + 4 checksums + 2 source archives
- CCU v0.3.0 release (broken): 2 assets, only auto-generated source archives
- Previous design doc: `docs/design/2026-03-11-tiered-pricing-yesterday-releases.md`
