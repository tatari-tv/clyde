# permit

Logs every Claude Code tool invocation (via the PreToolUse hook), classifies permission rules by
risk tier, and produces recommendations to tighten your permission posture. A member crate of the
[`clyde`](../README.md) umbrella workspace.

- Umbrella: `clyde permit <log|audit|suggest|report|clean|check|install|apply>`
- Compat shim: `claude-permit ...` (behavior-exact with the pre-merge tool, including the
  hook-safe `{}`-on-failure contract)

Library API: `permit::{PermitArgs, PermitCli, run}`. See the top-level README and
`docs/design/2026-06-24-clyde-umbrella-cli.md` for the umbrella architecture.
