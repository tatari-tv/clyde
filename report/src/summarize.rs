use eyre::{Context, Result, bail};
use log::{debug, info};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Markdown-path model, pinned. The markdown-source output is byte-identical to the pre-HTML
/// behavior, and the model is part of that contract - it must not move without re-baselining.
const MARKDOWN_MODEL: &str = "claude-opus-4-7";
/// Html-path model. Bumped to opus-4-8 (Scott, Phase 6 shakedown 2026-07-06) for its stronger
/// design/prose sense. Same request surface as 4-7 (adaptive-thinking-only, no sampling params)
/// and the same 128K output ceiling / 1M context, so HTML_MAX_OUTPUT_TOKENS below is unchanged.
const HTML_MODEL: &str = "claude-opus-4-8";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
/// Non-streaming markdown-source ceiling (unchanged from the pre-HTML design). The markdown path
/// stays byte-identical, so this value and the prompt below must not move.
const MARKDOWN_MAX_OUTPUT_TOKENS: u32 = 16_000;
/// Streaming html-source ceiling. Phase 0 observed a max 26.5K output on a 5x-synthetic month;
/// 64K is half the opus-4-7 128K output ceiling, so the named-exhaustion bail is a backstop that
/// will not fire for realistic months.
const HTML_MAX_OUTPUT_TOKENS: u32 = 64_000;
const HTTP_TIMEOUT: Duration = Duration::from_secs(300);
const MARKDOWN_SYSTEM_PROMPT: &str = "You are a precise technical writer producing markdown documents from structured data. Output exactly what is asked - no preamble, no commentary, no fenced code block wrapping the whole output.";
/// Phase 0-verified wording. The `\`-continued string is one logical line (no embedded newlines
/// beyond the single spaces the continuations preserve).
const HTML_SYSTEM_PROMPT: &str = "You are producing a complete, self-contained HTML document from structured data. \
     Output ONLY the HTML document - no preamble, no commentary, no markdown fences. \
     Your reply begins with <!doctype html> and ends with </html>.";

/// Markdown-source render: non-streaming, byte-identical to the pre-HTML behavior for a successful
/// `end_turn` response (the truncation unhappy path now bails loudly instead of silently clipping).
pub fn markdown(prompt: &str, json_body: &str, api_key: &str) -> Result<String> {
    debug!("summarize::markdown: json bytes={}", json_body.len());
    request(
        MARKDOWN_MODEL,
        MARKDOWN_SYSTEM_PROMPT,
        MARKDOWN_MAX_OUTPUT_TOKENS,
        false,
        prompt,
        json_body,
        api_key,
    )
}

/// Html-source render: streaming (SSE) so the connection keeps flowing bytes and the 300s idle
/// wall never fires on a long generation. The accumulated document is fence-stripped and validated
/// (doctype, closing tag, self-containment) before it is returned.
pub fn html(prompt: &str, json_body: &str, api_key: &str) -> Result<String> {
    debug!("summarize::html: json bytes={}", json_body.len());
    let raw = request(
        HTML_MODEL,
        HTML_SYSTEM_PROMPT,
        HTML_MAX_OUTPUT_TOKENS,
        true,
        prompt,
        json_body,
        api_key,
    )?;
    postprocess_html(&raw)
}

