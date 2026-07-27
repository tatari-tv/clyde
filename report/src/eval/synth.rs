//! The seeded fixture generator (design Phase 13).
//!
//! **`tatari-tv/clyde` is a PUBLIC repo, so no fixture may be derived from real session data.** A
//! redacted copy of a real window would publish real session titles and enrich summaries, and
//! redaction is not a sufficient control for narrative text: the titles ARE the sensitive payload,
//! and the eval needs them realistic. Every org, repo, title, summary, tag, commit sha and PR
//! reference below is INVENTED. Nothing here is sampled, paraphrased, or lightly edited from a real
//! transcript, and nothing in this module reads the catalog, the filesystem, or the network.
//!
//! Everything is derived from a fixed seed through [`Rng`] (splitmix64, no `rand` dependency), and
//! [`build`] overrides the report's `generated` timestamp with a fixed instant, so regenerating a
//! fixture produces byte-identical JSON. That is what makes a fixture diffable: a change in the
//! output is a change in the generator, never a change in the clock.
//!
//! The synthesized sessions go through the REAL [`crate::report::build_report`], so a fixture is
//! exactly the artifact `report collect` would have written for that window -- pricing, the
//! agent-type partition, the outcome rollup and the untracked-model gate all included. Pricing is
//! [`Pricing::embedded`] for the same reproducibility reason (see [`crate::eval`]).

use crate::outcome::{Outcomes, PrRef};
use crate::report::{CollectedSession, Report, build_report};
use chrono::{DateTime, TimeZone, Utc};
use claude_pricing::Pricing;
use common::metrics::{TokenTotals, price};
use common::repo::RepoSource;
use efficiency::{Compaction, CompactionTrigger, RawCounters, SessionEfficiency, SubagentEfficiency, finalize};
use eyre::Result;
use log::debug;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The host name every fixture reports. Invented, like everything else here.
const HOST: &str = "fixture-host";

/// The frozen `generated` timestamp stamped onto every fixture, replacing `build_report`'s
/// `Utc::now()`. Without this a regenerated fixture differs from the committed one in exactly one
/// field, every time, and the diff that is supposed to show a generator change shows a clock tick
/// instead.
const GENERATED: &str = "2026-06-01T12:00:00Z";

/// The invented employer org. The narrative's "What This Funded" section tiers by org against the
/// persona, so the fixture needs one org that matches the (equally invented) persona's organization.
/// The personal org is `jrivera` and the third-party one is `openpipe-oss`; all three appear in
/// [`WORK`], which is the single list of every repo a fixture can name.
const ORG_WORK: &str = "northwind-media";

/// The invented personal org.
const ORG_PERSONAL: &str = "jrivera";

/// The invented operator every synthesized Analytics export is scoped to. MUST equal the medium
/// fixture's `eval.yml` persona email, because `--reconcile` matches export rows against the
/// persona's work email and a mismatch would make the fixture's reconciliation fail closed instead
/// of rendering. `tests::the_medium_fixture_persona_is_the_export_operator` pins the pair.
pub const OPERATOR_EMAIL: &str = "jordan.rivera@northwind-media.example";

/// A second invented seat in the same org, present in the synthesized export so the operator filter
/// has something to filter OUT. Never the persona, never cited by any fixture.
const OTHER_OPERATOR_EMAIL: &str = "sam.okafor@northwind-media.example";

/// Priced models, real ids so the embedded pricing table can price them. Which ids appear is a
/// property of the fixture, not of any real window.
const MODEL_OPUS: &str = "claude-opus-4-7";
const MODEL_SONNET: &str = "claude-sonnet-4-6";
const MODEL_HAIKU: &str = "claude-haiku-4-5";

/// A model id absent from every pricing table, carrying NONZERO tokens: the pathological fixture's
/// untracked-model case (design Phase 6's success criterion, "a report with a genuinely unpriced
/// nonzero-token model still emits the understatement warning"). Invented, and deliberately not
/// prefix-matched by any family rule in the feed.
pub const MODEL_UNPRICED: &str = "claude-nimbus-2";

