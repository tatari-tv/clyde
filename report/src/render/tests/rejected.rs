#![allow(clippy::unwrap_used)]

//! Phase 2: a guard rejection persists the artifact it produced under
//! `xdg_data_dir()/clyde/rejected/` and the wrapped error names where it landed, so the next
//! rejection is evidence instead of a discarded paid render. `guarded` is the one call site both
//! `markdown_from_context` and `html_from_context` route through, so exercising it directly is
//! exercising exactly what both formats hit.

use super::*;
use crate::ENV_LOCK;
use std::fs;

/// THE phase criterion. A rejecting guard closure, run through `guarded` with `$XDG_DATA_HOME`
/// pointed at a fresh `TempDir`, must leave the artifact on disk under `clyde/rejected/` and the
/// surfaced error must name that exact path -- the one thing a discarded render never gave the
/// operator before this phase.
#[test]
fn a_rejected_render_is_persisted_and_the_error_names_the_path() {
    let guard = ENV_LOCK.lock().unwrap();
    let prior = std::env::var("XDG_DATA_HOME").ok();

    let dir = tempfile::TempDir::new().unwrap();
    unsafe { std::env::set_var("XDG_DATA_HOME", dir.path()) };

    let artifact = "# Report\n\nthe model invented a round 500 sessions here.";
    let err = guarded("markdown", "md", artifact, || {
        Err(eyre::eyre!(
            "markdown rendering introduced number(s) absent from the computed facts"
        ))
    })
    .unwrap_err();

    let rejected_dir = dir.path().join("clyde").join("rejected");
    let entries: Vec<_> = fs::read_dir(&rejected_dir)
        .unwrap_or_else(|e| panic!("expected {} to exist: {e}", rejected_dir.display()))
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1, "exactly one rejected artifact expected: {entries:?}");
    let written = &entries[0];
    assert!(
        written.file_name().unwrap().to_string_lossy().ends_with("-markdown.md"),
        "the filename must carry the kind and extension the spec names: {written:?}"
    );
    assert_eq!(
        fs::read_to_string(written).unwrap(),
        artifact,
        "the persisted bytes must be the exact artifact that was rejected, not a summary of it"
    );

    let message = err.to_string();
    assert!(
        message.contains(&written.display().to_string()),
        "the guard error must name the exact path the render was persisted to: {message:?}"
    );
    // `Report::wrap_err` pushes the new message to the top and keeps the original as the `source`
    // (its `Display` shows only the outermost message, matching every other `.context()` call in
    // this crate); the full chain is what proves nothing was lost.
    let chained = err.chain().map(|e| e.to_string()).collect::<Vec<_>>().join(" <- ");
    assert!(
        chained.contains("introduced number(s) absent"),
        "wrapping must not lose the original guard message: {chained:?}"
    );

    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
    }
    drop(guard);
}

/// The diagnostic must never rescue a render and never mask why one failed. Pointing
/// `$XDG_DATA_HOME` at a plain FILE (not a directory) makes every persist attempt fail at
/// `fs::create_dir_all` -- deterministically, with no dependency on filesystem permissions -- and
/// the guard's own error must come back byte-for-byte unchanged, carrying none of the "written to"
/// wrapping the success path adds.
#[test]
fn a_failed_persist_does_not_swallow_the_guard_error() {
    let guard = ENV_LOCK.lock().unwrap();
    let prior = std::env::var("XDG_DATA_HOME").ok();

    let dir = tempfile::TempDir::new().unwrap();
    let not_a_dir = dir.path().join("this-is-a-file-not-a-directory");
    fs::write(&not_a_dir, b"blocking any subdirectory from being created here").unwrap();
    unsafe { std::env::set_var("XDG_DATA_HOME", &not_a_dir) };

    const ORIGINAL_MESSAGE: &str = "html rendering made claim(s) the context block cannot support";
    let err = guarded("html", "html", "<html>fabricated claim</html>", || {
        Err(eyre::eyre!(ORIGINAL_MESSAGE))
    })
    .unwrap_err();

    assert_eq!(
        err.to_string(),
        ORIGINAL_MESSAGE,
        "a failed persist must propagate the guard's own error UNCHANGED, never wrapped and never rescued"
    );

    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
    }
    drop(guard);
}