/// Shared Anthropic Messages call. `stream=false` reads a single JSON response body; `stream=true`
/// reads the SSE body synchronously through the same `ureq` agent (no async runtime), accumulating
/// `text_delta` text and the terminal `message_delta`'s `stop_reason`. Both paths bail when
/// `stop_reason` is not `end_turn` (a max-tokens truncation is a loud, actionable error, never a
/// silently clipped artifact).
fn request(
    model: &str,
    system: &str,
    max_tokens: u32,
    stream: bool,
    prompt: &str,
    json_body: &str,
    api_key: &str,
) -> Result<String> {
    let user_msg = format!("{}\n\n```json\n{}\n```\n", prompt.trim_end(), json_body);
    debug!(
        "summarize::request: system bytes={} max_tokens={} stream={} prompt+json bytes={}",
        system.len(),
        max_tokens,
        stream,
        user_msg.len()
    );

    let body = MessagesRequest {
        model: model.into(),
        max_tokens,
        stream,
        system: system.into(),
        messages: vec![Message {
            role: "user".into(),
            content: user_msg,
        }],
    };

    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .build()
        .new_agent();

    info!("summarize::request: calling {} ({}) stream={}", ENDPOINT, model, stream);
    let mut response = agent
        .post(ENDPOINT)
        .header("x-api-key", api_key)
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
            "summarize::request: stream complete output_tokens={:?} stop_reason={:?} text bytes={}",
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

/// Bail unless the model finished on its own (`end_turn`). A `max_tokens` (or any non-`end_turn`)
/// stop is the named output-exhaustion failure mode: the artifact exceeded the model's output
/// ceiling, so it is truncated and must not be published. Pure, so the SSE-parse tests can drive it.
fn check_stop_reason(stop_reason: Option<&str>) -> Result<()> {
    match stop_reason {
        Some("end_turn") => Ok(()),
        other => bail!(
            "Anthropic API stopped with stop_reason={} (expected end_turn): the generated artifact \
             exceeded the model's output ceiling and was truncated. Re-run with --format markdown or \
             --format pdf, or narrow the window with a shorter --since, then try again.",
            other.unwrap_or("<missing>")
        ),
    }
}

/// Post-process a raw html-source model reply into a validated, self-contained HTML document.
/// Fails loudly and closed at each step (design "API Design", four steps): fence strip, doctype
/// assert, closing-tag/trailing-content assert, external-resource static check. Pure.
fn postprocess_html(raw: &str) -> Result<String> {
    debug!("summarize::postprocess_html: raw bytes={}", raw.len());
    // Step 1: trim and strip a single wrapping ```html / ``` fence pair (defense in depth).
    let doc = strip_fence(raw);

    // Step 2: assert the document starts with <!doctype html> or <html (case-insensitive).
    let head_lower = doc.trim_start().to_ascii_lowercase();
    if !(head_lower.starts_with("<!doctype html") || head_lower.starts_with("<html")) {
        let preview: String = doc.chars().take(120).collect();
        bail!(
            "html-source reply does not begin with <!doctype html> or <html; refusing to publish a \
             malformed artifact. First 120 chars received: {preview:?}"
        );
    }

    // Step 3: assert it ends with </html> (trailing whitespace allowed; trailing prose rejected).
    if !doc.trim_end().to_ascii_lowercase().ends_with("</html>") {
        let tail: String = doc
            .chars()
            .rev()
            .take(120)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        bail!(
            "html-source reply does not end with </html> (truncated or trailing prose); refusing to \
             publish. Last 120 chars received: {tail:?}"
        );
    }

    // Step 4: static external-resource check (load-bearing; marquee's CSP permits CDNs).
    check_self_contained(&doc)?;

    debug!("summarize::postprocess_html: validated bytes={}", doc.len());
    Ok(doc)
}

/// Strip a single wrapping ```html / ``` fence pair when the WHOLE reply is fenced; otherwise
/// return the trimmed input unchanged. Byte-slice free (line-based) per the crate lint. Pure.
fn strip_fence(raw: &str) -> String {
    let trimmed = raw.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }
    let mut lines: Vec<&str> = trimmed.lines().collect();
    // Need an opening fence line and a closing fence line at minimum.
    if lines.len() < 2 || lines.last().map(|l| l.trim()) != Some("```") {
        return trimmed.to_string();
    }
    lines.remove(0); // opening ```html / ```
    lines.pop(); // closing ```
    lines.join("\n").trim().to_string()
}

/// Reject any external-origin resource load or runtime network call. `<a href>` navigation is
/// exempt (a link navigates; it does not load a resource into the artifact). Pure.
fn check_self_contained(html: &str) -> Result<()> {
    debug!("summarize::check_self_contained: html bytes={}", html.len());
    let lower = html.to_ascii_lowercase();

    // Runtime network APIs — never legitimate in a self-contained dashboard.
    for needle in ["fetch(", "xmlhttprequest", "websocket"] {
        if lower.contains(needle) {
            bail!(
                "html-source reply uses a runtime network API (`{needle}`); the published dashboard \
                 must be fully self-contained (inline data only, no external calls)"
            );
        }
    }

    // CSS url(...) and @import pointing at an external origin (case-insensitive scan on `lower`).
    for piece in lower.split("url(").skip(1) {
        if let Some((inner, _)) = piece.split_once(')')
            && is_external_url(inner)
        {
            bail!(
                "html-source reply references an external resource via `url({})`; the dashboard must \
                 be self-contained",
                inner.trim()
            );
        }
    }
    for piece in lower.split("@import").skip(1) {
        let decl = piece.split_once(';').map(|(d, _)| d).unwrap_or(piece);
        if is_external_url(decl) {
            bail!(
                "html-source reply references an external stylesheet via `@import {}`; the dashboard \
                 must be self-contained",
                decl.trim()
            );
        }
    }

    // src= (any element) and href= (except <a> navigation) pointing at an external origin.
    for piece in html.split('<') {
        let Some((tag_body, _)) = piece.split_once('>') else {
            continue;
        };
        let tag_name = tag_body
            .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if tag_name.starts_with('!') {
            continue; // <!doctype ...>, comments
        }
        for (name, value) in parse_attrs(tag_body) {
            if !is_external_url(&value) {
                continue;
            }
            if name == "src" {
                bail!(
                    "html-source reply loads an external resource via src=\"{value}\" on <{tag_name}>; \
                     the dashboard must be self-contained"
                );
            }
            if name == "href" && tag_name != "a" {
                bail!(
                    "html-source reply loads an external resource via <{tag_name} href=\"{value}\">; \
                     the dashboard must be self-contained (<a href> hyperlinks are exempt)"
                );
            }
        }
    }
    Ok(())
}

