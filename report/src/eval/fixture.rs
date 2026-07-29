//! A fixture on disk: the collected artifact, its per-fixture spec, and the committed goldens.
//!
//! A fixture directory holds:
//!
//! ```text
//! report.json    the collected schema-v2 artifact  (REQUIRED)
//! eval.yml       this module's `Spec`              (optional; defaults apply)
//! prior.json     a prior-period artifact           (optional; lights up Month over Month)
//! analytics.json an Analytics cost export          (optional; lights up Reconciliation)
//! golden.md      the committed markdown render     (optional; required for the ci layer)
//! golden.html    the committed html render         (optional; required for the ci layer)
//! ```
//!
//! Only `report.json` is required, because `clyde report eval --fixture <local-dir>` points at an
//! UNCOMMITTED directory holding a real month (design Phase 13: the real-data eval stays a local
//! step, and `fixtures/report/local/` is gitignored). Such a directory has no goldens and no spec;
//! it gets the defaults and only the fresh-render path runs against it.

use eyre::{Context, Result, bail};
use log::debug;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The committed fixture root, relative to the workspace. `clyde report eval` with no `--fixture`
/// resolves the three named fixtures under it.
pub const FIXTURE_ROOT: &str = "fixtures/report";

/// The three committed fixtures, in the order they are evaluated: cheapest and simplest first, so a
/// broken run fails on the smallest artifact rather than the largest.
pub const COMMITTED: &[&str] = &["small", "medium", "pathological"];

const REPORT_FILE: &str = "report.json";
const SPEC_FILE: &str = "eval.yml";
const PRIOR_FILE: &str = "prior.json";
const ANALYTICS_FILE: &str = "analytics.json";
const GOLDEN_MARKDOWN: &str = "golden.md";

/// A citation shape a fixture's golden must actually exercise. These are the two false positives a
/// narrowed quotable-facts whitelist causes first (design Phase 10's success criterion 3), so at
/// least one committed fixture requires both: a criterion nothing asserts is a criterion nobody
/// meets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Citation {
    /// An UNTITLED session cited by its `short-id` (the Outlier Sessions table's own rule).
    UntitledShortId,
    /// A pull request referenced in prose: `#N`, `PR N`, or a full pull url.
    PrReference,
}

impl Citation {
    pub fn as_str(self) -> &'static str {
        match self {
            Citation::UntitledShortId => "untitled-short-id",
            Citation::PrReference => "pr-reference",
        }
    }
}

/// The judged dimensions, each scored 0 to 3 (design Phase 13). Serialized kebab-case in both
/// `eval.yml`'s floors and the judge's own verdict, so a floor and a score are the same vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Dimension {
    /// Every figure, repo, date and title the artifact states is in the context.
    CitationAccuracy,
    /// The top three `by-repo` rows and the top agent type are all present and characterized.
    Coverage,
    /// No speculative quantification, no arithmetic, the required disclosures present.
    ProhibitionCompliance,
    /// A reader who has not seen the data can follow it.
    Readability,
}

impl Dimension {
    /// Every dimension, in report order. The judge scores all four on every artifact; a missing one
    /// is a parse error, never a silent zero.
    pub const ALL: &'static [Dimension] = &[
        Dimension::CitationAccuracy,
        Dimension::Coverage,
        Dimension::ProhibitionCompliance,
        Dimension::Readability,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Dimension::CitationAccuracy => "citation-accuracy",
            Dimension::Coverage => "coverage",
            Dimension::ProhibitionCompliance => "prohibition-compliance",
            Dimension::Readability => "readability",
        }
    }
}

/// The per-fixture spec (`eval.yml`), committed beside the fixture it describes.
///
/// `deny_unknown_fields`: a typo'd key here would silently drop a required section or a floor, and
/// a floor nobody enforces is the exact "unmeasured quality" this whole phase exists to remove.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Spec {
    /// What this fixture is FOR, one line. Read by nothing; present so the directory explains
    /// itself to whoever opens it next.
    pub summary: String,
    /// Markdown section headings the render must emit. Matched as a `## <name>` line.
    #[serde(default)]
    pub require_sections: Vec<String>,
    /// Section headings the render must NOT emit -- the absent-section paths the pathological
    /// fixture exists to make bite (design: "The pathological one exists to make the
    /// absent-section paths bite").
    #[serde(default)]
    pub forbid_sections: Vec<String>,
    /// Citation shapes the golden must exercise (see [`Citation`]).
    #[serde(default)]
    pub require_citations: Vec<Citation>,
    /// Minimum judge score per dimension. A fresh render below ANY floor fails the eval.
    #[serde(default)]
    pub floors: BTreeMap<Dimension, u8>,
    /// A synthesized persona for the render context. The eval NEVER calls `persona::whoami()`:
    /// that would splice the operator's real name, title, team and email into a golden committed to
    /// a PUBLIC repo. The identity here is invented along with the rest of the fixture.
    #[serde(default)]
    pub persona: Option<crate::persona::PersonaBlock>,
}

