#![allow(clippy::unwrap_used)]

use super::*;

/// Wrap inner body markup in a minimal, valid self-contained document so a test can focus on the
/// one thing it is probing (fences, doctype, closing tag, a single external reference).
fn doc(inner: &str) -> String {
    format!("<!doctype html><html><head></head><body>{inner}</body></html>")
}

// ---- SSE parse + stop_reason ------------------------------------------------------------------

fn sse(stop_reason: &str, deltas: &[&str]) -> String {
    let mut out = String::new();
    out.push_str("event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\"}}\n\n");
    out.push_str("event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0}\n\n");
    for d in deltas {
        out.push_str(&format!(
            "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":{}}}}}\n\n",
            serde_json::to_string(d).unwrap()
        ));
    }
    out.push_str("event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n");
    out.push_str(&format!(
        "event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"{stop_reason}\",\"stop_sequence\":null}},\"usage\":{{\"output_tokens\":42}}}}\n\n"
    ));
    out.push_str("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
    out
}

#[test]
fn parse_sse_accumulates_text_and_reads_end_turn() {
    let body = sse("end_turn", &["<!doctype html>", "<html></html>"]);
    let outcome = parse_sse_stream(&body).unwrap();
    assert_eq!(outcome.text, "<!doctype html><html></html>");
    assert_eq!(outcome.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(outcome.output_tokens, Some(42));
    check_stop_reason(outcome.stop_reason.as_deref()).expect("end_turn must not bail");
}

#[test]
fn parse_sse_then_stop_reason_bails_on_max_tokens() {
    let body = sse("max_tokens", &["<!doctype html><html>", "truncated..."]);
    let outcome = parse_sse_stream(&body).unwrap();
    assert_eq!(outcome.stop_reason.as_deref(), Some("max_tokens"));
    // The pure SSE parse surfaces the truncation; check_stop_reason turns it into a loud, actionable
    // exhaustion error naming the escape hatches.
    let err = check_stop_reason(outcome.stop_reason.as_deref()).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("max_tokens"), "err names the stop_reason: {msg}");
    assert!(
        msg.contains("--format markdown") && msg.contains("--since"),
        "err directs to the named fallbacks: {msg}"
    );
}

#[test]
fn check_stop_reason_missing_bails() {
    let err = check_stop_reason(None).unwrap_err();
    assert!(format!("{err}").contains("<missing>"));
}

#[test]
fn parse_sse_bails_on_malformed_data_line() {
    let body = "event: content_block_delta\ndata: {not valid json}\n\n";
    let err = parse_sse_stream(body).unwrap_err();
    assert!(format!("{err}").contains("SSE data line"));
}

// ---- fence stripping --------------------------------------------------------------------------

#[test]
fn postprocess_strips_html_fence() {
    let raw = format!("```html\n{}\n```", doc("<p>ok</p>"));
    let out = postprocess_html(&raw).unwrap();
    assert!(out.starts_with("<!doctype html>"), "fence removed: {out}");
    assert!(out.ends_with("</html>"));
}

#[test]
fn postprocess_strips_bare_fence() {
    let raw = format!("```\n{}\n```", doc("<p>ok</p>"));
    let out = postprocess_html(&raw).unwrap();
    assert!(out.starts_with("<!doctype html>"));
}

#[test]
fn postprocess_accepts_unfenced_document() {
    let raw = doc("<p>ok</p>");
    let out = postprocess_html(&raw).unwrap();
    assert_eq!(out, raw);
}

#[test]
fn postprocess_accepts_uppercase_doctype() {
    let raw = "<!DOCTYPE HTML><HTML><body>ok</body></HTML>";
    let out = postprocess_html(raw).unwrap();
    assert_eq!(out, raw);
}

#[test]
fn postprocess_accepts_html_tag_without_doctype() {
    let raw = "<html lang=\"en\"><body>ok</body></html>";
    let out = postprocess_html(raw).unwrap();
    assert_eq!(out, raw);
}

// ---- doctype / closing-tag validation ---------------------------------------------------------

