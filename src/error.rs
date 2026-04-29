use std::path::PathBuf;

#[derive(thiserror::Error, Debug)]
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
}
