#!/usr/bin/env bash
# Install the clyde umbrella binary. All tooling is reached through clyde subcommands
# (`clyde report|cost|permit ...`); `clyde bootstrap` repoints the live integrations once
# clyde is on PATH.
set -euo pipefail

cd "$(dirname "$0")"

echo "Installing clyde umbrella..."
# --force so clyde overwrites any pre-merge standalone tools already on PATH during migration.
cargo install --force --path clyde --bin clyde

echo
echo "Installed: clyde"
echo "Next: run 'clyde bootstrap' to migrate config/data and repoint integrations,"
echo "then 'clyde doctor' to verify."
