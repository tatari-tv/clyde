---
name: permit
description: Audit and tune Claude Code permission rules from clyde's OWN logged approval history in sessions.db - classify current allow/deny rules by risk and suggest promoting commands you keep approving. Use for "audit my permission rules", "which allow-rules are risky", "what should I promote to always-allow", "why does Claude keep asking to run X". Distinct from a one-off "allow this command" edit - permit reasons from clyde's recorded PreToolUse events over time. Read-only by default; apply/install write to settings files.
user-invocable: true
argument-hint: "<audit | suggest | report | apply | install>"
---

# clyde:permit

`clyde permit` (formerly `claude-permit`) is a PreToolUse hook plus an audit
surface: it logs every permission decision to `sessions.db`, then classifies the
current allow/deny rules by risk and suggests which frequently-approved commands
are safe to promote to always-allow.

## Read-only analysis (safe, run freely)

```bash
clyde permit audit       # classify current permission rules by risk
clyde permit suggest     # suggest promotions based on observed approval patterns
clyde permit report      # session summary of permission activity
clyde permit check       # verify hook installation + DB connectivity
```

- Start with `audit` (what rules exist and how risky) and `suggest` (what to
  promote). `report` summarizes the events the hook has logged.

## Mutating actions (write to settings - confirm first)

**These modify Claude Code settings files; state what will change and get the
user's confirmation before running either.**

```bash
clyde permit apply       # apply audit recommendations to settings files
clyde permit install     # install the PreToolUse hook into Claude Code settings
```

- `apply` edits allow/deny rules in settings based on the audit; `install` wires
  the hook itself. Neither should fire speculatively - the user owns their
  permission posture.

## Plumbing (rarely agent-driven)

- `clyde permit log` reads hook JSON on stdin - this is how the PreToolUse hook
  records an event; you do not call it by hand.
- `clyde permit clean` prunes old events from the database (maintenance).
