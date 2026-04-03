use eyre::{Context, Result};
use log::debug;
use std::collections::HashMap;
use std::process::Command;

use crate::config::Config;
use crate::pricing::ModelPricing;
use crate::table;

// NOTE: This URL is intentionally duplicated in bin/update (bash script).
// bin/update is a dev-time bash script; this module is the compiled binary's --check.
// Sharing across bash/Rust is not worth the complexity for a single URL.
const PRICING_URL: &str = "https://platform.claude.com/docs/en/about-claude/pricing.md";

/// Display current pricing table
pub fn show(config: &Config) -> Result<()> {
    debug!("update::show: model_count={}", config.pricing.len());

    if config.pricing.is_empty() {
        eyre::bail!("No pricing data available.");
    }

    println!("Current pricing (per million tokens):\n");

    let mut models: Vec<_> = config.pricing.iter().collect();
    models.sort_by_key(|(name, _)| (*name).clone());

    let mut rows: Vec<Vec<String>> = Vec::new();
    for (name, p) in &models {
        rows.push(vec![
            name.to_string(),
            format!("${:.2}", p.input_per_mtok),
            format!("${:.2}", p.output_per_mtok),
            format!("${:.2}", p.cache_5m_write_per_mtok),
            format!("${:.2}", p.cache_1h_write_per_mtok),
            format!("${:.2}", p.cache_read_per_mtok),
        ]);
        if p.input_per_mtok_above_200k.is_some() {
            rows.push(vec![
                format!("  (>200K)"),
                format!("${:.2}", p.input_per_mtok_above_200k.unwrap_or(0.0)),
                format!("${:.2}", p.output_per_mtok_above_200k.unwrap_or(0.0)),
                format!("${:.2}", p.cache_5m_write_per_mtok_above_200k.unwrap_or(0.0)),
                format!("${:.2}", p.cache_1h_write_per_mtok_above_200k.unwrap_or(0.0)),
                format!("${:.2}", p.cache_read_per_mtok_above_200k.unwrap_or(0.0)),
            ]);
        }
    }

    println!(
        "{}",
        table::build(
            &["Model", "Input", "Output", "Cache5mW", "Cache1hW", "CacheR"],
            rows,
            &[1, 2, 3, 4, 5],
        )
    );

    Ok(())
}

/// Check if embedded pricing may be stale by comparing the compile-time
/// hash of the pricing page against the current live page.
///
/// Exit codes (for scripting):
///   0 = up to date
///   1 = may be stale
///   2 = fetch failed
pub fn check() -> Result<i32> {
    let embedded_hash = env!("PRICING_PAGE_SHA256");
    let version = env!("GIT_DESCRIBE");

    if embedded_hash.is_empty() {
        eprintln!("No baseline hash embedded. Run bin/update to establish one.");
        return Ok(2);
    }

    debug!("check: embedded_hash={}, fetching live page", embedded_hash);

    // Fetch the live pricing page
    let output = match Command::new("curl")
        .args(["-sS", "--max-time", "15", PRICING_URL])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            eprintln!("Could not fetch pricing page: {}. Skipping check.", e);
            return Ok(2);
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("Could not fetch pricing page: {}. Skipping check.", stderr.trim());
        return Ok(2);
    }

    let body = String::from_utf8(output.stdout).context("Invalid UTF-8 from pricing page")?;
    if body.is_empty() {
        eprintln!("Could not fetch pricing page: empty response. Skipping check.");
        return Ok(2);
    }

    // Compute SHA-256 of the fetched page via sha256sum
    // Note: we could use the sha2 crate instead, but shelling out keeps dependencies minimal
    let hash_output = Command::new("sh")
        .args([
            "-c",
            &format!("echo '{}' | sha256sum | cut -d' ' -f1", body.replace('\'', "'\\''")),
        ])
        .output();

    // Fallback: use printf to avoid issues with special chars
    let live_hash = match hash_output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => {
            // Try alternative approach using printf and piping
            let child = Command::new("sha256sum")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn();
            match child {
                Ok(mut proc) => {
                    if let Some(ref mut stdin) = proc.stdin {
                        use std::io::Write;
                        let _ = stdin.write_all(body.as_bytes());
                    }
                    let out = proc.wait_with_output().context("sha256sum failed")?;
                    String::from_utf8_lossy(&out.stdout)
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_string()
                }
                Err(e) => {
                    eprintln!("Could not compute hash: {}. Skipping check.", e);
                    return Ok(2);
                }
            }
        }
    };

    if live_hash == embedded_hash {
        println!("Pricing is up to date (matches {} build).", version);
        Ok(0)
    } else {
        println!(
            "The Anthropic pricing page has changed since ccu was built ({}).\n\
             Pricing may be outdated. Check for a new release or run bin/update to refresh.",
            version
        );
        Ok(1)
    }
}

/// Helper struct for parsing just the pricing section from YAML
#[derive(Debug, serde::Deserialize)]
pub struct PricingOnly {
    pub pricing: HashMap<String, ModelPricing>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pricing_url_points_to_raw_markdown() {
        assert!(PRICING_URL.ends_with(".md"), "URL should fetch raw markdown");
        assert!(PRICING_URL.contains("pricing"), "URL must fetch the pricing page");
    }

    #[test]
    fn test_embedded_hash_is_string() {
        // Just verify the compile-time constant is accessible
        let hash = env!("PRICING_PAGE_SHA256");
        // Hash is either empty (no baseline) or 64 hex chars
        assert!(
            hash.is_empty() || hash.len() == 64,
            "hash should be empty or 64 hex chars"
        );
    }
}
