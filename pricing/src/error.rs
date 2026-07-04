use std::path::PathBuf;

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum PricingError {
    #[error("unknown model: {0}")]
    UnknownModel(String),

    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("malformed pricing data at {source_label}: {message}")]
    Malformed { source_label: String, message: String },

    #[error("schema version {got} not supported (max {max})")]
    UnsupportedSchema { got: u32, max: u32 },

    #[cfg(feature = "fetch")]
    #[error("fetch failed for {url}: {message}")]
    Fetch { url: String, message: String },

    /// A reachable, schema-valid feed whose `data_version` is older than (or not
    /// comparable to) the embedded baseline. Distinct from `Fetch` so the caller
    /// can persist the dedicated stale-feed sidecar and surface the state instead
    /// of treating it as a plain transient fetch failure.
    #[cfg(feature = "fetch")]
    #[error(
        "fetched feed for {url} is stale (data_version {fetched:?} is not newer than embedded baseline {embedded})"
    )]
    StaleFeed {
        fetched: Option<String>,
        embedded: String,
        url: String,
    },
}