/// One invented repo and the work that happens in it: the `ai-title` shapes, the enrich summaries,
/// and the topic tags.
///
/// The vocabulary is PER REPO, and that is a correctness property rather than a flourish. A first
/// cut drew titles from one global pool, so a `northwind-media/tideline` session could be titled
/// "Harden the quill release script"; the judge caught it on the medium fixture's first real run and
/// scored citation-accuracy down for exactly that incoherence. A fixture whose sessions do not
/// belong to their repos is not a realistic window, and grading a narrative against one measures the
/// fixture rather than the render.
///
/// Titles and summaries are deliberately DIGIT-FREE. Both are identifiers in the quotable-facts
/// sets, so a number inside one would be maskable in prose without being a licensed figure, and a
/// fixture should not hand the model that ambiguity.
struct Work {
    repo: &'static str,
    /// `(ai-title, enrich summary)` PAIRS, never two independent lists. The judge caught the
    /// unpaired form too: a session titled "Trace a cold start in the ingest worker" whose summary
    /// described backoff-and-jitter work reads as a mislabeled row, and it scored citation-accuracy
    /// down for the mismatch. A title names what the session opened with and the summary digests
    /// what it did, so they describe ONE piece of work or the fixture is incoherent.
    tasks: &'static [(&'static str, &'static str)],
    tags: &'static [&'static str],
}

/// Every invented repo, with its own work. Three orgs: the employer, the personal one, and a
/// third-party open-source project.
const WORK: &[Work] = &[
    Work {
        repo: "northwind-media/beacon",
        tasks: &[
            (
                "Wire the ingest retry backoff",
                "Replaced the fixed retry delay with a bounded exponential backoff and a jitter window, so a \
             downstream blip no longer arrives back as a synchronized retry storm.",
            ),
            (
                "Trace a cold start in the ingest worker",
                "Traced a cold start in the ingest worker to a synchronous hostname lookup on the request \
             path, moved it behind a warmed cache, and added a regression test that fails when the \
             lookup happens inside the handler.",
            ),
            (
                "Audit the beacon retention policy",
                "Audited the retention policy against what the storage layer actually deletes, found two \
             buckets the sweeper never visited, and wired them into the same scheduled job.",
            ),
            (
                "Add a dead-letter queue to the ingest path",
                "Added a dead-letter queue so a record the parser rejects is parked with its failure \
             reason instead of being retried forever behind the live stream.",
            ),
            (
                "Cut the beacon startup probe timeout",
                "Measured what the slowest healthy boot actually needs and cut the startup probe timeout \
             to it, so a restarting pod stops being marked ready before its cache is warm.",
            ),
            (
                "Backfill the missing ingest metric labels",
                "Backfilled the missing labels on the ingest metrics so the per-tenant dashboard stops \
             collapsing every tenant into one series.",
            ),
        ],
        tags: &["ingest", "reliability"],
    },
    Work {
        repo: "northwind-media/tideline",
        tasks: &[
            (
                "Investigate the slow tideline query",
                "Profiled the slow dashboard query, found a missing composite index behind a filter the UI \
             always sends, added it, and recorded the before and after latency in the ticket.",
            ),
            (
                "Port the tideline dashboard to the new theme",
                "Ported the dashboard to the new theme tokens, removed the last of the hardcoded colors, and \
             verified contrast on the dense table view where the old palette failed.",
            ),
        ],
        tags: &["performance", "frontend"],
    },
    Work {
        repo: "northwind-media/almanac",
        tasks: &[
            (
                "Add pagination to the almanac API",
                "Added cursor pagination to the almanac list endpoint, kept the old offset parameter working \
             for existing callers, and documented the deprecation window in the API reference.",
            ),
            (
                "Repair the nightly almanac backfill",
                "Repaired the nightly backfill so it resumes from the last completed partition instead of \
             restarting the whole range whenever a single shard times out.",
            ),
        ],
        tags: &["api", "batch"],
    },
    Work {
        repo: "northwind-media/halyard",
        tasks: &[
            (
                "Draft the halyard rollout plan",
                "Drafted the rollout plan for the halyard cutover: the staged traffic shift, the rollback \
             trigger, and the two dashboards that have to be green before each stage advances.",
            ),
            (
                "Add a rollback trigger to halyard",
                "Added the rollback trigger the rollout plan called for, wired it to the same health check the \
             staged shift already reads, and rehearsed it against the staging fleet.",
            ),
        ],
        tags: &["rollout", "docs"],
    },
    Work {
        repo: "jrivera/sextant",
        tasks: &[
            (
                "Rename the sextant config keys",
                "Renamed the sextant configuration keys to match the house kebab-case convention and shipped a \
             loader that rejects the old spellings by name instead of silently ignoring them.",
            ),
            (
                "Fix the flaky snapshot test",
                "Rewrote the snapshot test harness so fixtures are compared structurally rather than by \
             rendered string, which removed the ordering flake that had been retried rather than fixed.",
            ),
        ],
        tags: &["config", "tests"],
    },
    Work {
        repo: "jrivera/driftwood",
        tasks: &[
            (
                "Split the driftwood parser module",
                "Split the parser into a tokenizer and a shape builder so the error path can name the offending \
             construct rather than reporting a generic parse failure at the top of the file.",
            ),
            (
                "Teach driftwood to report a line number",
                "Taught the parser to carry a line and a column through to its error type, so a malformed \
             document is reported where it broke instead of at the end of the file.",
            ),
        ],
        tags: &["refactor", "parser"],
    },
    Work {
        repo: "openpipe-oss/quill",
        tasks: &[
            (
                "Harden the quill release script",
                "Hardened the release script: it now refuses to publish when the working tree is dirty, when \
             the tag already exists, and when the changelog has no entry for the version being cut.",
            ),
            (
                "Document the quill plugin hooks",
                "Documented the plugin hooks against the code rather than the wiki, and dropped two hooks from \
             the reference that the loader has not called since the rewrite.",
            ),
        ],
        tags: &["release", "docs"],
    },
];

