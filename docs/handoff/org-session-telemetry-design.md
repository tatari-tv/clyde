# Handoff: Claude Code telemetry PRD (and the design it sits on)

**Branch:** `org-session-telemetry-design` (tatari-tv/clyde)
**HEAD:** `3e78754cd7c181ae16e00643eaffb0bd31cb7155`, unchanged from `main`. **Zero commits on this branch, and the branch is not on the remote.** Every work product is an untracked file under `docs/`.
**PR:** none.
**Updated:** 2026-09-24, end of the second PRD session (`e533ae12`). First session was `e0f10237`.

## Next action

Get Scott's answer to the open question below, apply it, republish.

**Open question (asked, unanswered):** a reviewer (Patrick) asked to trim `# Problem being solved` to its opening content, move it to the top, and make its four subsections standalone. Scott rejected the move (Metric to move stays first, per template). The subsections are currently standalone `#` sections (Current Context & Challenges, Problem Research & Evidence, Customer(s), shipped email). That also breaks the template's nesting. Asked Scott: put them back as `###` under Problem being solved? Do not decide this yourself.

Then, in Scott's order not yours:

1. Fill the `TBD` cells in the metadata panel and the two `TBD` targets in Success Metrics.
2. Decide Cursor scope: the spend report covers Cursor, the PRD's non-goals exclude other AI tools, so only the Claude part of the hand-assembled pull can stop. Stated that way in the PRD; Scott has not ruled.
3. Revise the 2026-09-21 design doc: its conversation-text corpus, opt-in upload, minimum-group gating and owner-only views are superseded by the PRD (the Tech Plan row says so). Scott's call how.
4. Make the tech spec trace to the PRD's numbered requirements (now D1-D10, S1-S16, G1-G3), then run `spec:tech-spec-review`.
5. Publish to Confluence when Scott says; space is his call.

## How the PRD is worked now

- **The published marquee post is the working copy.** Every edit to the markdown is republished immediately; the local file and the post must never differ. Scott checks the live page, not the file.
  - PRD: https://marquee.internal.tatari.dev/p/~scott-idler/prd-claude-code-telemetry/
  - API research: https://marquee.internal.tatari.dev/p/~scott-idler/anthropic-reporting-apis-vs-client-side-otel/ (linked from the PRD's Tech Plan row)
- **Render and replace** (the script that did this lives in a session scratchpad and dies with it; this is its body):
  ```bash
  pandoc -s --css prd.css --metadata pagetitle="PRD: Claude Code Telemetry" -f gfm docs/prd/2026-09-23-claude-code-telemetry.md -o <bundle>/index.html
  # put prd.css beside index.html, then (backgrounded, sandbox needs allowed_domains marquee.internal.tatari.dev):
  marquee replace https://marquee.internal.tatari.dev/p/~scott-idler/prd-claude-code-telemetry/ <bundle> --prompt "<verbatim last human turn>"
  ```
  Use `pagetitle`, not `title`: `title` renders a second H1 on top of the markdown's own. `prd.css` is ten lines of page styling; regenerate it if the scratchpad is gone.
- **Verify every republish** with `marquee read <url>` and a grep for the changed text. Say "republished" only with that proof.

## Scott's firm guidance (apply all of it, every edit)

Carried from session one, still in force:

1. **Template order and headings stand** (PRD Template, Confluence `~612d09990f8ff40068b7b15e/pages/2503442440`). Metric to move first. Advocate a change only with citations. The template lives in a personal space; no shared-space template was found.
2. **A PRD is the what and the why.** No mechanisms: no collectors, hooks, daemons, device management, GitHub joins, storage. Where a mechanism is needed, the PRD says "the tech spec chooses". Technical components opens "Context for the tech spec, not a design".
3. **Write for a reader with no context.** Define terms where first used.
4. **Purpose is company-wide trends.** The person page is secondary.
5. **Baseline credits what exists**: the spend report, Anthropic's Analytics API, and (added this session, all cited) Anthropic's admin dashboards, Analytics chat, Smart Reports and Surveys.
6. **Prompts and responses only as "not collected".** No Big Brother language.
7. **No unearned claims.** Every Anthropic claim in the PRD carries a docs link; every number cites its source.
8. **Don't name people in shared docs.**
9. Voice rules (`~/Claude/writing/VOICE.md`).

Decided this session (all reflected in the PRD):

