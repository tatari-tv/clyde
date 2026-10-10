# Design Document: Org-Wide Claude Code Session Telemetry

**Author:** Scott Idler
**Date:** 2026-09-21
**Status:** Draft
**Review Passes Completed:** 5/5 (draft, correctness, clarity, edge cases, excellence)
**Panel Rounds:** 3 of 3, cap reached. A round 4 needs an explicit override. All 17 must-fix across three rounds folded, none dropped or deferred. Every design decision is closed. 2 open questions remain, both evidence to be gathered rather than calls to be made.

## Summary

Collect Claude Code session data from every Tatari engineer into two corpora: a
structural one readable by every employee, and a prose one no human can read.
Prose analysis happens through an LLM-only query service that refuses to answer
about fewer than 10 distinct clients and logs every question publicly. The point is to
learn what Intercom learned (which skills work, where sessions die, what
problems recur) without building a capability that a future manager could turn
on an individual.

## Problem Statement

### Background

- Brian Scanlan (Intercom) on Claire Vo's *How I AI*: they collect all session
  data to S3, anonymize it, and run an LLM judge over months of it. Source and
  timestamps in References.
- Their telemetry is a commons, not a management dashboard: a shared key on
  laptops and "anyone can go in and kind of look through this data" [00:31:08].
- They name the hazard themselves [00:32:20]: "people put all sorts of stuff in
  their sessions. They yell at their sessions... people have personal
  relationships at times with Claude. And like we don't really want to know
  about that."
- Their primary consumer is the individual: a personalized-insights view, an org
  percentile, and a nudge that told Scanlan his own CLAUDE.md was the problem
  [00:32:55-34:09].
- Tatari today: no centralized Claude Code telemetry of any kind. Org push
  exists (`claude-enterprise-config/settings.json` via Anthropic
  server-managed settings) and carries settings, `claudeMd`, and announcements
  only.
- clyde already mines every signal Corpus A needs, locally, with no upload path
  in any crate.
- Anthropic's enterprise analytics API already exposes per-user token and cost
  to org owners. Per-person visibility predates this work; this design is
  strictly less identifying than the existing admin console.

### Problem

Two problems, and the second one is the hard one.

1. Nobody at Tatari can answer "which skills are working", "where do sessions
   die", or "what are people fighting with" at org scale. Every signal is
   stranded on individual laptops.
2. The obvious way to fix that (ship session transcripts to a central store)
   builds a surveillance capability. Reviewers raised, correctly, that it reads
   as Big Brother, that it would stoke fear among engineers even absent bad
   intent, and that a future manager or HR partner could request access for
   performance evaluation. Access control does not answer this: policy is not
   structure, and a promise does not survive a reorg.

Prose is not optional. Counters cannot tell you why a session went badly or what
someone was trying to do, and that is the whole value. So the design has to
carry prose and still make individual lookup impossible.

### Goals

- Read everything, org-wide: full session transcripts, prose included.
- Individual surveillance structurally impossible, not policy-prevented. The
  honest answer to "show me this person's sessions" is "no such field exists",
  not "I refuse".
- Every engineer sees their own numbers and their standing against the org.
- Org-level findings good enough to drive tooling changes the way Intercom's
  PR-description judge did.
- The guarantee survives the author leaving the company.

### Non-Goals

- Measuring individuals. Excluded permanently, by schema, not by policy.
- Any performance, comp, or promotion input. Excluded permanently.
- Replacing Anthropic's enterprise analytics API for token and cost reporting.
  Excluded: it already does that, per-user, and this design deliberately does
  less.
- Long-window retro-analysis of raw prose. Parked: raw prose TTLs at 30 days and
  findings persist, so a new question runs forward. Revisit if a question
  arrives that findings provably cannot answer.
- Real-time streaming. Parked: batch upload of dormant sessions is enough.
  Revisit when someone names a question that a 24-hour lag breaks.
- Mandatory enrollment. Excluded, and not by choice: see Rollout, no push
  channel for a binary exists.

## Proposed Solution

### Overview

The danger is not prose. The danger is the **join** of identity, content, and a
human who can look. Break each link structurally.

| Corpus | Holds | Keyed by | Who reads it |
|---|---|---|---|
| **A** | structural counters, zero prose | email, in the clear | every employee |
| **B** | redacted prose | client-generated random `pid` | no human, ever |

Corpus A has identity and no content. Corpus B has content and no identity.
Neither alone is surveillance, and no row anywhere maps an email to a `pid`.

Corpus A is deliberately **not** anonymous. Symmetry is its control: everyone
measured can read everyone else, including the CTO's numbers. That is Intercom's
Honeycomb property, and a dashboard that shows everyone to everyone is not a
management dashboard.

### What a skeptic should check

The objection this design answers is "a future manager or HR partner asks for
one person's sessions." Do not take the answer on faith. Six checks, each one
falsifiable by reading code or config rather than by trusting an author:

1. **Grep both service schemas for a row holding an email and a `pid`.** There
   is none. That is the guarantee; everything else is a supporting detail.
2. **Read the presign vendor's IAM policy.** No `kms:*` at all. The service that
   authorizes an upload holds no key and never sees the bytes.
3. **Read the query service's refusal paths.** Under 10 distinct `pid`s returns
   422. A query naming a person returns 422.
4. **Read `/queries`.** Every question ever asked, with its author. If someone
   goes looking for you, you can see them looking.
5. **Diff `/me` against `/corpus`.** `/corpus` has no identity column, so there
   is no page anywhere that maps a person to the skills they run. `/me` shows
   you your own, because you are allowed to see yourself.
6. **Read the ladder and the ceiling below.** An AWS admin defeats this. That is
   written down rather than argued away, and the bound on it is named.

The ask: shoot at checks 1 through 4, and at the four correlation constraints
under Architecture, before Phase 0 runs.

### Architecture

Four components. Panel round 1 deleted a fifth: there is no ingest service, for
the reason under Presigned upload below.

1. **Presign vendor** (new service). Verifies an Okta Bearer server-side. Signs
   an S3 POST Object **policy** whose key condition is
   `["starts-with","$key",""]`, so the client picks the object key and the
   vendor never learns it. Receives no `pid`, mints no key material, stores no
   association. Logs "an identity was authorized to upload", at hour resolution.
2. **Query service** (new service). The only reader of Corpus B. Holds the sole
   `kms:Decrypt` grant on the private half of an asymmetric CMK. Accepts a
   question, runs an LLM over the corpus, returns findings gated at 10 distinct
   `pid`s. Refuses queries that name or target an individual. Refuses verbatim
   spans longer than a sentence. Writes every query, its full text, and its
   author to a log every employee can read.
3. **Corpus A service** (same tier, marquee-shaped). S3 canonical with
   conditional writes, per-pod SQLite index as a disposable accelerator,
   all-employee Okta gate by attaching no route-level `SecurityPolicy`. Carries
   a write endpoint, which the round-1 doc omitted entirely, and **two read
   paths that are not the same query**: `/me` identity-scoped with full
   per-skill detail, `/corpus` email-stripped aggregates. The split is a
   privacy control, not a convenience.
4. **clyde client**. A new prose-export chokepoint (not an extension of
   `session/src/redact.rs`, see Phase 5), plus an `upload` crate that generates
   its own AEAD key, encrypts it to a published public key, and PUTs
   `wrapped_key || nonce || ciphertext` straight to S3.

**Why blind upload is the load-bearing control.** The naive design authenticates
the upload with Okta, which puts identity and content in the same request.
Identity is proven to one service; the bytes go somewhere else entirely. No
component, log, or table holds both an email and a `pid`.

**No service on the upload path, because the platform injects identity.** The
round-1 design had an ingest service behind the standard ingress, and that
inverts the whole control: `gitops-infra/istio-istiod/values.yaml:33` places
`x-auth-request-email` in `headersToUpstreamOnAllow`, the header is in the
ext_authz allow-list in all five overlays
(`overlays/{prod,ops,staging,test,apps-1}/patch-configmap.yaml:46`), and the
gateway sets `forwardAccessToken: true` and `passThroughAuthHeader: true`
(`envoy-gateway/base/security-policy-oidc.yaml:21-22`). Any service behind that
edge receives the uploader's email on the same request that carries the `pid`.

Removing the service also removes the Envoy access log, which is on by default
with no override anywhere in the `envoy-gateway/` tree and writes millisecond
timestamps and the full request path into Loki via Alloy, outside application
control. An hour-resolution requirement on application logs never governed that.

**S3 POST Object, not presigned PUT, and the difference is the whole control.**
A presigned PUT signature covers the **exact object key**, so the vendor has to
receive the key in order to sign it. That is the co-location problem again
wearing a different hat: if the key carries the `pid` the vendor learns the
`pid`, and if the key is opaque the join merely downgrades to identity-to-object
-name. Opaque is worse in one specific way: a ticket is transient and
discardable, but the object key is durably recorded in S3 as the object's own
name, forever, so one vendor log line carrying a presigned URL makes the pair
permanent.

S3's POST Object API signs a **policy document** instead of a URL, and the
policy supports `["starts-with", "$key", ""]`. The client picks the key at
upload time. The vendor signs "some object in this bucket under these
conditions" and never learns which object. That is a dissolution rather than a
mitigation: there is no pair to avoid logging, because the vendor never holds
one.

Three other debts it pays in the same move:

- `content-length-range` in the policy makes **S3 itself** enforce the padded
  size bucket. Size padding is unenforceable under a presigned PUT.
- The client choosing its own key lets that key be a fresh random opaque id,
  with `pid` and session uuid moved inside the AEAD. The marquee
  author-next-to-session page then joins to nothing.
- Only the query service, which already holds the sole `kms:Decrypt`, ever
  learns a `pid`.

