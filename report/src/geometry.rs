//! Geometry validation, as an allowlist over the SVG subtree (design Phase 11, "Geometry
//! validation, as an allowlist over the SVG subtree").
//!
//! The reason this is an allowlist and not a two-attribute spot check is worse than it looks:
//! **no HTML attribute has ever been number-checked.** `render::visible_text` strips all tag
//! markup INCLUDING attributes, and `reject_foreign_numbers` runs on its output, so the only thing
//! that ever kept model-authored geometry out of the artifact was prompt text. Phase 11 lifts that
//! prompt ban for `<svg>` and `<polyline>`; checking only `viewBox` and `points` would leave
//! `<path d>`, `x`/`y`/`x1`/`y1`, `cx`/`cy`/`r`, `rect width`/`height`, `text x`/`y` and
//! `transform` entirely unchecked, which is the whole class the ban existed to close.
//!
//! So, inside a chart subtree (anything within an `<svg>`):
//!
//! - the element must be in [`PERMITTED_ELEMENTS`];
//! - every attribute must be in [`PERMITTED_ATTRIBUTES`];
//! - and every attribute value containing a DIGIT must appear verbatim in the geometry fact set.
//!
//! The last rule is what makes this fail closed: an attribute nobody anticipated is rejected rather
//! than passing unexamined, and the only values that pass are the ones `chart.rs` computed. The
//! geometry set is separate from the prose whitelist (`quotable`), so a `points` string cannot
//! quietly license dozens of small integers in the narrative.

use crate::quotable::QuotableFacts;
use eyre::{Result, bail};
use log::{debug, trace};

/// The element that opens a chart subtree. Every `<svg>` in the artifact is treated as one: the
/// prompt authorizes no other SVG, so an `<svg>` that is not a chart is itself a violation.
const CHART_ELEMENT: &str = "svg";

/// Elements permitted inside a chart subtree. Anything else -- `path`, `circle`, `rect`, `line`,
/// `image`, `foreignObject` -- fails the render.
const PERMITTED_ELEMENTS: &[&str] = &["svg", "polyline", "g", "text", "title"];

/// Attributes permitted on those elements: the two verbatim geometry carriers, plus presentation
/// attributes that carry no geometry. Compared lowercased, because HTML attribute names are
/// case-insensitive and the authored spelling is `viewBox`.
///
/// Being permitted is NOT a licence to carry a number: a digit-bearing value still has to be in the
/// geometry set, so `stroke-width="2"` is rejected and belongs in the stylesheet.
const PERMITTED_ATTRIBUTES: &[&str] = &["viewbox", "points", "class", "fill", "stroke", "stroke-width"];

/// One parsed tag. Attribute names are lowercased; values are kept EXACTLY as authored, because the
/// geometry check is byte for byte.
#[derive(Debug, PartialEq)]
struct Tag {
    name: String,
    closing: bool,
    self_closing: bool,
    attrs: Vec<(String, String)>,
}

/// Reject an artifact whose chart subtrees contain anything the binary did not compute. `kind`
/// names the render path for the operator-facing error, mirroring `reject_foreign_numbers`.
///
/// `<script>` / `<style>` block contents are stripped first, for the same reason the prose guard
/// strips them: their numbers are authored CSS/JS geometry, not data, and an `<svg` inside a JS
/// string is not markup the reader ever sees.
pub(crate) fn reject_foreign_geometry(kind: &str, html: &str, facts: &QuotableFacts) -> Result<()> {
    let markup = crate::render::strip_blocks(&crate::render::strip_blocks(html, "script"), "style");
    let tags = tags(&markup);
    debug!(
        "geometry::reject_foreign_geometry: kind={kind} html_bytes={} tags={} licensed_geometry={}",
        html.len(),
        tags.len(),
        facts.geometry_count()
    );

    let mut depth = 0usize;
    let mut checked = 0usize;
    for tag in &tags {
        let opens = tag.name == CHART_ELEMENT && !tag.closing;
        if opens {
            depth += 1;
        }
        if depth > 0 {
            checked += 1;
            check(kind, tag, facts)?;
        }
        if (opens && tag.self_closing) || (tag.name == CHART_ELEMENT && tag.closing) {
            depth = depth.saturating_sub(1);
        }
    }
    debug!("geometry::reject_foreign_geometry: kind={kind} clean, chart-subtree tags checked={checked}");
    Ok(())
}

