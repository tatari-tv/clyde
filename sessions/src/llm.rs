//! The enrichment LLM seam: the [`Completer`] port the orchestrator depends on, and the real
//! [`AnthropicClient`] that implements it against the Anthropic Messages API.
//!
//! Per the workspace DI convention, the orchestrator is generic over `C: Completer`, so tests
//! inject a deterministic fake and never touch the network. The concrete client reads its key
//! from the environment (the **work** Anthropic key on desk — never inlined, never logged), sets
//! an explicit timeout, calls `error_for_status()`, and retries bounded on rate-limit / 5xx.

use std::time::Duration;

use eyre::{Context, Result, bail, eyre};
use log::{debug, warn};
use serde::Deserialize;
use serde_json::json;

/// The model enrichment pins. Stored per-row as `enrich_model` for provenance.
pub const ENRICH_MODEL: &str = "claude-haiku-4-5-20251001";
/// The enrichment prompt/schema version. Bumping it makes every row eligible for re-enrichment.
pub const ENRICH_PROMPT_VERSION: i64 = 1;

/// The model the prose-narration path ([`Narrator`]) pins. Reuses the enrichment model so the two
/// LLM callers share ONE pinned model on this host (siblings behave identically); a chatty prose
/// verdict is cheap on the same small model.
pub const NARRATE_MODEL: &str = ENRICH_MODEL;
/// Output-token cap for a prose narration. A verdict is a few sentences; a runaway reply is clamped.
const NARRATE_MAX_OUTPUT_TOKENS: u32 = 512;

/// Environment variable holding the Anthropic API key.
const API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
const API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const HTTP_TIMEOUT_SECS: u64 = 60;
const MAX_OUTPUT_TOKENS: u32 = 512;
/// Bounded in-call retries on rate-limit / 5xx before the call is reported failed (the durable
/// cross-run backoff is the `attempts` column; this is the within-call layer).
const MAX_HTTP_RETRIES: u32 = 3;
const RETRY_BACKOFF_MS: u64 = 2_000;
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
/// summary. Implemented by [`AnthropicClient`] in production and by a fake in tests.
pub trait Completer {
    /// Enrich `payload`. `payload` is the redacted high-signal body; implementations must never
    /// log it in full.
    fn enrich(&self, payload: &str) -> Result<LlmEnrichment>;
}

/// A general prose-completion port: turn a system + user prompt into free-text prose. Distinct from
/// [`Completer`] (which returns the structured enrichment JSON) because narration selects and
/// phrases pre-computed facts rather than producing a tag/summary schema. The `efficiency` crate's
/// Phase 8 narrative layer depends on this port so tests inject a deterministic fake and the
/// narration never touches the network. The real [`AnthropicClient`] implements it over the SAME
/// key/timeout/retry HTTP path as enrichment (one integration, no new LLM dependency).
pub trait Narrator {
    /// Complete `user` under `system`, returning the model's prose reply (trimmed, non-empty).
    /// Implementations must never log `user`/`system` in full — previews only, per the logging rule.
    fn narrate(&self, system: &str, user: &str) -> Result<String>;
}

/// The Anthropic Messages API client. Holds the key in memory only; never serializes or logs it.
pub struct AnthropicClient {
    http: reqwest::blocking::Client,
    api_key: String,
}

impl AnthropicClient {
    /// Build a client, reading the key from `ANTHROPIC_API_KEY`. Errors (without echoing the key)
    /// when the variable is unset — enrichment cannot ship without the work key on this host.
    pub fn from_env() -> Result<Self> {
        debug!("AnthropicClient::from_env");
        let api_key = std::env::var(API_KEY_ENV)
            .map_err(|_| eyre!("{API_KEY_ENV} not set; enrichment needs the work Anthropic key on this host"))?;
        if api_key.trim().is_empty() {
            bail!("{API_KEY_ENV} is empty");
        }
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self { http, api_key })
    }
}

/// Minimal view of the Messages API response we consume.
#[derive(Debug, Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
    usage: Usage,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

/// The JSON contract the model is asked to return.
#[derive(Debug, Deserialize)]
struct EnrichJson {
    tags: Vec<String>,
    summary: String,
}