/// The work belonging to one repo. Panics on an unknown slug, which can only be a typo in this
/// module: every caller passes a `WORK` entry's own `repo`.
fn work(repo: &str) -> &'static Work {
    WORK.iter()
        .find(|w| w.repo == repo)
        .unwrap_or_else(|| panic!("synth: no work vocabulary for repo {repo:?}"))
}

/// The subagent types the medium fixture delegates to. Invented names in the house
/// lowercase-hyphenated shape.
const AGENT_TYPES: &[&str] = &["phase-implementer", "code-reviewer", "doc-writer"];

/// Skill / MCP attribution tags for the medium fixture. These are an attribution TAG SET, never a
/// partition, which is exactly the shape the coverage strings exist to state.
const SKILLS: &[&str] = &["release-notes", "schema-review"];
const MCP_TOOLS: &[&str] = &["tracker-search", "wiki-write"];

/// A splitmix64 step. A fixed, self-contained PRNG so a fixture is reproducible from its seed with
/// no dependency whose version could change the stream underneath a committed golden.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A value in `lo..=hi`.
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        debug_assert!(lo <= hi);
        lo + self.next() % (hi - lo + 1)
    }

    /// One element of `items`, which must be non-empty.
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        let i = (self.next() % items.len() as u64) as usize;
        &items[i]
    }

    /// `true` with probability `numerator/100`.
    fn chance(&mut self, numerator: u64) -> bool {
        self.range(1, 100) <= numerator
    }

    /// A lowercase hex string of `chars` characters.
    fn hex(&mut self, chars: usize) -> String {
        let mut out = String::with_capacity(chars);
        while out.len() < chars {
            out.push_str(&format!("{:016x}", self.next()));
        }
        out.chars().take(chars).collect()
    }
}

/// Which synthesized window to build. One variant per committed fixture, plus the medium fixture's
/// prior period (which exists only to light up Month over Month on the medium render).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Single repo, no subagents, every session attributed by `git-origin`.
    Small,
    /// Multi-org, subagents, the full outcome mix, and a POSITIVE `(main-session)` residual (design
    /// Phase 5: without one the partition's residual row never emits and its criterion cannot bite).
    Medium,
    /// [`Kind::Medium`]'s prior period, same length so `prior.comparable` is `true`.
    MediumPrior,
    /// Zero outcomes, one unpriced NONZERO-token model, a multi-day gap, carried-in sessions, and an
    /// all-`path-guess` attribution.
    Pathological,
}

