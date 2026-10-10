# Privacy / anti-surveillance patterns in developer-tool telemetry (as of Sept 2026)

Research scope: concrete, shipped or published mechanisms (not theory) for AI-coding-agent
usage, IDE telemetry, and engineering-intelligence (DORA/SPACE-style) metrics that could
identify individual engineers. Every URL below was fetched and the quoted text confirmed to
exist on the live page (fetch date: 2026-09-23). Anything I could not verify by fetching was
dropped rather than reported secondhand.

Format per source: **who | URL | date | mechanism (quoted) | enforced in software or policy |
one-line Claude Code application**.

---

## 1. GitHub Copilot Metrics API, minimum-5 team threshold

- **Who:** GitHub (Microsoft)
- **URL:** https://docs.github.com/en/copilot/reference/copilot-usage-metrics/team-level-metrics ; corroborated by https://github.blog/changelog/2026-05-14-team-level-copilot-usage-metrics-now-available-via-api/
- **Date:** Changelog 2026-05-14; docs page current as of fetch
- **Mechanism (quoted):** *"Teams with fewer than 5 seated Copilot users on a given day are excluded from the user-teams report."*
- **Enforced:** In software, the API itself omits the `user-teams` join for sub-threshold teams; the org cannot turn this off.
- **Important caveat / negative finding:** This threshold protects only *team* aggregates. GitHub's **per-user usage report still returns individual, named user rows** with no k-anonymity floor, an admin can see any single engineer's daily Copilot activity directly. I could not find, after fetching `copilot-metrics.md`, `interpret-copilot-metrics`, and the REST reference, any GitHub statement restricting individual-level data from performance use; that "responsible use" framing exists only in third-party blogs (axify.io, koalr.com), not in GitHub's own docs.
- **Claude Code application:** A `sessions_by_team` view that only populates once a team has 5 or more active Claude Code users, while a separate `sessions_by_user` export remains fully identified for admins: k-anonymity at the team-rollup layer only, not at the individual layer.

## 2. Microsoft Viva Insights / Copilot Dashboard, configurable minimum group size

- **Who:** Microsoft
- **URL:** https://learn.microsoft.com/en-us/viva/insights/org-team-insights/copilot-dashboard
- **Date:** Page `ms.date` / `updated_at` shown as 2026-08-26 / 2026-08-27
- **Mechanism (quoted):** *"To protect individual privacy, metrics aren't shown for groups that are smaller than the minimum group size."* Also: *"Feature-level totals are subject to the minimum group size. For example, if your organization's minimum group size is 10, and there are only 2 users who have used the 'Analyze data' feature in Excel, a '--' appears for that row."* And on sentiment: *"The privacy threshold is not met if... The number of responses for a specific question is less than the minimum group size."* Satisfaction rate additionally requires *"at least 30 feedback responses from at least five unique users."* A companion setting, "minimum team size," "must be set to at least five" and gates which managers can see org insights at all.
- **Enforced:** In software, an admin-configured, numeric, org-wide setting (floor of 5) that the UI mechanically checks before rendering any cell; below it the product literally prints `--` instead of a number.
- **Claude Code application:** A configurable `MIN_GROUP_SIZE` (default 5) on any Claude Code adoption dashboard, such that a query for "prompt-acceptance rate, team=Payments-EU" renders a placeholder instead of a number whenever fewer than 5 distinct engineers contributed to it that period.

## 3. Cursor Enterprise analytics, admin-visible individual data (counter-example)

- **Who:** Cursor
- **URL:** https://cursor.com/docs/account/teams/analytics
- **Date:** current docs, fetched 2026-09-23
- **Mechanism (quoted):** *"Team admins have access to data for themselves and all other users in the team."* *"Team members without admin privileges can see data for themselves and in some cases (like the Usage Leaderboard) for select other users on the team."*
- **Enforced:** In software, but in the opposite direction from k-anonymity: this is a documented **lack** of a minimum-group-size gate. Any admin can see any single developer's per-day usage broken out by name, and there is no opt-out or threshold mentioned anywhere on this page.
- **Claude Code application:** Illustrates the failure mode to avoid: an admin console that lets a manager filter Claude Code metrics down to one named engineer with no floor, no consent gate, and no logging of who looked.

