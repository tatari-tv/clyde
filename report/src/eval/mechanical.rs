//! The mechanical layer: deterministic, offline, free checks over a rendered artifact.
//!
//! These are the checks `otto ci` runs against the COMMITTED goldens (design Phase 13: "Mechanical
//! checks, deterministic and free, run in `otto ci` against the goldens"). Nothing here calls a
//! model, reads the network, or looks at a clock, so a golden either passes forever or the change
//! that broke it is in this repo. `otto eval` runs the same checks against a FRESH render before
//! the judge sees it, so a paid run never scores an artifact the free layer already rejected.
//!
//! Every check answers one question, and a failure NAMES the offending value: a rejected render
//! costs a model call, so "something was wrong" is not an acceptable error.
//!
//! | check | question |
//! |---|---|
//! | `cited-repos` | is every `<org>/<repo>` the artifact names in the context? |
//! | `cited-dates` | is every calendar date the artifact names in the context? |
//! | `cited-titles` | is every quoted phrase the artifact attributes to the data in the context? |
//! | `required-sections` | did the render emit every section this fixture requires? |
//! | `forbidden-sections` | did it stay silent where the data says it must? |
//! | `required-citations` | did it exercise the citation shapes the whitelist most easily breaks? |
//! | `speculative-quantification` | is Hard prohibition 2's phrase list absent? |
//! | `em-dash` | is U+2014 absent, as both templates require? |
//! | `foreign-figures` | is every number licensed by a quotable fact? |
//! | `chart-geometry` | is every digit-bearing chart attribute one the binary computed? |

use crate::geometry;
use crate::quotable::RenderContext;
use crate::render::visible_text;
use eyre::Result;
use log::debug;
use regex::Regex;
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::OnceLock;

use super::fixture::{Citation, Spec};

/// Shortest quoted span treated as a claimed citation of the data, in chars and in words. Below
/// EITHER bar a quoted fragment is a label or a figure, not a title or a summary phrase, and
/// demanding it appear verbatim in the context rejects ordinary prose: a first cut at 12 chars and
/// one space flagged the table labels `"Lines written"` and `"lines replaced"`, which the prompt
/// itself tells the model to write. A session title clears both bars comfortably
/// ("Fix the flaky snapshot test" is 27 chars and 5 words).
const MIN_QUOTED_CHARS: usize = 20;
const MIN_QUOTED_WORDS: usize = 4;

/// Hard prohibition 2's phrase list, as the mechanical form of the ban both templates carry in
/// prose. Matched case-insensitively on word boundaries -- a bare substring scan makes `fte` match
/// "after".
///
/// "would have cost" is deliberately NOT on the list, and a first cut that included it rejected a
/// correct render on its first real run. The cache counterfactual is the ONE quantification both
/// prompts sanction ("only using the precomputed figures in `aggregates.cache`"), and "what the
/// same tokens would have cost at fresh-input rates" is how that binary-computed figure reads in
/// English. Banning the phrase would ban the exception.
const SPECULATIVE_PHRASES: &[&str] = &[
    "senior engineer",
    "engineering time",
    "engineer-hours",
    "person-hours",
    "man-hours",
    "would have required",
    "would have taken",
    "productivity lift",
    "pays for itself",
    "the math works out",
    "roi",
    "return on investment",
    "headcount",
    "fte",
    "full-time equivalent",
    "fully-loaded",
    "saves the company",
    "a small team",
];

/// The em-dash both templates ban by name.
const EM_DASH: char = '\u{2014}';

/// The YAML frontmatter delimiter the markdown template opens and closes its header block with.
const FRONTMATTER_FENCE: &str = "---";

/// Which render path an artifact came from. The prose guard runs over the raw markdown, or over an
/// HTML document's VISIBLE TEXT (authored CSS/JS numbers are geometry, not data), and the section
/// checks apply to the markdown structure only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Markdown,
    Html,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Markdown => "markdown",
            Kind::Html => "html",
        }
    }
}

/// One failed check. `check` is the stable name from the table above; `detail` names the offending
/// value so the operator can fix the artifact, the fixture, or the guard without re-reading it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Finding {
    pub check: String,
    pub detail: String,
}

fn finding(check: &str, detail: impl Into<String>) -> Finding {
    Finding {
        check: check.to_string(),
        detail: detail.into(),
    }
}

