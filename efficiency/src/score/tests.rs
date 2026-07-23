#![allow(clippy::unwrap_used)]

use std::path::Path;

use common::EfficiencyConfig;

use super::*;
use crate::extract::extract;
use crate::fold::fold;
use crate::metrics::{Compaction, CompactionTrigger, RawCounters, finalize};

const MULTI_SUBAGENT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../fixtures/efficiency/multi-subagent.jsonl"
);

/// Default `efficiency:` thresholds (floor 0.6, ceiling 0.05, auto-compaction on, gates
/// 20000 tokens / 3 turns) — the values a missing config resolves to.
fn default_config() -> EfficiencyConfig {
    EfficiencyConfig::default()
}

/// Build aggregate signals through the real `finalize` path so the derived metrics (cache share,
/// tool-error rate) are computed exactly as production computes them — not hand-set.
fn signals(raw: RawCounters) -> EfficiencySignals {
    finalize(raw)
}

fn auto_compaction() -> Compaction {
    Compaction {
        trigger: CompactionTrigger::Auto,
        pre_tokens: 100_000,
        post_tokens: 10_000,
        duration_ms: 5000,
    }
}

#[test]
fn eligible_degraded_flags_low_cache_high_error_and_auto_compaction() {
    // Eligible (27000 tokens >= 20000, 5 turns >= 3), low cache share (~0.037 < 0.6), high error
    // rate (2/10 = 0.2 > 0.05), one auto-compaction. ALL THREE flags must fire.
    let raw = RawCounters {
        input_tokens: 25_000,
        output_tokens: 1_000,
        cache_read_tokens: 1_000,
        turns: 5,
        tool_calls: 10,
        tool_errors: 2,
        compactions: vec![auto_compaction()],
        ..RawCounters::default()
    };
    let flags = score(&signals(raw), &default_config());

    assert_eq!(flags.len(), 3, "expected all three flags, got {flags:?}");
    match &flags[0] {
        EfficiencyFlag::LowCacheReadShare { observed, floor } => {
            assert!(*observed < *floor, "observed {observed} must be below floor {floor}");
            assert_eq!(*floor, 0.6);
        }
        other => panic!("expected LowCacheReadShare first, got {other:?}"),
    }
    match &flags[1] {
        EfficiencyFlag::HighToolErrorRate { observed, ceiling } => {
            assert!((*observed - 0.2).abs() < 1e-12, "rate = 2/10, got {observed}");
            assert_eq!(*ceiling, 0.05);
        }
        other => panic!("expected HighToolErrorRate second, got {other:?}"),
    }
    assert_eq!(flags[2], EfficiencyFlag::AutoCompaction { count: 1 });
}

#[test]
fn healthy_eligible_session_flags_nothing() {
    // Eligible, high cache share (~0.97), low error rate (1/100 = 0.01), no compaction. Zero flags.
    let raw = RawCounters {
        input_tokens: 1_000,
        output_tokens: 1_000,
        cache_read_tokens: 30_000,
        turns: 5,
        tool_calls: 100,
        tool_errors: 1,
        ..RawCounters::default()
    };
    let flags = score(&signals(raw), &default_config());
    assert!(
        flags.is_empty(),
        "a healthy eligible session must not flag, got {flags:?}"
    );
}

#[test]
fn ineligible_short_below_floor_does_not_flag_cache_waste() {
    // The eligibility gate proven: cache share ~0.09 is WELL below the 0.6 floor, but the session
    // is ~110 tokens / 2 turns — under BOTH gates — so cache-waste must NOT flag (false-positive
    // suppression on a short one-shot). No errors, no compaction -> zero flags total.
    let raw = RawCounters {
        input_tokens: 100,
        cache_read_tokens: 10,
        turns: 2,
        ..RawCounters::default()
    };
    let sig = signals(raw);
    assert!(
        sig.cache_read_share.unwrap() < 0.6,
        "precondition: share {:?} is below floor",
        sig.cache_read_share
    );
    let flags = score(&sig, &default_config());
    assert!(
        flags.is_empty(),
        "ineligible short session below the floor must not flag, got {flags:?}"
    );
}

#[test]
fn eligible_tokens_but_too_few_turns_does_not_flag_cache_waste() {
    // The turn gate specifically: plenty of tokens (>= 20000) but only 2 turns (< 3). Still
    // ineligible for cache-waste; low share must not flag. (No errors/compaction -> no other flag.)
    let raw = RawCounters {
        input_tokens: 30_000,
        cache_read_tokens: 100,
        turns: 2,
        ..RawCounters::default()
    };
    let flags = score(&signals(raw), &default_config());
    assert!(
        flags
            .iter()
            .all(|f| !matches!(f, EfficiencyFlag::LowCacheReadShare { .. })),
        "too-few-turns must suppress the cache-waste flag, got {flags:?}"
    );
    assert!(flags.is_empty(), "no other signal breached either, got {flags:?}");
}

#[test]
fn auto_compaction_flags_independent_of_eligibility() {
    // A tiny INELIGIBLE session (110 tokens, 1 turn) with a low share AND an auto-compaction:
    // the cache-waste flag is gated OUT, but the auto-compaction flag fires regardless of size.
    let raw = RawCounters {
        input_tokens: 100,
        cache_read_tokens: 10,
        turns: 1,
        compactions: vec![auto_compaction()],
        ..RawCounters::default()
    };
    let flags = score(&signals(raw), &default_config());
    assert_eq!(
        flags,
        vec![EfficiencyFlag::AutoCompaction { count: 1 }],
        "only the (ungated) auto-compaction flag, never cache-waste on an ineligible session"
    );
}