#[test]
fn postprocess_bails_on_leading_prose() {
    let raw = format!("Here is your dashboard:\n\n{}", doc("<p>x</p>"));
    let err = postprocess_html(&raw).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("does not begin with"), "{msg}");
    assert!(msg.contains("Here is your dashboard"), "preview named: {msg}");
}

#[test]
fn postprocess_bails_on_trailing_content_after_close() {
    let raw = format!("{} and that's the report!", doc("<p>x</p>"));
    let err = postprocess_html(&raw).unwrap_err();
    assert!(format!("{err}").contains("does not end with </html>"));
}

#[test]
fn postprocess_allows_trailing_whitespace_after_close() {
    let raw = format!("{}\n  \n", doc("<p>x</p>"));
    let out = postprocess_html(&raw).unwrap();
    assert!(out.ends_with("</html>"));
}

// ---- external-resource static check -----------------------------------------------------------

#[test]
fn postprocess_rejects_external_src() {
    let raw = doc("<img src=\"https://cdn.example.com/logo.png\">");
    let err = postprocess_html(&raw).unwrap_err();
    assert!(format!("{err}").contains("external resource"), "{err}");
}

#[test]
fn postprocess_rejects_external_link_href() {
    let raw = doc("<link rel=\"stylesheet\" href=\"https://fonts.googleapis.com/css?family=Inter\">");
    // <link href> loads a resource; only <a href> is exempt.
    let err = postprocess_html(&raw).unwrap_err();
    assert!(format!("{err}").contains("external resource"), "{err}");
}

#[test]
fn postprocess_accepts_anchor_href_hyperlink() {
    let raw = doc("<a href=\"https://github.com/tatari-tv/clyde/pull/42\">PR #42</a>");
    let out = postprocess_html(&raw).expect("<a href> hyperlinks are exempt");
    assert!(out.contains("github.com"));
}

#[test]
fn postprocess_accepts_inline_and_local_references() {
    // data: URIs, local anchors, and relative paths are self-contained / navigational — allowed.
    let raw = doc("<img src=\"data:image/png;base64,AAAA\"><a href=\"#top\">top</a>\
         <svg><rect fill=\"url(#grad)\"/></svg>");
    let out = postprocess_html(&raw).expect("inline/local references must pass");
    assert!(out.contains("data:image"));
}

#[test]
fn postprocess_rejects_external_css_url() {
    let raw = doc("<style>body{background:url(https://cdn.example.com/bg.png)}</style>");
    let err = postprocess_html(&raw).unwrap_err();
    assert!(format!("{err}").contains("url("), "{err}");
}

#[test]
fn postprocess_rejects_external_import() {
    let raw = doc("<style>@import \"https://cdn.example.com/theme.css\";</style>");
    let err = postprocess_html(&raw).unwrap_err();
    assert!(format!("{err}").contains("@import"), "{err}");
}

#[test]
fn postprocess_rejects_fetch_call() {
    let raw = doc("<script>fetch('https://api.example.com/data').then(r=>r.json())</script>");
    let err = postprocess_html(&raw).unwrap_err();
    assert!(format!("{err}").contains("network API"), "{err}");
}

#[test]
fn postprocess_rejects_websocket() {
    let raw = doc("<script>const s = new WebSocket('wss://x');</script>");
    let err = postprocess_html(&raw).unwrap_err();
    assert!(format!("{err}").contains("network API"), "{err}");
}

// ---- Transport port: fake-driven end-to-end over markdown/html ---------------------------------

/// One recorded trip through the port. A named struct rather than a tuple so each assertion reads
/// as the field it is checking.
#[derive(Clone, Debug)]
struct Recorded {
    job: Job,
    model: String,
    system: String,
    prompt: String,
    json_body: String,
}

/// Records what the port was handed and returns a canned reply. Mirrors how `sessions` and
/// `efficiency` fake their `Completer`/`Narrator` ports: a fake that records, never a mock.
struct FakeTransport {
    reply: String,
    seen: std::cell::RefCell<Vec<Recorded>>,
}

