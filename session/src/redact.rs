//! Secret scrubbing — defense-in-depth, **not** the trust boundary.
//!
//! The real Phase 2 boundary is the scope gate ([`crate::scope`]) plus the work account's
//! data-retention posture. This module is the net beneath it: a single chokepoint
//! ([`scrub`]) the enrich send path must call to strip high-confidence secret *shapes* (API
//! keys, bearer/AWS/GitHub/Slack tokens, PEM private-key blocks) from work-scoped content already
//! cleared to reach the work account, before the payload leaves the process.
//!
//! It deliberately matches only high-confidence shapes. Generic bearer tokens, DSNs, JWT
//! variants, PII, and proprietary URLs are out of scope and are *known* gaps — the scope gate,
//! not this regex, is what keeps personal content off the work account. Over-scrubbing is
//! acceptable: the redacted text only feeds tag/summary inference, never replaces the stored body.

use std::sync::OnceLock;

use log::debug;
use regex::Regex;

/// What a stripped secret is replaced with. Keeps the surrounding text readable for the LLM while
/// removing the live value.
const PLACEHOLDER: &str = "[redacted-secret]";

/// High-confidence secret shapes, in application order. Each is a self-contained pattern; ordering
/// only matters for the multiline PEM block (matched before line-oriented patterns can fragment it).
fn patterns() -> &'static [Regex] {
    static PATS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATS.get_or_init(|| {
        let raw = [
            // PEM private-key blocks (RSA/EC/OPENSSH/DSA/PGP/plain), whole block, multiline.
            r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
            // Anthropic / OpenAI style API keys: sk-, sk-ant-, sk-proj-.
            r"sk-(?:ant-|proj-)?[A-Za-z0-9_-]{20,}",
            // GitHub personal/OAuth/app/refresh tokens.
            r"gh[posur]_[A-Za-z0-9]{36,}",
            // Slack tokens.
            r"xox[baprs]-[A-Za-z0-9-]{10,}",
            // AWS access key IDs (long-term AKIA, temporary ASIA).
            r"(?:AKIA|ASIA)[0-9A-Z]{16}",
            // Bearer tokens in an Authorization-style context (redacts the token, keeps "Bearer").
            r"(?i)bearer\s+[A-Za-z0-9._~+/=-]{20,}",
            // Contextual secret assignments: `aws_secret_access_key = ...`, `api_key: ...`, etc.
            // Word-chars may surround the keyword (`aws_secret_access_key`), and the value is a
            // 12+ char high-entropy-shaped string.
            r#"(?i)[a-z0-9_]*(?:secret|token|api[_-]?key|access[_-]?key|password|passwd)[a-z0-9_]*\s*[:=]\s*["']?[A-Za-z0-9._~+/=-]{12,}["']?"#,
        ];
        raw.iter()
            .map(|p| Regex::new(p).expect("redact pattern is a valid regex"))
            .collect()
    })
}

/// Strip high-confidence secret shapes from `body`, returning the redacted text and the number of
/// secrets removed. A count of 0 means nothing matched. This is the only redaction entrypoint;
/// the enrich path must route every off-machine payload through it.
pub fn scrub(body: &str) -> (String, usize) {
    let mut text = body.to_string();
    let mut count = 0usize;
    for re in patterns() {
        count += re.find_iter(&text).count();
        text = re.replace_all(&text, PLACEHOLDER).into_owned();
    }
    debug!(
        "redact::scrub: input_chars={} redactions={}",
        body.chars().count(),
        count
    );
    (text, count)
}

#[cfg(test)]
mod tests;