impl Default for Spec {
    fn default() -> Self {
        Self {
            summary: "an uncommitted local fixture; no spec supplied".to_string(),
            require_sections: Vec::new(),
            forbid_sections: Vec::new(),
            require_citations: Vec::new(),
            floors: BTreeMap::new(),
            persona: None,
        }
    }
}

impl Spec {
    /// The floor for one dimension, or `0` when the spec sets none (nothing to regress below).
    pub fn floor(&self, dimension: Dimension) -> u8 {
        self.floors.get(&dimension).copied().unwrap_or(0)
    }
}

/// A loaded fixture directory.
#[derive(Debug)]
pub struct Fixture {
    /// The directory's own name (`small`, `medium`, ...), used in every report line.
    pub name: String,
    pub dir: PathBuf,
    pub spec: Spec,
    pub report: PathBuf,
    pub prior: Option<PathBuf>,
    pub analytics: Option<PathBuf>,
    /// The committed golden, when the directory carries one.
    pub golden_markdown: Option<String>,
}

impl Fixture {
    /// Load a fixture directory. Fails loudly when `report.json` is missing: an eval that silently
    /// skipped an unreadable fixture would report "all fixtures passed" having evaluated none.
    pub fn load(dir: &Path) -> Result<Self> {
        debug!("fixture::load: dir={}", dir.display());
        if !dir.is_dir() {
            bail!(
                "fixture directory {} does not exist; pass --fixture <dir> or run from the clyde \
                 workspace root so the committed fixtures under {FIXTURE_ROOT}/ resolve",
                dir.display()
            );
        }
        let report = dir.join(REPORT_FILE);
        if !report.is_file() {
            bail!(
                "fixture {} carries no {REPORT_FILE}; a fixture directory is a `report collect` \
                 artifact plus (optionally) its spec and goldens",
                dir.display()
            );
        }
        let spec = load_spec(&dir.join(SPEC_FILE))?;
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| dir.display().to_string());
        let fixture = Self {
            name,
            dir: dir.to_path_buf(),
            spec,
            report,
            prior: optional(dir, PRIOR_FILE),
            analytics: optional(dir, ANALYTICS_FILE),
            golden_markdown: read_optional(dir, GOLDEN_MARKDOWN)?,
        };
        debug!(
            "fixture::load: name={} prior={} analytics={} golden={}",
            fixture.name,
            fixture.prior.is_some(),
            fixture.analytics.is_some(),
            fixture.golden_markdown.is_some()
        );
        Ok(fixture)
    }

    /// Where the golden is written by `report eval --write-goldens`.
    ///
    /// This is the ONLY way to regenerate one against the fixture's own persona and the eval's
    /// pinned pricing -- a hand-run `report render` would splice in the operator's real identity and
    /// price against the live feed. The written artifact is a STUBBED render (no slot prose), which
    /// is what makes a golden byte-stable and its comparison free.
    pub fn golden_path(&self) -> PathBuf {
        self.dir.join(GOLDEN_MARKDOWN)
    }
}

/// Load `eval.yml`, or the defaults when the directory has none (the local-fixture case).
fn load_spec(path: &Path) -> Result<Spec> {
    if !path.is_file() {
        debug!("fixture::load_spec: no {}, using defaults", path.display());
        return Ok(Spec::default());
    }
    let body = fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_yaml::from_str(&body).with_context(|| format!("failed to parse the fixture spec at {}", path.display()))
}

fn optional(dir: &Path, name: &str) -> Option<PathBuf> {
    let path = dir.join(name);
    path.is_file().then_some(path)
}

fn read_optional(dir: &Path, name: &str) -> Result<Option<String>> {
    let path = dir.join(name);
    if !path.is_file() {
        return Ok(None);
    }
    let body = fs::read_to_string(&path).with_context(|| format!("failed to read golden at {}", path.display()))?;
    Ok(Some(body))
}

/// The default fixture set: the three committed directories under [`FIXTURE_ROOT`], resolved
/// against `root` (the process CWD in production).
pub fn committed_dirs(root: &Path) -> Vec<PathBuf> {
    COMMITTED
        .iter()
        .map(|name| root.join(FIXTURE_ROOT).join(name))
        .collect()
}

#[cfg(test)]
mod tests;