/// Everything the checks compare an artifact against, derived from the SERIALIZED context block --
/// exactly the bytes the model was handed. Taking the ground truth from the block rather than from
/// the `Report` means "exists in the context" is literally true, and a field a later phase adds is
/// covered the day it lands.
#[derive(Debug, Default)]
pub struct Ground {
    /// Every `<org>/<repo>` slug the context names.
    pub repos: BTreeSet<String>,
    /// The org half of every slug, plus every `by-org` row's `org`.
    pub orgs: BTreeSet<String>,
    /// The repo half of every slug.
    pub repo_names: BTreeSet<String>,
    /// Every `YYYY-MM-DD` the context carries, anywhere.
    pub dates: BTreeSet<String>,
    /// `short-id`s of sessions the context carries with a null `title`.
    pub untitled_short_ids: BTreeSet<String>,
    /// The top three `by-repo` rows, in the order the context pre-sorted them.
    pub top_repos: Vec<String>,
    /// The first `agent-type-costs` row's name, the coverage dimension's other target.
    pub top_agent_type: Option<String>,
    /// `true` when the context carries at least one chart, so an HTML render is expected to draw a
    /// `<polyline>` rather than fall back to a table.
    pub has_charts: bool,
}

impl Ground {
    /// Derive the ground truth from a serialized context block.
    pub fn from_context_json(json: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(json)?;
        let mut ground = Ground::default();
        ground.walk("", &value);
        for slug in &ground.repos {
            if let Some((org, name)) = slug.split_once('/') {
                ground.orgs.insert(org.to_string());
                ground.repo_names.insert(name.to_string());
            }
        }
        ground.top_repos = top_repos(&value);
        ground.top_agent_type = top_agent_type(&value);
        ground.has_charts = value
            .pointer("/aggregates/charts")
            .is_some_and(|c| c.as_object().is_some_and(|m| !m.is_empty()));
        debug!(
            "mechanical::Ground: repos={} orgs={} dates={} untitled={} top-repos={:?} top-agent={:?} charts={}",
            ground.repos.len(),
            ground.orgs.len(),
            ground.dates.len(),
            ground.untitled_short_ids.len(),
            ground.top_repos,
            ground.top_agent_type,
            ground.has_charts
        );
        Ok(ground)
    }

    fn walk(&mut self, key: &str, value: &Value) {
        match value {
            Value::Object(map) => {
                // A session row with an explicit null title is the untitled case the Outlier
                // Sessions table cites by `short-id`.
                if map.get("title").is_some_and(Value::is_null)
                    && let Some(sid) = map.get("short-id").and_then(Value::as_str)
                {
                    self.untitled_short_ids.insert(sid.to_string());
                }
                for (k, v) in map {
                    self.walk(k, v);
                }
            }
            Value::Array(items) => {
                for v in items {
                    self.walk(key, v);
                }
            }
            Value::String(s) => {
                if matches!(key, "repo" | "repository") {
                    self.repos.insert(s.clone());
                }
                if key == "org" {
                    self.orgs.insert(s.clone());
                }
                for m in date_pattern().find_iter(s) {
                    self.dates.insert(m.as_str().to_string());
                }
            }
            _ => {}
        }
    }
}

/// The top three `by-repo` slugs, in context order (pre-sorted by spend descending).
fn top_repos(value: &Value) -> Vec<String> {
    value
        .pointer("/aggregates/by-repo")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|r| r.get("repo").and_then(Value::as_str))
                .take(3)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The first `agent-type-costs` row's name (pre-sorted by spend descending).
