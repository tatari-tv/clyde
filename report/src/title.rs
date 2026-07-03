use eyre::{Context, Result};
use log::{debug, trace, warn};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::time::Duration;

pub const HAIKU_MODEL: &str = "claude-haiku-4-5";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const MAX_PREFIX_CHARS: usize = 4_000;
const MAX_OUTPUT_TOKENS: u32 = 64;
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Default)]
pub struct Prefix {
    pub user: String,
    pub assistant: String,
}

impl Prefix {
    pub fn is_usable(&self) -> bool {
        !self.user.trim().is_empty() || !self.assistant.trim().is_empty()
    }
}

pub fn extract_prefix(parent_jsonl: &Path) -> Result<Prefix> {
    trace!("title::extract_prefix: path={}", parent_jsonl.display());

    let file =
        std::fs::File::open(parent_jsonl).with_context(|| format!("failed to open {}", parent_jsonl.display()))?;
    let reader = BufReader::new(file);

    let mut prefix = Prefix::default();

    for line in reader.lines() {
        if !prefix.user.is_empty() && !prefix.assistant.is_empty() {
            break;
        }
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                warn!("title: error reading {}: {}", parent_jsonl.display(), e);
                continue;
            }
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let raw: RawLine = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(_) => continue,
        };
        match raw.entry_type.as_deref() {
            Some("user") if prefix.user.is_empty() => {
                if let Some(text) = raw.message.and_then(extract_text) {
                    prefix.user = truncate(&text, MAX_PREFIX_CHARS);
                }
            }
            Some("assistant") if prefix.assistant.is_empty() => {
                if let Some(text) = raw.message.and_then(extract_text) {
                    prefix.assistant = truncate(&text, MAX_PREFIX_CHARS);
                }
            }
            _ => {}
        }
    }

    Ok(prefix)
}

pub fn haiku(prefix: &Prefix, api_key: &str) -> Result<Option<String>> {
    if !prefix.is_usable() {
        return Ok(None);
    }
    let user_msg = build_user_message(prefix);

    let body = MessagesRequest {
        model: HAIKU_MODEL.into(),
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

    let response = agent
        .post(ENDPOINT)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .send_json(&body);

    let mut response = match response {
        Ok(r) => r,
        Err(e) => {
            warn!("title::haiku: request failed: {}", e);
            return Ok(None);
        }
    };

    let parsed: MessagesResponse = match response.body_mut().read_json() {
        Ok(p) => p,
        Err(e) => {
            warn!("title::haiku: failed to parse response: {}", e);
            return Ok(None);
        }
    };

    let raw_title = parsed
        .content
        .into_iter()
        .find_map(|c| if c.r#type == "text" { Some(c.text) } else { None })
        .unwrap_or_default();

    let cleaned = clean_title(&raw_title);
    if cleaned.is_empty() {
        debug!("title::haiku: model returned empty/unusable title; raw={:?}", raw_title);
        return Ok(None);
    }
    Ok(Some(cleaned))
}

const SYSTEM_PROMPT: &str = "You title Claude Code sessions. Output exactly 3 to 7 lowercase words separated by single spaces. \
No punctuation, no quotation marks, no leading or trailing whitespace, no markdown. \
Summarize the task the user asked the assistant to perform. Output only the title.";

fn build_user_message(prefix: &Prefix) -> String {
    format!(
        "User asked:\n---\n{}\n---\n\nAssistant began:\n---\n{}\n---\n\nTitle:",
        prefix.user.trim(),
        prefix.assistant.trim()
    )
}

pub fn clean_title(raw: &str) -> String {
    let lower = raw.trim().to_lowercase();
    let mut buf = String::with_capacity(lower.len());
    for ch in lower.chars() {
        if ch.is_alphanumeric() || ch.is_whitespace() || ch == '-' {
            buf.push(ch);
        }
    }
    let words: Vec<&str> = buf.split_whitespace().collect();
    if words.is_empty() {
        return String::new();
    }
    let pick = if words.len() > 7 { &words[..7] } else { &words[..] };
    if pick.len() < 3 {
        return String::new();
    }
    pick.join(" ")
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    s.get(..end).unwrap_or_default().to_string()
}

#[derive(Deserialize)]
struct RawLine {
    #[serde(rename = "type")]
    entry_type: Option<String>,
    message: Option<RawMessage>,
}

#[derive(Deserialize)]
struct RawMessage {
    #[serde(default)]
    content: serde_json::Value,
}

fn extract_text(msg: RawMessage) -> Option<String> {
    match msg.content {
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s),
        serde_json::Value::Array(blocks) => {
            for block in blocks {
                if let serde_json::Value::Object(map) = block
                    && map.get("type").and_then(|v| v.as_str()) == Some("text")
                    && let Some(text) = map.get("text").and_then(|v| v.as_str())
                    && !text.trim().is_empty()
                {
                    return Some(text.to_string());
                }
            }
            None
        }
        _ => None,
    }
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

pub fn api_key_from_env() -> Option<String> {
    std::env::var("ANTHROPIC_API_KEY").ok().filter(|s| !s.trim().is_empty())
}

#[cfg(test)]
mod tests;
