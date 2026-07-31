# Handoff: three defects the v0.18.0 runbook thread surfaced

**Author:** Scott Idler
**Date:** 2026-07-30
**Status:** Handoff brief. Findings are diagnosed to the file and line. Do not re-derive.
**Audience:** the next agent picking up `tatari-tv/clyde`

These three came out of the `#platform-internal` thread on the v0.18.0 runbook re-run
([permalink](https://tatari.slack.com/archives/C039YLDJW5T/p1785432360721679), 21 replies), not out of
any design doc. They were deliberately NOT fixed while executing
`docs/design/2026-07-30-excise-api-key-followups.md`: none of them were in it, and unrequested scope is
illegitimate regardless of quality. That doc's implementation notes record them; this brief is the
detail behind them.

**Finding A blocks the excision's AC6**, the last open acceptance criterion of
`docs/design/2026-07-29-excise-api-key.md`. Findings B and C are independent.

Every item carries the evidence that established it, plus what was ruled out. Two hypotheses from the
thread are corrected below, and one of mine is a null result. Do not re-measure what is measured.

## What the thread actually established (the good news first)

**Keyless works, confirmed by a teammate with no key.** Patrick Shelby, on v0.18.0: "ran clean for me,
keyless confirmed. `report render` worked with no `ANTHROPIC_API_KEY` in env." 131 sessions reported
for July. (Billing total redacted: another operator's Anthropic spend, in a committed doc.) That is
the half of AC6 the excision was about, and it passes. Keegan Ferrando re-ran it too
and commented on his reports' content.

What neither of them produced is the **enrichment percentage**, and Finding A is why.

## 1. `scope` classifies off `cwd` alone, so enrich is a no-op for anyone not launching from a repo checkout

**Status:** mechanism confirmed, measured on desk.lan, and it is the AC6 blocker. **Not a bug in the
routing invariant. A blast radius nobody priced.**

### The mechanism, pinned

`sessions/src/enrich.rs:105` re-derives scope at send time from the session's `cwd` and **nothing
else**:

```rust
let scope = session::classify(rec.cwd.as_deref().map(std::path::Path::new));
if !scope.is_work() { /* record skipped-personal, continue */ }
```

`session::classify` (`session/src/scope.rs:59-76`) is `Work` iff the path has a `repos/<work-org>`
adjacency, matched in the org slot only. Everything else, including a missing `cwd`, is `Personal`.

**This is deliberate and the module says so** (`session/src/scope.rs:18-20`):

> The cost is that a genuine work session run outside a `~/repos/tatari-tv/` path is classified
> personal and skipped (un-enriched), which is the acceptable failure direction (never the reverse).

So do not open this as "scope is broken". The fail-safe is correct: no personal content may reach the
work account. What was never priced is what that costs a user whose working style is `cwd`-hostile:
**0% enrichment coverage, and `report collect`'s Executive Summary, What This Funded, and Conclusion
render EMPTY.**

### The asymmetry that is the actual lead

`common/src/repo.rs:11-21` already attributes a repo by **four** strategies, in precedence order:

| rank | source | how |
|---|---|---|
| 0 | `git-origin` | `git remote get-url origin` in the cwd |
| 1 | `known-path` | longest-prefix hit in the learned path map |
| 2 | `files-touched` | unique argmax over repos the session edited files in |
| 3 | `path-guess` | cwd matches `<repo-root>/<org>/<repo>` (labeled as a guess) |

`scope` consults **none** of them. The catalog can know a session edited `tatari-tv/philo` and still
classify it personal, because `classify()` takes only `cwd`.

### Measured on desk.lan

```sql
-- sessions whose ATTRIBUTED repo is a work repo but whose scope is personal:
select count(*) from sessions where repo like 'tatari-tv/%' and scope='personal';   -- 21
-- all 21 were attributed by files-touched; zero by git-origin
select count(*) from sessions where scope='work';                                   -- 800
```

So even on the machine with the most `~/repos/tatari-tv/`-native workflow, 21 sessions are permanently
unenrichable despite provably editing work repos. On Patrick's machine it is **0 of 131**: he runs
`claude` from `~`, so every session comes back `repo: null` and gates `skipped-personal`. Keegan is
affected differently and, in his words, worse: he works from a local-git-only project folder and
spiders out, so his repo values are "less `null` and more just wrong".

### Why this needs a design doc, not a patch

Widening the gate is a **security change**, which is exactly why it is not a targeted fix. Today the
invariant is "the cwd is under a work org, therefore this content is work content." Trusting
`files-touched` instead means: *a session run in `$HOME` that touched one work file becomes eligible to
send its whole body to the work account*, and that body may include everything else the user did in
that session. The routing invariant has to be restated deliberately before any code moves. Questions
the doc has to answer:

- Which `RepoSource` ranks are trustworthy enough to confer work scope? `git-origin` is a strong claim;
  `path-guess` is explicitly a guess and probably must not.
- Does a mixed session (touched both a work repo and a personal one) become work, personal, or
  excluded? The current answer is "personal", and that may still be the right one.
- Should the remedy be classification at all, or a redaction/segmentation change so a mixed session can
  be sent safely?
- What tells the user their coverage is 0%? `report collect`'s `min-enrichment` warning fires, but
  Patrick read empty report sections rather than the warning.

### Fastest repro (no code, mutates nothing)

```bash
cp ~/.local/share/clyde/sessions.db "$TMPDIR/diag.db"
# any session whose repo is a work repo but whose scope is personal:
sqlite3 "$TMPDIR/diag.db" \
  "select session_id, repo, repo_source, cwd from sessions
   where repo like 'tatari-tv/%' and scope='personal' limit 5;"
clyde session enrich --db "$TMPDIR/diag.db" --dry-run <one-of-those-ids>   # -> skipped-personal
```

## 2. Dormancy reads the transcript's filesystem mtime, not session activity time

**Status:** mechanism confirmed. The thread's stated hypothesis is **wrong**; the effect it described is
real.

### The mechanism, pinned

`sessions/src/db.rs::enrich_candidates` filters `r.modified <= cutoff` in Rust, and `modified` is set in
`session/src/parse.rs:352-354`:

```rust
if let Some(mtime) = file_mtime(&file.path) {
    self.modified = Some(self.modified.map_or(mtime, |cur| cur.max(mtime)));
}
```

That is the **max filesystem mtime** across every file belonging to the session (parent plus subagent
files). Nothing inside the JSONL contributes to it. `sessions/src/db/query.rs:279` says as much:
`modified` "IS the transcript mtime, finding D1".

### Correcting the thread's hypothesis

Patrick guessed, and explicitly labelled it speculation, that "dormancy is computed off a timestamp that
`reindex` refreshes." **`reindex` does not write transcripts; it reads their mtimes.** The real exposure
is broader and worse: *anything* that rewrites a file under `~/.claude/projects/` resets dormancy for
that session. A file sync (Syncthing, Dropbox), a restore from backup, a `cp -r` onto a new machine, or
any tool that rewrites a JSONL in place will do it. That is a much larger surface than one clyde
subcommand, and it means dormancy is not a property of the session at all.

### The observation, both machines

Patrick, sessions dated July 1 through 30, run on July 30:

```
clyde session enrich --dry-run                      # considered: 0
clyde session enrich --dormant-after 1h --dry-run   # considered: 44
```

His July 1 sessions are ~29 days idle and must qualify at 7d. Every one of his mtimes is inside 7 days,
so something rewrote all of them.

desk.lan, for contrast, which is why this was never noticed here:

| age by `modified` | sessions |
|---|---|
| < 1 day | 29 |
| 1-7 days | 221 |
| 7-30 days | 1207 |
| > 30 days | 636 |

1,843 rows sit past the 7d cutoff, so sweeps here always find candidates. **This defect is invisible on
the maintainer's machine**, which is the reason it needs a test rather than a fix-and-eyeball.

### The fix direction is nearly free

The per-message timestamps are **already parsed**. `session/src/parse.rs:387-388` takes the MIN into
`created`:

```rust
if let Some(dt) = v.get("timestamp").and_then(Value::as_str).and_then(parse_ts) {
    self.created = Some(self.created.map_or(dt, |cur| cur.min(dt)));
}
```

A MAX of the same field is the session's true last-activity time, immune to file touches. That is a
schema addition (a new column plus a migration) and a change to what `enrich_candidates` filters on, so
it wants a doc, but the data needs no new parsing.

Two things to settle in that doc:

- **What `modified` is still legitimately for.** It is load-bearing elsewhere: the grown-since-last-
  enrichment probe compares `s.modified > s.enriched_modified` (`sessions/src/db.rs`), and export's
  `duration` is computed as mtime minus earliest ts (`query.rs:279`). Do not repurpose it; add
  alongside it.
- **Backfill.** Existing rows have no last-activity value, and a migration that leaves it NULL must
  decide whether NULL means "not dormant" (safe, matches `query.rs:307-308`'s existing choice) or
  triggers a re-parse.

### Downstream effect worth naming

With the runbook's `enrich` then `collect` order, the last 7 days of a month-to-date report can never be
enriched, because a session still inside the dormancy window is skipped and `collect` runs immediately
after. Patrick also noted `collect` needs a re-`reindex` if sessions ran while he worked through the
steps.

## 3. Cost reads ~30% under the `claude.ai` web UI

**Status:** reported, NOT diagnosed. One hypothesis refuted below. Treat the rest as leads.

### What was reported

Keegan: `ccu` used to land within 5-10% of the web UI (always low), but after the recent fixes "all the
tooling you've made is usually like at least 30% lower than the web UI shows". Scott, in-thread: "maybe
my accounting is off then ... its been very difficult to nail down. maybe there is more work to be
done." Patrick separately could not reconcile his own July total against the web UI.

### Refuted: it is not missing model pricing

The obvious first guess was that newer models are absent from the pricing feed, so their spend is
dropped. **Checked and false on desk.lan.** Every model present in the catalog has a pricing entry in
`pricing/data/pricing.json`:

```
claude-opus-4-7  claude-opus-4-8  claude-sonnet-5  claude-sonnet-4-6
claude-fable-5   claude-haiku-4-5  claude-opus-5
```

A null result, recorded so nobody spends the hour again. It does not clear the code path below.

### Real latent risk found while checking

`common/src/metrics.rs:138`:

```rust
pricing.calculate_usd(model, usage).ok()
```

An unpriceable model becomes `None` and **drops out of the sum**. It does `warn!` (the comment above it
says `claude-pricing` logs its own), so it is not silent in the log, but the total is quietly low rather
than loud. That is a fail-open default in cost math. It is not the cause of the current gap on this
machine (nothing is unpriced here), but on a machine using a model the feed lacks it would look exactly
like a ~30% undercount. `cost/src/oracle.rs:326` handles the same call with a `match` and is worth
comparing.

### Leads, none verified

- **Confirm the comparison is apples to apples first.** The web UI's usage page may include usage from
  claude.ai chats and the desktop app, not just Claude Code. If so, a gap is EXPECTED and there is no
  bug. Settle this before touching any math: it decides whether the rest of this section matters.
- Cache-token accounting: `pricing/src/pricing.rs:165` sums `input + cache_5m_write + cache_1h_write +
  cache_read`, with separate above-200k tiers. The 5m/1h write split and the tier boundary are the
  fiddliest part and the likeliest place for a systematic skew.
- Subagent/sidechain entries: confirm they are counted once, not zero times and not twice.
- `<synthetic>` appears as a model on 8 rows here; check it is excluded deliberately rather than
  accidentally priced or accidentally dropped.

## Suggested route

- **Finding A: `/create-design-doc`.** It is a change to the routing invariant that keeps personal
  content off the work account, so it gets the full funnel and a security section. It also unblocks the
  excision's AC6, so name that dependency in the doc.
- **Finding B: `/create-design-doc`.** Small in code, but it is a schema addition plus a migration, and
  the correctness bar is a regression test that fails on a machine where every mtime is fresh (that is
  the condition this repo's own maintainer machine cannot reproduce, per the table above).
- **Finding C: probe before designing.** The first question is whether the web UI is even the right
  yardstick. That is a measurement, not a design. If it turns out to be a real undercount, then a doc.
- `/review-panel` in Implementation Audit mode on
  `docs/design/2026-07-30-excise-api-key-followups.md`, which is now `Status: Implemented` and has not
  been audited.

## State of the work this came out of

`docs/design/2026-07-30-excise-api-key-followups.md` is `Status: Implemented`, all seven acceptance
criteria verified. As of this writing the commits are **local and unpushed**, on two branches, pending
Scott's approval at the finalization checkpoint:

- `tatari-tv/clyde`, branch `excise-api-key-followups`, 8 commits. Needs `bump --no-tag` to 0.19.0, a
  PR (main is gated: classic protection plus an org `workflows` ruleset admin does not bypass), and a
  post-merge `bump --tag-only`.
- `scottidler/claude`, branch `execute-criteria-and-ban-em-dashes`, 1 commit (the acceptance-criteria
  execution gate and the em-dash rule amendment). That repo's `main` is ungated.

If those have landed by the time you read this, ignore this section.

## A note on where this file lives

In the repo, not the OS temp dir, following
`docs/design/2026-07-30-excise-api-key-followups-handoff.md` and
`docs/design/2026-07-28-release-arc-handoff.md`. The content is a durable follow-up list rather than
conversation state, and a scratchpad path has been garbage-collected mid-session on this project
before.