10. **Internal tool, built by Tatari for Tatari.** Never "product" for this tool. Access Control is one line: internal only.
11. **It is the spend report, expanded and engineered.** Same Anthropic billing, pulled automatically, joined with what happens inside sessions.
12. **Per-person numbers are visible to every employee, same view for everyone, leadership included.** No role-only pages. Surveillance fear answered by no conversation text (D2) plus symmetry (S4); HR fear answered by policy (G1). Evidence for this is in the PRD's research section.
13. **No opt-in anywhere.** D9 (per-session data only in laptop logs: PR links, commit ids, interruptions, end reason, working directory and branch, feed-drop totals) is required on every company laptop, P1. Its collection mechanism is the tech spec's call.
14. **clyde is not part of the solution.** It is a Rust CLI, not a harvesting tool, and will not become one. The PRD mentions it only as today's workaround.
15. **Requirements are ordered by priority within each area** (P0, then P1, then P2), renumbered on any insert, every citation updated.
16. **Internal consistency is always on.** Before any republish, the document must agree with itself and with its linked sources. Leftovers from earlier framings (opt-in, owner-only pages, product language, device management as fact) are defects.

My recommendation, not decided: keep time and intensity measures (keyboard active time, session counts per day) off person pages.

## Read first, in this order

1. The live PRD (URL above) and its source, `docs/prd/2026-09-23-claude-code-telemetry.md` (242 lines).
2. The PRD Template page (above), then Scott's approved internal-tool precedent, Product One Pager: Marquee (v2), `PROD/pages/2523103278`.
3. The three strongest recent house PRDs, for register: Agentic MPE `MINT/pages/2682716371`, Beat the Production Affinity Model `MINT/pages/2689925325`, Deterministic Calculations in the Performance Report MCP `ir/pages/2670395488`.
4. `docs/research/2026-09-23-anthropic-analytics-api-vs-otel.md` (published, URL above).
5. `docs/research/2026-09-23-survey-*.md`. Known defect: `survey-fleet-pipelines.md:32` credits Intercom with peer-comparison percentile dashboards and leaderboards; first-party sources support only each engineer's own percentile.
6. marquee `~scott-idler/claude-code-telemetry-pipeline`: OTel field manifest and the "What OTel does not send" table (source for D9's field list and the "25 event kinds, ten gaps, three essential" line). It scopes the session summary to engineers and names clyde; the PRD supersedes both.
7. `docs/design/2026-09-21-org-wide-session-telemetry.md`: earlier design, pending revision (next action 3).

## Blockers, with probes

- **Design doc link in the PRD's Tech Plan row 404s**: the branch is not pushed. Probe: `git ls-remote --heads origin org-session-telemetry-design` (empty output on 2026-09-24). Pushing is Scott's call.
- **Survey docs are not reachable from the published PRD**: they are cited by repo path only. Same cause.

## Slack threads touched

- DM with T, reply on how this differs from Smart Reports: https://tatari.slack.com/archives/D04M1Q59PQ8/p1790207852880879?thread_ts=1790206192.751799&cid=D04M1Q59PQ8
- DM with Patrick, reply that his reorder breaks the template: https://tatari.slack.com/archives/D01GPF7MMT3/p1790261180175249?thread_ts=1790207717.482639&cid=D01GPF7MMT3

## State that does not survive this session

- Session scratchpads (`e0f10237`, `e533ae12`) hold the render bundle, `prd.css`, the republish script, Patrick's screenshot, and fetched Anthropic docs pages.
- Marquee Okta login is a cached token; any `marquee` command may trigger a login. Run them backgrounded.
- Bash sandbox: `marquee` needs `allowed_domains: ["marquee.internal.tatari.dev"]`. The `--context` summarizer (`claude -p`) cannot reach `api.anthropic.com` from the sandbox, so posts carry `--prompt` only.
- API keys (`ANTHROPIC_ENTERPRISE_SPEND_REPORTING_API_KEY`, `ANTHROPIC_API_ADMIN_KEY`) are shell env vars. Never print them.

## Suggested skills

- `marquee:replace` for every PRD edit; `marquee:read` to verify.
- `slack:read` / `slack:write` for reviewer threads; `slack read <permalink> --export <dir>` pulls attached screenshots to disk.
- `spec:tech-spec-review` once the tech spec traces to the PRD.
- `anthropic-usage-report` for further Analytics API pulls.
- `session-recall` for sessions `e0f10237` and `e533ae12` when a detail here is not enough.