impl Kind {
    /// The fixture directory name under `fixtures/report/`. `MediumPrior` has none: it is written
    /// INTO the medium fixture's directory as `prior.json`, not as a fixture of its own.
    pub fn dir_name(self) -> Option<&'static str> {
        match self {
            Kind::Small => Some("small"),
            Kind::Medium => Some("medium"),
            Kind::Pathological => Some("pathological"),
            Kind::MediumPrior => None,
        }
    }

    /// The seed this window is generated from. Distinct per kind so two fixtures never share a
    /// session id, a commit sha, or a PR number.
    fn seed(self) -> u64 {
        match self {
            Kind::Small => 1_013,
            Kind::Medium => 2_027,
            Kind::MediumPrior => 3_041,
            Kind::Pathological => 4_057,
        }
    }
}

/// Build one synthesized window as a schema-v2 [`Report`], through the real
/// [`crate::report::build_report`]. Deterministic: same `kind`, same bytes.
pub fn build(kind: Kind, pricing: &Pricing) -> Result<Report> {
    debug!("synth::build: kind={kind:?} seed={}", kind.seed());
    let mut rng = Rng::new(kind.seed());
    let (since, until, sessions) = match kind {
        Kind::Small => small(&mut rng, pricing),
        Kind::Medium => medium(&mut rng, pricing),
        Kind::MediumPrior => medium_prior(&mut rng, pricing),
        Kind::Pathological => pathological(&mut rng, pricing),
    };
    let mut report = build_report(&sessions, since, until, HOST, pricing, true, false)?;
    // Freeze the clock (see `GENERATED`). Parsing a const is infallible in practice; a broken const
    // would be caught by `tests::generated_const_parses`.
    report.generated = ts(GENERATED);
    debug!(
        "synth::build: kind={kind:?} sessions={} spend={} models={}",
        report.totals.sessions,
        report.totals.spend_usd,
        report.totals.models.len()
    );
    Ok(report)
}

/// The PER-USER Analytics cost export that lights up the medium fixture's Reconciliation section
/// (`render --reconcile`). Synthesized in the shape `reconcile::fold` parses -- every row carries
/// an `actor` (a row without one is an org-wide export, which `fold` rejects by name) -- with a
/// window that matches the medium report EXACTLY (a mismatch is a loud error by design) and a
/// billed figure deliberately ABOVE the modeled total, since `billed >= modeled` is the expected
/// relationship.
///
/// A SECOND actor's rows are included on purpose: a per-user export carries every seat in the org,
/// so a fixture with one actor would never exercise the filter that keeps another person's spend
/// out of a per-user report. Their amounts must not appear in any figure the render prints.
pub fn analytics_export(report: &Report) -> Result<String> {
    let modeled = report.totals.spend_usd;
    // The billed figure covers this operator's activity clyde never sees (claude.ai web, Cowork,
    // other clients, other hosts), so it is the modeled total plus an invented unseen slice, split
    // across the fixture's models plus one model the catalog never used.
    let rows = [
        (OPERATOR_EMAIL, MODEL_OPUS, modeled * 0.62 + 118.40),
        (OPERATOR_EMAIL, MODEL_SONNET, modeled * 0.28 + 41.15),
        (OPERATOR_EMAIL, MODEL_HAIKU, modeled * 0.10 + 6.05),
        (OPERATOR_EMAIL, "claude-opus-4-8", 73.90),
        // Another seat in the same org, at a deliberately large amount: if the operator filter ever
        // breaks, the billed figure moves by thousands and the fixture's own goldens stop matching.
        (OTHER_OPERATOR_EMAIL, MODEL_OPUS, 8_412.55),
        (OTHER_OPERATOR_EMAIL, MODEL_SONNET, 1_290.10),
    ];
    let records: Vec<serde_json::Value> = rows
        .iter()
        .map(|(email, model, amount)| {
            serde_json::json!({
                "model": model,
                // The real Analytics cost endpoints report MINOR UNITS, so a synthesized export
                // must too or the fixture stops being the shape production parses. `amount` here is
                // dollars; emit it as cents. See `reconcile::CostRecord::amount`.
                "amount": format!("{:.6}", (amount * 100.0).round()),
                "actor": {
                    "type": "user_actor",
                    "user_id": format!("user_{}", email.replace(['@', '.'], "_")),
                    "email": email,
                    "deleted": false,
                },
                "starting_at": report.since.to_rfc3339(),
                "ending_at": report.until.to_rfc3339(),
            })
        })
        .collect();
    Ok(serde_json::to_string_pretty(&records)? + "\n")
}

