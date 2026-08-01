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
>   negative warns and names what it is overriding.
>
> `clyde session scope --session <id> --clear`
> : drop the override and let the rules decide again.
>
> `clyde session reindex --clear-probe --session <id>`
> : clear a recorded probe result for named sessions, so the next pass re-observes. Use this when a
>   session was refused because of a transient failure that has since been fixed. It does NOT disable
>   the gate: if the cwd still declines conclusively, it re-records on the same pass. There is no
>   catalog-wide form, deliberately.
>
> `clyde doctor`
> : the counts that tell you which of the above you need. See below.

## 4. Reading `clyde doctor`

Add:

> `clyde doctor` now reports the effective `repo-root`, the per-rule resolution counts, and five
> routing counts. They are separate because they have DIFFERENT remedies:
>
> | count | what it means | what to do |
> |---|---|---|
> | probe-refused | a work slug was refused by a recorded conclusive negative | `--clear-probe` if the negative is stale |
> | host-refused | the remote's host is not in `work-remote-hosts` | add the host, or investigate |
> | null `repo_host` | indexed before v13; keeps its old authority | nothing; it resolves on reprobe |
> | overrides | an operator forced the decision | `session scope --list` to read why |
> | anchor/remote disagreement | the directory and the remote disagree | usually a fork, sometimes a misfiled clone |
> | indeterminate probes | git could not answer | check `safe.directory` and that git is installed |
>
> A host where EVERY probe is indeterminate has a `safe.directory` or missing-git problem, not a
> layout problem.

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
