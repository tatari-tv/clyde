#![allow(clippy::unwrap_used)]

//! The cache counterfactual: list price ("every token fresh input"), cache savings and the spend
//! they reconcile with, all on the per-session cents basis the headline uses.

use super::*;

/// A single model whose tokens fold cleanly: build a totals rollup with cache tokens, price it
/// against the embedded feed, and read the rates back via the public `lookup` so the hand-computed
/// value tracks the feed (never a hardcoded rate) yet still exercises the folding formula
/// independently of `compute`.
fn model_tokens(
    input: u64,
    output: u64,
    cache_read: u64,
    cache_5m: u64,
    cache_1h: u64,
    spend: Option<f64>,
) -> ModelTokens {
    ModelTokens {
        input,
        output,
        cache_5m_write: cache_5m,
        cache_1h_write: cache_1h,
        cache_read,
        total: input + output + cache_read + cache_5m + cache_1h,
        spend_usd: spend,
    }
}

/// One session carrying `models` and `actual_spend`, with the report's totals the same: the cache
/// counterfactual reads each session's own models, so a fixture needs a session, not just totals.
fn report_with_totals(models: Vec<(&str, ModelTokens)>, actual_spend: f64) -> Report {
    let mut session = entry(
        None,
        Some("a/b"),
        ts("2026-06-10T10:00:00Z"),
        "unused",
        0,
        Some(actual_spend),
    );
    session.models = models.iter().map(|(n, m)| (n.to_string(), m.clone())).collect();
    report_of(vec![session])
}

/// A report over `sessions`, its totals the union of their models (tokens summed, spend summed) and
/// its headline the sum of session cents, the shape `build_report` produces.
fn report_of(sessions: Vec<SessionEntry>) -> Report {
    let mut models: BTreeMap<String, ModelTokens> = BTreeMap::new();
    for s in &sessions {
        for (name, m) in &s.models {
            let row = models.entry(name.clone()).or_default();
            row.input += m.input;
            row.output += m.output;
            row.cache_read += m.cache_read;
            row.cache_5m_write += m.cache_5m_write;
            row.cache_1h_write += m.cache_1h_write;
            row.total += m.total;
            row.spend_usd = match (row.spend_usd, m.spend_usd) {
                (a, None) => a,
                (None, b) => b,
                (Some(a), Some(b)) => Some(a + b),
            };
        }
    }
    let ids: Vec<String> = (0..sessions.len()).map(|i| format!("s{i:03}")).collect();
    let mut report = report_with(
        "2026-06-01T00:00:00Z",
        "2026-07-01T00:00:00Z",
        ids.iter().map(String::as_str).zip(sessions).collect(),
    );
    report.totals.spend_usd = crate::cents::to_dollars(crate::report::session_cents_total(report.sessions.values()));
    report.totals.models = models;
    report
}

#[test]
fn cache_counterfactual_equals_hand_computed_value() {
    let p = pricing();
    // Token counts chosen small so folded input stays under the 200k long-context tier: the
    // standard per-mtok rates apply and the hand computation is a plain linear formula.
    let input = 1_000u64;
    let output = 500u64;
    let cache_read = 9_000u64;
    let cache_5m = 0u64;
    let cache_1h = 0u64;
    let actual_spend = 0.12_f64;
    let report = report_with_totals(
        vec![(
            "claude-opus-4-7",
            model_tokens(input, output, cache_read, cache_5m, cache_1h, Some(actual_spend)),
        )],
        actual_spend,
    );

    let aggregates = compute(&report, DEFAULT_OUTLIERS, &p);

    // cache-read-share = cache_read / (input + cache_read + cache_5m + cache_1h)
    assert_eq!(aggregates.cache.cache_read_share, "90.0%");
    assert_eq!(aggregates.cache.input_tokens_human, "1,000");
    assert_eq!(aggregates.cache.cache_read_tokens_human, "9,000");

    // Hand-compute the counterfactual from the feed's own published rates: ALL cache tokens folded
    // into input, cache fields zeroed. Rates read via the public API so this tracks the feed.
    let rates = p
        .lookup("claude-opus-4-7")
        .expect("opus-4-7 is priced in the embedded feed");
    let folded_input = input + cache_read + cache_5m + cache_1h;
    let expected_list_price =
        folded_input as f64 * rates.input_per_mtok / 1_000_000.0 + output as f64 * rates.output_per_mtok / 1_000_000.0;

    assert_eq!(
        aggregates.cache.list_price_equivalent.as_deref(),
        Some(format_usd(expected_list_price).as_str())
    );
    assert_eq!(
        aggregates.cache.cache_savings.as_deref(),
        Some(format_usd(expected_list_price - actual_spend).as_str())
    );
}