/// Parse one of this module's fixed RFC3339 consts. The consts are literals in this file, so a
/// failure here is a typo caught by the module's own tests, never a runtime input error.
fn ts(raw: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|e| panic!("synth: fixed timestamp {raw:?} is not RFC3339: {e}"))
}

/// Midnight UTC on a fixed calendar date.
fn day(year: i32, month: u32, date: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, date, 0, 0, 0)
        .single()
        .unwrap_or_else(|| panic!("synth: {year}-{month}-{date} midnight UTC is a real instant"))
}

/// The full parameter set for one synthesized session, so the per-kind builders read as data rather
/// than as a fifteen-argument call.
struct Spec<'a> {
    /// Session ordinal within its window; drives the uuid and every derived identifier.
    index: usize,
    repo: &'a str,
    source: RepoSource,
    /// `None` makes an UNTITLED session, which the Outlier Sessions table cites by `short-id` --
    /// one of the two citations Phase 10's whitelist is most likely to false-positive on, so every
    /// fixture carries some.
    title: Option<&'a str>,
    summary: Option<&'a str>,
    tags: &'a [&'a str],
    /// Whole days from the window's `since`. NEGATIVE makes a carried-in session (its `begin`
    /// predates the window, so `by-day` excludes it and `aggregates.carried-in` reports it).
    day_offset: i64,
    /// Hours the session ran.
    hours: i64,
    /// `(model id, token scale)`. Scale multiplies the per-session token shape.
    models: &'a [(&'a str, u64)],
    /// `(agent type, token scale)` for delegated work. The parent keeps `models` for itself, so a
    /// session with subagents always leaves a POSITIVE `(main-session)` residual.
    subagents: &'a [(&'a str, u64)],
    outcomes: Option<Outcomes>,
}

/// Turn one [`Spec`] into a [`CollectedSession`], building the efficiency object exactly the way
/// `efficiency::fold` does: the aggregate is the union of the parent's own counters and every
/// subagent's, so `report::check_fold_invariant` holds by construction.
fn session(spec: Spec<'_>, since: DateTime<Utc>, rng: &mut Rng, pricing: &Pricing) -> CollectedSession {
    let session_id = format!(
        "{}-{}-4{}-{}-{}",
        rng.hex(8),
        rng.hex(4),
        rng.hex(3),
        rng.hex(4),
        rng.hex(12)
    );
    let begin = since + chrono::Duration::days(spec.day_offset) + chrono::Duration::hours(rng.range(8, 16) as i64);
    let end = begin + chrono::Duration::hours(spec.hours);

    let mut parent = counters(spec.models, rng);
    parent.by_skill = tagged(SKILLS, spec.index, &parent, rng);
    parent.by_mcp_tool = tagged(MCP_TOOLS, spec.index + 1, &parent, rng);

    let mut subagents: Vec<SubagentEfficiency> = Vec::new();
    let mut aggregate = parent.clone();
    for (i, (agent_type, scale)) in spec.subagents.iter().enumerate() {
        let sub_models: Vec<(&str, u64)> = spec.models.iter().map(|(m, _)| (*m, *scale)).collect();
        let raw = counters(&sub_models, rng);
        aggregate.merge(&raw);
        subagents.push(SubagentEfficiency {
            agent_id: format!("agent_{}", rng.hex(10)),
            agent_type: Some((*agent_type).to_string()),
            signals: finalize(raw),
        });
        // Deterministic ordering regardless of how many agents a session spawned.
        debug_assert!(i < AGENT_TYPES.len() * 2);
    }
    aggregate.cost_usd = embedded_cost(&aggregate.by_model, pricing);

    CollectedSession {
        session_id: session_id.clone(),
        title: spec.title.map(str::to_string),
        summary: spec.summary.map(str::to_string),
        tags: spec.tags.iter().map(|t| (*t).to_string()).collect(),
        repo: Some(spec.repo.to_string()),
        repo_source: Some(spec.source),
        begin,
        end,
        jsonl_paths: vec![PathBuf::from(format!("/fixture/projects/{session_id}.jsonl"))],
        efficiency: SessionEfficiency {
            session_id,
            aggregate: finalize(aggregate),
            subagents,
            flags: Vec::new(),
        },
        outcomes: spec.outcomes,
    }
}

