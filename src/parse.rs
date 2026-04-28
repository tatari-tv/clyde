use chrono::{DateTime, Utc};
use std::path::PathBuf;

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
