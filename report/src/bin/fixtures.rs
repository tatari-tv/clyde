//! Regenerate the committed render-eval fixtures (design Phase 13).
//!
//! ```text
//! cargo run -p report --bin fixtures -- fixtures/report
//! ```
//!
//! Writes `report.json` for each of the three synthesized fixtures, plus the medium fixture's
//! `prior.json` and `analytics.json`. It does NOT touch `eval.yml` (hand-written, it is the spec)
//! or the goldens (model-authored, regenerated with `clyde report render`).
//!
//! Seeded and clock-frozen, so re-running it on an unchanged generator rewrites byte-identical
//! files: a diff here is a generator change, never a clock tick. Every org, repo, title and summary
//! it emits is INVENTED -- `tatari-tv/clyde` is public, and no fixture may be derived from real
//! session data.

#![deny(clippy::unwrap_used)]

use claude_pricing::Pricing;
use eyre::{Context, Result};
use report::eval::synth::{self, Kind};
use std::path::{Path, PathBuf};

/// Where the fixtures land when no path is given.
const DEFAULT_ROOT: &str = "fixtures/report";

fn main() -> Result<()> {
    // `Builder::new()`, not `from_default_env()`: the level here is FIXED at info, and the previous
    // form said otherwise while doing the same thing. `from_default_env()` reads `RUST_LOG` into the
    // builder and the `filter_level` that followed overwrote it, so the env var was already dead --
    // a name that promised configurability the code then took away. This fixture generator has no
    // flag surface at all: its real output is the `println!` progress below, and the logger only
    // reaches library internals. It deliberately does NOT go through `common::logging` -- that
    // policy always opens a tool log file under `clyde/logs/`, and a dev fixture generator is not
    // a tool and should not create one.
    env_logger::Builder::new().filter_level(log::LevelFilter::Info).init();
    let root: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_ROOT.to_string())
        .into();
    // The same embedded pin the eval itself uses: a fixture priced against the live feed is not
    // reproducible, and the goldens rendered from it would drift with the feed.
    let pricing = Pricing::embedded();

    for kind in [Kind::Small, Kind::Medium, Kind::Pathological] {
        let Some(name) = kind.dir_name() else { continue };
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
        let report = synth::build(kind, &pricing)?;
        write(&dir.join("report.json"), &json(&report)?)?;

        if kind == Kind::Medium {
            // `prior.json` lights up Month over Month; `analytics.json` lights up Reconciliation.
            // Both live inside the medium fixture rather than being fixtures of their own: they are
            // inputs to ONE render, not windows anyone evaluates on their own.
            let prior = synth::build(Kind::MediumPrior, &pricing)?;
            write(&dir.join("prior.json"), &json(&prior)?)?;
            write(&dir.join("analytics.json"), &synth::analytics_export(&report)?)?;
        }
    }
    Ok(())
}

fn json(report: &report::report::Report) -> Result<String> {
    Ok(serde_json::to_string_pretty(report).context("failed to serialize a synthesized fixture")? + "\n")
}

fn write(path: &Path, body: &str) -> Result<()> {
    std::fs::write(path, body).with_context(|| format!("failed to write {}", path.display()))?;
    println!("wrote {} ({} bytes)", path.display(), body.len());
    Ok(())
}
