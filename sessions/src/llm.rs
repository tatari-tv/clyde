//! The enrichment LLM seam: the [`Completer`] and [`Narrator`] ports the orchestrators depend on, and
//! the real [`ClaudeCli`] that implements BOTH over the keyless `common::llm::CliTransport`.
//!
//! Per the workspace DI convention, the orchestrator is generic over `C: Completer`, so tests
//! inject a deterministic fake and never touch the network.
//!
//! clyde handles NO credential here (design `2026-07-29-excise-api-key.md`). The `claude` binary the
//! user is already logged into owns auth end to end. The deleted `AnthropicClient` read an api key from
//! the environment and put it on the wire as a request header, which made both these commands dead on a
//! keyless host -- the wall a teammate hit on 2026-07-29 and the reason this exists. (The variable and
//! header are deliberately not spelled out anywhere in this crate: the design asserts a workspace-wide
//! grep for either name, and a comment naming them would satisfy the grep while proving nothing.) Its
//! within-call HTTP retry ladder went with it: the transport is fail-loud-never-retry by design, and
//! enrich already has the durable cross-run layer (the `attempts` column), which survives a restart.

use eyre::{Context, Result, bail};
use log::debug;
use serde::Deserialize;

use common::llm::{CliTransport, Completion, Job, Kind, Transport};

/// The model enrichment pins. Stored per-row as `enrich_model` for provenance.
pub const ENRICH_MODEL: &str = "claude-haiku-4-5-20251001";
/// The enrichment prompt/schema version. Bumping it makes every row eligible for re-enrichment.
pub const ENRICH_PROMPT_VERSION: i64 = 1;

/// The model the prose-narration path ([`Narrator`]) pins. Reuses the enrichment model so the two
/// LLM callers share ONE pinned model on this host (siblings behave identically); a chatty prose
/// verdict is cheap on the same small model.
pub const NARRATE_MODEL: &str = ENRICH_MODEL;
/// Output-token cap for a prose narration, and its enrichment sibling below.
///
/// Both are INERT over the cli transport and are kept because [`Job`] carries the field: the api path
/// SET this as a wire-level `max_tokens`, and the cli path can only CHECK it -- which it no longer does
/// for these two kinds, because measured output is dominated by CLI-side reasoning that never reaches
/// the reply (5,798 and 678 tokens against this 512, and it does not track payload size). The real
/// truncation contract is `stop_reason == "end_turn"`, and `Kind::max_output_tokens_key()` returns `None`
/// for both kinds so nothing pretends this number is a knob. See design Phase 0 Findings 3 and 10.
const NARRATE_MAX_OUTPUT_TOKENS: u32 = 512;
const MAX_OUTPUT_TOKENS: u32 = 512;
/// Upper bound on stored tags (the design specifies 3-7); a chatty reply is clamped, not rejected.
const MAX_TAGS: usize = 7;

const SYSTEM_PROMPT: &str = "\
You catalog past Claude Code coding sessions so they can be found later. Given the text of one \
complete session (user and assistant turns), produce a durable catalog entry. Respond with ONLY a \
JSON object, no prose, no markdown fences, matching exactly:
{\"tags\": [\"...\"], \"summary\": \"...\"}
- tags: 3 to 7 short lowercase search tags (single words or hyphenated), naming the technologies, \
the task, and the domain. No '#', no spaces within a tag.
- summary: 1 to 3 sentences describing what the session was about and what was decided or produced. \
Durable and specific; not a play-by-play.";

/// The structured result of enriching one session's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmEnrichment {
    pub tags: Vec<String>,
    pub summary: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

/// The enrichment port: turn one session's (already scope-gated, redacted) text into tags + a
/// summary. Implemented by [`ClaudeCli`] in production and by a fake in tests.
pub trait Completer {
    /// Enrich `payload`. `payload` is the redacted high-signal body; implementations must never
    /// log it in full.
    fn enrich(&self, payload: &str) -> Result<LlmEnrichment>;
}

/// A general prose-completion port: turn a system + user prompt into free-text prose. Distinct from
/// [`Completer`] (which returns the structured enrichment JSON) because narration selects and
/// phrases pre-computed facts rather than producing a tag/summary schema. The `efficiency` crate's
/// Phase 8 narrative layer depends on this port so tests inject a deterministic fake and the
/// narration never touches the network. The real [`ClaudeCli`] implements it over the SAME transport as
/// enrichment (one integration, no second credential, no second billing path).
pub trait Narrator {
    /// Complete `user` under `system`, returning the model's prose reply (trimmed, non-empty).
    /// Implementations must never log `user`/`system` in full — previews only, per the logging rule.
    fn narrate(&self, system: &str, user: &str) -> Result<String>;
}

/// The keyless enrichment/narration client: BOTH ports over one `common::llm::CliTransport`.
///
/// Holds no credential, reads no environment variable, and sends no header. The transport shells out to
/// the locally installed `claude`, which owns auth (and therefore expiry, rate limits, and plan caps)
/// end to end.
pub struct ClaudeCli {
    transport: CliTransport,
}

/// The install-and-login remedy, named on every failure to construct the client. Nothing here can be
/// fixed by setting a variable, so the error must not send the reader looking for one.
const CLAUDE_REMEDY: &str =
    "install Claude Code and log in once (`claude`, then /login); clyde needs no API key for this";

impl ClaudeCli {
    /// Resolve `claude` off PATH and log its version.
    ///
    /// A PRESENCE check, not a success check: it proves an executable of that name exists and nothing
    /// more (the transport logs the resolved binary and version at `info!`, and names both in every
    /// later failure). Called ONCE before the enrich sweep's loop, which is what makes
    /// `claude`-not-installed sweep-fatal by construction rather than by classification.
    pub fn resolve() -> Result<Self> {
        debug!("ClaudeCli::resolve");
        let transport = CliTransport::resolve().with_context(|| CLAUDE_REMEDY.to_string())?;
        Ok(Self { transport })
    }

    /// The enrich job, built where its pins live. `Kind::Enrich` is what selects the reasoning
    /// suppression and the prose fence inside the transport.
    fn enrich_job() -> Job<'static> {
        Job {
            kind: Kind::Enrich,
            model: ENRICH_MODEL,
            max_output_tokens: MAX_OUTPUT_TOKENS,
        }
    }

    /// The narrate job. Same model pin, its own `Kind`, and deliberately NOT reasoning-suppressed: the
    /// flag flips the verdict this prose carries (design Phase 0 Finding 13).
    fn narrate_job() -> Job<'static> {
        Job {
            kind: Kind::Narrate,
            model: NARRATE_MODEL,
            max_output_tokens: NARRATE_MAX_OUTPUT_TOKENS,
        }
    }
}

