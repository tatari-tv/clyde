use eyre::{Context, Result, bail};
use log::{debug, info};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const OPUS_MODEL: &str = "claude-opus-4-7";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const MAX_OUTPUT_TOKENS: u32 = 16_000;
const HTTP_TIMEOUT: Duration = Duration::from_secs(300);
const SYSTEM_PROMPT: &str = "You are a precise technical writer producing markdown documents from structured data. Output exactly what is asked - no preamble, no commentary, no fenced code block wrapping the whole output.";

pub fn opus(prompt_template: &str, json_body: &str, api_key: &str) -> Result<String> {
    let user_msg = format!("{}\n\n```json\n{}\n```\n", prompt_template.trim_end(), json_body);
    debug!("summarize::opus: prompt+json bytes={}", user_msg.len());

    let body = MessagesRequest {
        model: OPUS_MODEL.into(),
        max_tokens: MAX_OUTPUT_TOKENS,
        system: SYSTEM_PROMPT.into(),
        messages: vec![Message {
            role: "user".into(),
            content: user_msg,
        }],
    };

    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .build()
        .new_agent();

    info!("summarize::opus: calling {} ({})", ENDPOINT, OPUS_MODEL);
    let mut response = agent
        .post(ENDPOINT)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .send_json(&body)
        .with_context(|| "Anthropic API call failed")?;

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

    if text.trim().is_empty() {
        bail!("Anthropic API returned empty content");
    }
    Ok(text)
}

#[derive(Serialize)]
struct MessagesRequest {
    model: String,
    max_tokens: u32,
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
}

#[derive(Deserialize)]
struct ContentBlock {
    r#type: String,
    #[serde(default)]
    text: String,
}
