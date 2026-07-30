//! The keyless LLM transport, shared by every crate that needs one. Moved here (design
//! `2026-07-29-excise-api-key.md` Phase 1) from `report::summarize`, because `report` depends on
//! `sessions` and `efficiency`, so a transport that lived in `report` could never be reached from
//! them. `common` is the one crate both already depend on.
//!
//! `report`'s own api-key transport (`ApiTransport`) is NOT here: it stays in `report::summarize`
//! until Phase 4 deletes it, so it is not this crate's concern.

pub mod cli;

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
    /// `clyde session enrich`: one dormant session's redacted PROSE -> the catalog entry JSON
    /// (`{"tags":[...],"summary":"..."}`). One invocation per session, run unattended on a timer, so
    /// this is the only kind whose cost compounds over hundreds of calls -- which is why it is also
    /// the only kind that suppresses reasoning (see `cli::child_env`).
    Enrich,
    /// `clyde efficiency session --narrate`: a prose verdict over already-computed efficiency facts.
    /// One interactive invocation, never a sweep.
    Narrate,
}

impl Kind {
    /// The `clyde.yml` key carrying this job's output ceiling, or `None` when the ceiling is a
    /// compile-time const rather than something the user can set.
    ///
    /// Exists so a ceiling failure can name the ONE line that prevents the next one. Per-kind on
    /// purpose: naming a key that does not govern the failing job is a remedy that cannot remedy,
    /// which `cli.rs`'s module docs call worse than offering none.
    ///
    /// [`Kind::Enrich`] and [`Kind::Narrate`] return `None`, and that is the whole reason this is an
    /// `Option`: their ceilings live in `sessions::llm` as consts, so inventing
    /// `enrich-max-output-tokens` / `narrate-max-output-tokens` keys nobody asked for would advertise
    /// a knob that governs nothing. `None` is the honest answer, and `cli::check_envelope` reads it as
    /// "this kind has no configurable output budget, so there is no budget to enforce" (design
    /// `2026-07-29-excise-api-key.md`, API Design + Phase 0 Finding 3).
    pub fn max_output_tokens_key(self) -> Option<&'static str> {
        match self {
            Kind::Slot => Some("render.slot-max-output-tokens"),
            Kind::Judge => Some("render.judge-max-output-tokens"),
            Kind::Enrich | Kind::Narrate => None,
        }
    }

    /// The fence label the payload rides under in the user message.
    ///
    /// A property of the KIND, not of the transport, because it describes what the payload IS. `Slot`
    /// and `Judge` send curated JSON facts; `Enrich` sends a session's redacted prose and `Narrate`
    /// sends formatted prose facts, and labeling prose as ```` ```json ```` misdescribes the payload
    /// to the model.
    pub fn fence(self) -> &'static str {
        match self {
            Kind::Slot | Kind::Judge => "json",
            Kind::Enrich | Kind::Narrate => "text",
        }
    }
}

/// One completion's text plus the token counts the CLI billed for it.
///
/// [`Transport::complete`] returns the text alone, because `report`'s callers publish an artifact and
/// never account for it. `sessions` DOES account for it: `tokens_in`/`tokens_out` are durable columns
/// (`sessions::db`), and a token-budget gate that reads a zero it never observed is a fail-quietly bug
/// (design Data Model). So the counts travel out of the transport as data, on the concrete
/// [`cli::CliTransport::complete_with_usage`], rather than through a widened trait that every existing
/// implementation and test double would have to grow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub text: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

/// A transport failure the CALLER must not retry per-payload.
///
/// The one typed error `common::llm` exposes, so a sweep can tell "this session is bad" from "the
/// transport is down" by matching a variant rather than by reading a message (`rules/rust.md`: detect
/// a condition by matching a typed error variant, never by string-matching). The workspace is on
/// `eyre`, which carries no variants, so the transport attaches this as the report's error and
/// `sessions::enrich` recovers it with `err.downcast_ref::<TransportError>()`.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// `claude` is present but cannot serve a request at all: logged out, an expired or rejected
    /// credential, rate-limited, or the upstream API is down. Sweep-fatal. Charging a durable retry
    /// attempt for this would burn budget on a condition no retry can fix, and five unattended timer
    /// runs would silently retire the whole catalog (design G5).
    ///
    /// Named for the BROAD meaning ("the transport cannot serve requests right now"), not for auth:
    /// 429 and 5xx classify identically and are not authentication problems.
    #[error("the `claude` CLI is unavailable: {0}")]
    Unavailable(String),
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
///
/// `pub`, not `pub(crate)`: `report::summarize::api` (the api-key transport) still calls this
/// across the crate boundary until Phase 4 deletes that module. Visibility widened for the move;
/// no runtime behavior changed.
pub fn check_stop_reason(stop_reason: Option<&str>) -> Result<()> {
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