fn top_agent_type(value: &Value) -> Option<String> {
    value
        .pointer("/efficiency/agent-type-costs/0/name")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Run every check that applies to `artifact`. An empty result is a pass; the checks are all
/// independent, so every failure is reported rather than only the first.
pub fn check(kind: Kind, artifact: &str, context: &RenderContext, ground: &Ground, spec: &Spec) -> Vec<Finding> {
    debug!(
        "mechanical::check: kind={} artifact_bytes={} require-sections={} forbid-sections={} \
         require-citations={}",
        kind.as_str(),
        artifact.len(),
        spec.require_sections.len(),
        spec.forbid_sections.len(),
        spec.require_citations.len()
    );
    let prose = match kind {
        Kind::Markdown => artifact.to_string(),
        Kind::Html => visible_text(artifact),
    };
    let mut findings = Vec::new();
    findings.extend(em_dash(artifact));
    findings.extend(speculative(&prose));
    findings.extend(foreign_figures(&prose, context));
    findings.extend(cited_repos(&prose, ground));
    findings.extend(cited_dates(&prose, ground));
    findings.extend(cited_titles(&prose, context));
    if kind == Kind::Markdown {
        findings.extend(sections(artifact, spec));
        findings.extend(citations(artifact, ground, spec));
    }
    if kind == Kind::Html {
        findings.extend(chart_geometry(artifact, context, ground));
    }
    debug!("mechanical::check: kind={} findings={}", kind.as_str(), findings.len());
    findings
}

/// U+2014 anywhere in the artifact, including inside markup and CSS: both templates ban it outright
/// and no attribute or style legitimately needs one.
fn em_dash(artifact: &str) -> Vec<Finding> {
    match artifact.char_indices().find(|(_, c)| *c == EM_DASH) {
        None => Vec::new(),
        Some((at, _)) => {
            let around: String = artifact.chars().skip(at.saturating_sub(48)).take(96).collect();
            vec![finding(
                "em-dash",
                format!("the artifact contains U+2014, which both templates ban: ...{around}..."),
            )]
        }
    }
}

/// Hard prohibition 2's phrase list.
fn speculative(prose: &str) -> Vec<Finding> {
    speculative_pattern()
        .find_iter(prose)
        .map(|m| {
            finding(
                "speculative-quantification",
                format!(
                    "the prose contains {:?}, on Hard prohibition 2's banned list",
                    m.as_str()
                ),
            )
        })
        .collect()
}

/// The render-invents-nothing guard, run exactly as `render` runs it.
fn foreign_figures(prose: &str, context: &RenderContext) -> Vec<Finding> {
    context
        .facts
        .foreign_figures(prose)
        .into_iter()
        .map(|token| {
            finding(
                "foreign-figures",
                format!("the prose states {token:?}, which no quotable fact licenses"),
            )
        })
        .collect()
}

/// Every `<org>/<repo>` the artifact names must be one the context carries.
///
/// The scan is anchored on the context's OWN vocabulary rather than on a slug-shaped regex: a run
/// is checked when its left half is a known org (a corrupted repo NAME) or its right half is a
/// known repo name (a corrupted ORG). Anchoring this way is what keeps ordinary prose -- `and/or`,
/// `read/write`, `cache-read/cache-write` -- out of the check entirely, while still catching the
/// swap the design's own success criterion plants.
fn cited_repos(prose: &str, ground: &Ground) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for run in path_pattern().find_iter(prose) {
        // A slug at the end of a sentence carries the period with it (`.` is a legal slug
        // character, so the pattern cannot exclude it): `jrivera/sextant.` must compare as
        // `jrivera/sextant`, or every citation at a sentence end reads as fabricated.
        let segments: Vec<&str> = run.as_str().trim_end_matches(['.', '-', '_']).split('/').collect();
        for pair in segments.windows(2) {
            let (left, right) = (pair[0], pair[1]);
            let slug = format!("{left}/{right}");
            let anchored = ground.orgs.contains(left) || ground.repo_names.contains(right);
            if !anchored || ground.repos.contains(&slug) || !seen.insert(slug.clone()) {
                continue;
            }
            findings.push(finding(
                "cited-repos",
                format!("the artifact names the repo {slug:?}, which is not in the context"),
            ));
        }
    }
    findings
}

/// Every calendar date the artifact names must be one the context carries.
fn cited_dates(prose: &str, ground: &Ground) -> Vec<Finding> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    date_pattern()
        .find_iter(prose)
        .map(|m| m.as_str())
        .filter(|date| !ground.dates.contains(*date) && seen.insert(date))
        .map(|date| {
            finding(
                "cited-dates",
                format!("the artifact names the date {date:?}, which is not in the context"),
            )
        })
        .collect()
}

/// Every double-quoted phrase long enough to be a citation must appear verbatim in the context.
///
/// Quoting is a claim of verbatim provenance, so a quoted title, summary phrase or note the context
/// does not contain is a fabricated citation -- which is exactly what this check exists to catch.
/// Short quoted fragments and quoted figures are skipped ([`MIN_QUOTED_CHARS`], plus a required
/// space) so ordinary emphasis is never mistaken for a citation.
///
/// The YAML frontmatter block is excluded. Its `title:` value is a REQUIRED composite the prompt
/// tells the model to assemble (`"Claude Usage Report - <name> - <period>"`), so it is quoted
/// without being a citation of anything, and every fact inside it is checked by `cited-dates` and
/// `foreign-figures` anyway.
fn cited_titles(prose: &str, context: &RenderContext) -> Vec<Finding> {
    // Case-INSENSITIVE. A quote that opens a sentence legitimately lowercases the summary's first
    // letter ("documented the plugin hooks against the code rather than the wiki"), and a
    // case-sensitive compare rejected exactly that on a live render. A fabricated title does not
    // appear in the context in any case, so nothing is given up.
    let haystack = context.json.to_lowercase();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    quoted_pattern()
        .captures_iter(strip_frontmatter(prose))
        .filter_map(|c| c.get(1))
        .map(|m| m.as_str().trim())
        .filter(|span| span.chars().count() >= MIN_QUOTED_CHARS && span.split_whitespace().count() >= MIN_QUOTED_WORDS)
        .filter(|span| !haystack.contains(&span.to_lowercase()) && seen.insert((*span).to_string()))
        .map(|span| {
            finding(
                "cited-titles",
                format!("the artifact quotes {span:?}, which appears nowhere in the context"),
            )
        })
        .collect()
}

