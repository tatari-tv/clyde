#!/usr/bin/env bash
# Install the clyde umbrella binary plus the three compat shims (cr, ccu, claude-permit).
# The shims call their tool's run() in-process and behave identically to the pre-merge tools;
# `clyde bootstrap` does the clean repoint of the live integrations once clyde is on PATH.
set -euo pipefail

cd "$(dirname "$0")"

echo "Installing clyde umbrella + compat shims..."
cargo install --path clyde --bin clyde
cargo install --path report --bin cr
cargo install --path cost --bin ccu
cargo install --path permit --bin claude-permit

echo
echo "Installed: clyde, cr, ccu, claude-permit"
echo "Next: run 'clyde bootstrap' to migrate config/data and repoint integrations,"
echo "then 'clyde doctor' to verify."