impl Completer for AnthropicClient {
    fn enrich(&self, payload: &str) -> Result<LlmEnrichment> {
        debug!("AnthropicClient::enrich: payload_chars={}", payload.chars().count());
        let resp = self.messages(ENRICH_MODEL, SYSTEM_PROMPT, payload, MAX_OUTPUT_TOKENS)?;
        let text = Self::first_text(&resp)?;
        let parsed = parse_enrich_json(text).context("Anthropic response was not the expected JSON")?;
        let tags = normalize_tags(parsed.tags);
        if tags.is_empty() || parsed.summary.trim().is_empty() {
            bail!("Anthropic response had empty tags or summary");
        }
        Ok(LlmEnrichment {
            tags,
            summary: parsed.summary.trim().to_string(),
            tokens_in: resp.usage.input_tokens,
            tokens_out: resp.usage.output_tokens,
        })
    }
}

impl Narrator for AnthropicClient {
    fn narrate(&self, system: &str, user: &str) -> Result<String> {
        debug!(
            "AnthropicClient::narrate: system_chars={} user_chars={}",
            system.chars().count(),
            user.chars().count()
        );
        let resp = self.messages(NARRATE_MODEL, system, user, NARRATE_MAX_OUTPUT_TOKENS)?;
        let prose = Self::first_text(&resp)?.trim().to_string();
        if prose.is_empty() {
            bail!("Anthropic narration response had no prose");
        }
        Ok(prose)
    }
}

impl AnthropicClient {
    /// Build the Messages request and POST it (shared by [`Completer::enrich`] and
    /// [`Narrator::narrate`] so both callers ride ONE key/timeout/retry path). Returns the decoded
    /// response; callers pick out the text block via [`first_text`](Self::first_text).
    fn messages(&self, model: &str, system: &str, user: &str, max_tokens: u32) -> Result<MessagesResponse> {
        let body = json!({
            "model": model,
            "max_tokens": max_tokens,
            "system": system,
            "messages": [{ "role": "user", "content": user }],
        });
        self.post_with_retry(&body)
    }

    /// The first `text` content block of a response, or an error when none is present.
    fn first_text(resp: &MessagesResponse) -> Result<&str> {
        resp.content
            .iter()
            .find(|b| b.kind == "text")
            .map(|b| b.text.as_str())
            .ok_or_else(|| eyre!("Anthropic response had no text block"))
    }

    /// POST the request, retrying bounded on 429 / 5xx with linear backoff. Returns the decoded
    /// response or an error after the retries are exhausted.
    fn post_with_retry(&self, body: &serde_json::Value) -> Result<MessagesResponse> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let result = self
                .http
                .post(API_URL)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("content-type", "application/json")
                .json(body)
                .send();

            match result {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        return resp
                            .json::<MessagesResponse>()
                            .context("failed to decode Anthropic response");
                    }
                    let retryable = status.as_u16() == 429 || status.is_server_error();
                    let snippet = error_snippet(resp);
                    if retryable && attempt <= MAX_HTTP_RETRIES {
                        warn!("AnthropicClient: status {status} (attempt {attempt}/{MAX_HTTP_RETRIES}), retrying");
                        std::thread::sleep(Duration::from_millis(RETRY_BACKOFF_MS * attempt as u64));
                        continue;
                    }
                    bail!("Anthropic API returned {status}: {snippet}");
                }
                Err(e) => {
                    if attempt <= MAX_HTTP_RETRIES {
                        warn!("AnthropicClient: transport error (attempt {attempt}/{MAX_HTTP_RETRIES}): {e}");
                        std::thread::sleep(Duration::from_millis(RETRY_BACKOFF_MS * attempt as u64));
                        continue;
                    }
                    return Err(eyre!("Anthropic API transport error: {e}"));
                }
            }
        }
    }
}

/// A short, secret-free snippet of an error response body for diagnostics.
fn error_snippet(resp: reqwest::blocking::Response) -> String {
    match resp.text() {
        Ok(t) => t.chars().take(200).collect(),
        Err(_) => "<unreadable body>".to_string(),
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
