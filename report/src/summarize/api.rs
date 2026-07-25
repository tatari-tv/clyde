//! The `x-api-key` transport: today's direct `api.anthropic.com/v1/messages` call, unchanged.
//!
//! This is the opt-in path after the cli-default flip (`--llm api` / `render.llm: api`), and it must
//! not rot: the serialized request body is asserted byte-identical to the pre-transport behavior, so
//! a key holder's artifact is exactly what it always was.
//!
//! Everything api-specific lives here and never reaches the [`Transport`] port: the output ceilings,
//! the streaming choice, the endpoint, the version header, and the prompt/facts join.

use super::{Job, Kind, Transport, check_stop_reason, parse_sse_stream};
use eyre::{Context, Result, bail};
use log::{debug, info};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const ANTHROPIC_VERSION: &str = "2023-06-01";
const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const HTTP_TIMEOUT: Duration = Duration::from_secs(300);

impl Kind {
    /// Whether this job's response is delivered as SSE.
    ///
    /// API-PRIVATE, and the reason it is not a field on `Job`: the cli transport has no delivery
    /// choice to make, so a shared `stream` field would be one it must ignore. The output ceiling is
    /// the opposite case — both transports use it — so it IS a field on `Job`.
    ///
    /// Html streams so the connection keeps flowing bytes and the 300s idle wall never fires on a
    /// long generation; markdown reads a single JSON body. Derived from the KIND, not from a
    /// threshold over `max_tokens`, so one value never carries two meanings.
    fn streams(self) -> bool {
        matches!(self, Kind::Html)
    }
}

/// Reads `ANTHROPIC_API_KEY`, treating whitespace-only as absent. Lives here because after the
/// transport split this is the ONLY consumer of a key: `title::haiku` takes its key as a parameter.
pub fn api_key_from_env() -> Option<String> {
    std::env::var("ANTHROPIC_API_KEY").ok().filter(|s| !s.trim().is_empty())
}

/// The Anthropic Messages transport, holding the key it will send.
pub struct ApiTransport {
    api_key: String,
}

/// Hand-written and REDACTING. `#[derive(Debug)]` here would print the api key into any log line,
/// panic message, or `unwrap_err()` that ever formats this struct — the transport exists to keep
/// credential handling minimal, so it must not be the thing that leaks one. Only the length is
/// reported, which is enough to tell "empty" from "present" without burning the secret.
impl std::fmt::Debug for ApiTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiTransport")
            .field("api_key", &format_args!("<redacted, {} bytes>", self.api_key.len()))
            .finish()
    }
}

impl ApiTransport {
    /// Build from `ANTHROPIC_API_KEY`, or fail naming both remedies (there are two doors now).
    pub fn from_env() -> Result<Self> {
        debug!("ApiTransport::from_env");
        let api_key = api_key_from_env().ok_or_else(|| {
            eyre::eyre!(
                "--llm api requires ANTHROPIC_API_KEY, which is unset or empty; export a key, or drop \
                 --llm api to use the `claude` CLI with the login you already have"
            )
        })?;
        Ok(Self { api_key })
    }

    /// Construct with an explicit key. Test seam, and the shape a future config-supplied key needs.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
        }
    }
}

impl Transport for ApiTransport {
    fn complete(&self, job: Job<'_>, system: &str, prompt: &str, json_body: &str) -> Result<String> {
        let max_tokens = job.max_output_tokens;
        let stream = job.kind.streams();
        let body = build_body(job.model, system, max_tokens, stream, prompt, json_body);
        debug!(
            "ApiTransport::complete: job={job:?} system bytes={} stream={stream} prompt+json bytes={}",
            system.len(),
            body.messages.first().map(|m| m.content.len()).unwrap_or(0)
        );

        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(HTTP_TIMEOUT))
            .build()
            .new_agent();

        info!(
            "ApiTransport::complete: calling {ENDPOINT} ({}) stream={stream}",
            job.model
        );
        let mut response = agent
            .post(ENDPOINT)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .send_json(&body)
            .with_context(|| "Anthropic API call failed")?;

        let (text, stop_reason) = if stream {
            let sse = response
                .body_mut()
                .read_to_string()
                .with_context(|| "failed to read streaming Anthropic response")?;
            let outcome = parse_sse_stream(&sse)?;
            debug!(
                "ApiTransport::complete: stream complete output_tokens={:?} stop_reason={:?} text bytes={}",
                outcome.output_tokens,
                outcome.stop_reason,
                outcome.text.len()
            );
            (outcome.text, outcome.stop_reason)
        } else {
            let parsed: MessagesResponse = response
                .body_mut()
                .read_json()
                .with_context(|| "failed to parse Anthropic response")?;
            let text = parsed
                .content
                .into_iter()
                .filter_map(|c| if c.r#type == "text" { Some(c.text) } else { None })
                .collect::<Vec<_>>()
                .join("\n");
            (text, parsed.stop_reason)
        };

        if text.trim().is_empty() {
            bail!("Anthropic API returned empty content");
        }
        check_stop_reason(stop_reason.as_deref())?;
        Ok(text)
    }
}

/// Build the request body. Factored out of the send so a unit test can assert the serialized bytes
/// without a network call — this is what keeps the api path from rotting now that it is opt-in.
///
/// The `prompt`/`json_body` join lives HERE, not on the port: the api transport puts both in one
/// user message, while the cli transport delivers them on two channels. Same content, same order,
/// different mechanism.
fn build_body(
    model: &str,
    system: &str,
    max_tokens: u32,
    stream: bool,
    prompt: &str,
    json_body: &str,
) -> MessagesRequest {
    let user_msg = format!("{}\n\n```json\n{}\n```\n", prompt.trim_end(), json_body);
    MessagesRequest {
        model: model.into(),
        max_tokens,
        stream,
        system: system.into(),
        messages: vec![Message {
            role: "user".into(),
            content: user_msg,
        }],
    }
}

fn is_false(b: &bool) -> bool {
    !b
}

#[derive(Serialize)]
struct MessagesRequest {
    model: String,
    max_tokens: u32,
    /// Omitted entirely when false so the markdown-source request body stays byte-identical to the
    /// pre-HTML behavior.
    #[serde(skip_serializing_if = "is_false")]
    stream: bool,
    system: String,
    messages: Vec<Message>,
}

#[derive(Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct MessagesResponse {
    #[serde(default)]
    content: Vec<ContentBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct ContentBlock {
    r#type: String,
    #[serde(default)]
    text: String,
}

#[cfg(test)]
mod tests;
