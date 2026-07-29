pub mod api;
pub mod cli;

pub use api::{ApiTransport, api_key_from_env};
pub use cli::CliTransport;

use eyre::{Result, bail};

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

/// WHICH kind of completion a job is. Stays a `Copy` enum because it IS a compile-time fact:
/// nothing about the choice is user-configurable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// One prose SLOT of the deterministic document (design "Render Inversion"): a few sentences of
    /// digit-free prose with `{{fact:key}}` placeholders where figures go. Several per render, each
    /// its own subprocess, each over a curated brief rather than the whole context block.
    Slot,
    /// `clyde report eval`'s judge: a small JSON verdict over an already-rendered artifact
    /// (design Phase 13). It rides the existing transport rather than a second client, so the eval
    /// inherits `--llm` and needs no second credential.
    Judge,
}

impl Kind {
    /// The `clyde.yml` key carrying this job's output ceiling.
    ///
    /// Exists so a ceiling failure can name the ONE line that prevents the next one. Per-kind on
    /// purpose: naming a key that does not govern the failing job is a remedy that cannot remedy,
    /// which `cli.rs`'s module docs call worse than offering none.
    ///
    /// Each arm now names a key named for ITS OWN job. That was not true before design "Render
    /// Inversion": [`Kind::Judge`] used to name `render.markdown-max-output-tokens`, a key named for
    /// the whole-document authoring job, and the doc comment here had to argue it was not a stand-in.
    /// The key is `render.judge-max-output-tokens`, so there is no longer an argument to make.
    pub fn max_output_tokens_key(self) -> &'static str {
        match self {
            Kind::Slot => "render.slot-max-output-tokens",
            Kind::Judge => "render.judge-max-output-tokens",
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
    /// `render.model`, which pins both the slot job and the judge.
    pub model: &'a str,
    /// `render.slot-max-output-tokens` for a slot, `render.judge-max-output-tokens` for the judge.
    ///
    /// SHARED by both transports: the api transport SETS it as `max_tokens` on the wire, and the cli
    /// transport -- which cannot set a ceiling at all -- CHECKS the returned `usage.output_tokens`
    /// against it.
    pub max_output_tokens: u32,
}

/// Bail unless the model finished on its own (`end_turn`). A `max_tokens` (or any non-`end_turn`)
/// stop is the named output-exhaustion failure mode: the artifact exceeded the model's output
/// ceiling, so it is truncated and must not be published. Pure, so tests can drive it directly.
fn check_stop_reason(stop_reason: Option<&str>) -> Result<()> {
    match stop_reason {
        Some("end_turn") => Ok(()),
        other => bail!(
            "Anthropic API stopped with stop_reason={} (expected end_turn): the generated artifact \
             exceeded the model's output ceiling and was truncated. Raise the ceiling named by \
             this job's config key, or narrow the window with a shorter --since, then try again.",
            other.unwrap_or("<missing>")
        ),
    }
}

#[cfg(test)]
mod tests;
