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
