use super::*;

#[test]
fn format_int_comma_groups_large_numbers() {
    assert_eq!(format_int(35_373), "35,373");
    assert_eq!(format_int(1_234_567), "1,234,567");
    assert_eq!(format_int(0), "0");
    assert_eq!(format_int(999), "999");
}

#[test]
fn format_usd_comma_groups_dollars_and_pads_cents() {
    assert_eq!(format_usd(0.6), "$0.60");
    assert_eq!(format_usd(1_234.5), "$1,234.50");
    assert_eq!(format_usd(0.0), "$0.00");
}

#[test]
fn format_usd_handles_negatives_sign_before_dollar_no_stray_comma() {
    // cache_savings can go negative; sign goes before the `$`, magnitude is comma-grouped.
    assert_eq!(format_usd(-1.01), "-$1.01");
    assert_eq!(format_usd(-500.0), "-$500.00");
    assert_eq!(format_usd(-1_234.5), "-$1,234.50");
    assert_eq!(format_usd(-0.01), "-$0.01");
    assert_eq!(format_usd(-1_000_000.0), "-$1,000,000.00");
}

#[test]
fn format_optional_usd_renders_untracked_for_none() {
    assert_eq!(format_optional_usd(Some(1.5)), "$1.50");
    assert_eq!(format_optional_usd(None), "(untracked)");
}

#[test]
fn format_tokens_human_matches_exact_design_examples() {
    // Design doc definitions: "9.53B" / "287.8M" / "35,373" style.
    assert_eq!(format_tokens_human(9_530_000_000), "9.53B");
    assert_eq!(format_tokens_human(287_800_000), "287.8M");
    assert_eq!(format_tokens_human(35_373), "35,373");
}

#[test]
fn format_tokens_human_boundary_at_one_million_and_one_billion() {
    assert_eq!(format_tokens_human(999_999), "999,999");
    assert_eq!(format_tokens_human(1_000_000), "1.0M");
    assert_eq!(format_tokens_human(999_999_999), "1000.0M");
    assert_eq!(format_tokens_human(1_000_000_000), "1.00B");
}

#[test]
fn short_id_is_uuid_prefix_for_bare_and_merged_keys() {
    // Normal report: bare UUID key -> first 8 chars.
    assert_eq!(short_id("9d4c1f28-1234-5678-9abc-def012345678"), "9d4c1f28");
    // Merged report: `host/uuid` key -> still the UUID prefix, never `laptop/9`.
    assert_eq!(short_id("laptop/9d4c1f28-1234-5678-9abc-def012345678"), "9d4c1f28");
    // A host containing hyphens or dots must not confuse the split on `/`.
    assert_eq!(short_id("my-host.local/9d4c1f28-abcd"), "9d4c1f28");
    // Shorter-than-8 UUID portion falls back to the whole portion, panic-free.
    assert_eq!(short_id("host/abc"), "abc");
    assert_eq!(short_id("abc"), "abc");
}