## 4. Cursor privacy/data governance, content exclusion and on-device classification

- **Who:** Cursor
- **URL:** https://cursor.com/docs/enterprise/privacy-and-data-governance
- **Date:** current docs, fetched 2026-09-23
- **Mechanism (quoted):** *"Most models run under Cursor's ZDR agreements, so providers don't store inputs or outputs or train on your data."* On its Conversation Insights classifier: *"All classification runs on-device"* and *"Default classifiers ensure no PII or sensitive data leaves the machine."* Enterprises can *"disable it via Disable Conversation Insights in team settings."*
- **Enforced:** Mixed. Zero-data-retention is a contractual guarantee with the model provider; on-device classification is a software mechanism; the disable toggle is a policy lever exposed as a setting.
- **Claude Code application:** Run prompt/response classification (for example, "was this a refactor vs. a secrets-adjacent query") locally in the CLI before any signal leaves the laptop, and give admins a hard off-switch for that whole pipeline.

## 5. Google Gemini Code Assist, event metadata excludes request content, but per-user_id is exposed with no floor

- **Who:** Google Cloud
- **URL:** https://docs.cloud.google.com/gemini/docs/codeassist/security-privacy-compliance ; https://docs.cloud.google.com/gemini/docs/codeassist/generate-metrics
- **Date:** current docs, fetched 2026-09-23
- **Mechanism (quoted):** *"Telemetry data includes data that describes the technical operation of the product. Examples of telemetry data include the following: An event indicating that a request was made (but not the contents of the request)."* Standard/Enterprise editions are described as stateless: they *"don't store prompts and responses in Google Cloud"* by default.
- **Enforced:** In software for content exclusion (the telemetry schema simply has no field for prompt text).
- **Negative finding:** The metrics-generation doc ships sample BigQuery SQL such as `SELECT DISTINCT labels.user_id AS user, DATE(timestamp) AS use_date` with **no minimum-group-size or suppression logic anywhere on the page**. Unlike GitHub's or Microsoft's product, Google ships the raw per-engineer `user_id` join and leaves any aggregation floor entirely up to the customer's own BigQuery views.
- **Claude Code application:** Exporting Claude Code OTel metrics with `user.account_uuid` per event (which the Claude Code OTel spec already does) is the same pattern: content is excluded by schema, but nothing stops a downstream Honeycomb/BigQuery view from being sliced to n=1 unless the *consuming team* adds its own k-anonymity layer.

## 6. JetBrains AI Assistant, two-tier collection, tiered opt-in default, 1-year retention, EEA residency

- **Who:** JetBrains
- **URL:** https://www.jetbrains.com/help/ai/data-collection-and-use-policy.html
- **Date:** current policy, fetched 2026-09-23
- **Mechanism (quoted):**
  - Two categories: *"generalized, anonymous statistics about your interactions with IDE features"* vs. *"complete data about interactions with large language models... the full text of inputs sent by the IDE to the large language model and its responses, including source code snippets."*
  - Default differs by license: individual **non-commercial** tier, *"We may collect analytics as specified above by default based on our legitimate interest. You may opt out in the product settings"* (default-on, opt-out); individual-**commercial** and **organizational** tiers, *"We start collecting analytics only with your explicit consent"* (default-off, opt-in); Community Edition, *"Data collection is disabled and can't be enabled."*
  - Retention: *"We apply a 1-year data retention period for this data."*
  - *"We strictly prohibit any attempts to de-identify users. The collected data remains stored within the European Economic Area (EEA)."* (Read in context this is a prohibition on downstream re-identification of pseudonymized users, paired with EEA data residency, GDPR-driven.)
- **Enforced:** By policy/contract, with the opt-in/opt-out state itself enforced as a software toggle per license type.
- **Claude Code application:** Ship Claude Code with detailed transcript capture default-off for any workspace tied to a commercial/enterprise account (opt-in per org), default-on-with-a-visible-toggle only for a free individual tier, and store any detailed data in-region for EU seats, with a contractual promise against re-identifying pseudonymized users.

