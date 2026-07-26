//! The judged layer: a model scores a FRESH render 0 to 3 on four dimensions (design Phase 13).
//!
//! This is the paid half, and the reason `otto eval` exists separately from `otto ci` (design
//! Non-Goals: "Making the *judged* render eval part of `otto ci`"). It runs over the existing
//! [`crate::summarize::Transport`], so it inherits `--llm` and needs no second credential.
//!
//! The judge is handed a BRIEF: the render's own context block, verbatim, plus the two coverage
//! targets ([`Brief::top_by_repo`], [`Brief::top_agent_type`]) named separately so the rubric has
//! something to point at. The brief is built by the binary from the same context the render used,
//! so the judge and the artifact are looking at one set of facts -- see [`Brief`] for the two
//! narrower briefs that were tried first and what each of them mis-scored.

use crate::summarize::{Job, Kind, Transport};
use eyre::{Context, Result, bail};
use log::debug;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::fixture::{Dimension, Spec};

/// The judge's system prompt. Terse on purpose: the instruction lives in [`JUDGE_PROMPT`], and a
/// system prompt that also carried the rubric would be two places to keep one rubric.
const JUDGE_SYSTEM_PROMPT: &str = "You are grading a generated report against the data it was \
     generated from. You output ONLY a JSON object. No preamble, no commentary, no markdown fences.";

/// Highest score any dimension can carry.
pub const MAX_SCORE: u8 = 3;

/// How many `by-repo` rows the coverage dimension is scored against (design Phase 13: "coverage of
/// the top three `by-repo` rows and the top agent type").
const TOP_REPOS: usize = 3;

/// The rubric. Every dimension is scored on the same 0-to-3 scale with the same meaning per point,
/// so a floor of 2 means the same thing everywhere.
pub const JUDGE_PROMPT: &str = r#"Grade the rendered report below against the brief that follows it.

`brief.context` is the DATA the report was written from, verbatim and complete: the same block the
renderer was handed, per-session list included. Treat it as complete. A figure, a repo, a date, a
session id or a quoted phrase that appears anywhere in it is SUPPORTED, whatever section of the
report cites it, and whether or not it is also in `top-by-repo` or `aggregates.outliers`.

Score each of these four dimensions as an integer from 0 to 3:

- `citation-accuracy`: every repo, date, session, figure and quoted phrase the report states is one
  `brief.context` supports, AND the report attributes each one to the right thing -- a session's
  title matched to its own repo and its own described work, a figure matched to the row it came
  from. 3 = nothing stated that the context does not carry, nothing mis-attributed. 2 = one soft
  claim that overreaches the context, or one mis-attribution. 1 = several. 0 = a fabricated figure,
  or a fabricated repo, date, or session.
- `coverage`: the report names and characterizes EVERY repo listed under `top-by-repo` in the brief,
  and names the `top-agent-type`. 3 = all of them, each with something said about it. 2 = all of
  them named, at least one only listed in a table with nothing said about it. 1 = one of them
  missing entirely. 0 = two or more missing entirely, or the top row absent.
- `prohibition-compliance`: the report MAY state any figure the brief carries, and that includes
  the precomputed `unit-costs` ratios, the coverage percentages and the cache figures -- the binary
  computed those, and quoting them verbatim is required rather than arithmetic. Do NOT score a
  quoted `unit-costs` value as the report doing its own math. What IS banned: any estimate of
  hours, effort, headcount, productivity, ROI or counterfactual value; any figure the brief does
  not carry; a sum, difference or percentage the report derived itself from two brief figures; and
  framing a `unit-costs` ratio as a price ("each commit cost $X" rather than "a ratio of $X per
  commit"). The pricing-basis sentence must be present verbatim. 3 = clean. 2 = one borderline
  evaluative phrase. 1 = one clear violation. 0 = several, or an explicit cost-justification
  argument.
- `readability`: a reader who has not seen the data can follow it. 3 = clear, well-ordered, no
  filler. 2 = readable with some padding or repetition. 1 = hard to follow. 0 = incoherent.

Reply with EXACTLY this JSON shape and nothing else. Every `reason` is one sentence naming the
specific evidence for the score:

{"citation-accuracy":{"score":3,"reason":"..."},
 "coverage":{"score":3,"reason":"..."},
 "prohibition-compliance":{"score":3,"reason":"..."},
 "readability":{"score":3,"reason":"..."}}
"#;

/// One dimension's verdict.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Score {
    pub score: u8,
    pub reason: String,
}

/// The judge's whole verdict: one [`Score`] per [`Dimension`], all four required.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Verdict {
    pub citation_accuracy: Score,
    pub coverage: Score,
    pub prohibition_compliance: Score,
    pub readability: Score,
}

impl Verdict {
    pub fn get(&self, dimension: Dimension) -> &Score {
        match dimension {
            Dimension::CitationAccuracy => &self.citation_accuracy,
            Dimension::Coverage => &self.coverage,
            Dimension::ProhibitionCompliance => &self.prohibition_compliance,
            Dimension::Readability => &self.readability,
        }
    }

    /// Dimensions scoring below their floor, with the score and the floor, in report order. Empty
    /// means the artifact cleared every floor this fixture sets. Reads the floor through
    /// [`Spec::floor`], so "an unset floor is zero" is defined once rather than here as well.
    pub fn regressions(&self, spec: &Spec) -> Vec<(Dimension, u8, u8)> {
        Dimension::ALL
            .iter()
            .filter_map(|d| {
                let floor = spec.floor(*d);
                let score = self.get(*d).score;
                (score < floor).then_some((*d, score, floor))
            })
            .collect()
    }
}

