#!/usr/bin/env bash
# Install the clyde umbrella binary plus the three compat shims (cr, ccu, claude-permit).
# The shims call their tool's run() in-process and behave identically to the pre-merge tools;
# `clyde bootstrap` does the clean repoint of the live integrations once clyde is on PATH.
set -euo pipefail

cd "$(dirname "$0")"

echo "Installing clyde umbrella + compat shims..."
# --force so the shims overwrite the pre-merge standalone tools (claude-report's cr,
# claude-cost-usage's ccu, claude-permit) that are already on PATH during migration.
cargo install --force --path clyde --bin clyde
cargo install --force --path report --bin cr
cargo install --force --path cost --bin ccu
cargo install --force --path permit --bin claude-permit

echo
echo "Installed: clyde, cr, ccu, claude-permit"
echo "Next: run 'clyde bootstrap' to migrate config/data and repoint integrations,"
echo "then 'clyde doctor' to verify."
