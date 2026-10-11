//! The HTML builder: the only way anything reaches the page.
//!
//! Four emit paths, and no other:
//!
//! - [`Page::raw`] writes structural markup. It takes `&'static str` only, so it can carry tags and
//!   class names chosen at compile time and never a value read from the report.
//! - [`Page::text`] writes an escaped text node.
//! - [`Page::attr`] writes an escaped human-readable attribute (`title`, `aria-label`, `data-tip`).
//! - [`Page::geom`] writes a numeric SVG geometry attribute, and refuses anything that is not
//!   geometry.
//!
//! Every [`Page::text`] and [`Page::attr`] value is recorded in a ledger (test builds only), which
//! is what the number-licensing test reads: the page's figures are checked where they enter, so the
//! test needs no HTML parser. Slot prose enters the ledger as its interpolated Markdown, before
//! comrak converts it; comrak's output is the one raw insertion of non-static markup and adds no
//! text of its own.

use std::fmt::Write as _;

/// A page under construction.
pub(super) struct Page {
    out: String,
    /// Every text node and human-readable attribute value written, in order.
    #[cfg(test)]
    pub(super) ledger: Vec<String>,
}

impl Page {
    pub(super) fn new() -> Self {
        Self {
            out: String::with_capacity(64 * 1024),
            #[cfg(test)]
            ledger: Vec::new(),
        }
    }

    /// Structural markup, compile-time literals only.
    pub(super) fn raw(&mut self, markup: &'static str) -> &mut Self {
        self.out.push_str(markup);
        self
    }

    /// An escaped text node.
    pub(super) fn text(&mut self, value: &str) -> &mut Self {
        self.record(value);
        self.out.push_str(&escape(value));
        self
    }

    /// ` name="value"`, value escaped. For human-readable attributes only; geometry goes through
    /// [`Page::geom`].
    pub(super) fn attr(&mut self, name: &'static str, value: &str) -> &mut Self {
        self.record(value);
        let _ = write!(self.out, " {name}=\"{}\"", escape(value));
        self
    }

    /// ` name="value"` for numeric SVG geometry: digits, spaces, commas, dots and minus signs only.
    /// Not recorded: a coordinate is not a figure a reader reads.
    pub(super) fn geom(&mut self, name: &'static str, value: &str) -> &mut Self {
        assert!(
            is_geometry(value),
            "geom() takes numeric SVG geometry only, got {name}={value:?}"
        );
        let _ = write!(self.out, " {name}=\"{value}\"");
        self
    }

    /// [`Page::geom`] for one integer coordinate.
    pub(super) fn geom_int(&mut self, name: &'static str, value: i64) -> &mut Self {
        let _ = write!(self.out, " {name}=\"{value}\"");
        self
    }

    /// One prose slot: its interpolated, already-verified Markdown, converted by comrak in its
    /// default (safe) mode, which drops raw HTML and `javascript:` links.
    pub(super) fn slot(&mut self, markdown: &str) -> &mut Self {
        self.record(markdown);
        self.out
            .push_str(&comrak::markdown_to_html(markdown, &comrak::Options::default()));
        self
    }

    pub(super) fn finish(self) -> String {
        self.out
    }

    #[cfg(test)]
    fn record(&mut self, value: &str) {
        self.ledger.push(value.to_string());
    }

    #[cfg(not(test))]
    fn record(&mut self, value: &str) {
        log::trace!("html::Page::record: bytes={}", value.len());
    }
}

/// `true` when `value` is a geometry string: non-empty, and only digits, spaces, commas, dots and
/// minus signs.
fn is_geometry(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, ' ' | ',' | '.' | '-'))
}

/// Escape for a text node or a double-quoted attribute value.
pub(super) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}