#[test]
fn auto_compaction_flag_disabled_suppresses_it() {
    // Config drives behavior: with auto-compaction-flag off, an auto-compaction raises no flag.
    let raw = RawCounters {
        turns: 5,
        compactions: vec![auto_compaction()],
        ..RawCounters::default()
    };
    let cfg = load_config("auto-compaction-flag: false\n");
    let flags = score(&signals(raw), &cfg);
    assert!(
        flags.is_empty(),
        "auto-compaction-flag off must suppress it, got {flags:?}"
    );
}

#[test]
fn manual_compaction_does_not_flag() {
    // Only AUTO compactions flag (a session that ran to the wall). A manual compaction is a
    // deliberate user action and must not raise the auto-compaction flag.
    let raw = RawCounters {
        turns: 5,
        compactions: vec![Compaction {
            trigger: CompactionTrigger::Manual,
            pre_tokens: 100_000,
            post_tokens: 10_000,
            duration_ms: 5000,
        }],
        ..RawCounters::default()
    };
    let flags = score(&signals(raw), &default_config());
    assert!(flags.is_empty(), "a manual compaction must not flag, got {flags:?}");
}

#[test]
fn custom_floor_flips_the_verdict() {
    // Tests bite on the threshold: share 0.5 does NOT breach the default 0.6 floor here because the
    // session is short/ineligible; raise eligibility by size and lower the floor, and the same
    // share crosses. Proves the flag is driven by the CONFIGURED floor, not a hardcoded constant.
    let raw = RawCounters {
        input_tokens: 10_000,
        cache_read_tokens: 10_000,
        turns: 5,
        ..RawCounters::default()
    }; // share = 10000/20000 = 0.5, total 20000 tokens (eligible), 5 turns.
    let sig = signals(raw);
    assert!((sig.cache_read_share.unwrap() - 0.5).abs() < 1e-12);

    // Floor 0.6: 0.5 < 0.6 -> flags.
    let flags = score(&sig, &default_config());
    assert!(
        flags
            .iter()
            .any(|f| matches!(f, EfficiencyFlag::LowCacheReadShare { .. })),
        "0.5 share is below the 0.6 default floor -> must flag, got {flags:?}"
    );

    // Floor 0.4: 0.5 >= 0.4 -> no cache flag.
    let cfg = load_config("cache-read-share-floor: 0.4\n");
    let flags = score(&sig, &cfg);
    assert!(
        flags
            .iter()
            .all(|f| !matches!(f, EfficiencyFlag::LowCacheReadShare { .. })),
        "0.5 share is at/above a 0.4 floor -> must not flag, got {flags:?}"
    );
}

#[test]
fn no_tool_calls_never_flags_error_rate() {
    // tool_error_rate is None when tool_calls == 0 (n/a, never NaN) -> no HighToolErrorRate flag.
    let raw = RawCounters {
        turns: 5,
        tool_calls: 0,
        tool_errors: 0,
        ..RawCounters::default()
    };
    let sig = signals(raw);
    assert_eq!(sig.tool_error_rate, None);
    let flags = score(&sig, &default_config());
    assert!(flags.is_empty(), "no tool calls -> no error-rate flag, got {flags:?}");
}

#[test]
fn scored_on_multi_subagent_fixture_proves_gate_end_to_end() {
    // End-to-end through the real pipeline: extract -> fold -> scored. multi-subagent aggregate is
    // 2325 total tokens (< 20000 gate) so it is INELIGIBLE: cache_read_share (300/2250 = 0.133) is
    // below the floor yet must NOT raise a cache-waste flag. The non-gated flags DO fire: 2/3 tool
    // errors (> 0.05) and the one auto-compaction.
    let file = extract(Path::new(MULTI_SUBAGENT)).unwrap();
    let session = fold("fix00007-e000-4000-a000-000000000007", &[file]);

    // Precondition: the aggregate share is genuinely below the floor (so the gate, not the value,
    // is what suppresses the cache flag).
    assert!(session.aggregate.cache_read_share.unwrap() < 0.6);
    assert_eq!(session.aggregate.raw.tool_calls, 3);
    assert_eq!(session.aggregate.raw.tool_errors, 2);

    let session = scored(session, &default_config());

    assert!(
        session
            .flags
            .iter()
            .all(|f| !matches!(f, EfficiencyFlag::LowCacheReadShare { .. })),
        "ineligible-by-tokens session must not flag cache-waste, got {:?}",
        session.flags
    );
    assert!(
        session
            .flags
            .iter()
            .any(|f| matches!(f, EfficiencyFlag::HighToolErrorRate { .. })),
        "2/3 tool errors is above the ceiling, got {:?}",
        session.flags
    );
    assert!(
        session.flags.contains(&EfficiencyFlag::AutoCompaction { count: 1 }),
        "the parent auto-compaction must flag, got {:?}",
        session.flags
    );
}

/// Deserialize an [`EfficiencyConfig`] from a YAML fragment (the fields under `efficiency:`),
/// exercising the real kebab-case serde path + per-field defaults for the config-driven tests.
fn load_config(yaml: &str) -> EfficiencyConfig {
    serde_yaml::from_str(yaml).expect("valid efficiency config fragment")
}