#[test]
fn cache_counterfactual_absent_when_a_cache_bearing_model_is_unpriced() {
    let report = report_with_totals(
        vec![(
            "claude-does-not-exist-9",
            // Nonzero cache reads on an unpriced model: the whole counterfactual is unknowable.
            model_tokens(1_000, 500, 4_000, 0, 0, None),
        )],
        0.0,
    );

    let aggregates = compute(&report, DEFAULT_OUTLIERS, &pricing());

    assert_eq!(aggregates.cache.list_price_equivalent, None);
    assert_eq!(aggregates.cache.cache_savings, None);

    // The Option::is_none skip means the keys are ABSENT from the serialized context, never "$0".
    let json = serde_json::to_value(&aggregates.cache).unwrap();
    assert!(json.get("list-price-equivalent").is_none());
    assert!(json.get("cache-savings").is_none());
    // The non-pricing fields are still present regardless.
    assert!(json.get("cache-read-share").is_some());
}

#[test]
fn cache_savings_renders_negative_when_actual_spend_exceeds_counterfactual() {
    let p = pricing();
    // Write-heavy, low-reuse profile: cache writes are priced above base input, so actual spend
    // can exceed the "fold everything into fresh input" counterfactual and cache_savings goes
    // negative. It must render "-$..." (sign before dollar), never a corrupted "$-..." figure the
    // model copies verbatim into the report.
    let input = 1_000u64;
    let output = 500u64;
    let cache_read = 0u64;
    let cache_5m = 9_000u64;
    let cache_1h = 0u64;

    let rates = p
        .lookup("claude-opus-4-7")
        .expect("opus-4-7 is priced in the embedded feed");
    let folded_input = input + cache_read + cache_5m + cache_1h;
    let list_price =
        folded_input as f64 * rates.input_per_mtok / 1_000_000.0 + output as f64 * rates.output_per_mtok / 1_000_000.0;

    // Force actual spend above the counterfactual so savings is negative.
    let actual_spend = list_price + 1.0;
    let report = report_with_totals(
        vec![(
            "claude-opus-4-7",
            model_tokens(input, output, cache_read, cache_5m, cache_1h, Some(actual_spend)),
        )],
        actual_spend,
    );

    let aggregates = compute(&report, DEFAULT_OUTLIERS, &p);
    let savings = aggregates.cache.cache_savings.as_deref().unwrap();
    assert_eq!(savings, format_usd(list_price - actual_spend).as_str());
    assert!(
        savings.starts_with("-$"),
        "negative savings must render sign-before-dollar, got {savings}"
    );
}

#[test]
fn cache_counterfactual_present_when_unpriced_model_has_no_cache_tokens() {
    let p = pricing();
    // A priced cache-bearing model plus an unpriced model with ZERO cache tokens: the latter does
    // not nullify the counterfactual (only cache-bearing unpriced models do).
    let report = report_with_totals(
        vec![
            ("claude-opus-4-7", model_tokens(1_000, 500, 9_000, 0, 0, Some(0.12))),
            ("claude-does-not-exist-9", model_tokens(1_000, 0, 0, 0, 0, None)),
        ],
        0.12,
    );

    let aggregates = compute(&report, DEFAULT_OUTLIERS, &p);
    assert!(aggregates.cache.list_price_equivalent.is_some());
    assert!(aggregates.cache.cache_savings.is_some());
}

