use chrono::{DateTime, Utc};
use eyre::Result;
use log::{trace, warn};
use serde::Deserialize;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_5m_write_tokens: u64,
    pub cache_1h_write_tokens: u64,
    pub cache_read_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct AssistantEntry {
    pub session_id: String,
    pub timestamp: DateTime<Utc>,
    pub model: String,
    pub usage: TokenUsage,
    pub message_id: Option<String>,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ParseResult {
    pub entries: Vec<AssistantEntry>,
    pub cwd: Option<PathBuf>,
}

#[derive(Deserialize)]
struct RawEntry {
    #[serde(rename = "type")]
    entry_type: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    timestamp: Option<String>,
    cwd: Option<String>,
    message: Option<RawMessage>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
}

#[derive(Deserialize)]
struct RawMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<RawUsage>,
}

#[derive(Deserialize)]
struct RawUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation: Option<RawCacheCreation>,
}

#[derive(Deserialize)]
struct RawCacheCreation {
    ephemeral_5m_input_tokens: Option<u64>,
    ephemeral_1h_input_tokens: Option<u64>,
}

pub fn parse_jsonl_file(path: &Path) -> Result<ParseResult> {
    trace!("parse::parse_jsonl_file: path={}", path.display());

    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut result = ParseResult::default();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                warn!("parse: error reading line from {}: {}", path.display(), e);
                continue;
            }
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let raw: RawEntry = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                warn!("parse: failed to parse JSON line in {}: {}", path.display(), e);
                continue;
            }
        };

        if result.cwd.is_none()
            && let Some(cwd_str) = raw.cwd.as_deref()
            && !cwd_str.is_empty()
        {
            result.cwd = Some(PathBuf::from(cwd_str));
        }

        if let Some(entry) = convert_raw_entry(raw) {
            result.entries.push(entry);
        }
    }

    Ok(result)
}

fn convert_raw_entry(raw: RawEntry) -> Option<AssistantEntry> {
    if raw.entry_type.as_deref() != Some("assistant") {
        return None;
    }

    let session_id = raw.session_id?;
    let timestamp_str = raw.timestamp?;
    let request_id = raw.request_id;
    let message = raw.message?;
    let message_id = message.id;
    let model = message.model?;
    let usage = message.usage?;
    let timestamp = timestamp_str.parse::<DateTime<Utc>>().ok()?;

    let (cache_5m, cache_1h) = if let Some(cc) = &usage.cache_creation {
        (
            cc.ephemeral_5m_input_tokens.unwrap_or(0),
            cc.ephemeral_1h_input_tokens.unwrap_or(0),
        )
    } else {
        (usage.cache_creation_input_tokens.unwrap_or(0), 0)
    };

    Some(AssistantEntry {
        session_id,
        timestamp,
        model,
        usage: TokenUsage {
            input_tokens: usage.input_tokens.unwrap_or(0),
            output_tokens: usage.output_tokens.unwrap_or(0),
            cache_5m_write_tokens: cache_5m,
            cache_1h_write_tokens: cache_1h,
            cache_read_tokens: usage.cache_read_input_tokens.unwrap_or(0),
        },
        message_id,
        request_id,
    })
}

#[cfg(test)]
mod tests;