## 7. VS Code / Microsoft telemetry, per-user toggle, hashed identifiers, GDPR-driven opt-out ease

- **Who:** Microsoft
- **URL:** https://code.visualstudio.com/docs/configure/telemetry
- **Date:** current docs, fetched 2026-09-23
- **Mechanism (quoted):** Four-level `telemetry.telemetryLevel` setting (`all`/`error`/`crash`/`off`); *"if you don't want to send any telemetry data to Microsoft, you can set the telemetry.telemetryLevel user setting to off."* Identification uses *"a hash of the network adapter NIC on the desktop and a randomly assigned UUID on the web"*, explicitly noted as *"not reliable enough for us to 'provide your data.'"* The GDPR-era change made it *"easier to opt out of telemetry collection by placing a notification in product."*
- **Enforced:** In software (a literal per-machine setting), with the identifier itself deliberately weak/hashed rather than a real account ID.
- **Claude Code application:** A local `claude config set telemetry off` (already a real Claude Code setting pattern) backed by a hashed machine ID rather than an email/SSO identity in the exported OTel resource attributes.

## 8. DX (getdx.com), "surveillance vs. intelligence" framing, guard against reward-linked individual metrics

- **Who:** DX
- **URL:** https://getdx.com/blog/tools-measure-developer-productivity/ ; https://getdx.com/research/measuring-developer-productivity-with-the-dx-core-4/
- **Date:** current blog/research pages, fetched 2026-09-23
- **Mechanism (quoted):** *"If your tools only provide surveillance (tracking 'what' happened), they create friction. If they provide intelligence (explaining 'why' it happened and how it felt), they create alignment."* On one individual-adjacent metric in Core 4 (diffs per FTE): recommends counterbalancing *"with oppositional metrics like the Developer Experience Index"* and, critically, guards it *"by not setting targets or rewards tied to them."*
- **Enforced:** By policy/product philosophy. DX documents this as design guidance for how customers should configure the platform, not as a hard-coded restriction I could verify in the product itself.
- **Claude Code application:** Never wire a per-engineer Claude Code acceptance-rate or prompt-count metric to a bonus/rating formula; publish it only alongside a qualitative counter-signal (for example, a survey score) so it cannot be read as a standalone verdict.

## 9. LinearB, explicit anti-stack-ranking position (policy, not a product gate)

- **Who:** LinearB
- **URL:** https://linearb.io/blog/data-driven-dev-team
- **Date:** current blog post, fetched 2026-09-23
- **Mechanism (quoted):** *"When it comes to members of your dev team, it does not make sense to rank them using personal performance statistics."* *"You can run a highly data and metrics-driven dev organization with absolutely zero performance management statistics."* *"If we track tons of individual metrics, we're showing our people that individual stats matter more than team accomplishments."*
- **Enforced:** By policy/marketing position only. I separately confirmed via LinearB's own privacy policy (https://linearb.io/privacy-policy) that the product *does* ingest per-employee Git/system activity data ("data regarding how systems and code are used, accessed, developed, tested and deployed by employees"), so this is a stated philosophy for customers to adopt, not a hard technical restriction LinearB enforces on its own dashboards.
- **Claude Code application:** Publish a written internal norm ("Claude Code session data is discussed only in aggregate at retro; nobody pulls one engineer's session log to justify a rating") while being honest that the underlying data still supports doing so if someone chooses to.

## 10. Jellyfish, "not for individual performance" as an explicit editorial position

- **Who:** Jellyfish
- **URL:** https://jellyfish.co/blog/engineering-metrics-how-data-driven-management-can-go-wrong/
- **Date:** current blog post, fetched 2026-09-23
- **Mechanism (quoted):** *"Using engineering metrics to evaluate an individual's 'performance' or 'productivity' without qualitative context can be incredibly harmful."* *"Engineering metrics shouldn't be used to determine whether specific teams and individuals are performing better than others."*
- **Enforced:** By policy/editorial guidance in Jellyfish's own content; independent user reviews (G2/Gartner, not used as primary sources here) suggest Jellyfish's actual permissioning is coarse-grained rather than enforcing this at the product level, so treat this as a stated norm rather than a verified software gate.
- **Claude Code application:** Same as LinearB, a documented team norm, not a UI restriction, unless paired with an actual access-control gate.