/// Acceptance Criterion 3's last clause: "The render still fails and still writes nothing to the
/// output path." `guarded` (exercised above) proves the guard error survives; this proves the
/// OTHER half -- that `run`'s generate-then-route ordering never reaches the write -- by calling
/// `generate_then_route` itself, the exact function both the markdown and html branches of `run`
/// route through, rather than reimplementing its shape in the test. `generate` fails the way a
/// guard rejection does; `route` is a real filesystem write into a `TempDir`, so a bug that let
/// `route` run anyway would leave a real file behind, not just flip a boolean.
#[test]
fn a_guard_rejection_writes_nothing_to_the_output_path() {
    let dir = tempfile::TempDir::new().unwrap();
    let output_path = dir.path().join("would-be-written.md");

    let result = generate_then_route(
        || -> Result<String> {
            Err(eyre::eyre!(
                "markdown rendering introduced number(s) absent from the computed facts"
            ))
        },
        |artifact: &String| {
            fs::write(&output_path, artifact).unwrap();
            Ok(OutputDest::File(output_path.clone()))
        },
    );

    assert!(
        result.is_err(),
        "a rejected generation must still surface as an Err from run's own path"
    );
    assert!(
        !output_path.exists(),
        "route must never execute when generate returns Err: the output path must not exist on disk"
    );
}

/// Acceptance Criterion 3's last clause, at the WIRING level: "The render still fails and still
/// writes nothing to the output path."
///
/// Three tests cover this clause between them, and each sees a failure the others cannot:
/// - `a_guard_rejection_writes_nothing_to_the_output_path`, directly above, pins the
///   `generate_then_route` HELPER: given a failing generate, route never runs.
/// - `render_run_gates_on_schema_version_before_touching_the_api` (`render/tests.rs`) pins a
///   failure BEFORE generation.
/// - this one pins that `run` is actually WIRED to that helper, with generation on the failing
///   side. Inline the wrong order back into `run` and the helper test still passes green while
///   real renders start publishing rejected artifacts. That gap is what this closes.
///
/// It drives the real `run` with a real `RenderConfig` and a real output path. A missing
/// `--template` is the cheapest generation failure that needs no transport: the guards themselves
/// sit behind a live LLM call (`markdown_from_context` resolves its transport internally via
/// `resolve_selected_transport`), so a genuine end-to-end rejection would need a transport injected
/// into the hot render path, a refactor this branch deliberately did not take on.
///
/// BITES: hoist routing above generation in `run` (route first, generate second) and the existence
/// assertion fails.
#[test]
fn a_generation_failure_writes_nothing_to_the_output_path() {
    let tmp = TempDir::new().unwrap();
    let json_path = tmp.path().join("claude-report.json");
    let out = tmp.path().join("out.md");
    fs::write(&json_path, serde_json::to_string_pretty(&sample_report()).unwrap()).unwrap();

    let cfg = RenderConfig {
        llm: crate::cli::Llm::Auto,
        markdown_model: "claude-opus-4-8".into(),
        html_model: "claude-opus-4-8".into(),
        markdown_max_output_tokens: common::config::DEFAULT_MARKDOWN_MAX_OUTPUT_TOKENS,
        html_max_output_tokens: common::config::DEFAULT_HTML_MAX_OUTPUT_TOKENS,
        input: json_path,
        output: Some(out.clone()),
        format: crate::cli::Format::Markdown,
        space: None,
        template: Some(tmp.path().join("this-template-does-not-exist.md")),
        prompt: None,
        include_tradeoffs: false,
        pdf_engine: "wkhtmltopdf".into(),
        outliers: crate::aggregate::DEFAULT_OUTLIERS,
        prior: None,
        reconcile: None,
        reconcile_user: None,
    };

    let err = run(&cfg, &pricing()).expect_err("a missing template must fail the render");
    // Pin WHY it failed. Without this the test would pass on any error at all, including one raised
    // before generation, and would stop testing the generation-then-routing ordering it exists for.
    let msg = format!("{err:#}");
    assert!(
        msg.contains("this-template-does-not-exist.md"),
        "the failure must be the missing template inside generation, not something earlier: {msg}"
    );
    assert!(
        !out.exists(),
        "generation failed, so routing must never have run and the output path must be untouched"
    );
}