/// A pricing feed with one long-context-tiered model, loaded through the user-override path (the
/// embedded feed carries no tiered model today). `f` runs with it under the crate's env lock.
fn with_tiered_pricing<R>(f: impl FnOnce(&Pricing) -> R) -> R {
    let guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("clyde-cache-tier-test");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("pricing.json"),
        r#"{"schema_version":2,"data_version":"tier-test","min_library_version":"0.0.0","pricing":{
            "claude-tier-test":{"input_per_mtok":3,"output_per_mtok":15,"cache_5m_write_per_mtok":3.75,
            "cache_1h_write_per_mtok":6,"cache_read_per_mtok":0.3,"input_per_mtok_above_200k":6,
            "output_per_mtok_above_200k":22.5,"cache_5m_write_per_mtok_above_200k":7.5,
            "cache_1h_write_per_mtok_above_200k":12,"cache_read_per_mtok_above_200k":0.6}}}"#,
    )
    .unwrap();
    let prior = std::env::var("XDG_CONFIG_HOME").ok();
    // SAFETY: serialized by `ENV_LOCK`, the crate's one lock for environment mutation.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", tmp.path()) };
    let pricing = Pricing::with_user_override("clyde-cache-tier-test").unwrap();
    match prior {
        // SAFETY: as above.
        Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
    }
    drop(guard);
    let rates = pricing.lookup("claude-tier-test").expect("the override feed loaded");
    assert_eq!(
        rates.input_per_mtok_above_200k,
        Some(6.0),
        "precondition: a tiered model"
    );
    f(&pricing)
}

/// A session of `claude-tier-test` priced at its own tier, the way collect prices it.
fn tiered_session(pricing: &Pricing, input: u64, output: u64, cache_read: u64) -> SessionEntry {
    let totals = common::metrics::TokenTotals {
        input,
        output,
        cache_5m_write: 0,
        cache_1h_write: 0,
        cache_read,
        total: input + output + cache_read,
    };
    let model = ModelTokens::from_totals("claude-tier-test", &totals, pricing);
    let mut session = entry(
        None,
        Some("a/b"),
        ts("2026-06-10T10:00:00Z"),
        "unused",
        0,
        model.spend_usd,
    );
    session.models = BTreeMap::from([("claude-tier-test".to_string(), model)]);
    session
}

/// Audit finding: list price was priced on the WINDOW-WIDE token union, so a model whose month of
/// input crossed 200k got its list price at the long-context tier while every session's spend was
/// at the standard one, and "cache savings" was overstated by the tier gap. Each session here stays
/// under 200k total input; their union does not. The list price must be each session at its OWN
/// (standard) tier, summed in cents. BITES: price `totals.models` again and the list moves to the
/// premium rates.
#[test]
fn cache_list_price_prices_each_session_at_its_own_tier() {
    with_tiered_pricing(|pricing| {
        let sessions: Vec<SessionEntry> = (0..3)
            .map(|_| tiered_session(pricing, 10_000, 2_000, 140_000))
            .collect();
        let report = report_of(sessions);
        assert!(
            report.totals.models["claude-tier-test"].input + report.totals.models["claude-tier-test"].cache_read
                > 200_000,
            "precondition: the union crosses the long-context threshold"
        );
        let cache = compute(&report, DEFAULT_OUTLIERS, pricing).cache;

        // One session's list price at the standard tier: 150k folded input at $3, 2k output at $15.
        let per_session = cents::to_cents(cents::round_cents(150_000.0 * 3.0 / 1e6 + 2_000.0 * 15.0 / 1e6));
        assert_eq!(
            cache.list_price_cents,
            Some(3 * per_session),
            "each session at its own tier"
        );
        let union_premium = cents::to_cents(450_000.0 * 6.0 / 1e6 + 6_000.0 * 22.5 / 1e6);
        assert_ne!(
            cache.list_price_cents,
            Some(union_premium),
            "never the union's premium tier"
        );

        let spend = crate::report::session_cents_total(report.sessions.values());
        assert_eq!(cache.spend_cents, spend);
        assert_eq!(cache.cache_savings_cents, Some(3 * per_session - spend));
        assert_eq!(
            cache.cache_savings.as_deref(),
            Some(format_usd(cents::to_dollars(3 * per_session - spend)).as_str())
        );
    });
}