/// One scope's raw counters for a `(model, scale)` list. The token shape is cache-heavy, which is
/// what a real agentic session looks like and what makes `cache-read-share` a meaningful signal.
fn counters(models: &[(&str, u64)], rng: &mut Rng) -> RawCounters {
    let mut raw = RawCounters::default();
    for (model, scale) in models {
        let s = *scale;
        // Cache-heavy and at real agentic scale: a session that costs cents makes a "monthly usage
        // report" read as a toy, and the narrative's whole job is to characterize spend.
        let totals = TokenTotals {
            input: 38_000 + s * 3_137,
            output: 11_000 + s * 907,
            cache_5m_write: 240_000 + s * 14_011,
            cache_1h_write: if rng.chance(35) { 55_000 + s * 3_119 } else { 0 },
            cache_read: 5_800_000 + s * 121_919,
            total: 0,
        };
        let totals = TokenTotals {
            total: totals.input + totals.output + totals.cache_5m_write + totals.cache_1h_write + totals.cache_read,
            ..totals
        };
        raw.input_tokens += totals.input;
        raw.output_tokens += totals.output;
        raw.cache_5m_write_tokens += totals.cache_5m_write;
        raw.cache_1h_write_tokens += totals.cache_1h_write;
        raw.cache_read_tokens += totals.cache_read;
        raw.by_model.insert((*model).to_string(), totals);
        *raw.model_mix.entry((*model).to_string()).or_default() += rng.range(6, 40);
    }
    raw.turns = rng.range(8, 60);
    raw.tool_calls = rng.range(10, 180);
    raw.tool_errors = raw.tool_calls * rng.range(0, 9) / 100;
    raw.bash_command_failures = raw.tool_errors / 2;
    raw.interrupts_structured = if rng.chance(20) { rng.range(1, 2) } else { 0 };
    raw.interrupts_text = if rng.chance(12) { 1 } else { 0 };
    if rng.chance(15) {
        let pre_tokens = rng.range(140_000, 190_000);
        raw.compactions.push(Compaction {
            trigger: CompactionTrigger::Auto,
            pre_tokens,
            post_tokens: pre_tokens / 4,
            duration_ms: rng.range(4_000, 30_000),
        });
    }
    for _ in 0..rng.range(4, 12) {
        raw.turn_durations_ms.push(rng.range(1_200, 240_000));
    }
    raw
}

/// A skill / MCP attribution bucket over a slice of a scope's tokens. These are TAGS: they cover
/// part of the scope and never sum to it, which is what the coverage strings state.
fn tagged(
    names: &[&str],
    salt: usize,
    scope: &RawCounters,
    rng: &mut Rng,
) -> BTreeMap<String, efficiency::WorkloadCost> {
    let mut out = BTreeMap::new();
    if !rng.chance(45) {
        return out;
    }
    let name = names[salt % names.len()];
    let tokens = scope.total_tokens() / rng.range(4, 12);
    out.insert(
        name.to_string(),
        efficiency::WorkloadCost {
            tokens,
            cost_usd: (tokens as f64 / 1_000_000.0 * 1.85 * 100.0).round() / 100.0,
        },
    );
    out
}

/// The catalog's own embedded-priced `cost_usd` scalar for a scope. `report` re-prices from
/// `by_model` at read time and never reads this, but a real `efficiency_json` carries it, so a
/// fixture that omitted it would not be the artifact collect actually writes.
fn embedded_cost(by_model: &BTreeMap<String, TokenTotals>, pricing: &Pricing) -> f64 {
    let sum: f64 = by_model
        .iter()
        .filter_map(|(m, t)| price(m, &t.as_usage(), pricing))
        .sum();
    (sum * 100.0).round() / 100.0
}

/// Outcomes for a session that produced something. `commits`/`prs` carry invented shas and PR
/// numbers; `repo` is the PR's own repository, which the narrative may cite in prose.
fn outcomes(repo: &str, index: usize, rng: &mut Rng) -> Outcomes {
    let commits: Vec<String> = (0..rng.range(1, 4)).map(|_| rng.hex(40)).collect();
    let prs: Vec<PrRef> = (0..rng.range(0, 2))
        .map(|k| {
            let number = 100 + (index as u64) * 3 + k;
            PrRef {
                number,
                url: format!("https://github.com/{repo}/pull/{number}"),
                repository: Some(repo.to_string()),
            }
        })
        .collect();
    let files_edited = rng.range(1, 22);
    Outcomes {
        commits,
        prs,
        confluence_writes: if rng.chance(15) { 1 } else { 0 },
        jira_writes: if rng.chance(20) { rng.range(1, 3) } else { 0 },
        slack_messages: if rng.chance(25) { rng.range(1, 4) } else { 0 },
        files_edited,
        lines_written: files_edited * rng.range(20, 140),
        lines_replaced: files_edited * rng.range(4, 40),
        repos_touched: BTreeMap::from([(repo.to_string(), files_edited)]),
    }
}

