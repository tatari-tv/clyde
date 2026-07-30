#![allow(clippy::unwrap_used)]

use super::*;

/// Each kind names its OWN ceiling key. A ceiling failure quotes this key as the remedy, so a shared
/// or crossed value would send the reader to a line that does not govern the job that failed -- the
/// "remedy that cannot remedy" `cli.rs`'s module docs reject.
///
/// BITES: return the same key from both arms and one assertion fails.
#[test]
fn each_kind_names_its_own_ceiling_key() {
    assert_eq!(
        Kind::Slot.max_output_tokens_key(),
        Some("render.slot-max-output-tokens")
    );
    assert_eq!(
        Kind::Judge.max_output_tokens_key(),
        Some("render.judge-max-output-tokens")
    );
    assert_ne!(Kind::Judge.max_output_tokens_key(), Kind::Slot.max_output_tokens_key());
}

/// The kinds whose ceiling is a compile-time const name NO config key, and that `None` is what
/// `cli::check_envelope` reads as "no configurable budget, so nothing to enforce". Enumerated by kind so
/// a fifth kind cannot inherit an answer by accident.
///
/// BITES: invent an `enrich-max-output-tokens` key for either arm and this fails -- which is also the
/// unrequested config scope the design rejected.
#[test]
fn the_non_configurable_kinds_name_no_ceiling_key() {
    assert_eq!(Kind::Enrich.max_output_tokens_key(), None);
    assert_eq!(Kind::Narrate.max_output_tokens_key(), None);
}

/// The fence label describes the PAYLOAD, so the two kinds that send prose must not be labeled json.
///
/// BITES: return `"json"` from the Enrich/Narrate arm and this fails.
#[test]
fn the_fence_label_matches_what_each_kind_actually_sends() {
    assert_eq!(Kind::Slot.fence(), "json");
    assert_eq!(Kind::Judge.fence(), "json");
    assert_eq!(Kind::Enrich.fence(), "text");
    assert_eq!(Kind::Narrate.fence(), "text");
}

/// The one typed variant is downcastable off an `eyre::Report`, which is the whole mechanism
/// `sessions::enrich` uses to tell a dead transport from a bad session. If this ever stopped holding,
/// every sweep-fatal failure would silently become a per-session charge.
#[test]
fn a_transport_error_survives_the_trip_through_eyre() {
    let report: eyre::Report = TransportError::Unavailable("logged out".into()).into();
    assert!(report.to_string().contains("logged out"));
    assert!(
        matches!(
            report.downcast_ref::<TransportError>(),
            Some(TransportError::Unavailable(_))
        ),
        "the variant must be recoverable from the report: {report}"
    );
    // A plain eyre error must NOT downcast to it, or the classifier would fire on everything.
    let ordinary = eyre::eyre!("some per-session failure");
    assert!(ordinary.downcast_ref::<TransportError>().is_none());
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