/// Required and forbidden markdown sections, matched on the `## <name>` heading line.
fn sections(markdown: &str, spec: &Spec) -> Vec<Finding> {
    let headings: BTreeSet<&str> = markdown
        .lines()
        .filter_map(|l| l.strip_prefix("## "))
        .map(str::trim)
        .collect();
    let mut findings = Vec::new();
    for want in &spec.require_sections {
        if !headings.contains(want.as_str()) {
            findings.push(finding(
                "required-sections",
                format!("the render omitted the required `## {want}` section"),
            ));
        }
    }
    for banned in &spec.forbid_sections {
        if headings.contains(banned.as_str()) {
            findings.push(finding(
                "forbidden-sections",
                format!("the render emitted `## {banned}`, which this fixture's data cannot support"),
            ));
        }
    }
    findings
}

/// The citation shapes the fixture requires its golden to exercise (design Phase 10's criterion 3,
/// re-run here against the real goldens).
fn citations(markdown: &str, ground: &Ground, spec: &Spec) -> Vec<Finding> {
    let mut findings = Vec::new();
    for want in &spec.require_citations {
        let present = match want {
            Citation::UntitledShortId => ground.untitled_short_ids.iter().any(|sid| markdown.contains(sid)),
            Citation::PrReference => pr_pattern().is_match(markdown),
        };
        if !present {
            findings.push(finding(
                "required-citations",
                format!(
                    "the render never exercised the `{}` citation this fixture requires; the \
                     quotable-facts whitelist is not proven against it",
                    want.as_str()
                ),
            ));
        }
    }
    findings
}

/// The chart-geometry allowlist, run exactly as the html render path runs it, plus the positive
/// half: when the context carries charts, the artifact must actually draw one.
fn chart_geometry(html: &str, context: &RenderContext, ground: &Ground) -> Vec<Finding> {
    let mut findings = Vec::new();
    if let Err(e) = geometry::reject_foreign_geometry("html", html, &context.facts) {
        findings.push(finding("chart-geometry", format!("{e}")));
    }
    if ground.has_charts && !html.contains("<polyline") {
        findings.push(finding(
            "chart-geometry",
            "the context carries precomputed charts but the artifact draws no <polyline>; the \
             geometry the binary computed reached the model and was dropped",
        ));
    }
    findings
}

/// The document past its leading `---` YAML frontmatter block, or the whole document when it has
/// none. Line-based, so no byte slicing (crate lint).
fn strip_frontmatter(prose: &str) -> &str {
    let trimmed = prose.trim_start();
    if !trimmed.starts_with(FRONTMATTER_FENCE) {
        return prose;
    }
    let opener_end = prose.len() - trimmed.len() + FRONTMATTER_FENCE.len();
    let closer = format!("\n{FRONTMATTER_FENCE}");
    for (at, _) in prose.match_indices(&closer) {
        if at >= opener_end {
            return prose.get(at + closer.len()..).unwrap_or(prose);
        }
    }
    prose
}

fn date_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\d{4}-\d{2}-\d{2}").expect("date pattern is a valid regex"))
}

/// A maximal slash-joined run of path-ish segments: `northwind-media/beacon`,
/// `github.com/northwind-media/beacon/pull/118`.
fn path_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"[A-Za-z0-9][A-Za-z0-9._-]*(?:/[A-Za-z0-9][A-Za-z0-9._-]*)+")
            .expect("path-run pattern is a valid regex")
    })
}

fn quoted_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#""([^"\n]{1,400})""#).expect("quoted-span pattern is a valid regex"))
}

fn pr_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(#\d+|\bPRs? \d+|/pull/\d+)").expect("pr-reference pattern is a valid regex"))
}

fn speculative_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let alternation = SPECULATIVE_PHRASES.join("|");
        Regex::new(&format!(r"(?i)\b(?:{alternation})\b")).expect("speculative-phrase pattern is a valid regex")
    })
}

#[cfg(test)]
mod tests;
