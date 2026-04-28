use std::path::Path;

pub fn detect(cwd: &Path) -> Option<String> {
    log::trace!("repo::detect: cwd={}", cwd.display());
    None
}
