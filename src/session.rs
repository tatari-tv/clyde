use crate::parse::TokenUsage;
use chrono::{DateTime, Utc};
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct TokenTotals {
    pub input: u64,
    pub output: u64,
    pub cache_5m_write: u64,
    pub cache_1h_write: u64,
    pub cache_read: u64,
    pub total: u64,
}

impl TokenTotals {
    pub fn add(&mut self, usage: &TokenUsage) {
        self.input += usage.input_tokens;
        self.output += usage.output_tokens;
        self.cache_5m_write += usage.cache_5m_write_tokens;
        self.cache_1h_write += usage.cache_1h_write_tokens;
        self.cache_read += usage.cache_read_tokens;
        self.total = self.input + self.output + self.cache_5m_write + self.cache_1h_write + self.cache_read;
    }
}

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub session_id: String,
    pub repo: Option<String>,
    pub cwd: Option<PathBuf>,
    pub begin: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub models: BTreeSet<String>,
    pub tokens: TokenTotals,
    pub jsonl_paths: Vec<PathBuf>,
    pub title: Option<String>,
}