impl FakeTransport {
    fn new(reply: impl Into<String>) -> Self {
        Self {
            reply: reply.into(),
            seen: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// The single recorded call, or a panic if the count is not exactly one.
    fn only_call(&self) -> Recorded {
        let seen = self.seen.borrow();
        assert_eq!(seen.len(), 1, "expected exactly one transport call, got {}", seen.len());
        seen[0].clone()
    }
}

impl Transport for FakeTransport {
    fn complete(&self, job: Job, model: &str, system: &str, prompt: &str, json_body: &str) -> Result<String> {
        self.seen.borrow_mut().push(Recorded {
            job,
            model: model.to_string(),
            system: system.to_string(),
            prompt: prompt.to_string(),
            json_body: json_body.to_string(),
        });
        Ok(self.reply.clone())
    }
}

/// A transport that always fails, to prove the error propagates rather than yielding an artifact.
struct FailingTransport;

impl Transport for FailingTransport {
    fn complete(&self, _: Job, _: &str, _: &str, _: &str, _: &str) -> Result<String> {
        bail!("transport exploded")
    }
}

#[test]
fn markdown_passes_job_model_and_system_prompt_through() {
    let t = FakeTransport::new("# Report\n\nprose");
    let out = markdown(&t, "some-model", "instruction", "{\"k\":1}").unwrap();
    assert_eq!(out, "# Report\n\nprose");
    let call = t.only_call();
    assert_eq!(call.job, Job::Markdown);
    assert_eq!(call.model, "some-model");
    assert_eq!(call.system, MARKDOWN_SYSTEM_PROMPT);
    // prompt and json_body stay SEPARATE across the port; joining is the transport's business.
    assert_eq!(call.prompt, "instruction");
    assert_eq!(call.json_body, "{\"k\":1}");
}

#[test]
fn html_passes_job_model_and_system_prompt_through() {
    let t = FakeTransport::new(doc("<h1>hi</h1>"));
    let out = html(&t, "other-model", "instruction", "{\"k\":1}").unwrap();
    assert!(out.starts_with("<!doctype html>"));
    let call = t.only_call();
    assert_eq!(call.job, Job::Html);
    assert_eq!(call.model, "other-model");
    assert_eq!(call.system, HTML_SYSTEM_PROMPT);
    assert_eq!(call.prompt, "instruction");
    assert_eq!(call.json_body, "{\"k\":1}");
}

#[test]
fn html_postprocesses_whatever_the_transport_returns() {
    // The guard runs AFTER the transport, so it is transport-agnostic: a fenced reply is stripped
    // no matter which transport produced it. This is the safety argument for a second transport.
    let t = FakeTransport::new(format!("```html\n{}\n```", doc("<p>x</p>")));
    let out = html(&t, "m", "p", "{}").unwrap();
    assert!(out.starts_with("<!doctype html>"), "fence should be stripped: {out}");
    assert!(out.trim_end().ends_with("</html>"));
}

#[test]
fn html_bails_when_the_transport_returns_a_non_document() {
    // Proven to bite: swap this reply for a valid document and the test fails.
    let t = FakeTransport::new("Here is your dashboard!");
    let err = html(&t, "m", "p", "{}").unwrap_err().to_string();
    assert!(
        err.contains("<!doctype html>"),
        "should name the doctype requirement: {err}"
    );
}

#[test]
fn html_bails_when_the_transport_returns_an_externally_dependent_document() {
    let t = FakeTransport::new(doc("<script src=\"https://cdn.example.com/x.js\"></script>"));
    let err = html(&t, "m", "p", "{}").unwrap_err().to_string();
    assert!(err.contains("self-contained"), "should name self-containment: {err}");
}

#[test]
fn markdown_propagates_a_transport_failure() {
    let err = markdown(&FailingTransport, "m", "p", "{}").unwrap_err().to_string();
    assert!(err.contains("transport exploded"), "got: {err}");
}

#[test]
fn html_propagates_a_transport_failure() {
    let err = html(&FailingTransport, "m", "p", "{}").unwrap_err().to_string();
    assert!(err.contains("transport exploded"), "got: {err}");
}
