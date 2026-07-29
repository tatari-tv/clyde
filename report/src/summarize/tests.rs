#![allow(clippy::unwrap_used)]

use super::*;

/// Each kind names its OWN ceiling key. A ceiling failure quotes this key as the remedy, so a shared
/// or crossed value would send the reader to a line that does not govern the job that failed -- the
/// "remedy that cannot remedy" `cli.rs`'s module docs reject.
///
/// BITES: return the same key from both arms and one assertion fails.
#[test]
fn each_kind_names_its_own_ceiling_key() {
    assert_eq!(Kind::Slot.max_output_tokens_key(), "render.slot-max-output-tokens");
    assert_eq!(Kind::Judge.max_output_tokens_key(), "render.judge-max-output-tokens");
    assert_ne!(Kind::Judge.max_output_tokens_key(), Kind::Slot.max_output_tokens_key());
}

/// `end_turn` is the ONLY acceptable stop: any other value means the reply hit the output ceiling
/// and is truncated, and a truncated artifact must never be published.
#[test]
fn only_end_turn_is_accepted() {
    assert!(check_stop_reason(Some("end_turn")).is_ok());
    for bad in ["max_tokens", "stop_sequence", "tool_use", "refusal"] {
        assert!(check_stop_reason(Some(bad)).is_err(), "{bad} must not pass");
    }
}

#[test]
fn a_truncation_error_names_the_stop_reason_and_a_remedy() {
    let err = check_stop_reason(Some("max_tokens")).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("max_tokens"), "the error quotes the stop reason: {msg}");
    assert!(
        msg.contains("config key") || msg.contains("--since"),
        "the error names a remedy: {msg}"
    );
}

#[test]
fn check_stop_reason_missing_bails() {
    let err = check_stop_reason(None).unwrap_err();
    assert!(format!("{err}").contains("<missing>"));
}