## 11. Swarmia, team-level default, no individual disclosure on surveys

- **Who:** Swarmia
- **URL:** https://help.swarmia.com/features/run-developer-experience-surveys/how-we-show-your-survey-responses.md ; https://www.swarmia.com/software-engineering-intelligence-platform/
- **Date:** current docs, fetched 2026-09-23
- **Mechanism (quoted):** *"We don't disclose which rating each individual has given"* and survey *"ratings [are reported] in aggregate by team."* Explicit risk disclosure rather than a hard floor: *"In some cases, such as very small teams, inferring identities from team aggregates might be possible."* Positioning: *"Swarmia is built around system-level insight and feedback, not individual monitoring."*
- **Enforced:** Partly in software (ratings are aggregated by team before display; there is no UI path to see one person's rating), but Swarmia itself documents that it has **no numeric k-anonymity floor**, it relies on disclosure of the re-identification risk for small teams rather than suppressing the view.
- **Claude Code application:** Aggregate Claude Code satisfaction/friction survey responses by team by default, but explicitly warn admins in the UI when a team is small enough that "aggregate" could still mean one person.

## 12. SPACE framework (academic), individual-level measurement as the core caution

- **Who:** Nicole Forsgren, Margaret-Anne Storey, Chandra Maddila, Tom Zimmermann, Brian Houck, Jenna Butler, ACM Queue, Feb 2021 (still the standard reference cited across the industry as of 2026)
- **URL:** https://www.microsoft.com/en-us/research/publication/the-space-of-developer-productivity-theres-more-to-it-than-you-think/
- **Date:** ACM Queue, Vol 19(1), pp. 20-48, published February 2021
- **Mechanism (quoted):** *"Developer productivity is about more than an individual's activity levels or the efficiency of the engineering systems... and it cannot be measured by a single metric or dimension."* The framework's five dimensions (Satisfaction, Performance, Activity, Communication, Efficiency) are explicitly designed to be read together, at team level, specifically to prevent any one activity count from being read as an individual verdict.
- **Enforced:** By methodology/guidance only. This is an academic framework, not software; every vendor cluster above (DX, LinearB, Jellyfish, Swarmia) explicitly cites SPACE as the intellectual basis for their team-level defaults.
- **Note on DORA specifically:** I could not, after fetching `dora.dev/guides/dora-metrics/` and the 2024 State of DevOps report page directly, find primary-source language on dora.dev itself stating "never measure individuals," that framing is ubiquitous in secondary commentary (getdx.com, dev.to, and so on) but I am not citing dora.dev for a quote it does not contain. What I *could* verify on dora.dev is a collaboration framing: *"Sharing all five metrics across development, operations, and release teams fosters collaboration and shared ownership."*
- **Claude Code application:** Report Claude Code impact as a small basket of team-level signals (adoption, review-cycle time, defect escape rate, a satisfaction pulse) shown together, specifically so no single per-engineer number can be lifted out as a standalone rating.

## 13. Intercom, content exclusion plus pseudonymized transcripts plus symmetric internal transparency

- **Who:** Intercom (Brian Scanlan, Senior Principal Engineer)
- **URL:** https://ideas.fin.ai/p/how-we-use-claude-code-today-at-intercom (blog, dated Mar 19, 2026) and https://www.aviator.co/podcast/ai-engineering-intercom-brian-scanlan (podcast transcript, dated Apr 23, 2026)
- **Mechanism (quoted):**
  - Blog: *"Privacy-first: we explicitly never capture user prompts, messages, or tool input."* *"Session transcripts sync to S3 (with username SHA256-hashed for privacy)."*
  - Podcast: *"This is a public, internally accessible data set. Anyone can use it to browse and see what's going on."* *"We also collect the full session transcripts. We publish 100% of the details of every single session that people are using inside of Intercom."*
- **Reconciling the two:** These describe two different layers, a lightweight event-telemetry stream to Honeycomb (14 event types: `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, and others) that explicitly excludes prompt/tool-input *content*, plus a separate full-session-transcript pipeline to S3 that does capture content but pseudonymizes the username via SHA256 and makes the resulting corpus browsable by any engineer internally rather than restricted to managers, symmetric ("everyone can see everyone's") rather than top-down visibility.
- **Enforced:** In software, the exclusion is a property of what the base plugin instruments; the hashing and internal-only access are both engineered, not just promised.
- **Claude Code application:** This is close to a direct template: instrument Claude Code with an OTel exporter that never includes `UserPromptSubmit` payload text, sync full session transcripts to a bucket keyed by a salted hash of the engineer's SSO identity rather than their name, and give every engineer (not just managers) read access to the same adoption dashboard.

## 14. Honeycomb, internal Claude Code adoption measurement (per-user_id fields exist; no aggregation policy documented)

- **Who:** Honeycomb (Mae Capozzi)
- **URL:** https://www.honeycomb.io/blog/measuring-claude-code-roi-adoption-honeycomb
- **Date:** January 22, 2026
- **Mechanism (quoted):** The exported OTel schema includes `user.account_uuid: Unique user identifier` and `session.id: Individual Claude Code session`; the author's framing question was *"Were engineers regularly using it to develop in one of our repos?"*
- **Enforced / caveat:** This post documents that Honeycomb's own internal dogfood setup captures a per-engineer UUID and per-session IDs; it does **not** state any aggregation, k-anonymity, or anonymization policy applied on top before humans view the data. Included because it is a real, citable description of Claude Code's own OTel telemetry surface (which any org, including Tatari, inherits by default), but it is a *counter-example* of unaddressed individual identifiability, not a privacy mechanism.
- **Claude Code application:** This is literally Claude Code's own OTel output, `user.account_uuid` and `session.id` already exist on every event Tatari would collect; whatever k-anonymity/aggregation layer gets built has to be added downstream (the GitHub/Microsoft team-threshold pattern above), because Claude Code does not ship one itself.

## 15. Apple, differential privacy for Apple Intelligence prompt trends

- **Who:** Apple
- **URL:** https://machinelearning.apple.com/research/differential-privacy-aggregate-trends
- **Date:** Apple Machine Learning Research blog (current as fetched)
- **Mechanism (quoted):** *"Randomly polling participating devices for whether they've seen a particular fragment, and devices respond anonymously with a noisy signal."* *"Hundreds of people using the same term are needed before the word can be discoverable."* *"Apple only sees commonly used prompts, cannot see the signal associated with any particular device, and does not recover any unique prompts."* Participation is opt-in: *"For users who opt in to share Device Analytics with Apple, we use differentially private methods..."*
- **Enforced:** In software, the noise/randomized-response step happens on-device before transmission, so the server mathematically cannot recover a single user's raw value.
- **Claude Code application:** A "which prompts/skills are trending" feature could inject calibrated noise client-side before any prompt fragment is ever reported, guaranteeing a minimum population (for example, "hundreds of sessions") before any fragment becomes visible in a dashboard, a hard technical floor rather than an admin-configured one.

## 16. Google RAPPOR, local differential privacy deployed at Chrome scale

- **Who:** Ulfar Erlingsson, Vasyl Pihur, Aleksandra Korolova (Google), ACM CCS 2014
- **URL:** https://arxiv.org/abs/1407.6981
- **Date:** Published November 2014; the underlying technique remains the standard citation for local-DP telemetry as of 2026
- **Mechanism (quoted):** *"Randomized Aggregatable Privacy-Preserving Ordinal Response, or RAPPOR, is a technology for crowdsourcing statistics from end-user client software, anonymously, with strong privacy guarantees."* It permits *"statistics to be collected on the population of client-side strings with strong privacy guarantees for each client, and without linkability of their reports."*
- **Enforced:** In software, each client independently randomizes its own report (two-stage randomized response) before it ever leaves the machine, so even the collector cannot reconstruct a specific client's true value with confidence, and repeated reports from the same client do not become newly linkable over time.
- **Claude Code application:** A small platform team could apply the same two-stage randomized-response trick to something like "which internal skill did you invoke" telemetry, so that even the team running the collector cannot say with confidence which skill any *specific* engineer used, only the population-level frequency.

## 17. Mozilla Glean/Firefox telemetry, data-minimization pipeline plus review gate

- **Who:** Mozilla
- **URL:** https://docs.telemetry.mozilla.org/tools/guiding_principles ; https://www.mozilla.org/en-US/privacy/principles/
- **Date:** current docs, fetched 2026-09-23
- **Mechanism (quoted):** Principle: *"Collect what we need, de-identify where we can and delete when no longer necessary."* Pipeline behavior: geo lookup *"allowing the pipeline to discard source IP addresses to protect user privacy"*; user-agent parsing *"allowing the pipeline to discard the raw user agent string to protect user privacy"*; retention: *"partitions older than 30 days are automatically cleaned up."* Every Glean metric requires a data review before it can ship: *"it is impossible to collect measurements that haven't been declared."*
- **Enforced:** In software (the pipeline mechanically discards IP/UA at ingest, and storage auto-expires after 30 days) plus process (a mandatory human data-review gate before any new metric can be added to the SDK).
- **Claude Code application:** Require a lightweight "data review" checklist (what field, why, retention) before any new OTel attribute can be added to the Claude Code exporter config, and auto-expire raw session data after a fixed window (for example, 30 days) rather than keeping it indefinitely.

## 18. Germany, works-council co-determination (BetrVG Section 87(1) No. 6) shaping vendor design

- **Who:** German Works Constitution Act (Betriebsverfassungsgesetz), as explained in two independent legal/consulting write-ups
- **URL:** https://www.startuprad.io/post/betriebsrat-works-council-german-software-deals ; corroborated by https://compound.law/en-DE/compliance/ai-employee-monitoring/
- **Date:** current pages, fetched 2026-09-23
- **Mechanism (quoted):** Statutory language paraphrase per both sources: *"the introduction and use of technical devices designed to monitor the behavior or performance of employees"* triggers works-council co-determination. Scope is deliberately broad: *"any system objectively suitable for monitoring falls under co-determination, regardless of whether the employer intends to use it that way. Manager dashboards, productivity scores, insider-risk alerts, QA scoring, activity logs, and AI-driven workforce analytics can all trigger the rule."* On the legal basis for any employee data use: *"Consent is usually a weak primary basis in employment relationships"*; the employer instead must show *"the tool is necessary, proportionate, and narrowly configured for a legitimate workplace purpose"* (GDPR plus BDSG Section 26). The practical fix cited: *"Configuration options for co-determination, the toggles (anonymization, aggregation, access controls, logging limits) that let a customer satisfy the council without losing the product's value."*
- **Enforced:** By law. This is not optional or a vendor's choice; introducing any tool objectively capable of behavior/performance monitoring in a German entity with a works council legally requires negotiating a works agreement (Betriebsvereinbarung) first, and covert deployment is unlawful.
- **Claude Code application (relevant given Tatari may have EU staff):** Before turning on any per-engineer Claude Code telemetry for German-based staff, Tatari would need a works-council agreement (or to ship only the aggregated/anonymized, team-threshold view for that entity), and the product needs literal on/off toggles for anonymization, aggregation floor, access control, and log retention scoped per legal entity, exactly the pattern GitHub/Microsoft ship globally by default for privacy, but which becomes a *legal requirement* rather than a nicety for EU/German seats.

---

## Ranked mechanisms (most-repeated across sources)

1. **Minimum group size / k-anonymity threshold before a breakdown renders.** GitHub Copilot Metrics API (5-seat team floor), Microsoft Viva Insights/Copilot Dashboard (configurable min-group-size, min 5; 30+ responses from 5+ users for satisfaction), Swarmia (soft/disclosed version, no hard floor). The most concrete, most reused pattern found, and the most directly transplantable to Claude Code telemetry.
2. **Content/prompt exclusion by schema** (telemetry records that an event happened, never its payload). Google Gemini Code Assist ("not the contents of the request"), Intercom's base plugin ("never capture user prompts, messages, or tool input"), Apple (device-side noise before transmission means no raw content ever leaves).
3. **Tiered opt-in vs. default-on gated by license/employment context.** JetBrains (non-commercial default-on/opt-out; commercial and org default-off/opt-in; Community cannot enable at all), Apple (Device Analytics opt-in), VS Code (default-on, one-setting opt-out, GDPR-driven).
4. **"Not for individual performance evaluation" as an explicit written policy.** LinearB, Jellyfish, DX, and the SPACE academic framework all state this; none of the four enforce it as a hard software gate on their own admin dashboards (LinearB and Jellyfish both still ingest/expose per-employee data underneath the policy).
5. **Pseudonymization of identifiers** (hashed username/machine ID instead of plaintext identity). Intercom (SHA256-hashed usernames on session transcripts), VS Code (hashed NIC plus random UUID), JetBrains (prohibits "attempts to de-identify" pseudonymized users, EEA residency).
6. **Local/on-device or client-side randomization so the server never sees a raw individual value.** Apple differential privacy, Google RAPPOR, Cursor's on-device Conversation Insights classifier. Most rigorous mathematically, least commonly implemented outside of Apple/Google-scale platforms.
7. **Symmetric, org-wide internal transparency** ("everyone can see everyone's" adoption data). Only clearly documented at Intercom: a "public, internally accessible data set" that any engineer, not just managers, can query, paired with the content-exclusion and pseudonymization mechanisms above so that broad visibility does not equal individual surveillance.
8. **Retention limits plus a mandatory data-review gate before new telemetry ships.** Mozilla Glean (30-day partition expiry, no un-reviewed metric can be collected), JetBrains (1-year cap on detailed data). Also the least discussed elsewhere, a design lever most engineering-intelligence vendors do not publish.

Legal backstop for the EU cluster specifically: **BetrVG Section 87(1) No. 6** turns items 1, 2, 3, and 5 above from best-practice into a hard requirement for any German (and likely broader EU) entity. Tatari should treat "configurable anonymization / aggregation floor / access control / retention limit, settable per legal entity" as the actual compliance surface, not just a UX nicety.

---

## Top 5 searches that returned nothing relevant or unverifiable

1. `"public query log" OR "queryable by everyone" internal analytics engineering team transparency blog`, returned only generic log-analytics-tooling documentation (Dynatrace, Google Cloud Logging, Elastic), nothing about a company publishing a literal public query log of who-queried-what.
2. `engineering blog "not surveillance" AI coding agent telemetry convinced developers trust`, returned generic AI-coding-adoption trend pieces (The New Stack, developer-tech.com), no first-person "how we convinced our developers this wasn't surveillance" essay.
3. `"treat telemetry as a commons" OR "telemetry is a commons" engineering`, no company uses this exact framing in a fetchable primary source; closest hits were generic OpenTelemetry/observability explainers unrelated to the privacy angle.
4. `DX getdx.com "individual" developer metrics policy do not use for performance review`, DX's own site has no single page stating a formal "we do not show individual metrics" policy; the closest verifiable material is the "surveillance vs. intelligence" blog framing and the Core-4 research paper's caution against reward-linking one metric.
5. `dora.dev "individual" performance "not intended" OR "should not be used" developers metrics`, dora.dev's own guide and 2024 report pages contain no direct quote warning against individual measurement; that framing is ubiquitous in secondary commentary (getdx.com, dev.to, Medium) but not on dora.dev itself, so it is not attributed to DORA as a primary source here.

(A sixth near-miss worth noting: `LinearB "individual" developer metrics privacy "we do not" OR "not used for performance"`, LinearB's actual *privacy policy* page has no such language; the anti-stack-ranking position exists only in a separate marketing blog post, which is what is cited above.)
