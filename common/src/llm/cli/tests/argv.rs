#![allow(clippy::unwrap_used)]

//! Split out of the former single 1,322-line `cli/tests.rs` (design Phase 6). The section banners in
//! that file were already the module boundaries; each submodule below is one contiguous run of them.

use super::*;

// ---- AC8: the argv must carry every isolation flag --------------------------------------------

#[test]
fn argv_carries_every_isolation_flag_by_name() {
    let spawn = transport().build_spawn(job(Kind::Slot), "SYS", "INSTRUCTION");
    let args = &spawn.args;
    let pos = |needle: &str| args.iter().position(|a| a == needle);

    // Each asserted BY NAME so none can be dropped silently in a later refactor.
    assert!(pos("--safe-mode").is_some(), "missing --safe-mode: {args:?}");
    assert!(
        pos("--strict-mcp-config").is_some(),
        "missing --strict-mcp-config: {args:?}"
    );
    assert!(
        pos("--no-session-persistence").is_some(),
        "missing --no-session-persistence: {args:?}"
    );
    // `--tools ""` is the structural tool-kill: the flag followed by an EMPTY string.
    let tools = pos("--tools").expect("missing --tools");
    assert_eq!(
        args[tools + 1],
        "",
        "--tools must be followed by an empty string: {args:?}"
    );
    // One turn.
    let turns = pos("--max-turns").expect("missing --max-turns");
    assert_eq!(args[turns + 1], "1");
    // JSON envelope.
    let fmt = pos("--output-format").expect("missing --output-format");
    assert_eq!(args[fmt + 1], "json");
}

#[test]
fn argv_carries_the_prompt_slot_verbatim_for_every_kind() {
    // The slot `sessions::llm::ENRICH_REASSERT` rides in. It must land as the value of `-p` for every
    // kind, and an EMPTY prompt must still occupy the slot rather than being dropped -- a dropped arg
    // would shift every following flag by one and silently misalign the whole argv.
    for kind in ALL_KINDS {
        let spawn = transport().build_spawn(job(kind), "SYS", "REASSERT-SENTINEL");
        let args = &spawn.args;
        let p = args.iter().position(|a| a == "-p").expect("missing -p");
        assert_eq!(
            args[p + 1],
            "REASSERT-SENTINEL",
            "the prompt must be the value of -p for {kind:?}: {args:?}"
        );

        let empty = transport().build_spawn(job(kind), "SYS", "");
        let pe = empty.args.iter().position(|a| a == "-p").expect("missing -p");
        assert_eq!(
            empty.args[pe + 1],
            "",
            "an empty prompt must still occupy the slot for {kind:?}: {:?}",
            empty.args
        );
        assert_eq!(
            empty.args.len(),
            args.len(),
            "an empty prompt must not change the argv length for {kind:?}"
        );
    }
}

#[test]
fn argv_never_passes_a_fallback_model() {
    let spawn = transport().build_spawn(job(Kind::Judge), "SYS", "INSTRUCTION");
    // A fallback model would let the CLI silently swap models, defeating the canonicalModel guard.
    assert!(
        !spawn.args.iter().any(|a| a.contains("fallback")),
        "no --fallback-model may be passed: {:?}",
        spawn.args
    );
}

#[test]
fn argv_carries_the_configured_model_and_the_shared_system_prompt() {
    let spawn = transport().build_spawn(
        Job {
            kind: Kind::Slot,
            model: "some-configured-model",
            max_output_tokens: DEFAULT_SLOT_MAX_OUTPUT_TOKENS,
        },
        "THE-SYSTEM",
        "THE-INSTRUCTION",
    );
    let args = &spawn.args;
    let after = |needle: &str| {
        let i = args.iter().position(|a| a == needle).expect(needle);
        args[i + 1].clone()
    };
    // The config-resolved pin must reach --model (AC11's cli half).
    assert_eq!(after("--model"), "some-configured-model");
    assert_eq!(after("--system-prompt"), "THE-SYSTEM");
    // The instruction rides argv; the facts ride stdin.
    assert_eq!(after("-p"), "THE-INSTRUCTION");
    assert_eq!(spawn.program, PathBuf::from("/usr/local/bin/claude"));
}