Costs, both unverified: `aws-sdk-s3` in Rust exposes `presigned()` on PutObject
and GetObject and has no presigned-POST-policy helper (there is no Rust
equivalent of boto3's `generate_presigned_post`), so the policy and its
signature are hand-built; and POST Object is `multipart/form-data`, which is
more client code than a PUT.

**Single-use is bucket versioning, NOT `If-None-Match`.** An earlier draft
asserted a signed `If-None-Match: *` per key and cited marquee as precedent.
Both were wrong. AWS's conditional-writes documentation lists the header as
available on PutObject, CompleteMultipartUpload and CopyObject; the string
"POST Object" does not appear on that page at all. And marquee's citation is
`.put_object().if_none_match("*")`
(`marquee/core/src/storage/s3.rs:307-317`), a PutObject call, so it is not
precedent for POST Object.

What that left behind, stated because it is the part that matters: with an
empty-prefix policy, no conditional creation, and versioning off, **any
authenticated employee holding a valid policy can overwrite any known key and
destroy the prior ciphertext.** The AAD binding rejects the replacement on read
but cannot recover the object. Random object ids make keys hard to guess, which
is not the same as prohibited. `object_count` is not server-enforced, and
"bound to the manifest digest" had no enforcement point anywhere.

So: **versioning ON, with `NoncurrentVersionExpiration` at 30 days, and the
reader takes the earliest version per key.** First version wins; later versions
are inert. This reverses the earlier "versioning off" choice and it resolves
that choice's own stated objection properly, since `NoncurrentVersionExpiration`
is exactly the lifecycle rule for not letting an expired object linger as a
noncurrent version. Versioning is also the only route to S3 write-once at all,
because Object Lock requires it.

**The presigned policy is a bearer capability**, and there is in-house
precedent and in-house awareness: `tatari-offline-reporting-api` presigns
`get_object` with a 900-second default and comments at `app.py:909` that
"Presigned download_url values are short-lived bearer credentials; do not
cache." Ours is a write capability rather than a read, scoped by prefix and
size.

**Policy lifetime versus the randomized delay is a trilemma, and it has to be
stated rather than implied.** The usable deadline is
`min(policy expiration, signing credential expiration)`.
`AssumeRoleWithWebIdentity` defaults to one hour, rotating credentials does not
extend an already-issued signature, and role chaining caps a session at one hour
regardless of the role's configured maximum, so an intermediate `AssumeRole` hop
buys nothing. Verified in the SDK marquee pins (`aws-sdk-s3` 1.135.0, via
`marquee/core/Cargo.toml:10`): `presigning.rs:39-41` documents that "Credential
expiration time takes priority over the expires_in value", and `:23`/`:65` cap
presigned requests at one week. That one-week guard is an **SDK** guard, which a
hand-built POST path does not get, so the vendor enforces its own ceiling.
The 43200-second precedent at `terraform/modules/partner-s3-access/main.tf:58`
exists but is a human-assumed partner role rather than IRSA, and
`eks-irsa-role?ref=v2` is not checked out, so the vendor's effective ceiling is
**unverified** and F3b must measure it.

Resolution: **delay first, then presign, then upload immediately.** The delay
exists to decorrelate the upload from session activity, not from the vendor
call, so taking the policy after the delay keeps the delay arbitrarily long
while the policy stays inside its one-hour ceiling. The vendor additionally
returns the effective deadline, refuses to issue unless remaining credential
lifetime covers the retry window, and a client whose policy expires during
laptop sleep reacquires and restarts.

**Asymmetric hybrid encryption, because the vendor must not see a `pid`.**
Round 1 had the vendor call `kms:GenerateDataKey` while holding the verified
identity, and the encryption context bound to `pid` forced the vendor to receive
the `pid`. That is a co-location, not a correlation, and "we do not persist the
association" is not "the issuer cannot establish it."

Instead: one KMS asymmetric CMK. The public half is published and baked into the
client. The client generates its own AEAD key locally, encrypts it to that
public key, and uploads. Only the query service holds `kms:Decrypt` on the
private half. The vendor never receives a `pid`, never returns key material, and
stops being the highest-value target in the system.

**Dropping the KMS-side condition is correct, and the reason is precise.**
valet's property is that an attacker "cannot move a victim's ciphertext under
their own key and let valet decrypt-and-redeem it" (`crypto.rs:10-17`).
Decrypt-and-**redeem** is the operative phrase: valet's two call sites
(`grants.rs:318-334` vend, `:378-389` redeem) hand back a usable Slack
credential. Corpus B's query service returns k=10-gated findings and never hands
plaintext to a caller, so that oracle does not exist and the property does not
transfer. The IAM condition
`StringLike { "kms:EncryptionContext:pk" = "?*#?*" }`
(`platform-infra/accounts/tatari-tv/prod/us-east-1/valet/dynamodb-custom.tf:115-117`)
has no counterpart here, and the doc does not claim parity.

**The AAD binding stays, for cohort-count integrity rather than
confidentiality.** Write-once creation prevents replacement; it does not prevent
copying one ciphertext into N unused locations under N fabricated `pid`s, which
inflates the distinct-`pid` count and pushes a 9-`pid` cohort past the gate. So:

- AAD covers a canonical encoding of `pid`, object id, `schema_version`, and
  **key id**.
- The reader must **compare those fields against the object's actual storage
  location**. Authenticating attacker-supplied envelope fields without that
  comparison buys nothing.

**Decided: the public key is baked into the clyde binary.** Unauthenticated
distribution is fine; unauthenticated trust is not, and a substituted public key
means the laptop encrypts to the attacker. Baking it in makes the binary the
trust root, and clyde already self-updates through `renew`, so shipping a new
binary is an existing routine rather than a new burden. The cost is accepted:
**rotating the key requires a clyde release.**

Rejected: trust-on-first-use over an unauthenticated fetch (the classic hole);
a signed key manifest verified against an embedded trust anchor (works, and it
only moves the pinning problem one level, so it is the option to reach for only
if key rotation ever needs to outpace clyde releases, which nothing suggests);
and fetching the key over an Okta-authenticated call, which re-adds an
authenticated request to the upload path and gives back what removing the ingest
service just bought.

**The retirement rule, so nobody has to judge it later.** KMS asymmetric
rotation is manual, and "stop encrypting with this key" is a different date from
"stop decrypting it". Both are now fixed:

- Stop **encrypting** with key N the day key N+1 ships.
- Stop **decrypting** with key N thirty days later. That falls out of the prose
  TTL for free: after 30 days nothing key N locked still exists.

**The client carries its key's encrypt-until date and fails closed past it.**
This is the operational consequence of baking the key in: a laptop that has not
run `clyde update` in six weeks would otherwise encrypt to a retired key and
upload blobs nobody can ever read. So the baked-in material is the key **plus**
its encrypt-until date, and an expired client refuses to upload and says why
rather than uploading garbage.

**Envelope fields.** KMS asymmetric `Decrypt` takes no encryption context and
requires an explicit key id **and** algorithm, so both ride in the envelope in
the clear, where the reader can select on them, and both are covered by the AAD
so they cannot be tampered with. Size is a non-issue: RSA-2048 OAEP-SHA-256
ciphertext is always **256 bytes**, the modulus size, so `wrapped_key_len` reads
256. (190 bytes is the maximum plaintext *input*, `256 - 2*32 - 2`, not the
output; an earlier draft stated it as the output.) The format is versioned from
the first release, so a later change supports both for a window.

Worth knowing before Phase 7 is scoped: clyde's workspace has no crypto
dependency today. No `aes-gcm`, `ring`, `rsa`, or `chacha20`, and no `aws-sdk-*`
in any `Cargo.toml`. This is the client's first.

What still harvests from valet: the `Cipher` trait seam (`crypto.rs:55`) and the
idea of a context-bound test double. What does not: `KmsCipher`, and
`crypto/fake.rs`, whose reversible length-prefixed frame (`fake.rs:1-8`) cannot
model AEAD. That test double is work inside the phase, not a freebie.

**Corpus B object ids are minted fresh, never the session uuid.** marquee
already publishes the pair: `marquee/server/src/render.rs:381` renders Author
and `:389` renders Session as adjacent rows on the all-employee post page, over
a record carrying `author_email` (`core/src/model.rs:41`) and `source_session`
(`:101`). Keying Corpus B on `<pid>/<session-uuid>` would have handed the join
to anyone with a browser, needing no logs, no timestamps, and no AWS access.

**The four correlation constraints that remain.** Splitting the request is
necessary and not sufficient.

- **Timing.** The vendor leg and the S3 PUT are separate requests from separate
  sources at separate times. The client uploads on a randomized delay measured
  in hours. Vendor logs at hour resolution.
- **S3-side logging is in scope, and application log settings do not reach it.**
  S3 server access logs and CloudTrail data events record the laptop's egress IP
  and a millisecond timestamp against the object key. Either both are disabled
  for this bucket, or the bucket is fronted so the source IP is not the laptop.
  Phase 0 decides which, and this is the open attack surface the design has not
  closed.
- **Ticket unlinkability.** Presigned URLs are random and scoped to one object
  key, never derived from identity, and the vendor stores no pair.
- **Volume fingerprinting.** The heaviest Claude Code user in the company is
  identifiable from bytes alone. Blob sizes pad to buckets, and no read surface
  exposes a per-`pid` session count.

### Data Model

**Corpus A**, from `efficiency/src/metrics.rs` `RawCounters` (`:82-138`):
`input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_5m_write_tokens`,
`cache_1h_write_tokens`, `cost_usd`, `turns`, `turn_durations_ms`,
`compactions`, `tool_calls`, `tool_errors`, `bash_command_failures`,
`interrupts_structured`, `interrupts_text`, `web_search_requests`,
`web_fetch_requests`, `effort_high`, `effort_xhigh`, `model_mix`, `by_model`,
`by_skill` (`:126`), `by_mcp_tool` (`:128`), `unpriced_models`.

Plus the **full** `efficiency/src/outcome.rs` `Outcomes` (`:54-93`), sets
included: `commits` as a `Vec<String>` of SHAs (`:54`), `prs` as a
`Vec<PrRef>` carrying urls (`:55`, `:98-103`), `repos_touched` keyed by slug,
plus `confluence_writes`, `jira_writes`, `slack_messages`, `files_edited`,
`lines_written`, `lines_replaced`. (Round 1 wrongly described `commits` and
`prs` as counters; they are sets, and that matters below.)

**Why the sets stay, reversing an earlier reduction.** An interim draft cut
SHAs, urls and repo slugs to counts. That was wrong twice over:

- **It broke a correctness property the code exists to provide.** The rollup
  dedups commits and PRs **globally across sessions**, and its own comment says
  why: "a PR opened from one host and referenced from another must count once,
  not twice" (`report/src/outcome.rs:49-53`). Those totals feed `per_commit` and
  `per_pr` (`report/src/aggregate.rs:940-941`). Per-session counts double-count:
  one PR worked across three sessions becomes three, and org cost-per-PR comes
  out 3x too cheap. A confident wrong number in a leadership deck is worse than
  an absent one.
- **It was protecting data GitHub already publishes.** A SHA is a join key, not
  a secret. Every commit and PR in the `tatari-tv` org is already visible with
  authorship attached to anyone with GitHub access. The reason to remove them
  was the linkage in the next section, and the read-path split closes that
  linkage structurally: `/corpus` carries no identity column, so there is no
  public page mapping a person to a SHA.

Two alternatives were considered and rejected. **Keyed tokens** (an HMAC under a
service-held key, so the service dedups without publishing SHAs) works but costs
a second key to custody in order to hide something GitHub shows you.
**Client-side dedup** is cheaper but partial: each laptop dedups only its own
transcripts, so a PR touched from two machines counts twice, which is exactly the
multi-host case the global dedup was written for.

Accepted cost, stated: "already visible in GitHub" means visible to `tatari-tv`
org members, roughly engineering, while Corpus A is readable by every employee.
So this publishes commit and PR references slightly wider than GitHub does, in
aggregate form only, with no identity attached on the public path.

**The linkage problem, and the two-part fix.** Corpus A is email-keyed. Any
field it *publishes* that can also serve as a Corpus B cohort predicate turns
Corpus A into a public membership function for that cohort: a finding about "10
clients that used graphify" is anonymous, and a public per-email skill map names
exactly which emails use graphify. Intersecting `by_skill`, `by_mcp_tool` and
`by_model` (`efficiency/src/metrics.rs:115-128`, all keyed maps) narrows a
cohort to a handful of people.

Three properties of the problem, because two of them defeat the obvious fixes:

- **Membership, not magnitude.** Bucketing the values does nothing. Retaining
  the key is the disclosure, however coarsely its count is reported, and
  intersecting three coarsened maps restores the precision.
- **The lever is whether the key set is published per-email at all**, not what
  is stored and not how values are rounded.
- **A prose rule cannot enforce it.** "No Corpus A field may be expressible as
  a Corpus B predicate" reads well and is not testable: with free-text
  questions, "expressible" includes paraphrase and inference. It stays a design
  principle. What is enforceable is a finite grammar plus per-field
  classification plus rejection tests.

**Decided (D plus B): store it all, split the read paths, and close the question
grammar.**

- **Corpus A stores per-email skill, MCP-tool and model keys unchanged.** No
  field is dropped for this, and the individual-insights view keeps full detail.
- **`/me` is identity-scoped.** It serves skill-keyed detail and percentiles for
  the calling identity only, because you are allowed to see yourself.
- **`/corpus` is email-stripped.** Same aggregates, no identity column, so no
  read surface anywhere answers "which emails ran graphify."
- **Corpus B gets a closed, server-enforced predicate grammar** (Phase 9), so a
  graphify cohort cannot be requested in the first place. This is needed
  independently of the read-path split, per the third property above.
- **Residual, stated:** the grammar does not close the **output** channel. A
  finding phrased as "people working on video pipelines hit X" still narrows to
  whoever is visibly working on video. The verbatim-span cap bounds it; nothing
  eliminates it.
- **Fallback if the grammar strangles the questions worth asking:** option C
  from the round-2 analysis, per-skill detail stays local and Corpus A publishes
  org-level distributions only. Recorded so it is not re-derived.

SHAs, urls and repo slugs stay IN Corpus A, decided separately above: the
read-path split is what closes the join, so hiding a join key GitHub already
publishes bought nothing and cost a correct cost-per-PR.

Plus identity, from `report/src/persona.rs:56` `whoami()`, which shells out to
`persona whoami --json`: `email`, `team`, `organization`, `department`.

**`whoami` must fail closed here.** Today it returns `None` on any failure and
report prints "persona whoami failed; rendering anonymously" (`persona.rs:91-92`).
Corpus A is keyed on email, so an anonymous row is a corrupt row: it inflates
counts and belongs to nobody. A Corpus A upload with no identity is refused, not
degraded.

**Subagent transcripts are in scope and are the larger half.** The census found
1,950 subagent files against 2,807 sessions. They carry the same prose hazards
and are attributed by `agentId`, so they ride the same redactor and the same
`pid`. A design that silently covers only parent transcripts would miss most of
the corpus.

Three constraints the schema must respect:

- **Ship raw counters, never derived ratios.** The aggregation invariant
  (`efficiency/src/metrics.rs:5-11`) is that raw counters union and `finalize`
  recomputes every ratio and percentile from the union. Uploading a
  pre-computed `cache_read_share` makes every org rollup wrong.
- **`lines_written` is not a diff stat** (`outcome.rs:76-79`). An Edit rewriting
  a 3-line block counts 3 written and 3 replaced. Do not label it churn.
- **Edited file paths are not available** (`outcome.rs:88-91` deliberately drops
  them, keeping only slug counts). Corpus A does not get them, and must not
  re-mine them.

**Excluded from Corpus A**, and this is a filtering step with a test obligation,
not a no-op. All four prose fields live on `SessionRecord`
(`sessions/src/model.rs:20-23`: `title`, `first_prompt`, `summary`, `tags`).
Three of them ride through into report's own structs, twice
(`report/src/report.rs:97-106` and `:212-220`: `title`, `summary`, `tags`).
`first_prompt` does **not**: it is not a field anywhere in `report/src`,
appearing only in a doc comment at `report.rs:216` and in test fixtures. So the
exclusion test must assert on `title`, `summary`, and `tags`, which are live
carriers, and treat `first_prompt` as a regression guard rather than a current
leak.

Observed on main:
```
$ grep -rn 'pub first_prompt' report/src/
NOT a field anywhere in report/src
$ grep -n 'pub title\|pub summary\|pub tags' report/src/report.rs
97:    pub title: Option<String>,
103:    pub summary: Option<String>,
106:    pub tags: Vec<String>,
212:    pub title: Option<String>,
218:    pub summary: Option<String>,
220:    pub tags: Vec<String>,
```

**Corpus B** object layout, one per session:

```
s3://<bucket>/<object-id>.env          <- flat. NO pid in the key.
  [u32 BE wrapped_key_len][wrapped_key][12-byte nonce][AEAD ciphertext]
  AAD covers: <object-id> || <key-id> || <algorithm> || schema_version
  plaintext payload begins with: {pid, session_uuid, redactor_version, ...}
```

`<object-id>` is minted fresh and random by the client. It is **never** the
session uuid, which marquee already publishes next to an author email
(`marquee/server/src/render.rs:381`, `:389`). The client keeps its own local
map from session to object id so it can avoid re-uploading; that map never
leaves the laptop.

**The `pid` lives inside the AEAD-protected payload. Not in the key, and not in
cleartext metadata.** An earlier draft had `s3://<bucket>/<pid>/<object-id>`,
which contradicted the POST Object design above: the whole point of a
`starts-with` policy is that the vendor never learns the key, and a `pid`-
prefixed key defeats it the moment any log line carries the key. Cleartext
object metadata is no better, because it is client-suppliable, so authenticating
it proves nothing about who wrote it.

Moving the `pid` inside the ciphertext stops one specific thing and not the
general one, and an earlier draft overclaimed here. It prevents **relocating an
existing ciphertext** under a different `pid`. It does **not** prevent authoring
new ones: the public key is published, so one employee can mint N AEAD keys, N
`pid`s and N object ids and produce N internally consistent envelopes that all
verify. That is the accepted floor-of-one under Security, not a new hole, and no
additional sybil control is being proposed for it. The query service reads the
`pid` after decryption, and it is the only reader, so nothing else needs it.

The AAD binding remains, over `<object-id>`, `<key-id>`, `<algorithm>` and
`schema_version`, and the reader compares the authenticated object id against
the object's actual key. That replaces valet's KMS encryption context, which
asymmetric decrypt does not offer.

Cleartext object metadata, deliberately minimal: `schema_version` and
`uploaded_date` (day resolution, not timestamp). No email, no host, no repo, no
branch, no session uuid, no `pid`.

### API Design

```
POST /presign       (vendor)   Authorization: Bearer <okta>
                    body: {object_count, manifest_digest}
                    -> 200 {policy, signature, fields, expires_at}
                    -> 401 bad/expired/wrong-aud token | 503 IdP outage, cold JWKS
                    NOTE: the response signs a POLICY, not a url. The policy
                    carries ["starts-with","$key",""] plus content-length-range,
                    so the vendor never learns which object is written. The
                    request carries no pid and no object id.

POST <bucket>       (S3 POST Object, multipart/form-data, no Tatari service
                     in the path). Client mints the key.
                    -> 204 | 403 expired or altered policy
                           | 400 EntityTooLarge / EntityTooSmall (off-range)
                    NOTE: content-length-range bounds the FILE content, not the
                    whole multipart body. There is no conditional-create here;
                    single-use comes from bucket versioning, see Architecture.

POST /corpus-a      (corpus A) Authorization: Bearer <okta>
                    body: the Corpus A record, identity from the verified token
                    -> 204 | 401 | 409 same (email, session, day) already written
                    NOTE: this endpoint is deliberately identity-bearing. Corpus
                    A is email-keyed by design, so it takes the ordinary
                    authenticated path and shares nothing with the Corpus B legs.

POST /ask           (query)    Authorization: Bearer <okta>
                    -> 200 {finding, cohort_size, query_id}
                    -> 422 cohort_size < 10 | 422 query targets an individual

GET  /queries       (query)    all-employee. the public query log.
GET  /me            (corpus A) all-employee. your own numbers plus percentiles.
GET  /corpus        (corpus A) all-employee. everyone's numbers.
```

The 401 vs 503 split on `/presign` is load-bearing and copied from
`valet/main/src/middleware.rs:1-13`: a bad token is a client re-login, an IdP
outage with a cold JWKS cache is a retry. They must not be collapsed.

**Idempotency.** The client mints the key, so a retry replays the same POST
under the same key. Bucket versioning makes the first version authoritative, so
"S3 wrote but the response was lost" recovers without a redemption ledger, an
orphan, or a garbage collector. One `/presign` call covers a batch, though note
the batch binding is a client-side convention: `object_count` and the manifest
digest have no server-side enforcement point under a `starts-with` policy. This also
removes the round-1 contradiction between batch tickets, a per-session 409, and
a "redeems exactly once" criterion, which could not all be true at once.

One unsolved case, and an earlier draft had its consequence backwards. Losing
the local session-to-object map while the `pid` **survives** produces duplicate
objects and duplicate analytical weight, but no additional distinct `pid`, so
the k=10 gate is unaffected. Only losing the `pid` as well inflates the distinct
count. Versioning does not help either way, because the re-upload uses a new
key. Same class as `pid` rotation, unclosed for the same reason.

### Implementation Plan

Deterministic and cheap first. LLM and expensive last. Corpus A and the
individual view ship before Corpus B exists, so the first thing the system ever
does for an engineer is tell them something useful about their own work.

#### Phase 0: Spikes. Zero code.
**Model:** sonnet

- **F1: KMS key policy denying decrypt to humans.** Sandbox account
  `767398024441` first, then dev `us-west-2`, never prod. **The round-2 version
  of this spike was defeated three ways, all verified against the repos, so the
  shape below is the third revision.**
  Statements, and the separation between them is the correction:
  - **Deny** on `kms:Decrypt`, `kms:ReEncryptFrom`, `kms:CreateGrant`,
    `kms:GenerateDataKey`, `kms:GenerateDataKeyWithoutPlaintext`,
    `kms:ScheduleKeyDeletion` and `kms:ReplicateKey`, with `ArnNotLike
    aws:PrincipalArn` naming **only the query-service role**.
  - A **separate** allow for `ci-terraform-plan` and `ci-terraform-apply`
    covering `kms:DescribeKey` and `kms:GetKeyPolicy` only. The round-2 draft
    exempted the CI roles from the decrypt deny itself, which made
    "query service only" false.
  - `kms:PutKeyPolicy` restricted to **one named break-glass role**, never to
    all humans. The round-2 draft put it in an all-human never-deny statement,
    which meant any holder could simply delete the Deny. That is the defect
    that made the whole control decorative.
  - `MultiRegion = false` stated explicitly on the key, so `kms:ReplicateKey`
    has nothing to replicate to.
  Three corrections to the reasoning, not just the policy:
  - **The root statement is NOT inert, and the round-2 doc said it was.** A KMS
    key-policy principal of `arn:aws:iam::<acct>:root` is the standard
    account-delegation idiom meaning "this account's IAM may grant these
    actions". It is not the root user. `scp-deny-root.tf` denies `["*"]` under
    `StringLike aws:PrincipalArn = arn:aws:iam::*:root`, and its own header
    comment says it "Denies every action for any principal authenticated as an
    account's **root user**". Verified by reading the file. So the SCP blocks
    root *sessions* and leaves the key-policy delegation intact, which is
    exactly what makes the `PutKeyPolicy` defect above reachable.
  - **The CMK lives in a member account**, and SCPs never apply to the
    management account. State that rather than implying org-wide coverage.
  - **The CloudTrail alarm has no event source today.** The org trail excludes
    KMS management events org-wide:
    `exclude_management_event_sources = ["kms.amazonaws.com", "rdsdata.amazonaws.com"]`
    at `terraform/compositions/centralized-logging/tatari-mgmt/cloudtrail.tf:11-14`
    and again in `modules/aws/cloudtrail/cloudtrail.tf:17`. Verified by reading
    both files. The `ReEncryptFrom` to `ReEncrypt` event-name fix from round 2
    was correct and insufficient: nothing KMS reaches that trail at all. So F1
    must stand up a KMS-capable trail for this account, and **prove the alarm
    fires end to end**, or the mitigation in Security is fiction.
  Count correction: four SCPs plus one RCP (`rcp-deny-sse-c.tf`), not five SCPs.
  The claim that no SCP covers KMS key policies still holds.
  **F1 PARTIAL RESULT, 2026-09-22, live: the alarm half is PROVEN BLOCKED.**
  The third sub-question, does the alarm actually fire, is answered no against
  deployed state rather than inferred from terraform. `get-event-selectors` on
  the only trail in the org returns
  `"ExcludeManagementEventSources": ["rdsdata.amazonaws.com", "kms.amazonaws.com"]`.
  KMS is excluded on the live org trail, so **no KMS event reaches it**, and the
  Security section's CloudTrail-alarm mitigation is fiction until a KMS-capable
  trail is stood up. That is now a measured fact, not a prediction. The other
  two sub-questions (the deny holds; no lockout as a named non-root role) still
  need a sandbox account and remain open.

- **F1b: asymmetric CMK round trip.** Client-side hybrid encrypt to a published
  public key, decrypt only as the query-service role, and confirm a blob whose
  AAD names a different `pid` fails authentication.
- **F2: does an unfenced HTTPRoute admit any Okta principal?** The round-1
  premise was wrong and needs re-testing, not re-probing. The gateway's jwt
  provider validates only issuer and audience `api://default`
  (`envoy-gateway/base/security-policy-oidc.yaml:23-28`), and the Okta default
  authorization server carries `client_whitelist ALL_CLIENTS`, `scope_whitelist
  *`, `group_whitelist EVERYONE`, and `client_credentials` among its grant types
  (`terraform-okta/composition/okta-authorization-server.tf:18`, `:28-36`). So
  any Okta client passes, **including a non-human service client**, and
  `all-humans` gates only the browser leg through an app assignment scoped to
  `contains(["apps-1"], each.key)` (`oauth2-proxy.tf:43-46`), not globally.
  Test the Bearer leg and a machine principal, not just a browser session.
- **F3: the S3 correlation surface, measured, and it is a dependency rather than
  a hole.** Committed terraform in platform-infra has zero
  `aws_s3_bucket_logging` resources and zero `logging{}` blocks, zero
  `event_selector`/`advanced_event_selector`/`data_resource`, zero `flow_log`,
  and zero `vpc_endpoint`. The only data-event control is the account-bootstrap
  flag, and it is `enable_data_events_trail = false` in all three compositions
  that declare it (`main.tf:25` in each). Sharpest fact: `tatari-tv`, the
  account where marquee and valet live and where these services land, has **no
  account-baseline composition at all**, so there the module is not even
  instantiated.
  Qualifications, stated rather than smoothed over: an enabled trail exists in
  the separate `tatari-tv/terraform` repo scoped to the tfstate replica bucket
  (unverified, repo not mounted for the review); nobody checked deployed AWS
  state, only committed terraform; and S3 server access log timestamps are
  **second** resolution, not millisecond, so even enabled they are a materially
  weaker join than the Envoy log was.
  So the three surfaces do not reconstruct the join today. But this design
  converts a default-**ON** leak (Envoy, which has no override anywhere) into a
  default-**OFF** leak (S3 data events, one boolean away in a module that
  already supports it). It belongs in the doc as a named dependency with an
  alarm, exactly the way `cleanupPeriodDays` is treated, not as a closed hole.
  **Honest framing matters here: "we never turn them on", not "we disable
  them."** S3 server access logging is opt-in and the legacy module defaults it
  off (`terraform/modules/aws/buckets/variables.tf:53-61`); only hulu and xandr
  enable it. The single enabled data-event trail is scoped to one bucket ARN,
  the tfstate replica. Nothing is being switched off for this design; it is
  riding a default that a future change can flip.
  Rejected: a VPC endpoint does not help (a gateway endpoint cannot serve a
  laptop over VPN, and an interface endpoint gives connectivity, not anonymity).
  CloudFront is worse: it adds a viewer-IP-plus-path log surface, and Tatari's
  existing distributions enable logging.
  What F3 must now measure, against live state rather than terraform: the
  bucket, trail and flow-log selectors actually deployed in the chosen account.

  **F3 RESULT, run 2026-09-22 against LIVE state in account 878256633362 as
  `SREAdmin`.** All three surfaces measured. Two confirm the terraform reading,
  one contradicts it.

  *Trails.* `describe-trails --include-shadow-trails` returns exactly **one**:
  `org-events`, an organization trail, multi-region, global service events on,
  log-file validation on, KMS-encrypted, home region us-west-2, owned by the
  logging account `768721000427`, writing to `tatari-cloudtrail-org-events`.

  *Data events: OFF, confirmed live.* `get-event-selectors` on that trail
  returns `"DataResources": []` with `ReadWriteType: All` and
  `IncludeManagementEvents: true`. So no S3 object-level event is captured
  anywhere in the org today. The terraform reading was right.

  *Flow logs: none.* `describe-flow-logs` in us-west-2 returns `[]`.

  *S3 server access logging: the doc was WRONG here.* The claim above that
  "only hulu and xandr enable it" came from committed terraform. Live state, a
  sweep of every bucket in the account with zero errors: **58 of 254 buckets
  (23%) have server access logging enabled**, 196 do not. Every one of the 58
  targets a central `tatari-logs-*` bucket, concentrated in
  `tatari-logs-prod-us-east-1` (19), `tatari-logs-staging-us-east-1` (9),
  `tatari-logs-dev-us-east-1` (6), `tatari-logs-staging-us-west-2` (5), with the
  rest spread across test, dev, us-west-1 and two `*-logs` siblings. So access
  logging is not the rare exception the terraform suggested: it is a standing
  convention applied to roughly a quarter of buckets, with an established
  central destination a new bucket would plausibly be pointed at by default.

  **Net effect on the design.** The join still does not reconstruct today: no
  data events, no flow logs, and second-resolution access logs even where they
  are on. The "default-OFF leak" framing survives for CloudTrail data events.
  It does **not** survive for server access logging: at 23% adoption with a
  house-standard target bucket, the design's bucket getting logging switched on
  by a future module default or a well-meaning copy-paste is a likelier path
  than the doc assumed. The named-dependency-plus-alarm treatment should cover
  `aws_s3_bucket_logging` on the design's own bucket explicitly, not just the
  data-event boolean.
- **F3b: S3 POST Object from hand-built Rust.** Does S3 accept a hand-built
  `["starts-with", "$key", ""]` policy without an SDK helper; does
  `content-length-range` reject an off-size body with 400 EntityTooLarge or
  EntityTooSmall; what exactly does the vendor log at issuance; and what is the
  vendor role's effective signing-credential ceiling under IRSA, which decides
  the M2 trilemma. Validate against AWS's published POST signature test vector.
  Include deliberate-tampering cases: a correctly signed but overly permissive
  policy authorizes uploads nobody intended, so the spike must show what an
  altered form field does rather than only that the happy path works.
  Dropped from this spike: the `If-None-Match` question, answered no from the
  primary source.
- **F4: CLOSED, no spike needed.** `hugo` already calls Bedrock from a pod via
  IRSA holding no key at all: `bedrock:InvokeModel` and
  `InvokeModelWithResponseStream` on `foundation-model/*` and
  `inference-profile/*`, plus `bedrock-agentcore:{CreateEvent,ListEvents,
  ListMemoryRecords}` on `memory/*`, at
  `platform-infra/accounts/tatari-tv/prod/us-east-1/hugo/irsa-custom.tf:13-45`.
  SigV4 off the pod role, no API key, no ExternalSecret, nothing to rotate. Dev
  and integration stacks exist too. **Copy the auth pattern, not the policy:**
  hugo's grant also carries AgentCore memory and X-Ray tracing scope that this
  design has not accounted for and does not need. Not verified: hugo is not
  checked out locally (`~/repos/tatari-tv/hugo` does not exist), so the
  terraform was read and none of its application code was.
- **F5: residual PII surviving `scrub`.** Needs no code.
  `clyde session enrich --dry-run --show-payload <dir>` already writes each
  post-`scrub` payload to a file (`sessions/src/enrich.rs:50-51`,
  `write_payload_dump` at `:410-418`, gated at `:258-270`). Count surviving
  emails, distinct `@tatari.tv` addresses, at-mentions, `/home|/Users` paths,
  40-hex SHAs, `eyJ`-prefixed JWTs, and `tatari.(dev|tv|tools)` urls.
  Two caveats the measurement must state: payloads are dumped only for sessions
  the scope gate cleared as work-scoped (`skipped_personal` counted separately at
  `:381-390`), which is the correct denominator; and `enrich` caps the body
  before scrubbing (`scrub(&capped)` at `:253`), so a full-transcript residual is
  larger than what this measures.

  **F5 RESULT, run 2026-09-22 on `desk.lan`: FAIL.** Command as specified:
  `clyde session enrich --all --dry-run --show-payload <dir>`, 37s, exit 0.

  Denominator, from the run's own JSON summary: `considered` 3620,
  `skipped-personal` 2000, `skipped-empty` 1, `would-enrich` **1619**. So 1619
  post-`scrub` payloads (64 MB) are the measured corpus, and `scrub` reported
  **487 redactions** total across them. Counts over that dir:

  | Class | Occurrences | Distinct |
  |---|---|---|
  | `@tatari.tv` addresses | 1534 | **182** |
  | `/home/<user>/` or `/Users/<user>/` paths | 6631 | 16 |
  | 40-hex SHAs | 919 | 334 |
  | `@handle` at-mentions | 608 | 79 |
  | `tatari.(dev\|tv\|tools)` urls | 1002 | 277 |
  | `eyJ`-prefixed JWTs | 2 | 2 |

  **The finding that changes the design: 182 distinct `@tatari.tv` addresses
  survive, not 1.** They appear in 286 of the 1619 payloads. The author's own
  address is 390 of the 1534 occurrences; the other 181 addresses are other
  people. 95 are in `firstname.lastname@` form; a 9-address sample of that form
  was checked against `persona person_lookup` and **9 of 9 resolved to real
  current employees**, including one outside Engineering entirely
  (Revenue/Services). The control fixture `ada.lovelace@tatari.tv` did not
  resolve, so the form split is a usable proxy: roughly 95 real colleagues, with
  the remaining 81 single-token addresses a mix of real short handles
  (`reno@`, `peter@`, `brian@`) and test fixtures (`alice@`, `a@`, `attacker@`,
  `admin@`).

  The `/home` and `/Users` paths carry the same problem at smaller scale: of 16
  distinct users, `saidler` is the author and `keegan`, `stephen`, `luke`,
  `noahvito` are other people, the rest being placeholders (`user`, `someone`,
  `me`, `x`, `runner`, `app`).

  **Consequence for the design.** Redaction was scoped as hygiene on the
  author's own data. It is not: the corpus carries third-party identities, so
  uploading it is a consent question about people who never ran `clyde`. The
  Security and Privacy sections and the Phase 5 zero-survivor criterion both
  rest on a premise this measurement falsifies. Note also the caveat above,
  `enrich` caps the body before scrubbing, so the full-transcript residual is
  strictly larger than these numbers.
- **F6: do managed settings honor a `hooks` block?** Must run outside the Bash
  sandbox, since both `/etc/claude-code/managed-settings.json` and its `.d` form
  are in this session's deny-list. If yes, the uploader can assume it is
  *called* without assuming it is *installed*, a much weaker premise than the
  design currently rests on.

  **F6 PARTIAL RESULT, run 2026-09-22: the premise was wrong, and the question
  has moved.** The deny-list is a *write* deny, not a read deny, so the probe
  ran. It returned `No such file or directory` for both
  `/etc/claude-code/managed-settings.json` and the `/etc/claude-code/` directory
  itself. **The enterprise file path does not exist on this machine.** Tatari's
  managed policy is nonetheless in force, so it arrives by another route:
  `~/.claude/remote-settings.json`, pushed from the Claude enterprise console.
  Its current payload carries `claudeMd`, `permissions`, `availableModels`,
  `enforceAvailableModels`, `model`, `autoUpdatesChannel` and
  `companyAnnouncements`, and **no `hooks` key**.

  What that settles and what it does not. It settles that the design must target
  the console-pushed remote-settings channel, not a file an admin drops on each
  box, which also means Linux coverage is no longer the Kandji/Intune gap: the
  console reaches every machine running Claude Code regardless of OS. It does
  **not** settle whether that channel honors `hooks`. Evidence for, short of a
  console test: `remote-settings.json` declares
  `$schema: json.schemastore.org/claude-code-settings.json`, and that schema
  (fetched 2026-09-22, HTTP 200, 142 properties) **does define `hooks`**, as
  "Lifecycle event hooks that run at configurable points during Claude Code
  operation". Schema validity is not runtime enforcement, so the remaining half
  of F6 is: push a no-op `hooks` block from the enterprise console and observe
  whether it fires. That needs console access, not a shell.

  Related, and it re-scopes three other spikes: the `~/.aws` and `~/.ssh` reads
  denied in this session are **Tatari's own managed policy**, not a sandbox
  quirk. `remote-settings.json` sets `permissions.deny` to exactly
  `["Read(~/.aws/**)", "Read(~/.ssh/**)"]` and
  `disableBypassPermissionsMode: "disable"`. F1, F1b, F3 and F3b therefore
  cannot be run by an agent under org policy at all, by design, and not merely
  because credentials are absent.
- **Needs a human, not a command:** F2's group half. Any Okta token held by the
  author carries `org-engineering`, so a 200 from an unfenced route does not
  distinguish the gateway default from the author's own membership. One person in
  `org-product` or `org-data-science` runs the same request. Membership is
  HR-derived and exists in no repo as a roster.
- **Success criteria:** F1, F1b, F2, F3, F5 and F6 each have a recorded command
  output in this doc, pass or fail. F1 additionally proves no lockout with
  `get-key-policy` and a re-`put-key-policy` **as a named non-root role**.

#### Phase 1: Corpus A extraction, prose excluded
**Model:** sonnet
- New `corpus-a` serializer in `report`, emitting only the fields named in Data
  Model plus `persona whoami` identity.
- Negative test that `title`, `summary`, `tags` cannot appear, plus a regression
  guard on `first_prompt`.
- **Success criteria:** a test round-trips a fixture session carrying non-empty
  `title`, `summary`, and `tags` and asserts all three are absent from the
  serialized artifact; `grep -c 'pub title\|pub summary\|pub tags'` against the
  Corpus A struct returns 0.

#### Phase 2: Corpus A service, read AND write
**Model:** opus
**Blocked by Phase 4**, which round 1 caught and the round-1 doc did not say:
this service verifies an Okta Bearer, so without the shared crate it becomes the
third hand-copy of the verifier.
- One new repo, one Cargo workspace, three binaries (Corpus A, vendor, query)
  with separate deployments and separate IAM roles. Repo separation is not a
  security boundary here; the role and the key policy are.
- marquee-shaped: S3 canonical with conditional writes
  (`marquee/core/src/storage.rs:20-33`), per-pod SQLite accelerator rebuilt from
  S3 on boot (`core/src/index.rs:1-8`), corpus snapshot on a background timer
  (`core/src/corpus.rs:11-15`).
- **`POST /corpus-a` write endpoint.** Round 1 omitted every Corpus A write
  path, which made the whole ship-order argument impossible: Phase 3's criterion
  assumed an uploaded session with nothing able to upload one.
- `catalog-info.yaml` must set `tatari.tv/release-strategy: "semver"` or every
  merge to main ships straight to prod ungated.
- Dedicated S3 bucket with its own lifecycle. Do **not** ride the shared
  datalake temp bucket the way marquee's IRSA does, and never attach
  `S3-tatari-datalake-temp-prod-us-east-1-RegularUser` (it carries
  `s3:DeleteObjectVersion`).
- **Success criteria:** `sdv probe` returns `/status /deployed /version`; a
  `POST /corpus-a` round-trips and is readable at `GET /corpus`; a replayed
  identical write returns 409 rather than double-counting.

#### Phase 3: Corpus A client and individual insights
**Model:** sonnet
- clyde gains the subcommand that posts the Phase 1 artifact to `/corpus-a`.
- `/me`: your counters, your percentiles against the org, the nudges.
- **Success criteria:** a clyde-side post lands and is visible at `/me`; a user
  with zero sessions gets an explanatory empty state, not a 500.

#### Phase 4: Shared Okta verifier crate
**Model:** opus
- The verifier is already forked once: `valet/main/src/auth.rs:1-13` says it
  forks `marquee_core::auth` and strips `dev_auth`. A third hand-copy is wrong.
- Extract to a shared crate. Carry the 401/503 split, `validate_nbf`
  (`auth.rs:132`), stale-if-unreachable JWKS (`auth/jwks.rs:1-9`), the refresh
  floor (`auth.rs:29`), and the readiness seam. Do **not** reintroduce
  `dev_auth`.
- **Home: `tatari-tv/okta-auth-rs`, converted to a workspace, with the verifier
  as a second crate beside the existing `okta-auth`.** That repo already owns
  the client half of this exact integration (PKCE browser login, token cache,
  transparent refresh, `okta-auth` 0.7.0, public, MIT) and is a sibling of
  `renew` and `mcp-io-rs`. One repo means one flat version covering both halves
  of the Okta story, and two crates rather than one means a pod does not link
  the client's `tiny_http` and `open` dependencies, which is the dependency
  objection that rules out depending on `marquee_core` or valet directly.
  Rejected: a fourth sibling repo, which buys separation the crate boundary
  already provides and adds a version to keep in step.
- Release order: publish the crate, adopt it in the telemetry services first,
  migrate valet and marquee after.
- **Blocks Phase 6 only.** Round 2 said it also blocked Phase 2; round 3
  corrected that, and the correction comes with a route: `marquee_core::auth` is
  already public (`marquee/core/src/lib.rs:10`), so Corpus A could depend on it
  rather than hand-copying. The catch is that marquee's verifier lacks
  `validate_nbf`, which valet added under audit (`valet/src/auth.rs:131`), so
  depending on marquee directly imports a weaker verifier. Order: publish the
  extracted crate, adopt it in the telemetry services first, migrate valet and
  marquee after.
- Extraction is deterministic work, so front-loading it does not violate the
  deterministic-before-LLM rule.
- **Success criteria:** valet's existing `auth/tests.rs` suite passes unmodified
  against the shared crate; `rg -c dev_auth` in the new crate returns 0.

#### Phase 5: Prose export chokepoint
**Model:** opus
A **separate** function, not an extension of `scrub`. Both panel seats and the
lead converged on this, and the reason is the contract: `session/src/redact.rs`
is 71 lines whose header says it is "not the trust boundary" and licenses
over-scrubbing precisely because its output "only feeds tag/summary inference,
never replaces the stored body" (`:1-12`). Corpus B's contract is the exact
inverse. One function cannot carry both.
- New `session/src/export.rs`, reusing the seven secret helpers from `redact.rs`
  rather than duplicating them. Invoked immediately before encryption, and the
  only path to a Corpus B payload.
- Twelve classes: absolute paths, hostnames, cwd, git remotes, branch names,
  commit SHAs, PR urls, Jira keys, Slack ids, email addresses, at-mentions,
  person names by NER. Home-path usernames are the sleeper: nothing in
  `redact.rs` touches them today.
- Subagent transcripts route through the same seam.
- **Weakest premise in the design, stated as such.** Both seats say plainly that
  regex plus NER is inadequate for free prose, and they are right. This phase
  reduces the residual; it does not eliminate it. The k=10 gate, the verbatim
  span cap, and the 30-day TTL are what stand behind it.
- **Success criteria**, and this is the third attempt at them because the first
  two were both bypassable. Zero survivors is necessary and nowhere near
  sufficient: a redactor emitting the constant string "Session processed." scores
  zero survivors in all twelve classes and is not an all-placeholder payload
  either. So:
  - **Labeled negatives.** A fixture corpus with known PII at known offsets, and
    the criterion is recall against that label set, not absence of matches.
  - **Preservation assertions.** Named spans that must SURVIVE intact (the
    skill name, the error class, the tool name), so an over-redactor fails.
  - **A coverage denominator.** Report survivors per class over occurrences per
    class, so a class with zero occurrences cannot be reported as passing.
  - **Fail-closed NER.** If the model is unavailable or errors, the export
    refuses rather than emitting unredacted prose.
  - **A constant-output mutation test.** Replace the body of `export` with a
    constant and assert the suite goes red.

#### Phase 6: Presign vendor
**Model:** opus
**Blocked by Phase 4.**
- Okta verify via the Phase 4 crate. Signs an S3 POST Object policy carrying
  `["starts-with","$key",""]` plus `content-length-range`, one call per batch,
  bound to the manifest digest. Hand-built policy and signature: `aws-sdk-s3`
  has no presigned-POST helper in Rust.
- Holds no KMS grant of any kind. Receives no `pid`, no object key, and no
  object content. It cannot know which object a caller writes.
- Stores no association between identity and anything it issues.
- **Success criteria:** the vendor's IAM policy contains no `kms:*` action; the
  vendor's request schema and audit schema have no `pid` field and no object-key
  field; the audit log carries no timestamp finer than hour resolution.

#### Phase 7: `upload` crate
**Model:** opus
- New workspace member, added to `members` at `Cargo.toml:2`. Depends on
  `common` and `session`. Must not depend on `sessions` or `report`, which
  inverts the existing layering. `ureq` is already a workspace dep
  (`Cargo.toml:37`).
- Client-generated `pid`, 256 bits, stored `0600`. Client-generated AEAD key
  encrypted to the **baked-in** public key, which ships with its encrypt-until
  date; past that date the client refuses to upload and names the reason. Fresh random object ids with a
  local-only session-to-object map. Randomized multi-hour upload delay, bounded
  by the policy's expiry, which temporary signing credentials cap. Size padding
  enforced server-side by the policy's `content-length-range`. Retry queue
  replaying the same POST under the same key, with bucket versioning making the
  first version authoritative.
- Corpus B backfills only inside the 30-day window; older sessions would expire
  on arrival.
- One `Command` variant in `clyde/src/cli.rs`. Opt-in flag, off by default.
- **Success criteria:** `clyde upload --dry-run` prints the object layout and
  uploads nothing; a captured outbound request carries no email and no session
  uuid; two runs over the same session produce the same object id and the
  earliest stored version stays authoritative. "Same object count" alone is not
  enough: it masks an overwrite.

#### Phase 8: Corpus B bucket and lifecycle
**Model:** sonnet
No ingest service exists, so this phase is infrastructure only.
- Dedicated bucket. Versioning **on**, with `NoncurrentVersionExpiration` at 30
  days so an overwrite cannot destroy a prior ciphertext and an expired object
  still leaves no lingering copy. Readers take the earliest version per key.
- Whatever F3 decided about S3 server access logs and CloudTrail data events.
- **Success criteria:** an object older than 30 days is absent and has no
  noncurrent version; a second POST to an existing key leaves the first version
  authoritative; the bucket's logging configuration matches F3's recorded
  decision.

#### Phase 9: Query service
**Model:** opus
- LLM over Corpus B. k=10 cohort gate. Individual-targeting refusal. Verbatim
  span cap. Public query log.
- **A closed, server-enforced predicate grammar.** Free-text questions are
  parsed into a finite set of allowed predicate shapes, and anything that does
  not parse is rejected rather than passed to the model. Every Corpus A field
  carries an explicit classification saying whether it may appear as a
  predicate.
- Bedrock via IRSA, copying `hugo`'s auth pattern and not its policy.
- **Success criteria:** a query whose matching set is 9 `pid`s returns 422 and
  writes a refusal to the public log; a query naming a person returns 422; a
  query expressing a predicate outside the grammar returns 422 without the
  model being invoked; the per-field classification table has an entry for every
  field the Corpus A serializer emits, asserted by a test that fails when a new
  field is added without one.

#### Phase 10: Governance and enrollment
**Model:** sonnet
- Data-use policy co-signed by Security, Legal, and HR.
- Review board: two elected ICs with veto over new query classes.
- Quarterly public report: queries run, findings produced, refusals issued.
- **Provenance, marked because a reviewer could not establish it from the
  history:** the elected-IC veto board and the quarterly report were proposed
  during this design's discussion as answers to the stated Big Brother
  objection, and adopted when the doc was commissioned. They are not necessary
  consequences of telemetry, and a future reader should treat them as a chosen
  governance posture that can be changed on its own terms.
- `claudeMd` enrollment line in `claude-enterprise-config/settings.json`.
- Public participation counter.
- **Success criteria:** the policy document exists and is linked from `/queries`;
  the participation counter renders.

## Acceptance Criteria

- [ ] No component, log, or table anywhere in the system holds both an email and
      a `pid`. Verified by grepping the vendor's request schema and audit schema
      for a `pid` field, and the Corpus B object metadata for an identity field.
- [ ] The presign vendor's IAM policy grants no `kms:*` action at all, and the
      query-service role is the only principal the CMK's key policy permits to
      decrypt.
- [ ] A query resolving to fewer than 10 distinct `pid`s returns 422 and appears
      in the public query log as a refusal.
- [ ] The Phase 5 export function leaves zero survivors in all twelve redaction
      classes across the work-scoped corpus, and rejects a payload consisting
      only of placeholders.
- [ ] Corpus A's serialized artifact contains none of `title`, `summary`,
      `tags`, given a fixture session where all three are non-empty, while
      commit SHAs, PR urls and repo slugs survive so cross-session dedup keeps
      working.
- [ ] `GET /corpus` emits no identity field for any caller, while `GET /me`
      emits skill-keyed detail only for the calling identity, asserted by a test
      that requests another identity's detail and gets a refusal.

Observed on main, for the criteria that can run today:

- The email-and-`pid` criterion cannot run: no service exists. Phase 6
  introduces the vendor schema it asserts against.
- The IAM and key-policy criterion cannot run: neither exists. Phase 0 F1 proves
  the key policy is achievable before any of it is built.
- The k=10 criterion cannot run: no query service exists. Phase 9.
- The export criterion has a runnable baseline today via Phase 0 F5, using
  `clyde session enrich --dry-run --show-payload`, so the gap is measured before
  Phase 5 changes anything.
- The Corpus A criterion's premise is verified above under Data Model for the
  prose fields, and under Data Model's linkage rule for the outcome fields:
  `commits` is a `Vec<String>` of SHAs and `prs` a `Vec<PrRef>` carrying urls
  (`efficiency/src/outcome.rs:54-55`, `:98-103`).

## Resolved Decisions

- **2026-09-21, k = 10** for the cohort gate. Scott.
- **2026-09-21, prose opt-in is per-user once**, not per-session. Scott.
- **2026-09-21, Corpus A is read by `all-humans`, achieved by attaching no
  route-level `SecurityPolicy`.** Closed by research, superseding an
  `org-engineering` versus new-group question. `org-engineering` is HR-derived
  from `organization == "Engineering"`
  (`terraform-okta/composition/group-rules.tf:56-69`) and excludes
  `org-product` and `org-data-science`. "R&D" is not a group that exists. The
  `oauth2-ingress-gateway` default admits any authenticated Okta user, and every
  observability backend opts out of that default; marquee rides it
  (`gitops-infra/prometheus/README.md:24-26`).
- **2026-09-21, "pinned by SCP" is withdrawn.** Five SCPs exist, none touch KMS,
  and management-account stacks are local-apply-only by a human with
  `OrgManagementAdmin` (`platform-infra/AGENTS.md:80-87`). Recorded as a named
  gap under Security, not as a control.
- **2026-09-21, client-side envelope encryption** replaces lifting valet's
  `KmsCipher`. Reason in Architecture.
- **2026-09-22, the Phase 4 verifier crate lives in `tatari-tv/okta-auth-rs`**,
  converted to a workspace, as a second crate beside `okta-auth`. Closed by the
  author rather than escalated: that repo already owns the client half of the
  same integration, one repo keeps one flat version across both halves, and the
  crate boundary keeps `tiny_http` and `open` out of a pod. Rejected: a fourth
  sibling repo.
- **2026-09-22, the public key is baked into the clyde binary.** Scott's call
  over a signed key manifest and over any runtime fetch. Consequences accepted
  with it: rotating the key requires a clyde release; the retirement rule is
  fixed as "stop encrypting with key N when N+1 ships, stop decrypting with N
  thirty days later" so it lines up with the prose TTL and needs no judgment;
  and the baked-in material is the key plus its encrypt-until date, so a stale
  client fails closed instead of uploading blobs nobody can decrypt. The
  envelope carries key id and algorithm in the clear for reader selection, both
  covered by AAD, versioned from the first release.
- **2026-09-22, commit SHAs, PR urls and repo slugs STAY in Corpus A**,
  reversing an interim reduction to counts. Scott's call over per-session counts
  (breaks global cross-session dedup, making org cost-per-PR wrong by the
  sessions-per-PR factor), keyed HMAC tokens (a second key to custody, to hide a
  join key GitHub already publishes), and client-side dedup (partial: per
  machine, so a PR touched from desk and lappy counts twice, which is the
  multi-host case `report/src/outcome.rs:49-53` exists for). Justification: a
  SHA is a join key rather than a secret, it is already visible with authorship
  to `tatari-tv` org members, and the join it enabled is closed structurally by
  the read-path split rather than by hiding it. Accepted cost: Corpus A is
  readable by every employee while GitHub is engineering-wide, so this publishes
  those references slightly wider, in aggregate only, with no identity on the
  public path.
- **2026-09-22, the `by_skill` linkage closes as D plus B: store everything,
  split the read paths, close the question grammar.** Scott's call over options
  A (drop the fields) and C (keep detail local only). Corpus A stores per-email
  skill, MCP-tool and model keys unchanged; `/me` is identity-scoped and serves
  full detail; `/corpus` is email-stripped so no read surface answers "which
  emails ran graphify"; and Phase 9 enforces a closed predicate grammar, which
  is required independently because a prose rule cannot be enforced against
  paraphrase. Residual left open on purpose: the grammar does not close the
  output channel, since a finding that describes a subgroup still narrows it.
  Option C is the recorded fallback if the grammar strangles useful questions.
- **2026-09-22, S3 POST Object replaces presigned PUT**, and blind-signature
  `pid` attestation is rejected. Both from panel round 2; reasoning in
  Architecture and Alternatives.
- **2026-09-21, the doc lands in clyde.** The service repo does not exist yet,
  and Phases 1, 3, 5, and 7 are clyde-side. Blast radius below.

Closed by panel round 1, 2026-09-22, both seats converging unless noted:

- **No ingest service. Presigned S3 PUT instead.** The platform injects
  `x-auth-request-email` into every upstream request
  (`gitops-infra/istio-istiod/values.yaml:33`, plus the ext_authz allow-list in
  all five overlays at `overlays/*/patch-configmap.yaml:46`), so any service
  behind the standard ingress receives the uploader's email alongside the `pid`.
  Removing the service also removes the Envoy access log, which is on by default
  with no override anywhere in `envoy-gateway/` and carries millisecond
  timestamps and the request path into Loki.
- **Asymmetric hybrid encryption, not `kms:GenerateDataKey`.** The round-1
  vendor held the verified identity and the `pid` in-process, which is a
  co-location rather than a correlation. Adopting the panel's distinction: "we
  do not persist the association" is not "the issuer cannot establish it."
- **Corpus B object ids are minted fresh, never the session uuid.** marquee
  already renders author email and session id as adjacent rows on an
  all-employee page (`marquee/server/src/render.rs:381`, `:389`).
- **Corpus A drops SHAs, PR urls, and repo slugs to counts**, and adopts the
  linkage rule under Data Model. The round-1 doc described `commits` and `prs`
  as counters; they are a `Vec<String>` and a `Vec<PrRef>`
  (`efficiency/src/outcome.rs:54-55`, `:98-103`).
- **One repo, one Cargo workspace, three binaries**, separate deployments and
  separate IAM roles. Repo separation is not a security boundary; the role and
  the key policy are.
- **Idempotent retry against one logical redemption**, bound to the batch
  manifest digest, rather than fresh-ticket-only with orphan collection.
- **Design is keyed as `team-product-design`, not an `organization` value.**
  There is no Design organization: the authored list is Business Solutions, Data
  Science, Engineering, Finance, Finance and Legal, Legal, Marketing, People
  Operations, Product, Revenue (`terraform-okta/composition/locals.tf:50-61`).
  Design appears only as a team: Product Design (`:132`) and Graphic Design
  (`:110`), which become `team-product-design` and `team-graphic-design` per the
  name rule at `groups.tf:55`. This replaces a round-1 open question that
  assumed terraform could not answer it.
- **Phase 4 blocks Phase 2 as well as Phase 6.** The Corpus A service verifies a
  Bearer, so without the shared crate it becomes the third hand-copy.
- **F1 corrections**: `kms:ReEncryptFrom` and `kms:CreateGrant` join the deny
  list and the CloudTrail alarm, since both reach plaintext without calling
  `Decrypt`; `kms:ScheduleKeyDeletion` leaves the all-human allow so no engineer
  can destroy Corpus B; and the root-allow statement is inert because
  `scp-deny-root.tf:14-28` denies `["*"]` to `arn:aws:iam::*:root` org-wide, so
  the no-lockout proof runs as a named non-root role. Also a count correction:
  four SCPs plus one RCP (`rcp-deny-sse-c.tf`), not five SCPs. The claim that no
  SCP covers KMS key policies holds.
- **Phase 5 is a separate chokepoint, not an extension of `scrub`**, and regex
  plus NER is recorded as the design's weakest premise rather than as adequate.

## Alternatives Considered

### Hashed email, or email plus MAC address
- **Description:** `pid = SHA256(salt + email)`, or with a MAC mixed in.
- **Pros:** deterministic, nothing for the user to keep.
- **Cons:** the adversary holds the input set. 500 employees is 500 hashes. MAC
  addresses are inventoried by Kandji and Intune
  (`security-operations/security_operations/kandji_helper.py:29`), by DHCP, and
  by the WiFi controller; the vendor OUI collapses the top 24 bits and fleet
  purchasing clusters the rest; and MAC randomization is default-on and
  per-SSID, so the value is not even stable.
- **Why not chosen:** hashing hides an input only when the adversary cannot
  enumerate it. Both inputs are enumerable, so the pseudonym is reversible.

### Aggregates only, prose never leaves the laptop
- **Description:** client computes counters, uploads tallies, no prose anywhere.
- **Pros:** strongest possible guarantee, least code, mostly already built.
- **Cons:** cannot answer why a session went badly or what someone was trying to
  do.
- **Why not chosen:** prose-derived insight is the goal, not a nice-to-have.
  This alternative is Corpus A alone, which this design ships as Phases 1
  through 3 and then goes further.

### Local LLM extraction, upload only the verdict
- **Description:** the laptop runs the judge and ships structured findings from a
  closed vocabulary.
- **Pros:** prose never leaves; a new extractor re-runs locally against data
  still on disk.
- **Cons:** the taxonomy must be right up front; no new question can be asked of
  the corpus, only of future sessions; a local model or a metered call per
  session.
- **Why not chosen:** it trades away the org-scale LLM judge that is the
  demonstrated win in the prior art. Kept in the Addendum as the fallback if
  Phase 0 F1 fails.

### Central prose corpus with access controls
- **Description:** the Intercom shape. Ship everything, gate reads with IAM and
  policy.
- **Pros:** simplest, and demonstrably works at another company.
- **Cons:** access control is policy. It does not survive a reorg, a subpoena,
  or an HR exception, and it cannot be defended to a skeptical engineer.
- **Why not chosen:** it fails the one goal that motivated the design.

### Blind-signature `pid` attestation
- **Description:** the client blinds its `pid`, the vendor issues exactly one
  blind signature per identity, the client unblinds it and presents it with the
  upload. Vendor never sees a `pid`; verifier never sees an identity. k=10 would
  then mean ten humans.
- **Pros:** the only proposal on the table that raises the distinct-human floor
  above one.
- **Cons, and the first one is dispositive:**
  - It fails this doc's own auditability objection verbatim. The sentence used
    to reject OPRF below ("a service to build and rotate, and nobody in the
    building will audit it") applies unmodified. The derive-versus-rate-limit
    distinction is fair about purpose and irrelevant to the objection that
    actually killed OPRF.
  - `kms:Sign` exposes standard padded algorithms, not RFC 9474's raw blinded
    private-key operation, so the signing key must be **self-custodied** in a
    design whose whole premise is zero key custody outside KMS.
  - A compromised issuer mints enough credentials to defeat the floor without
    decrypting anything.
  - `blind-rsa-signatures` (RFC 9474) has no audit evidence, and
    `grep -c blind-rsa-signatures Cargo.lock` returns 0 in clyde, which has no
    crypto dependency at all today.
  - Recovery has no good answer. A laptop reimage is the common case, not the
    tail: blindly issuing a second credential lets the old and new `pid`s
    coexist and both get counted, and the vendor cannot revoke a `pid` it never
    saw.
  - It does not deliver the guarantee anyway. A visible `(pid, signature)` pair
    is copyable, so without proof of possession the result is distinct enrolled
    credentials, not ten independent authors.
- **Why not chosen:** killed by both review seats against this doc's own
  standard. The replacement is not another mechanism, it is stating the floor of
  one plainly, which Security now does.

### OPRF / blind pseudonym derivation
- **Description:** RFC 9497 VOPRF so the key holder never learns the identity.
- **Pros:** determinism without trusting a key holder.
- **Cons:** a service to build and rotate, and nobody in the building will audit
  it.
- **Why not chosen:** over-engineered for the threat model, and it solves the
  pseudonym problem, which blind upload already solves more cheaply.

### KMS-held deterministic pseudonym
- **Description:** `pid = HMAC(K, email)` with `K` in KMS.
- **Pros:** deterministic, no client secret to lose.
- **Cons:** leaves a standing re-identification path, one audited API call away.
- **Why not chosen:** the guarantee degrades to key-policy hygiene, which Phase
  0 F1 has not yet proven is even durable here.

## Technical Considerations

### Dependencies

- **clyde:** `ureq = "3.3.0"` already a workspace dep at `Cargo.toml:37`, so the
  HTTP client is not a new workspace dependency, only a new crate-level one. An
  AEAD crate is new and needs a local declaration with a reason comment.
- **`renew`** already pinned at tag v0.3.3 and surfaced as `clyde update`
  (`clyde/src/cli.rs:82-83`). The uploader inherits it.
- **Bedrock precedent:** `hugo` already invokes Bedrock from a pod on SigV4 off
  its IRSA role with no key to rotate
  (`platform-infra/accounts/tatari-tv/prod/us-east-1/hugo/irsa-custom.tf:13-45`).
  The query service copies that rather than holding an API key. Counter-example
  worth naming so nobody copies the wrong one: the Python products do hold keys
  (`agent-platform/agent_platform/utils/openai_factory.py`,
  `creative-analysis-service/tests/unit/test_env_key_preflight.py`). And
  `terraform-okta/composition/anthropic.tf` is an Okta app assignment for Claude
  Code seats, not a service credential.
- **Client holds no credential either**, and the design preserves that:
  `clyde/common/src/llm.rs` routes every model call through `trait Transport`
  (`:23-25`) with `CliTransport` shelling out to the `claude` CLI, so the module
  can state that clyde "reads, stores, and transmits no credential" (`:6-7`).
- **Cross-repo blast radius:** clyde (client), one new service repo (three
  binaries in one workspace), platform-infra (S3, asymmetric CMK, key policy,
  IRSA), gitops-infra (HTTPRoute with no `SecurityPolicy`), valet and marquee
  (Phase 4 verifier migration), claude-enterprise-config (`claudeMd` enrollment
  line), and terraform-okta if the Corpus A audience needs a group that does not
  exist yet.
- **Ship order forced:** Phase 0 -> Phase 4 (valet and marquee migrate, and it
  blocks both services) -> Phases 1 through 3 (clyde plus Corpus A, no prose
  anywhere) -> Phases 5 through 8 (prose path) -> Phase 9 -> Phase 10. Note the
  change from round 1: Phase 4 moved ahead of Phase 2, because the Corpus A
  service verifies a Bearer and would otherwise become the third hand-copy of
  the verifier.
- **IAM hard constraint:** every grant is a separate `aws_iam_policy` plus
  `aws_iam_role_policy_attachment` in a hand-authored `*-custom.tf`, never
  `inline_policies`. The `irsa` mixin regenerates `irsa.tf` wholesale, and bot
  PR #2424 silently stripped valet's dynamodb and kms policies on 2026-07-16,
  breaking every token vend
  (`platform-infra/accounts/tatari-tv/prod/us-east-1/valet/dynamodb-custom.tf:56-68`).

### Performance

- Upload is batch, on dormant sessions. Corpus measurements: 2,807 sessions,
  1,950 subagent files, 3.0 GB over Jun to Sep 2026 on one machine.
- Prose is under 5% of bytes (human prompts 2%, assistant text 2%). Tool output
  plus its structured duplicate is ~40% and harness injections 17%, so Corpus B
  should carry prose and drop the rest rather than shipping whole transcripts.
- Assistant lines duplicate `usage` per message id. Dedupe or overcount 2 to 5x.
- Schema drift is live: newer lines carry snake_case `session_id` beside
  `sessionId`.
- **Backfill is asymmetric between the corpora.** Corpus A wants history and
  should backfill everything on disk. Corpus B's raw prose TTLs at 30 days, so
  backfilling a session from four months ago uploads bytes that expire almost
  immediately. Corpus B backfills only inside the retention window, and Phase 7
  should not pretend otherwise.
- **The participation counter is a vendor-side number.** Counting humans
  requires identity, which only the presign vendor has. Counting distinct
  `pid`s in the bucket counts machines, and with a client-minted `pid` it does
  not even count those reliably. The public counter reads from the vendor.
- The upload window is 90 days, set centrally by `cleanupPeriodDays` in
  `claude-enterprise-config/settings.json`. Someone can shorten it without
  telling us.

### Security

**Blast-radius ladder.**

| Access | Yield |
|---|---|
| Corpus A, `/corpus` | org-level aggregates, no identity column. |
| Corpus A, `/me` | your own numbers, skills, commits and PRs. Yours already. |
| Corpus A stored records | email beside skills, commits and PRs. Commits and PRs are already in GitHub with authorship; the skill map is the part the read-path split keeps off any public page. |
| Corpus B ciphertext | nothing. |
| Corpus B decrypted | redacted prose, `pid`-keyed, no identity. |
| Corpus A's public view joined to a Corpus B **finding** | nothing: `/corpus` carries no identity column, so there is no membership function to join. |
| Corpus A's `/me` view joined to a finding | your own membership only, which you already know. |
| Corpus A joined to Corpus B **objects** | nothing. No email-to-`pid` row exists to steal. |
| Vendor log alone | identities that asked to upload. No `pid`, no content, no key material. |
| S3 access logs plus CloudTrail | nothing today: all selectors are off and the account has no baseline composition. A named dependency, not a closed hole. F3 |
| Query service | findings only, k=10, every query in the public log. |
| Full AWS admin, bypassing the service | everything. |

**The round-1 ladder was wrong on one row and it mattered.** It asserted "Corpus
A joined to Corpus B: still nothing." That holds for the objects and fails for
the findings. Corpus A is email-keyed and all-employee, so it publishes the
membership function for any cohort predicate it can express. A finding about
"10 `pid`s that used graphify" is anonymous exactly until Corpus A is read to
list which emails use graphify. The linkage rule under Data Model is what closes
it, and the `by_skill` half of that rule is still open.

**Stated, not hidden.** That last row is the ceiling, and no mechanism closes
it. An AWS admin who rewrites the key policy and reads S3 directly defeats this
design. Three things bound it and none of them are a guarantee: CloudTrail
alarms on `kms:Decrypt` outside the query service role, posted to a public
channel; two-person approval on key-policy changes; and code review on
platform-infra. There is no SCP covering KMS key policies, and valet's own CMK
carries no key policy at all today (default root `kms:*` delegating to IAM), so
the durability of the Phase 0 F1 control rests on review alone.

Second residual risk, inherited from valet's own disclosure
(`valet/main/docs/design/2026-07-02-valet.md:556`): code execution inside the
query-service pod collapses the layers at the app boundary, because that one
role holds `kms:Decrypt` and corpus read.

**Four attacks the k=10 gate does not stop on its own.**

- **Differencing.** Ten overlapping queries whose result sets differ by one
  person reconstruct that person. The gate is per-query; the attack is across
  queries. Mitigations: a per-asker query budget, cumulative-disclosure
  accounting per cohort, and the public query log, which makes the pattern of
  ten suspiciously-adjacent questions visible to everyone. Not eliminated.
- **Prompt injection from the corpus.** Corpus B is user-written prose, and the
  query service's LLM reads it. Someone can type an instruction into their own
  session aimed at the future reader of the corpus. The corpus is data, never
  instructions: the query LLM gets structured output only, no tool access, no
  network, and no ability to emit anything but a finding plus a cohort count.
- **The LLM is itself a re-identification vector.** Asked the right way, a model
  reading prose can characterize an author. The individual-targeting refusal is
  a policy control running inside a machine, which makes it the weakest link in
  an otherwise structural design. Stated plainly rather than papered over: this
  is the one place the guarantee depends on a classifier behaving.
- **`pid` counts are not human counts, and the enforceable floor is one.** A
  `pid` is client-generated and unverified, so nothing stops one client minting
  many. k=10 therefore means ten **pseudonyms**, and the distinct-human floor is
  **1**. Raising k buys nothing against a client that mints `pid`s.
  The honest disposition, and it is weaker than a structural guarantee: **k=10
  is a sybil-vulnerable control with the public query log as an accountability
  backstop, not a structural fix.** `rm -rf` the `pid` file ten times and the
  gate opens for a cohort of one human. `pid` rotation against k=10 is a named,
  unclosed gap.
  Blind-signature `pid` attestation was considered and killed; see Alternatives.
  What is bought instead, cheaply: **per-identity upload rate limiting at the
  vendor**, which already holds the identity and needs no new mechanism. It
  bounds sybil creation without pretending to prevent it. And the user-facing
  wording changes from "10 people" to **"10 distinct clients"**, because that is
  what the gate actually counts.
- **Corpus A was the fifth attack**, and the read-path split plus the closed
  predicate grammar is what answers it. Its public projection carries no
  identity column, so a Corpus B finding has no membership function to join
  against. What remains is the output channel below.
- **Restricting query predicates does not close the output channel.** Even with
  a closed grammar, a finding that *describes* a subgroup recreates the
  disclosure through its own prose. The span cap bounds it; nothing eliminates
  it.

**Other controls.** Identity is verified, never trusted. valet re-verifies the
Bearer itself (`valet/main/src/auth.rs:110-141`) while explicitly refusing to
trust the edge (`auth.rs:1-13`), and both new services do the same. Note the
correction: the claim that no repo at Tatari consumes `X-Auth-Request-Email` is
false, `org-metrics/config/middleware/oauth2_proxy_backend.py:8` does exactly
that. The conclusion stands on its own evidence, which is that the platform
injects the header (`gitops-infra/istio-istiod/values.yaml:33`), so a service
that trusts it inherits whatever the edge asserts.

**"All-employee" is broader than it sounds.** The gateway's jwt provider
validates only issuer and audience (`security-policy-oidc.yaml:23-28`), and the
Okta default authorization server carries `group_whitelist EVERYONE`,
`client_whitelist ALL_CLIENTS`, and `client_credentials`
(`terraform-okta/composition/okta-authorization-server.tf:18`, `:28-36`). So an
unfenced route admits any Okta principal, including a non-human service client,
not merely every employee. For a corpus keyed by email in the clear that is a
wider audience than the symmetry argument assumes, and F2 must test it.

### Testing Strategy

- `otto ci` gates a new clyde member: `lint` (whitespace, no `_`-prefixed
  bindings, no em-dash in `*.rs` or `*.pmt`, using `grep` because ripgrep is not
  installed on the CI runner), `bloat` at 1500 lines per file, `check`
  (`--workspace --all-targets --all-features` plus a separate non-`fetch` build
  of `claude-pricing`), `test`.
- `cargo-mutants` threshold is zero unannotated survivors, and `--workspace` is
  required or it silently scopes to `clyde` alone.
- Tests as `#[cfg(test)] mod tests;` with a sibling `<mod>/tests.rs`. No
  `tests/` dir for new crates.
- Negative tests are the point here: prose-field exclusion, absence of
  `kms:Decrypt`, the k=10 refusal, ticket single-use. Break each to prove it
  fails.

### Rollout Plan

- **No push channel for a binary exists at Tatari.** Managed settings ship
  settings and markdown only. `tatari-skills` is opt-in per plugin and
  markdown-only. Every binary path is pull: an auth-gated install script plus
  `renew` self-update. Kandji is live and manages Apple, Intune manages Windows,
  and Linux is managed by neither.
- So participation is voluntary by construction, not by promise. That is a
  stronger sentence than any policy commitment, and the doc should say it.
- Enrollment rides `clyde update`. `claudeMd` carries the instruction, which
  reaches the agent on every laptop; `companyAnnouncements` carries the
  human-facing notice.
- Whether Kandji can push a binary is console state, unexercised in any repo,
  and needs IT sign-off. Treated as a dependency with a named owner, not an
  assumption.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Phase 0 F1 fails: no durable key policy denying humans | Med | High | Fall back to the local-extraction alternative in the Addendum. Corpus A and Phases 1-3 ship regardless |
| `renew` enrollment silently no-ops without GitHub auth | High | Med | Fail loudly with that diagnosis, per persona-cli's documented two-step |
| A stale client encrypts to a retired public key, producing unreadable blobs | Med | Med | The baked-in key ships with an encrypt-until date and the client fails closed past it. Depends on `renew` working, which is the row above |
| A future engineer rewrites the key policy | Med | High | CloudTrail alarm to a public channel plus two-person approval. Named gap, not closed |
| Prose redactor leaves PII | **Confirmed** | **High** | **F5 ran 2026-09-22 and the redactor DOES leave PII: 182 distinct `@tatari.tv` addresses across 1619 payloads, 9 of 9 sampled resolving to current employees.** No longer a likelihood, it is a measured fact. Phase 5's zero-survivor criterion now has a concrete 182-address gap to close, and the residual is third-party identity, not just the author's |
| Stylometry re-identifies an author from redacted prose | Med | Med | k=10 gate plus the verbatim-span cap. Not eliminated: state it |
| `cleanupPeriodDays` shortened centrally, shrinking the upload window | Low | Med | Alarm on the value; treat 90 days as a dependency, not a constant |
| S3 access logs and CloudTrail data events rejoin identity to content by egress IP and millisecond timestamp | Low **today**, Med **drift** | High | **Measured 2026-09-22, live.** CloudTrail data events are off org-wide (`DataResources: []` on the sole trail) and there are no flow logs, so the join does not reconstruct today. But server access logging is on for **58 of 254 buckets** with a house-standard `tatari-logs-*` target, so this is a drift risk, not an absent one. Alarm on `aws_s3_bucket_logging` for the design's bucket, not only on the data-event boolean |
| The platform re-injects identity into any future service added to the upload path | Med | High | There IS no service on the upload path, by construction. Any later proposal to add one re-opens `values.yaml:33` and must be refused |
| Differencing attack: overlapping queries reconstruct one person | Med | High | Per-asker query budget, cumulative-disclosure accounting, public query log makes the pattern visible. Not eliminated |
| Prompt injection from corpus prose against the query LLM | Med | High | Corpus is data, never instructions: structured output only, no tool access, no network from the query LLM |
| Low opt-in makes findings unrepresentative | High | Med | Publish the participation counter and the cohort size on every finding |
| Corpus A percentiles identify the outlier at the bottom | Med | Med | Scanlan hit this too ("someone has to be down the bottom of every percentile"). Bucket, do not rank, and never surface a named bottom |

## Open Questions

Panel round 1 closed four of the round-1 questions and they moved to Resolved
Decisions. Rounds 2 and 3 opened three more, and Scott closed all three on
2026-09-22. F5 and F3 both ran on 2026-09-22 and their results are recorded,
and F5's failure added one. **Three remain.** Two are evidence somebody has to
go get; the third, opened by F5, is a consent question that no command can
answer. The doc is not ready to build until all three close.

- [x] **F5 results: recorded 2026-09-22, FAIL.** 182 distinct `@tatari.tv`
      addresses survive `scrub` across 1619 payloads. See Phase 0 F5. This
      closes the F5 half of the criterion (a recorded output, pass or fail) and
      opens the consent question below.
- [x] **F3 results: recorded 2026-09-22, live.** Trail, data-event and flow-log
      selectors all measured in account 878256633362. Data events off, no flow
      logs, but server access logging on for 58 of 254 buckets, which
      contradicts the terraform reading the doc carried. See Phase 0 F3.
- [ ] F1, F1b, F2, F3b, F6 results. F1's alarm sub-question is closed
      **negative** as of 2026-09-22 (KMS excluded on the live org trail); its
      deny-holds and no-lockout halves still need a sandbox account.
      **Access note, 2026-09-22:** AWS reads are reachable once an `aws-vault`
      SSO session is live, by pointing `AWS_CONFIG_FILE` at the committed
      `dotfiles/HOME/.aws/config` (Tatari's managed policy denies `~/.aws`
      itself) and allowlisting the service hosts. That is how F3 and F1's alarm
      half were measured. F1's remaining halves need a **sandbox account**, not
      just credentials. F3b needs a write path, not a read. F2's group half
      needs a colleague in `org-product` or `org-data-science`. F6's remaining
      half needs the Claude enterprise console.
- [ ] **NEW, opened by F5:** whether uploading a corpus that names 182 other
      employees is permissible at all without their consent, and if so under
      what mechanism. This is not an engineering spike. It is a question for
      Legal or People Ops, and it gates Phase 5 independently of whether the
      redactor is improved.
- [ ] Whether Kandji is configured with a software-deployment Blueprint. Needs
      console access, not a command.

## References

- Brian Scanlan, Intercom, on Claire Vo's *How I AI*:
  https://www.youtube.com/watch?v=BRDKft0-dUU. Timestamps cited: [00:23:47-25:48]
  LLM judge on PR descriptions, [00:31:08-31:15] shared Honeycomb key and
  "anyone can go in", [00:32:10-32:19] session data to S3 and "we anonymize
  this", [00:32:20-32:35] the stated hazard, [00:32:55-34:09] personalized
  insights, [00:34:31] "not just throwing them an API key". Auto-captions
  transcribe "Claude" as "Cloud" throughout.
- Vault note:
  `~/repos/scottidler/obsidian/notes/how-intercom-2xd-engineering-velocity-with-claude-code-brian-scanlan.md`
- JSONL schema: https://marquee.internal.tatari.dev/p/~scott-idler/claude-code-session-jsonl-schema/
- Harvesting post: https://marquee.internal.tatari.dev/p/~scott-idler/harvesting-claude-code-session-data/
- valet design doc: `valet/main/docs/design/2026-07-02-valet.md`
- Prior sessions: `8d4e3775` (transcript ingest), `000985e0` (prior-art survey,
  17 videos), `4df829ca` (schema census, structure-only decision), `1366e3fd`
  (tatari-skills versus Intercom tiering, telemetry gap)

## Addendum: roads not taken

**Local-extraction fallback.** If Phase 0 F1 proves a durable human-deny key
policy is not achievable, the design degrades to: the laptop runs an LLM over
its own prose and uploads structured findings from a closed vocabulary. Prose
never leaves. Cost: the taxonomy must be right up front, and no new question can
be asked of past sessions, only of future ones. Recorded here so it is not
re-derived under pressure.

**Why `report/src/outcome.rs` is not the mining surface.** An earlier draft named
it. Mining moved to `efficiency/src/outcome.rs:261` in the
`report-collect-once-render-from-data` work; `report/src/outcome.rs` is now 110
lines of re-exports plus a cross-session dedupe rollup.