/// The facts the judge grades against, built by the binary from the render's own context block.
///
/// [`Self::context`] is the context block BYTE FOR BYTE, not a subset. Two subsets were tried and
/// both mis-scored on their first real run. A hand-picked subset made the judge call legitimate
/// by-day, reconciliation and cache figures unsupported. Dropping only `sessions[]` (on the theory
/// that `aggregates.outliers` covers every session the narrative cites) then made it call
/// legitimate citations of the sessions BELOW the outlier cut fabricated -- and score citation
/// accuracy 1 for it.
///
/// The lesson both times: a judge asked "is every claim supported" has to be holding exactly what
/// supported it. The cost is real -- a 1,500-session local window is a ~900KB judge input -- and it
/// is the right trade for the one call per fixture this makes.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Brief {
    /// The `by-repo` rows the coverage dimension is scored against, in context order. Called out
    /// separately from `context` so the rubric has a name to point at.
    top_by_repo: Vec<Value>,
    /// The first `agent-type-costs` row, the coverage dimension's other target.
    top_agent_type: Value,
    /// The context block the render was written from, verbatim.
    context: Value,
}

/// Cut the brief out of a serialized context block. An absent field stays absent from `context`
/// exactly as it was absent from the render's own input, so "the period had no outcomes" and "the
/// report should not have mentioned outcomes" are the same fact for the judge as they were for the
/// renderer.
pub fn brief(context_json: &str) -> Result<Brief> {
    let value: Value =
        serde_json::from_str(context_json).context("failed to re-parse the context block for the judge brief")?;
    let top_by_repo = value
        .pointer("/aggregates/by-repo")
        .and_then(Value::as_array)
        .map(|rows| rows.iter().take(TOP_REPOS).cloned().collect())
        .unwrap_or_default();
    let top_agent_type = value
        .pointer("/efficiency/agent-type-costs/0")
        .cloned()
        .unwrap_or(Value::Null);
    let brief = Brief {
        top_by_repo,
        top_agent_type,
        context: value,
    };
    debug!(
        "judge::brief: top-by-repo={} top-agent-type={} context_bytes={}",
        brief.top_by_repo.len(),
        !brief.top_agent_type.is_null(),
        context_json.len()
    );
    Ok(brief)
}

/// Score one artifact. `model` is `--judge`'s pin; `ceiling` is the shared markdown output ceiling
/// (`render.markdown-max-output-tokens`), which is also the key the cli transport names if the
/// verdict ever exceeds it -- so the remedy it prints is the key that actually governs.
pub fn score<T: Transport>(transport: &T, model: &str, ceiling: u32, artifact: &str, brief: &Brief) -> Result<Verdict> {
    let body = serde_json::to_string(&JudgeInput { artifact, brief }).context("failed to serialize the judge input")?;
    debug!(
        "judge::score: model={model} ceiling={ceiling} artifact_bytes={} input_bytes={}",
        artifact.len(),
        body.len()
    );
    let job = Job {
        kind: Kind::Judge,
        model,
        max_output_tokens: ceiling,
    };
    let raw = transport.complete(job, JUDGE_SYSTEM_PROMPT, JUDGE_PROMPT, &body)?;
    let verdict = parse(&raw)?;
    debug!(
        "judge::score: citation={} coverage={} prohibition={} readability={}",
        verdict.citation_accuracy.score,
        verdict.coverage.score,
        verdict.prohibition_compliance.score,
        verdict.readability.score
    );
    Ok(verdict)
}

/// What the judge is handed: the artifact and the brief, in one JSON body.
#[derive(Serialize)]
struct JudgeInput<'a> {
    artifact: &'a str,
    brief: &'a Brief,
}

/// Parse a judge reply into a [`Verdict`], tolerating a fenced or prose-wrapped object by taking
/// the outermost `{...}` span. A reply that does not parse, or a score outside 0..=3, is a LOUD
/// error: a judged eval that silently defaulted an unparseable verdict to a passing score would
/// report quality it never measured.
pub fn parse(raw: &str) -> Result<Verdict> {
    let trimmed = raw.trim();
    let start = trimmed.find('{');
    let end = trimmed.rfind('}');
    let body = match (start, end) {
        (Some(s), Some(e)) if e > s => trimmed.get(s..=e).unwrap_or(trimmed),
        _ => bail!("the judge reply contains no JSON object: {:?}", preview(trimmed)),
    };
    let verdict: Verdict = serde_json::from_str(body)
        .with_context(|| format!("the judge reply is not the expected verdict shape: {:?}", preview(body)))?;
    for dimension in Dimension::ALL {
        let score = verdict.get(*dimension).score;
        if score > MAX_SCORE {
            bail!(
                "the judge scored `{}` at {score}, above the {MAX_SCORE}-point scale",
                dimension.as_str()
            );
        }
    }
    Ok(verdict)
}

/// First 200 chars of a reply, for an error message that shows what came back without dumping a
/// whole model response into the terminal.
fn preview(raw: &str) -> String {
    raw.chars().take(200).collect()
}

#[cfg(test)]
mod tests;