/// The JSON contract the model is asked to return.
#[derive(Debug, Deserialize)]
struct EnrichJson {
    tags: Vec<String>,
    summary: String,
}

impl Completer for ClaudeCli {
    fn enrich(&self, payload: &str) -> Result<LlmEnrichment> {
        debug!("ClaudeCli::enrich: payload_chars={}", payload.chars().count());
        // The redacted session text rides STDIN, never argv: these payloads run to 500KB
        // (`enrich::SEND_CAP_CHARS`) and argv is the ARG_MAX hazard the transport's docs warn about. So
        // the instruction slot is empty and the schema instruction is the system prompt, exactly as
        // measured in Phase 0.
        //
        // Deliberately NO `.context()` on this call: the transport attaches
        // `common::llm::TransportError` for a sweep-fatal failure, and `sessions::enrich` recovers it by
        // downcast. Wrapping it is safe for eyre, but leaving the seam bare keeps the one mechanism G5
        // depends on impossible to break by accident.
        let Completion {
            text,
            tokens_in,
            tokens_out,
        } = self
            .transport
            .complete_with_usage(Self::enrich_job(), SYSTEM_PROMPT, "", payload)?;
        let parsed = parse_enrich_json(&text).context("the `claude` CLI reply was not the expected JSON")?;
        let tags = normalize_tags(parsed.tags);
        if tags.is_empty() || parsed.summary.trim().is_empty() {
            bail!("the `claude` CLI reply had empty tags or summary");
        }
        debug!(
            "ClaudeCli::enrich: tags={} tokens_in={tokens_in} tokens_out={tokens_out}",
            tags.len()
        );
        Ok(LlmEnrichment {
            tags,
            summary: parsed.summary.trim().to_string(),
            tokens_in,
            tokens_out,
        })
    }
}

impl Narrator for ClaudeCli {
    fn narrate(&self, system: &str, user: &str) -> Result<String> {
        debug!(
            "ClaudeCli::narrate: system_chars={} user_chars={}",
            system.chars().count(),
            user.chars().count()
        );
        // `user` on stdin for the same reason enrich's payload is, and no token counts to keep: narrate
        // is one interactive call with nothing durable to account for.
        let prose = self
            .transport
            .complete(Self::narrate_job(), system, "", user)?
            .trim()
            .to_string();
        if prose.is_empty() {
            bail!("the `claude` CLI narration returned no prose");
        }
        debug!("ClaudeCli::narrate: prose_chars={}", prose.chars().count());
        Ok(prose)
    }
}

/// Enforce the tag contract on a model reply: lowercase, trim, collapse internal whitespace to a
/// single hyphen (the design's "no spaces within a tag"), drop empties, dedupe preserving order,
/// and clamp to `MAX_TAGS`. A reply with too many or sloppy tags is normalized, not rejected.
fn normalize_tags(raw: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tag in raw {
        let norm = tag.trim().to_lowercase();
        if norm.is_empty() {
            continue;
        }
        let norm = norm.split_whitespace().collect::<Vec<_>>().join("-");
        if !out.contains(&norm) {
            out.push(norm);
        }
    }
    out.truncate(MAX_TAGS);
    out
}

/// Parse the model's reply as the enrichment JSON. Tolerates leading/trailing prose or fences by
/// falling back to the outermost `{…}` span before giving up.
fn parse_enrich_json(text: &str) -> Result<EnrichJson> {
    if let Ok(v) = serde_json::from_str::<EnrichJson>(text.trim()) {
        return Ok(v);
    }
    let start = text.find('{');
    let end = text.rfind('}');
    if let (Some(s), Some(e)) = (start, end)
        && e >= s
        && let Some(slice) = text.get(s..=e)
    {
        return serde_json::from_str::<EnrichJson>(slice).context("embedded JSON did not match schema");
    }
    bail!("no JSON object found in model reply")
}

#[cfg(test)]
mod tests;