/// SMALL: one repo, no subagents, every session attributed by `git-origin`, a short window. The
/// baseline shape -- if a check fails here it is not the fixture's complexity that broke it.
fn small(rng: &mut Rng, pricing: &Pricing) -> (DateTime<Utc>, DateTime<Utc>, Vec<CollectedSession>) {
    let since = day(2026, 3, 2);
    let until = day(2026, 3, 8);
    let work = work(&format!("{ORG_WORK}/beacon"));
    let mut sessions = Vec::new();
    for index in 0..9usize {
        let model = if index % 3 == 0 { MODEL_OPUS } else { MODEL_SONNET };
        let scale = rng.range(3, 40);
        // ROTATED, not sampled. Nine sessions drawing randomly from one repo's task list gave three
        // sessions the byte-identical title and summary, and the narrative then grouped them into
        // one theme and cited an arbitrary subset -- which the judge read, correctly, as
        // cross-attribution. Rotating keeps the repeats to the minimum the counts force.
        let task = &work.tasks[index % work.tasks.len()];
        sessions.push(session(
            Spec {
                index,
                repo: work.repo,
                source: RepoSource::GitOrigin,
                // Every fourth session is untitled, so the outlier table has a `short-id` citation
                // to make even on this smallest fixture.
                title: (index % 4 != 0).then_some(task.0),
                summary: Some(task.1),
                tags: work.tags,
                day_offset: (index % 6) as i64,
                hours: rng.range(1, 4) as i64,
                models: &[(model, scale)],
                subagents: &[],
                outcomes: Some(outcomes(work.repo, index, rng)),
            },
            since,
            rng,
            pricing,
        ));
    }
    (since, until, sessions)
}

/// MEDIUM: three orgs, seven repos, subagents on roughly half the sessions, the full outcome mix,
/// all four `repo-source` values, and a POSITIVE `(main-session)` residual on every delegating
/// session (the parent always keeps its own tokens). This is the fixture the judge's coverage
/// dimension is really measured on.
fn medium(rng: &mut Rng, pricing: &Pricing) -> (DateTime<Utc>, DateTime<Utc>, Vec<CollectedSession>) {
    let since = day(2026, 4, 1);
    let until = day(2026, 4, 30);
    (since, until, medium_sessions(since, 44, rng, pricing))
}

/// MEDIUM's prior period: the same shape over the preceding 30 days, so `prior.comparable` is
/// `true` and the Month over Month section compares equal ground. Fewer sessions, so the
/// period-over-period movement is a real one the narrative can name.
fn medium_prior(rng: &mut Rng, pricing: &Pricing) -> (DateTime<Utc>, DateTime<Utc>, Vec<CollectedSession>) {
    let since = day(2026, 3, 2);
    let until = day(2026, 3, 31);
    (since, until, medium_sessions(since, 31, rng, pricing))
}