/// Audit finding: 100 sessions at $0.004 each with NO cache used read as spend $0.00 (each session
/// rounds to zero cents) but savings $0.40 (the f64 union price, rounded once). A window that used
/// no cache saves nothing. BITES: price the list from the union again and savings read $0.40.
#[test]
fn cache_savings_are_zero_when_no_session_used_the_cache_even_below_a_cent() {
    let pricing = pricing();
    let rates = pricing.lookup("claude-opus-4-7").unwrap();
    // $0.004 of fresh input per session at the feed's own rate.
    let input = (0.004 / rates.input_per_mtok * 1e6).round() as u64;
    let sessions: Vec<SessionEntry> = (0..100)
        .map(|_| {
            let totals = common::metrics::TokenTotals {
                input,
                output: 0,
                cache_5m_write: 0,
                cache_1h_write: 0,
                cache_read: 0,
                total: input,
            };
            let model = ModelTokens::from_totals("claude-opus-4-7", &totals, &pricing);
            let mut s = entry(
                None,
                Some("a/b"),
                ts("2026-06-10T10:00:00Z"),
                "unused",
                0,
                model.spend_usd,
            );
            s.models = BTreeMap::from([("claude-opus-4-7".to_string(), model)]);
            s
        })
        .collect();
    let report = report_of(sessions);
    let union = common::metrics::price(
        "claude-opus-4-7",
        &TokenUsage {
            input_tokens: input * 100,
            output_tokens: 0,
            cache_5m_write_tokens: 0,
            cache_1h_write_tokens: 0,
            cache_read_tokens: 0,
        },
        &pricing,
    )
    .unwrap();
    assert!(
        union >= 0.395,
        "precondition: the union prices to about $0.40, got {union}"
    );

    let cache = compute(&report, DEFAULT_OUTLIERS, &pricing).cache;
    assert_eq!(cache.spend_cents, 0, "each session rounds to zero cents");
    assert_eq!(cache.list_price_cents, Some(0), "no cache: list price IS spend");
    assert_eq!(cache.cache_savings_cents, Some(0));
    assert_eq!(cache.cache_savings.as_deref(), Some("$0.00"));
}

/// A cache-bearing model the artifact recorded no spend for makes the comparison unknowable even
/// when the render's feed can price it: list price would be compared with a spend that is not there.
#[test]
fn cache_counterfactual_absent_when_a_cache_bearing_model_has_no_recorded_spend() {
    let report = report_with_totals(
        vec![("claude-opus-4-7", model_tokens(1_000, 500, 9_000, 0, 0, None))],
        0.0,
    );
    let cache = compute(&report, DEFAULT_OUTLIERS, &pricing()).cache;
    assert_eq!(cache.list_price_cents, None);
    assert_eq!(cache.cache_savings, None);
}

/// A model that used no cache has a list price equal to its recorded spend, cent for cent, and the
/// window saves nothing. Two sessions, so the per-session cents must ADD. BITES: drop or negate the
/// no-cache branch's contribution and the list price misses the spend.
#[test]
fn cache_list_price_of_cache_free_sessions_is_their_spend() {
    let sessions: Vec<SessionEntry> = [1.23, 4.56]
        .iter()
        .map(|spend| {
            let mut s = entry(None, Some("a/b"), ts("2026-06-10T10:00:00Z"), "unused", 0, Some(*spend));
            s.models = BTreeMap::from([(
                "claude-opus-4-7".to_string(),
                model_tokens(10_000, 2_000, 0, 0, 0, Some(*spend)),
            )]);
            s
        })
        .collect();
    let cache = compute(&report_of(sessions), DEFAULT_OUTLIERS, &pricing()).cache;
    assert_eq!(cache.spend_cents, 579);
    assert_eq!(cache.list_price_cents, Some(579));
    assert_eq!(cache.cache_savings_cents, Some(0));
}
