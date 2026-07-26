pub mod api;
pub mod cli;

pub use api::{ApiTransport, api_key_from_env};
pub use cli::CliTransport;

use eyre::{Context, Result, bail};
use log::debug;
use serde::Deserialize;

/// One prose completion: the job's system prompt plus its instruction and facts -> the model's text
/// reply. Implementations own their own transport knobs, so nothing here leaks an api-only or
/// cli-only concept.
///
/// `prompt` and `json_body` stay SEPARATE arguments deliberately. The api transport joins them into
/// one user message; the cli transport must deliver them over two different channels (instruction
/// on argv, facts on stdin), and a pre-joined string would force it to either re-split a 500KB blob
/// or push the whole thing through argv into `ARG_MAX`.
pub trait Transport {
    fn complete(&self, job: Job<'_>, system: &str, prompt: &str, json_body: &str) -> Result<String>;
}

/// WHICH artifact a job produces. Stays a `Copy` enum because it IS a compile-time fact: the two
/// arms are the two call sites in `render.rs`, and nothing about the choice is user-configurable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Markdown,
    Html,
    /// `clyde report eval`'s judge: a small JSON verdict over an already-rendered artifact
    /// (design Phase 13). It rides the existing transport rather than a second client, so the eval
    /// inherits `--llm` and needs no second credential.
    Judge,
}

impl Kind {
    /// The `clyde.yml` key carrying this job's output ceiling.
    ///
    /// Exists so a ceiling failure can name the ONE line that prevents the next one. Per-kind on
    /// purpose: naming the markdown key on an html failure would be a remedy that cannot remedy, which
    /// `cli.rs`'s module docs call worse than offering none. A compile-time fact about the kind, unlike
    /// the ceiling itself.
    ///
    /// [`Kind::Judge`] names the MARKDOWN key, and that is not a stand-in: the eval passes
    /// `render.markdown-max-output-tokens` as the judge's ceiling, so the key this error names is
    /// the key that actually governs it. The eval deliberately adds no ceiling key of its own -- a
    /// four-object verdict cannot plausibly need a budget the markdown document does not.
    pub fn max_output_tokens_key(self) -> &'static str {
        match self {
            Kind::Markdown | Kind::Judge => "render.markdown-max-output-tokens",
            Kind::Html => "render.html-max-output-tokens",
        }
    }
}

/// A render job with its user-configurable pins RESOLVED from `clyde.yml`.
///
/// Every per-job tunable lands here. Both fields were once compile-time facts reachable as methods on
/// the old `Job` enum (`Job::model()`, then `Job::max_output_tokens()`), and both stopped being
/// expressible the moment a user could set them: a `Copy` enum arm cannot return a `String` it does
/// not own, nor a number it has not read. Bundling them means the NEXT configurable per-job value is
/// a field, not a sixth argument on [`Transport::complete`].
#[derive(Clone, Copy, Debug)]
pub struct Job<'a> {
    pub kind: Kind,
    /// `render.markdown-model` / `render.html-model`.
    pub model: &'a str,
    /// `render.markdown-max-output-tokens` / `render.html-max-output-tokens`.
    ///
    /// SHARED by both transports: the api transport SETS it as `max_tokens` on the wire, and the cli
    /// transport — which cannot set a ceiling at all — CHECKS the returned `usage.output_tokens`
    /// against it. Streaming, by contrast, is api-private and lives in `api.rs`, because the cli
    /// transport has no delivery choice to make and a `stream` field it ignored would be a lying
    /// field.
    pub max_output_tokens: u32,
}

const MARKDOWN_SYSTEM_PROMPT: &str = "You are a precise technical writer producing markdown documents from structured data. Output exactly what is asked - no preamble, no commentary, no fenced code block wrapping the whole output.";
/// Phase 0-verified wording. The `\`-continued string is one logical line (no embedded newlines
/// beyond the single spaces the continuations preserve).
const HTML_SYSTEM_PROMPT: &str = "You are producing a complete, self-contained HTML document from structured data. \
     Output ONLY the HTML document - no preamble, no commentary, no markdown fences. \
     Your reply begins with <!doctype html> and ends with </html>.";

/// Markdown-source render over any transport. The `Job` is assembled HERE, from the config-resolved
/// pins the caller passes, so `render.rs` never constructs one.
pub fn markdown<T: Transport>(
    transport: &T,
    model: &str,
    max_output_tokens: u32,
    prompt: &str,
    json_body: &str,
) -> Result<String> {
    debug!(
        "summarize::markdown: model={model} max_output_tokens={max_output_tokens} json bytes={}",
        json_body.len()
    );
    let job = Job {
        kind: Kind::Markdown,
        model,
        max_output_tokens,
    };
    transport.complete(job, MARKDOWN_SYSTEM_PROMPT, prompt, json_body)
}

/// Html-source render over any transport. The returned document is fence-stripped and validated
/// (doctype, closing tag, self-containment) before it is handed back.
///
/// [`postprocess_html`] runs HERE, after the transport returns, so it is transport-agnostic: it
/// cannot be weakened by swapping the delivery mechanism. Same property as `reject_foreign_numbers`
/// in `render.rs`. That is the whole safety argument for adding a second transport.
pub fn html<T: Transport>(
    transport: &T,
    model: &str,
    max_output_tokens: u32,
    prompt: &str,
    json_body: &str,
) -> Result<String> {
    debug!(
        "summarize::html: model={model} max_output_tokens={max_output_tokens} json bytes={}",
        json_body.len()
    );
    let job = Job {
        kind: Kind::Html,
        model,
        max_output_tokens,
    };
    let raw = transport.complete(job, HTML_SYSTEM_PROMPT, prompt, json_body)?;
    postprocess_html(&raw)
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