/// One tag against the allowlist. Fails on the first violation, naming it: a rejected render costs
/// a paid model call, so the error has to say exactly what to fix.
fn check(kind: &str, tag: &Tag, facts: &QuotableFacts) -> Result<()> {
    trace!(
        "geometry::check: element={} closing={} attrs={}",
        tag.name,
        tag.closing,
        tag.attrs.len()
    );
    if !PERMITTED_ELEMENTS.contains(&tag.name.as_str()) {
        log::warn!(
            "geometry::check: {kind} path REJECTED -- <{}> inside a chart subtree",
            tag.name
        );
        bail!(
            "{kind} rendering put a <{}> inside an <svg> chart subtree; only {} are permitted there -- \
             chart geometry is copied from the context block, never authored; refusing to emit the artifact",
            tag.name,
            PERMITTED_ELEMENTS.join(", ")
        );
    }
    for (name, value) in &tag.attrs {
        if !PERMITTED_ATTRIBUTES.contains(&name.as_str()) {
            log::warn!(
                "geometry::check: {kind} path REJECTED -- attribute {name} on <{}> inside a chart subtree",
                tag.name
            );
            bail!(
                "{kind} rendering wrote `{name}` on <{}> inside an <svg> chart subtree; only {} are \
                 permitted there -- refusing to emit the artifact",
                tag.name,
                PERMITTED_ATTRIBUTES.join(", ")
            );
        }
        if !value.chars().any(|c| c.is_ascii_digit()) {
            continue;
        }
        if !facts.licenses_geometry(value) {
            log::warn!(
                "geometry::check: {kind} path REJECTED -- unlicensed geometry in {name} on <{}>",
                tag.name
            );
            bail!(
                "{kind} rendering wrote `{name}=\"{value}\"` on <{}> inside an <svg> chart subtree, and \
                 that value is not one the binary computed -- every number inside a chart is copied \
                 verbatim from `aggregates.charts`; refusing to emit the artifact",
                tag.name
            );
        }
        trace!("geometry::check: {name} on <{}> matches licensed geometry", tag.name);
    }
    Ok(())
}

/// Every tag in `markup`, in document order. Tolerant by design (the input is model-authored, not
/// guaranteed well-formed) and char-based per the crate's no-string-slice lint. Comments and
/// doctypes are skipped; a bare `<` in text is not a tag.
fn tags(markup: &str) -> Vec<Tag> {
    let chars: Vec<char> = markup.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if chars.get(i) != Some(&'<') {
            i += 1;
            continue;
        }
        if chars.get(i + 1) == Some(&'!') {
            i = skip_bang(&chars, i);
            continue;
        }
        let (tag, next) = parse_tag(&chars, i);
        if let Some(tag) = tag {
            out.push(tag);
        }
        i = next.max(i + 1);
    }
    out
}

/// Skip a `<!-- comment -->` or a `<!doctype ...>`, returning the index just past it. An unclosed
/// comment consumes the remainder, which drops it from the scan the same way `strip_blocks` drops
/// an unclosed block.
fn skip_bang(chars: &[char], start: usize) -> usize {
    let comment = chars.get(start + 2) == Some(&'-') && chars.get(start + 3) == Some(&'-');
    let mut i = start + 2;
    while i < chars.len() {
        if comment {
            if chars.get(i) == Some(&'-') && chars.get(i + 1) == Some(&'-') && chars.get(i + 2) == Some(&'>') {
                return i + 3;
            }
        } else if chars.get(i) == Some(&'>') {
            return i + 1;
        }
        i += 1;
    }
    chars.len()
}

/// Parse one tag starting at the `<` in `chars`, returning it and the index just past its `>`.
/// `None` when what follows the `<` is not an element name (a stray `<` in prose).
fn parse_tag(chars: &[char], start: usize) -> (Option<Tag>, usize) {
    let mut i = start + 1;
    let closing = chars.get(i) == Some(&'/');
    if closing {
        i += 1;
    }
    let mut name = String::new();
    while let Some(&c) = chars.get(i) {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':' {
            name.push(c.to_ascii_lowercase());
            i += 1;
        } else {
            break;
        }
    }
    if name.is_empty() {
        return (None, start + 1);
    }

    let mut attrs = Vec::new();
    let mut self_closing = false;
    loop {
        while chars.get(i).is_some_and(|c| c.is_whitespace()) {
            i += 1;
        }
        match chars.get(i) {
            None => break,
            Some('>') => {
                i += 1;
                break;
            }
            Some('/') => {
                self_closing = true;
                i += 1;
            }
            Some(_) => {
                let (attr, next) = parse_attr(chars, i);
                if let Some(attr) = attr {
                    attrs.push(attr);
                }
                i = next.max(i + 1);
            }
        }
    }
    (
        Some(Tag {
            name,
            closing,
            self_closing,
            attrs,
        }),
        i,
    )
}

/// Parse one attribute at `start`, returning `(name, value)` and the index just past it. A valueless
/// attribute yields an empty value (which carries no digit and so passes the geometry check on its
/// name alone). Quoted, single-quoted and unquoted values are all handled; the value is returned
/// EXACTLY as authored.
fn parse_attr(chars: &[char], start: usize) -> (Option<(String, String)>, usize) {
    let mut i = start;
    let mut name = String::new();
    while let Some(&c) = chars.get(i) {
        if c.is_whitespace() || c == '=' || c == '>' || c == '/' {
            break;
        }
        name.push(c.to_ascii_lowercase());
        i += 1;
    }
    if name.is_empty() {
        return (None, i);
    }
    while chars.get(i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }
    if chars.get(i) != Some(&'=') {
        return (Some((name, String::new())), i);
    }
    i += 1;
    while chars.get(i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }

    let mut value = String::new();
    match chars.get(i) {
        Some(&quote) if quote == '"' || quote == '\'' => {
            i += 1;
            while let Some(&c) = chars.get(i) {
                i += 1;
                if c == quote {
                    break;
                }
                value.push(c);
            }
        }
        _ => {
            while let Some(&c) = chars.get(i) {
                if c.is_whitespace() || c == '>' {
                    break;
                }
                value.push(c);
                i += 1;
            }
        }
    }
    (Some((name, value)), i)
}

#[cfg(test)]
mod tests;
