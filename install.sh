#!/usr/bin/env bash
# Install the clyde umbrella binary.
# `clyde bootstrap` migrates config/data and repoints the live integrations once clyde is on PATH.
set -euo pipefail

cd "$(dirname "$0")"

echo "Installing clyde..."
cargo install --force --path clyde --bin clyde

echo
echo "Installed: clyde"
echo "Next: run 'clyde bootstrap' to migrate config/data and repoint integrations,"
echo "then 'clyde doctor' to verify."
