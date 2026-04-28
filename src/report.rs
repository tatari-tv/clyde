use crate::session::SessionSummary;
use chrono::{DateTime, Utc};
use eyre::Result;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Default)]
pub struct ExistingTitles(pub HashMap<String, String>);

pub fn load_existing_titles(path: &Path) -> ExistingTitles {
    log::trace!("report::load_existing_titles: path={}", path.display());
    ExistingTitles::default()
}

pub fn write_yaml(
    path: &Path,
    summaries: &[SessionSummary],
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    host: &str,
) -> Result<usize> {
    log::trace!(
        "report::write_yaml: path={} summaries={} since={} until={} host={}",
        path.display(),
        summaries.len(),
        since,
        until,
        host
    );
    Ok(summaries.len())
}
