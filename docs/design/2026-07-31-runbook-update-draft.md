# Runbook update draft (register item 10)

**NOT published.** The runbook is a marquee post, so updating it is outward-facing and is Scott's to
send: <https://marquee.internal.tatari.dev/p/~scott-idler/claude-usage-report-pipeline-runbook>

Publish with `marquee:replace` against that URL once the branch is released. This file is the diff to
apply, not a replacement post: it names the sections that change and what they should say.

Everything below is gated on the whole branch shipping. Per the Rollout Plan the phases are
independently COMMITTABLE but not independently RELEASABLE, so none of this is true until the release
lands.

---

## 1. Lift the interim "do not run enrich" guidance

The register's standing instruction is **do not run `clyde session enrich` on v0.22.0**, because a
later reindex could flip a personal session to work and queue its transcript for the work Anthropic
account. That is closed: a git-origin work slug is now refused when a conclusive negative probe
precedes it.

Replace the interim warning with:

> `clyde session enrich` is safe to run again as of <VERSION>. The v0.22.0 defect (a later reindex
> could upgrade a recorded `personal` decision to `work`) is fixed: clyde now records the earlier
> FAILED probe and refuses a work slug that one precedes.
>
> Upgrade first. On v0.22.0 the defect is still live.

`report collect` and `render` never consulted scope, so anyone who kept using the reporting half was
never exposed and needs no action.

## 2. Correct the `~` layout claim (register item 4)

The runbook and PR #82's body both imply Patrick's bare `~` layout was fixed by the git-origin branch.
**It was not.** Rule 1 blocks `$HOME` by design (a git-tracked home would otherwise attribute every
session to the dotfiles repo), so a session run from `~` has no signal that can place it and stays
`personal`.

Replace any "all four layouts now resolve" phrasing with:

> Three of the four measured layouts resolve through the git remote: `~/code/work/<repo>`,
> `~/Projects/<repo>`, and `~/git/tatari/<repo>`. A session run from a bare `~` does NOT, and cannot:
> `$HOME` is a blocked root, so rule 1 refuses to attribute it at all. Those sessions stay `personal`.
> Working from inside a checkout is what fixes them.

The old test that asserted otherwise never bit; it hand-built a catalog row the resolver can never
emit. It has been replaced.

## 3. New operator commands

Add a section:

> **When a routing decision is wrong**
>
> `clyde session scope --list`
> : every session carrying an operator override, with its reason, who set it, and when.
>
> `clyde session scope --session <id> --set work|personal --reason <text>`
> : force a decision. `--reason` is required and stored. Forcing `work` over a recorded conclusive
>   negative warns and names what it is overriding; forcing `personal` on a session that was already
>   enriched warns that the transcript has been sent and the override cannot un-send it.
>
> `clyde session scope --session <id> --clear`
> : drop the override and let the rules decide again.
>
> **Both `--set` and `--clear` take effect on the next ORDINARY `clyde session enrich`.** No `--all`,
> no flags, nothing else to run: the override re-offers the row it applies to, and the next scheduled
> pass picks it up. (Do NOT reach for `--all` -- it re-enriches the whole catalog and clobbers every
> manually-set tag. It was the only workaround before the release that lands these fixes, and it is no longer needed for this.)
>
> One case an override does NOT rescue: a session already enriched, or one that has exhausted its
> retry budget. An override records the correct scope going forward; it does not re-send or re-try.
>
> `clyde session reindex --clear-probe --session <id>`
> : clear a recorded probe result for named sessions, so the next pass re-observes. Use this when a
>   session was refused by a STALE conclusive negative (`NoOrigin` or `NotARepo`) that no longer
>   describes the cwd, typically because the remote was added or the checkout was restored. A
>   transient failure is `Indeterminate` and never records anything, so there is nothing to clear for
>   one. It does NOT disable
>   the gate: if the cwd still declines conclusively, it re-records on the same pass. There is no
>   catalog-wide form, deliberately.
>
> `clyde doctor`
> : the counts that tell you which of the above you need. See below.

## 4. Reading `clyde doctor`

Add:

> `clyde doctor` reports the effective `repo-root`, the per-rule resolution counts, and then routing
> in TWO groups. **The distinction is the whole point of reading it.**
>
> **`routing decisions:`** -- what actually DECIDED each row. These are counts of decisions, not of
> rows carrying a condition: each one is produced by running the real classifier over the catalog, so
> it cannot disagree with the enrich gate. The group SUMS to the catalog row count, because every
> session is decided by exactly one basis. Listed in the classifier's own precedence order, so it
> reads top-down the way a decision is made.
>
> | decision | what decided the row | what to do |
> |---|---|---|
> | override | an operator forced it | `session scope --list` to read why |
> | cwd-anchor | the directory's `repos/<org>` anchor | nothing; this is the ordinary case |
> | git-origin | the remote's slug | nothing; this is the ordinary case |
> | touch-set | the set of repos the session edited | nothing |
> | host-refused | a work slug REFUSED because its host is not in `work-remote-hosts` | add the host, or investigate |
> | probe-refused | a work slug REFUSED by a recorded conclusive negative | `--clear-probe` if the negative is stale |
>
> **`routing conditions:`** -- facts PRESENT on rows, which did NOT decide anything on their own. A
> row can carry a condition while something earlier in the precedence already decided it.
>
> | condition | what it means | what to do |
> |---|---|---|
> | probe-recorded | rows carrying a conclusive negative, decided or not | `--clear-probe` a stale one |
> | host-unknown | indexed before v13; keeps its old authority | nothing; it resolves on reprobe |
> | anchor/remote | the directory and the remote disagree | usually a fork, sometimes a misfiled clone |
> | blocked / outside-root / indeterminate | live re-probe outcomes on this machine right now | see below |
>
> **Why the two groups exist.** Before the release that lands these fixes, `doctor` answered each routing
> line with its own SQL count over a single column and read them all as decisions. They were not. The
> classifier returns at
> the cwd anchor BEFORE it ever reads the probe record or the host, so a row could satisfy
> `repo_probe IS NOT NULL` while that condition decided nothing. On the maintainer's own catalog with
> shipped config, `probe-refused` read 326 and the number of decisions a probe refusal had made was
> **zero**. If you screenshotted or filed a ticket against the old numbers, they were wrong and these
> are not; a refusal count is now a count of DECISIONS.
>
> A host where EVERY live probe is indeterminate has a `safe.directory` or missing-git problem, not a
> layout problem.

## 4b. The export contract: `schema-version` is now 2

Add a section wherever the runbook describes `clyde session export`:

> **`clyde session export` now emits `schema-version: 2`, and `scope` means something different.**
>
> Through v1, `scope` was computed from the session's working directory alone. That rule ignores
> everything clyde later learned to read: an operator's override, the git remote the session's own
> checkout points at, and the set of repos the session actually edited. So a session clyde had DECIDED
> was work could still export as `personal`, and an operator who ran `session scope --set work` got
> the opposite of what they asked for on the wire. On the maintainer's catalog, 31 rows were already
> exporting a scope that contradicted the catalog.
>
> In v2, `scope` is the decision that was actually made, first match wins:
>
>     operator override  ->  the scope the routing gate recorded  ->  the working-directory rule
>
> The field's type and vocabulary are unchanged (`"work" | "personal"`, never null). Only its MEANING
> changed, and only for rows where the directory rule disagreed with the real decision.
>
> **Any consumer pinned to `schema-version: 1` must be updated before this release reaches it.** A
> consumer that hard-fails on an unrecognized version will break at the release, by design -- that is
> what the bump is for. One that tolerates the version keeps working, but keeps consuming the old
> (wrong) scope semantics until it is updated. If you store or key off `scope`, expect some values to
> differ from what the same session exported under v1: the session did not change, the earlier answer
> was wrong.
>
> Full contract: `docs/session-export-contract.md`, section "What changed in v2".

## 5. The new config key

Add to the config section:

> `work-remote-hosts` (default `[github.com]`) is the list of hosts a git remote may confer WORK scope
> from. An SSH `Host` alias is resolved against it rather than compared literally, so
> `git@github-work:tatari-tv/x` still works. A remote on any other host still ATTRIBUTES a repo; it
> just cannot make the transcript shippable.

## 6. What did NOT change

Worth stating, because the register's readers were told to expect a coverage improvement:

> This release NARROWS the routing gate. On the maintainer's catalog it changed zero rows' decisions,
> which is the intended result: the populations it can strip (a remote on a non-allowlisted host, a
> session whose cwd was once observed to have no origin) do not exist there. Coverage from v0.22.0's
> layout-independence win is preserved intact and was measured to confirm it.