/// The shared medium-window body, so the period and its prior are built by ONE generator rather
/// than two that could drift into incomparable shapes.
fn medium_sessions(since: DateTime<Utc>, count: usize, rng: &mut Rng, pricing: &Pricing) -> Vec<CollectedSession> {
    // One `repo-source` per repo, so every rule in the four-rule chain is represented and a row's
    // provenance is a stable property of the repo rather than a coin flip per session.
    let sources = [
        RepoSource::GitOrigin,
        RepoSource::GitOrigin,
        RepoSource::KnownPath,
        RepoSource::FilesTouched,
        RepoSource::GitOrigin,
        RepoSource::PathGuess,
        RepoSource::KnownPath,
    ];
    let mut sessions = Vec::new();
    for index in 0..count {
        // Weighted toward the first repos so `by-repo` has a clear top three for the coverage
        // dimension to be scored against, rather than a flat distribution with no story.
        let pick = (rng.range(0, 9) as usize).min(WORK.len() - 1);
        let (work, source) = (&WORK[pick], sources[pick]);
        let task = rng.pick(work.tasks);
        let models: Vec<(&str, u64)> = match index % 5 {
            0 => vec![(MODEL_OPUS, rng.range(20, 90))],
            1 => vec![(MODEL_SONNET, rng.range(10, 50)), (MODEL_HAIKU, rng.range(2, 12))],
            2 => vec![(MODEL_OPUS, rng.range(30, 120)), (MODEL_SONNET, rng.range(5, 25))],
            3 => vec![(MODEL_HAIKU, rng.range(3, 18))],
            _ => vec![(MODEL_SONNET, rng.range(8, 44))],
        };
        let subagents: Vec<(&str, u64)> = if rng.chance(45) {
            (0..rng.range(1, 3))
                .map(|k| (AGENT_TYPES[(index + k as usize) % AGENT_TYPES.len()], rng.range(4, 30)))
                .collect()
        } else {
            Vec::new()
        };
        sessions.push(session(
            Spec {
                index,
                repo: work.repo,
                source,
                title: (index % 5 != 0).then_some(task.0),
                // Deliberately partial enrichment, so `enrichment-coverage` states a real gap and
                // the prompt's title-fallback path is exercised.
                summary: rng.chance(65).then_some(task.1),
                tags: work.tags,
                day_offset: rng.range(0, 28) as i64,
                hours: rng.range(1, 6) as i64,
                models: &models,
                subagents: &subagents,
                outcomes: rng.chance(70).then(|| outcomes(work.repo, index, rng)),
            },
            since,
            rng,
            pricing,
        ));
    }
    sessions
}

/// PATHOLOGICAL: every absent-section path at once. Zero outcomes anywhere, one unpriced
/// NONZERO-token model, a multi-day gap in the middle of the window, sessions carried in from
/// before `since`, and an attribution that is entirely `path-guess` -- so every `by-repo` row is a
/// marked guess and nothing in the window is observed.
fn pathological(rng: &mut Rng, pricing: &Pricing) -> (DateTime<Utc>, DateTime<Utc>, Vec<CollectedSession>) {
    let since = day(2026, 5, 1);
    let until = day(2026, 5, 20);
    let work = work(&format!("{ORG_PERSONAL}/driftwood"));
    // Days 6 through 13 are deliberately absent: a run of `active: false` rows the narrative must
    // cite as a gap rather than infer from a missing row.
    let active_days = [0i64, 1, 2, 4, 5, 14, 15, 17, 18];
    let mut sessions = Vec::new();
    for (index, offset) in active_days.iter().enumerate() {
        let models: Vec<(&str, u64)> = if index == 2 {
            vec![(MODEL_UNPRICED, rng.range(10, 30))]
        } else {
            vec![(MODEL_SONNET, rng.range(4, 26))]
        };
        sessions.push(session(
            Spec {
                index,
                repo: work.repo,
                source: RepoSource::PathGuess,
                title: (index % 3 != 0).then_some(rng.pick(work.tasks).0),
                // No enrich coverage at all: the narrative has titles and nothing else, which is
                // the weakest evidence base the prompt has to degrade to.
                summary: None,
                tags: &[],
                day_offset: *offset,
                hours: rng.range(1, 3) as i64,
                models: &models,
                subagents: &[],
                outcomes: None,
            },
            since,
            rng,
            pricing,
        ));
    }
    // Carried in: `begin` predates `since`, `end` lands inside the window, so the M2 session-level
    // window pulls them in whole and `aggregates.carried-in` is the only place they appear.
    for index in 0..3usize {
        sessions.push(session(
            Spec {
                index: 100 + index,
                repo: work.repo,
                source: RepoSource::PathGuess,
                title: Some(rng.pick(work.tasks).0),
                summary: None,
                tags: &[],
                day_offset: -2 - index as i64,
                hours: (48 + index * 6) as i64,
                models: &[(MODEL_SONNET, rng.range(6, 20))],
                subagents: &[],
                outcomes: None,
            },
            since,
            rng,
            pricing,
        ));
    }
    (since, until, sessions)
}

#[cfg(test)]
mod tests;
