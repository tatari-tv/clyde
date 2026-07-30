#![allow(clippy::unwrap_used)]

//! Split out of the former single 1,322-line `cli/tests.rs` (design Phase 6). The section banners in
//! that file were already the module boundaries; each submodule below is one contiguous run of them.

use super::*;

// ---- Phase 3: the ceiling no longer rejects valid enrichment work -------------------------------

/// The Phase 0 blocker (Finding 3), fixed. Measured `output_tokens` on real enrich payloads were 5,798
/// and 678 against a 512 const, so the ceiling check as written rejected 100% of enrich calls -- and the
/// value does not track payload size, so no low ceiling was safe. `stop_reason` is the direct truncation
/// signal the ceiling was only ever a proxy for.
///
/// BITES: drop the `if let Some(ceiling_key)` gate and the first assertion fails.
#[test]
fn the_output_ceiling_is_not_enforced_for_the_kinds_whose_ceiling_is_a_const() {
    // Probe A's real output count, an order of magnitude over the 512 const, on a natural stop.
    let json = envelope_json(
        false,
        "success",
        "end_turn",
        "{\"tags\":[],\"summary\":\"s\"}",
        5_798,
        &real_model_usage(),
    );
    for kind in [Kind::Enrich, Kind::Narrate] {
        let out = check_full(&json, kind)
            .unwrap_or_else(|e| panic!("{kind:?} must accept a natural stop over the const ceiling: {e}"));
        assert_eq!(out.tokens_out, 5_798, "the count is still reported, just not enforced");
    }
    // And the SAME envelope still fails for the kinds whose ceiling is a user budget.
    assert!(check(&json, Kind::Slot).is_err(), "a slot's budget is still enforced");

    // Truncation is still fatal for the new kinds: `stop_reason` is the whole contract now, so if it
    // stopped biting there would be nothing left.
    let truncated = envelope_json(
        false,
        "success",
        "max_tokens",
        "half a json obj",
        140,
        &real_model_usage(),
    );
    for kind in [Kind::Enrich, Kind::Narrate] {
        let err = check(&truncated, kind).unwrap_err().to_string();
        assert!(err.contains("stop_reason=max_tokens"), "{kind:?}: {err}");
        assert!(err.contains("truncated"), "{kind:?}: {err}");
    }
}

/// The token counts leave the transport as DATA, because `sessions` persists them.
#[test]
fn a_completion_carries_the_token_counts_it_was_billed() {
    let json = envelope_with_usage(Some(
        r#"{"input_tokens":10,"cache_creation_input_tokens":4336,"cache_read_input_tokens":0,"output_tokens":5798}"#,
    ));
    let out = check_full(&json, Kind::Enrich).unwrap();
    assert_eq!(out.text, "x");
    assert_eq!(out.tokens_in, 4346, "summed across every bucket the CLI bills");
    assert_eq!(out.tokens_out, 5798);
}

// ---- Phase 3: reasoning is suppressed for EXACTLY one kind --------------------------------------

/// Enumerated by kind, over the BUILT child `Command`, so a future refactor cannot silently extend the
/// setting to a kind that was never measured. `Kind::Narrate`'s exclusion is load-bearing: the flag
/// deterministically flips its verdict (3/3 inefficient with reasoning, 3/3 efficient without, Finding
/// 13), which the design's Non-Goal forbids. `Slot`/`Judge` would silently change `report render` and
/// `report eval`.
///
/// BITES: remove the `if kind == Kind::Enrich` conditional in `child_env` (either direction) and this
/// fails.
#[test]
fn reasoning_is_suppressed_for_enrich_and_for_no_other_kind() {
    let guard = ENV_LOCK.lock().unwrap();
    let built: Vec<(Kind, Vec<(String, String)>)> = ALL_KINDS
        .iter()
        .map(|kind| {
            let cmd = transport().build_spawn(job(*kind), "SYS", "").to_command();
            let env: Vec<(String, String)> = cmd
                .get_envs()
                .filter_map(|(k, v)| Some((k.to_string_lossy().into_owned(), v?.to_string_lossy().into_owned())))
                .collect();
            (*kind, env)
        })
        .collect();
    drop(guard);

    for (kind, env) in built {
        let value = env
            .iter()
            .find(|(k, _)| k == MAX_THINKING_TOKENS)
            .map(|(_, v)| v.clone());
        match kind {
            Kind::Enrich => assert_eq!(
                value.as_deref(),
                Some(THINKING_DISABLED),
                "enrich runs once per session; the 67%-cheaper, 9x-faster path is the measured one"
            ),
            other => assert_eq!(
                value, None,
                "{other:?} must keep today's behavior: setting this changes what it produces"
            ),
        }
    }
}
