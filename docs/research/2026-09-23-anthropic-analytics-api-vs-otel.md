# Anthropic reporting APIs vs client-side OTel: what we can get today

**Author:** Scott Idler
**Date:** 2026-09-23
**Method:** live probes against `api.anthropic.com` with Tatari's Claude Enterprise Analytics key and Admin API key, 2026-09-23 19:00-20:30 UTC. Every field below was observed on the wire, not read from docs. Raw responses (emails masked) in the session scratchpad `anthropic-api-probe/`. The OTel side is the 2.1.280 console-exporter probe from 2026-09-22, catalogued in the marquee post `~scott-idler/claude-code-telemetry-pipeline`.

## TL;DR

- Anthropic already gives us **org-level skill, plugin and connector adoption**, per user, per product, per day, with invocation counts and attributed spend. This is where Ryan Colley's spend report gets its "Skill & connector adoption" section. His note that invocation counts are not reported for our org is stale: `invocation_count` is populated on every one of the 709 skill rows returned for the last 30 days.
- Two keys, two APIs, seven Analytics endpoints and three Admin endpoints answered. The Admin API's Claude Code report covers **API-key actors only** (two rows per day for us), not Enterprise seat users. Everything about our 145 monthly Claude Code seat users comes from the Analytics API.
- What the APIs do **not** have is anything inside a session: no tool mix, no interrupts or stalls, no hook or MCP health, no per-request latency, no repo or branch, no PR or commit identity, no cache efficiency per request, no session length, no timing at all below one day. That is exactly the OTel plus envelope territory, and it does not overlap.
- Recommendation for the design: pull the Analytics API nightly into the same store as billing and adoption truth (it already is, via Ryan's report), and scope OTel plus the envelope to session behavior. Do not rebuild skill adoption counts from OTel; reconcile against the API instead.

## 1. The two APIs

| | Claude Enterprise Analytics API | Admin API |
|---|---|---|
| Base | `/v1/organizations/analytics/*` | `/v1/organizations/*` |
| Key | Analytics key, primary owner creates it in claude.ai org settings. Env var `ANTHROPIC_ENTERPRISE_SPEND_REPORTING_API_KEY` | Admin key `sk-ant-admin...`. Env var `ANTHROPIC_API_ADMIN_KEY` |
| Covers | Claude Enterprise seats: Claude Code, Cowork, chat, Claude Design, Office agents, Claude in Chrome, claude-tag (Slack), research, voice | Claude Platform (developer API): workspaces, API keys, and Claude Code sessions authenticated with an API key |
| Identity | `actor.email`, `actor.name`, `actor.user_id` on per-user endpoints | `api_key_id`, `workspace_id`; Claude Code rows carry `api_key_name` |
| Array params | `group_by[]=x&group_by[]=y`. **A repeated bare `group_by=` is silently ignored**: the call returns 200 with the dimension absent. Cost me one probe round | Same syntax, but it returns 400 with a hint instead of ignoring |
| Rate limit | 60 req/min per org, shared across endpoints | separate |
| Freshness | `data_refreshed_at` on every response; 2026-09-23T19:18Z at probe time. Docs say up to 24h lag, revisions up to 30 days | same class |

Auth headers on both: `x-api-key`, `anthropic-version: 2023-06-01`.

## 2. Analytics API: every endpoint, every field

Time params differ by endpoint, and the API tells you which it wants: the four report endpoints take `starting_at`/`ending_at` (ISO timestamps); `summaries`, `users`, `skills`, `connectors`, `plugins` take `starting_date`/`ending_date` (YYYY-MM-DD) or a single `date`.

### 2.1 `usage_report` (org, time-bucketed) and `user_usage_report` (per user, flat)

Bucketed response: `data[]` is `{starting_at, ending_at, results[]}`; per-user response: `data[]` is one flat row per user with `actor{type=user_actor, user_id, name, email, deleted}`. `bucket_width` 1m|1h|1d, `limit` capped at 31 for 1d.

Valid `group_by[]` (from the API's own 400 text): `product`, `model`, `context_window`, `inference_geo`, `speed`, `slack_channel_id`, `slack_conversation_type`, `teams_channel_id`, `claude_tag_category`, `claude_tag_user_id`, `rbac_group_id`, `claude_project_id`, `cost_type`, `token_type`. Every one of these appears as a key on every row, null unless grouped.

| Field | Type | Observed values (Tatari, last 30 days) |
|---|---|---|
| `uncached_input_tokens` | int | 5.3M to 32M per day org-wide |
| `cache_creation.ephemeral_5m_input_tokens` | int | the 5m/1h split OTel does not send |
| `cache_creation.ephemeral_1h_input_tokens` | int | |
| `cache_read_input_tokens` | int | 2.0B to 12B per day |
| `output_tokens` | int | |
| `server_tool_use.web_search_requests` | int | |
| `requests` | int | 9.8K to 76K per day |
| `total_tokens` | int | per-user endpoint only |
| `product` | str | `chat`, `claude_code`, `cowork`, `claude-tag`, `claude_design`, `research`, `office_agent`, `claude_in_chrome`, `claude_in_xcode`, `voice_mode` |
| `model` | str | `claude-sonnet-5`, `claude-haiku-4-5-20251001`, `claude-opus-5`, `claude-fable-5`, `claude-opus-4-8`, `claude-fable-5-1`, `claude-sonnet-4-6`, `claude-opus-4-7`, `claude-opus-4-6`, `no-model` |
| `context_window` | str | `0-200k`, `200k-1M` |
| `inference_geo` | str | `global` only, on Enterprise |
| `speed` | str | `standard` only observed |
| `rbac_group_id` | str | 3 groups configured |
| `claude_tag_category` | str | `engaged`, `monitoring`, `dm`, `proactive`, `scheduled` (Claude in Slack) |
| `slack_channel_id` | str | 15 channels in 3 days |
| `claude_tag_user_id` | str | Slack user ids, 187 distinct |

### 2.2 `cost_report` (org, bucketed) and `user_cost_report` (per user, flat)

Same dimensions as usage. Amounts are **decimal strings in cents**: `"160803.863735"` is $1,608.04.

| Field | Type | Notes |
|---|---|---|
| `amount` | str (cents) | what we pay |
| `list_amount` | str (cents) | list price before any discount |
| `currency` | str | `USD` |
| `requests` | int | |
| `cost_type` | str | `tokens`, `web_search`, `code_execution` when grouped |
| `token_type` | str | one of the five token fields when grouped |

The `description` dimension the Admin API has (`"Claude Sonnet 5 - Input Tokens, Cache Hit - US Inference"`) does **not** exist on the Analytics API.

### 2.3 `summaries` (org, daily)

`summaries[]` rows, one per day. No pagination, no group_by.

| Field | Observed 2026-09-22 |
|---|---|
| `assigned_seat_count` | 354 |
| `pending_invite_count` | 0 |
| `daily/weekly/monthly_active_user_count` | 242 / 303 / 335 |
| `claude_code_daily/weekly/monthly_active_user_count` | 100 / 125 / 142 |
| `cowork_*_active_user_count` | 114 / 183 / 222 |
| `chat_*_active_user_count` | 85 / 158 / 258 |
| `claude_design_*_active_user_count` | 12 / 34 / 70 |
| `office_agent_*_active_user_count` | 2 / 3 / 7 |
| `science_*_active_user_count`, `science_entitled_user_count` | 0 |
| `daily/weekly/monthly_adoption_rate` | 68.17 / 85.35 / 94.93 (percent of seats) |

This is the denominator for the PRD's coverage metric: 142 monthly Claude Code users.

### 2.4 `users` (per user, per day or per range)

One row per seat user with `user{type, id, email_address}`, `last_activity_date`, `web_search_count`, and per-surface metric blocks. Only `group_by[]=rbac_group_id` is supported here, which adds `rbac_group_id` and `rbac_group_name`.

`claude_code_metrics` (the block that matters for us):

| Field | Type |
|---|---|
| `core_metrics.distinct_session_count` | int |
| `core_metrics.commit_count` | int |
| `core_metrics.pull_request_count` | int |
| `core_metrics.lines_of_code.added_count` / `removed_count` | int |
| `core_metrics.artifacts_created_count` | int |
| `tool_actions.{edit_tool,multi_edit_tool,write_tool,notebook_edit_tool}.accepted_count` / `rejected_count` | int |

Other blocks per user: `chat_metrics` (conversations, messages, thinking messages, projects, artifacts, skills used, connectors used, files uploaded, shared views), `cowork_metrics` (sessions, actions, file edits per tool, dispatch turns, messages, skills, connectors, plugins, artifacts), `office_metrics.{excel,powerpoint,word,outlook}`, `design_metrics`, `science_metrics`.

### 2.5 `skills` (per skill, per range or day)

This is the one that changes the PRD's Current Context. One row per skill name across all surfaces, paginated (`next_page`). `group_by[]` supports `product`, `rbac_group_id`, **`user_id`**, so per-user-per-skill is available. Filter `product=claude_code` works.

| Field | Type | Notes |
|---|---|---|
| `skill_name` | str | the slash name: `commit`, `code-review`, `artifact-design`, `triage` |
| `skill_display_name` | str or null | null for ours |
| `distinct_user_count` | int | |
| `invocation_count` | int | **populated on all 709 rows** |
| `enable_count` | int or null | 0 or null observed |
| `share_status` | str or null | `public` on Anthropic's bundled skills (`xlsx`, `docx`, `pptx`, `morning`, `file-reading`) |
| `estimated_overage_spend` | str (cents) | spend attributed to sessions where the skill was used |
| `attributed_list_price` | str (cents) | |
| `claude_code_metrics.distinct_session_skill_used_count` | int | sessions in Claude Code |
| `cowork_metrics.distinct_session_skill_used_count` | int | |
| `chat_metrics.distinct_conversation_skill_used_count` | int | |
| `office_metrics.{excel,powerpoint,word,outlook}.distinct_session_skill_used_count` | int | |
| `user_id` | str | only with `group_by[]=user_id` |
| `product` | str | only with `group_by[]=product` |

Tatari, 2026-08-23 to 2026-09-22: **709 distinct skill names, 454 used in at least one Claude Code session, 8,717 invocations.** Top Claude Code skills by sessions: `triage` 455 (2 users), `conduct` 293 (3), `jira-management` 261 (39), `commit` 218 (31), `code-review` 196 (44), `artifact-design` 186 (109 users across surfaces), `pr` 168 (28), `review-pr` 101 (11), `release` 92 (3), `worktree-management` 89 (3), `thoughts-sync` 85 (24), `publish` 84 (18), `handoff` 63 (5), `plan` 63 (12), `promote` 59 (18).

What it cannot tell you: whether an invocation completed or was abandoned, how long it ran, what it cost per invocation (spend is per skill per period), which repo, or what happened next.

### 2.6 `plugins` (per plugin)

| Field | Type | Notes |
|---|---|---|
| `plugin_name` | str | `anthropic-skills`, `conductor`, `pr`, `marquee`, `superpowers`, `coderabbit`, `platform`, `spec`, `slack` ... |
| `plugin_id` | str or null | `figma@knowledge-work-plugins` style; null for ours |
| `distinct_user_count` | int | |
| `invocation_count` | int | |
| `install_count` | int | 0 to 4 observed; mostly 0, so not a reliable install denominator |
| `claude_code_metrics.distinct_session_plugin_used_count` | int or null | null on the 30-day range, populated on single-day queries |
| `cowork_metrics.distinct_session_plugin_used_count` | int or null | same |

62 plugins in 30 days. `anthropic-skills` 138 users / 1,583 invocations, `conductor` 44 / 1,328, `pr` 9 / 251, `marquee` 20 / 152, `superpowers` 14 / 79, `coderabbit` 8 / 77, `platform` 10 / 70, `spec` 8 / 30.

### 2.7 `connectors` (per MCP connector)

| Field | Type | Notes |
|---|---|---|
| `connector_name` | str | a UUID |
| `connector_display_name` | str or null | `Slack`, `Gmail`, `Google Drive`, `Atlassian`, `Granola`, `HubSpot`, `Apollo.io`, `n8n`, `Pendo`; null on about a third of rows |
| `distinct_user_count` | int | |
| `read_call_count` / `write_call_count` / `unclassified_call_count` | int | |
| `managed_auth_distinct_user_count` / `individual_auth_distinct_user_count` | int or null | |
| `claude_code_metrics.distinct_session_connector_used_count` | int | 26 of 76 connectors have Claude Code sessions |
| `cowork_metrics.distinct_session_connector_used_count`, `chat_metrics.distinct_conversation_connector_used_count`, `office_metrics.*` | int | |

Connectors here are claude.ai-registered MCP connectors. Our laptop-configured MCP servers (clyde, persona, marquee, slack, oracle) do not appear by name; they show up as the null-display-name rows at best.

## 3. Admin API (Claude Platform, not seats)

- `GET /v1/organizations/usage_report/messages`: per API key / workspace / model / service_tier / context_window token buckets. Same five token fields plus `server_tool_use.web_search_requests`. Values we saw: `service_tier` `standard`|`batch`, `context_window` `0-200k`|`200k-1M`, models down to `claude-sonnet-4-5-20250929`.
- `GET /v1/organizations/cost_report`: `amount` in cents-as-string, `description` (`"Claude Opus 4.8 - Input Tokens - Batch - US Inference"`), `cost_type` `tokens`|`web_search`, `inference_geo` `us`|`not_available`.
- `GET /v1/organizations/usage_report/claude_code?starting_at=YYYY-MM-DD`: per actor per day. Fields: `actor{type, api_key_name}`, `terminal_type`, `customer_type`, `subscription_type`, `is_remote`, `core_metrics{num_sessions, lines_of_code{added,removed}, commits_by_claude_code, pull_requests_by_claude_code}`, `tool_actions{edit,multi_edit,write,notebook_edit}{accepted,rejected}`, `model_breakdown[]{model, tokens{input,output,cache_read,cache_creation}, estimated_cost{currency, amount}}`. **For Tatari this returns `api_actor` rows only, two per day** (service API keys running Claude Code). Enterprise seat users are not in it.
- `GET /v1/organizations/users`, `/workspaces`, `/me`: directory. Workspaces show `inference_data_retention.type = disabled` on all five sampled.

## 4. Side by side: what answers which question

Legend: **API** = Analytics API as of today. **OTel** = Claude Code's exporter with our env block (25 metadata events, 8 metrics, 6 span types). **Env** = the per-session envelope clyde would emit from the transcript.

| Question | API | OTel | Env |
|---|---|---|---|
| Active users, seats, adoption rate by surface, daily | **yes** (`summaries`) | derive from `session.count` | no |
| Tokens and cost per user per model per product, with 5m/1h cache split | **yes**, billing truth | approximate (`cost_usd` is TTL-aware but cache split is summed) | cache split yes |
| Skill invocations and distinct users per skill, per user, per product | **yes** (`skills`, `group_by[]=user_id`) | yes (`skill_activated`, needs `OTEL_LOG_TOOL_DETAILS` for names) | yes (`attributionSkill`) |
| Plugin usage and distinct users | **yes** (`plugins`) | yes, plus `plugin_loaded` = installed denominator | partial |
| Installed but never fires | no (`install_count` unreliable) | **yes** (`plugin_loaded` vs `skill_activated`) | no |
| Skill invoked vs abandoned mid-flight | no | partial (`tool_result` after `Skill` use) | **yes** |
| Connector / MCP usage | yes for claude.ai connectors; laptop MCP servers unnamed | **yes** (`mcp_server_connection`, `mcp_server.name`, `mcp_tool.name`) | yes |
| MCP connection failures, hook failures, hook inventory | no | **yes** | no |
| Tool mix, tool success rate, tool duration | edit/write accept and reject counts per user per day only | **yes** per call (`tool_result`, `tool_decision`) | yes aggregated |
| Permission decisions and their source (config, hook, user) | accept/reject counts for edit tools only | **yes** | partial |
| Interrupts (Esc mid-turn) | no | no | **yes** (only source) |
| Session end reason, session length | no (session counts only) | no end event; length inferred | **yes** |
| Where sessions stall: retries, refusals, API errors, latency, TTFT | no | **yes** (`api_error`, `api_refusal`, `api_retries_exhausted`, `ttft_ms`) | partial |
| Compactions | no | yes (`compaction`) | yes |
| Repo, owner, branch, cwd | no | repo yes (`vcs.*` opt-in); branch only on commits with details | **yes** |
| Commit SHAs and PR URLs, cross-session dedup | counts only (`commit_count`, `pull_request_count` per user per day) | SHA with details; PR count only | **yes** (only source of PR URLs) |
| Cost per merged PR | no | no | **yes** with GitHub join |
| Model, effort, speed per request | model per user per day | **yes** per request | yes |
| Managed-settings health (who runs the policy we shipped) | no | **yes** (`managed_settings_resolved`) | no |
| Claude Code version, entrypoint (cli, vscode), terminal, OS | `terminal_type` on the Admin report only | **yes** | version yes |
| Keyboard active time | no | **yes** (`active_time.total`) | no |
| History before we switch it on | **yes**, back to 2026-01-01 | no | 30 days of laptop transcripts |
| Granularity | one day, one user, one skill | per event, per request, millisecond | per session |
| Delivery guarantees | Anthropic's | in-memory queue, loses tail on exit | durable (built from disk) |
| Content (prompts, responses, tool output) | none | off by policy | none |
| Cowork, chat, Claude Design, Office, Slack | **yes** | no | no |

## 5. What this changes

- **Goal 1 (curate tatari-skills by usage) is answerable today from the API.** Invocations and distinct users per skill, per user, per product, per day, back to January. The design should pull `skills`, `plugins`, `connectors`, `users` and `summaries` nightly into the store next to `billing/`, and the ring 0 review should start from that data now, not after OTel ships. OTel adds the abandonment signal and the installed-but-idle denominator on top.
- **Goals 2 and 4 (where sessions die, cost per PR) are still OTel plus envelope.** Nothing in either Anthropic API sees inside a session.
- **Coverage metric denominator:** `summaries.claude_code_monthly_active_user_count` (142 today), not the spend report's 348 all-product active users.
- **Reconciliation:** OTel `skill_activated` counts per skill per day should match `skills.invocation_count` per day within the exporter's drop rate. That is a free correctness check on the pipeline, and a measurement of exporter loss without waiting for the envelope.
- **Ryan's report:** the skill table should switch from sessions to `invocation_count` and can add per-user-per-skill via `group_by[]=user_id`. Connector rows with null display names are laptop MCP servers or unregistered connectors; that is a data-quality gap on Anthropic's side.
- **PRD Current Context** needs one more correction: "no per-invocation skill counts" is false. The accurate line is "per-day skill invocation counts and distinct users exist; nothing below one day or inside a session does."

## 6. Gotchas, all hit during the probe

- `group_by[]` with brackets, always. The Analytics API returns 200 and ignores a bare repeated `group_by=`; the Admin API returns 400 with a hint.
- Report endpoints take `starting_at`; `summaries`, `users`, `skills`, `connectors`, `plugins` take `starting_date`/`ending_date` or `date`. Wrong one is a 400 that names the right one.
- `limit` on `usage_report`/`cost_report` is capped at the bucket count (31 for `1d`). Per-user and per-skill endpoints paginate with `next_page` and accept 100+.
- Amounts are strings in cents everywhere, including `estimated_overage_spend` on skills.
- `plugins.*_metrics.distinct_session_plugin_used_count` is null on multi-day ranges and populated on a single `date`. Query per day and sum.
- `connector_name` is a UUID; `connector_display_name` is the human name and is null for roughly a third of rows.
- `inference_geo` is `global` for everything on Enterprise; `speed` is `standard` only. Neither is a useful dimension for us yet.
- The Admin `usage_report/claude_code` endpoint is API-key actors only. Do not build seat-user dashboards on it.
- 60 requests per minute per org, shared. A full nightly pull (five endpoints, 30 days, pagination) fits in a few minutes with a 1s sleep.

## 7. Probe inventory

Analytics: `usage_report`, `user_usage_report`, `cost_report`, `user_cost_report`, `summaries`, `users`, `skills`, `connectors`, `plugins`: all 200. Tried and 404: `skills_report`, `skill_usage_report`, `connectors_report`, `adoption_report`, `integrations_report`, `user_activity_report`, `engagement_report`, `sessions_report`, `users_report`, `projects_report`, `skill_usage`, `connector_usage`, `adoption`, `integrations`, `feature_usage`, `features`, `tools`, `mcp`, `skills_usage_report`, `connectors_usage_report`, `usage_report/skills`, `skill_report`, `connector_report`, `user_skills`, `user_connectors`, `conversations`, `sessions`, `activity`, `events`, `audit_logs`, `compliance`.

Admin: `usage_report/messages`, `cost_report`, `usage_report/claude_code`, `users`, `workspaces`, `me`: all 200.
