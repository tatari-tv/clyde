//! Precedence tests for the one projects-dir resolver.
//!
//! The DEFAULT arm is deliberately not asserted here: it reads `$HOME`, and an env-mutating unit
//! test would have to serialize against every other test in the crate for a branch the integration
//! suite already drives end to end (`clyde/tests/matrix.rs`, AC7). These two cover the two arms that
//! are pure.

use super::*;

/// Build a `Config` with `projects-dir` set, through the real deserializer rather than a hand-built
/// struct: the field is private, and going through serde is also what proves the KEY spelling is
/// what an operator would actually write in `clyde.yml`.
fn cfg_with_projects_dir(path: &str) -> Config {
    serde_yaml::from_str(&format!("projects-dir: {path}\n")).expect("parse clyde.yml")
}

#[test]
fn the_flag_outranks_the_config_file() {
    let cfg = cfg_with_projects_dir("/from/config");
    let resolved = resolve(Some(Path::new("/from/flag")), &cfg).expect("resolve");
    assert_eq!(resolved, PathBuf::from("/from/flag"));
}

#[test]
fn the_config_file_outranks_the_platform_default() {
    let cfg = cfg_with_projects_dir("/from/config");
    let resolved = resolve(None, &cfg).expect("resolve");
    assert_eq!(
        resolved,
        PathBuf::from("/from/config"),
        "an absent flag must fall to config, NOT straight to ~/.claude/projects: that skip is \
         register item 8"
    );
}

#[test]
fn an_absent_config_key_reads_as_absent_not_as_the_default() {
    let cfg = Config::default();
    assert_eq!(
        cfg.configured_projects_dir(),
        None,
        "Config::projects_dir() folds in the platform default; the resolver needs the raw answer"
    );
}