/// True when the (possibly quoted) value points at an external origin: `http://`, `https://`, a
/// protocol-relative `//host`, or `ftp://`. `data:` URIs, `#anchors`, `mailto:`, and relative paths
/// are NOT external. Pure.
fn is_external_url(raw: &str) -> bool {
    let v = raw.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    let lower = v.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("//")
        || lower.starts_with("ftp://")
}

/// Parse the attributes of a tag body (`a href="x" class='y' disabled`) into `(name, value)` pairs,
/// names lowercased. A tiny char-based tokenizer (byte-slice free per the crate lint); good enough
/// for the static self-containment check on model-authored HTML. Pure.
fn parse_attrs(tag_body: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    let mut chars = tag_body.chars().peekable();
    // Skip the tag name.
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            break;
        }
        chars.next();
    }
    loop {
        while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
            chars.next();
        }
        let mut name = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() || c == '=' || c == '/' {
                break;
            }
            name.push(c);
            chars.next();
        }
        if name.is_empty() {
            // Consume one char to guarantee progress (e.g. a stray '/').
            if chars.next().is_none() {
                break;
            }
            continue;
        }
        while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
            chars.next();
        }
        if chars.peek() == Some(&'=') {
            chars.next();
            while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
                chars.next();
            }
            let mut value = String::new();
            match chars.peek().copied() {
                Some(q @ '"') | Some(q @ '\'') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if c == q {
                            break;
                        }
                        value.push(c);
                    }
                }
                _ => {
                    while let Some(&c) = chars.peek() {
                        if c.is_whitespace() || c == '>' {
                            break;
                        }
                        value.push(c);
                        chars.next();
                    }
                }
            }
            attrs.push((name.to_ascii_lowercase(), value));
        } else {
            attrs.push((name.to_ascii_lowercase(), String::new()));
        }
        if chars.peek().is_none() {
            break;
        }
    }
    attrs
}

/// The accumulated result of reading an Anthropic SSE stream body.
#[derive(Debug)]
struct StreamOutcome {
    text: String,
    stop_reason: Option<String>,
    output_tokens: Option<u64>,
}

/// Parse an Anthropic SSE body: accumulate `text_delta` text across `content_block_delta` events
/// and read the terminal `message_delta`'s `stop_reason`/`usage`. A malformed `data:` line is a
/// hard error (fail loudly). Pure, so it is unit-testable with injected fixtures.
fn parse_sse_stream(body: &str) -> Result<StreamOutcome> {
    debug!("summarize::parse_sse_stream: body bytes={}", body.len());
    let mut text = String::new();
    let mut stop_reason = None;
    let mut output_tokens = None;
    for line in body.lines() {
        let Some(payload) = line.trim_start().strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        let event: SseEvent =
            serde_json::from_str(payload).with_context(|| "failed to parse Anthropic SSE data line")?;
        match event.r#type.as_str() {
            "content_block_delta" => {
                if let Some(delta) = event.delta
                    && delta.r#type.as_deref() == Some("text_delta")
                    && let Some(t) = delta.text
                {
                    text.push_str(&t);
                }
            }
            "message_delta" => {
                if let Some(delta) = event.delta
                    && let Some(sr) = delta.stop_reason
                {
                    stop_reason = Some(sr);
                }
                if let Some(usage) = event.usage {
                    output_tokens = usage.output_tokens;
                }
            }
            _ => {}
        }
    }
    Ok(StreamOutcome {
        text,
        stop_reason,
        output_tokens,
    })
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

/// One SSE event's JSON payload. Unknown fields (`index`, `stop_sequence`, ...) are tolerated by
/// design — this is a wire frame from a newer peer, not an owned config struct.
#[derive(Deserialize)]
struct SseEvent {
    r#type: String,
    delta: Option<SseDelta>,
    usage: Option<SseUsage>,
}

#[derive(Deserialize)]
struct SseDelta {
    r#type: Option<String>,
    text: Option<String>,
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct SseUsage {
    output_tokens: Option<u64>,
}

#[cfg(test)]
mod tests;
