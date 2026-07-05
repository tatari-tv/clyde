#![allow(clippy::unwrap_used)]

use std::sync::Mutex;

use super::*;
use crate::since::DateTz;

// Env-var mutation isn't safe under parallel tests; serialize the env-touching ones.
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn config_default_is_utc() {
    let cfg = Config::default();
    assert_eq!(cfg.date_tz(), DateTz::Utc);
}

#[test]
fn load_from_missing_file_yields_defaults() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    let cfg = load_from(&path).unwrap();
    assert_eq!(cfg, Config::default());
    assert_eq!(cfg.date_tz(), DateTz::Utc);
}

#[test]
fn load_from_empty_file_yields_defaults() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "").unwrap();
    // serde_yaml treats an empty document as null; with all fields defaulted this is still valid.
    let cfg = load_from(&path).unwrap();
    assert_eq!(cfg.date_tz(), DateTz::Utc);
}

#[test]
fn load_from_local() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "date-tz: local\n").unwrap();
    let cfg = load_from(&path).unwrap();
    assert_eq!(cfg.date_tz(), DateTz::Local);
}

#[test]
fn load_from_utc_explicit() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "date-tz: utc\n").unwrap();
    let cfg = load_from(&path).unwrap();
    assert_eq!(cfg.date_tz(), DateTz::Utc);
}

#[test]
fn render_format_defaults_to_markdown() {
    assert_eq!(Config::default().render_format(), FormatConfig::Markdown);
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "date-tz: utc\n").unwrap();
    assert_eq!(load_from(&path).unwrap().render_format(), FormatConfig::Markdown);
}

#[test]
fn load_from_reads_render_format() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "render:\n  format: marquee-html\n").unwrap();
    assert_eq!(load_from(&path).unwrap().render_format(), FormatConfig::MarqueeHtml);
}

#[test]
fn load_from_rejects_unknown_render_field() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "render:\n  bogus: 1\n").unwrap();
    assert!(
        load_from(&path).is_err(),
        "deny_unknown_fields should reject `render.bogus`"
    );
}

#[test]
fn load_from_rejects_bad_render_format() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "render:\n  format: docx\n").unwrap();
    assert!(load_from(&path).is_err(), "unknown format variant should fail to parse");
}

#[test]
fn load_from_rejects_unknown_field() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "date-tz: utc\nbogus: 1\n").unwrap();
    assert!(load_from(&path).is_err(), "deny_unknown_fields should reject `bogus`");
}

#[test]
fn load_from_rejects_bad_enum() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("clyde.yml");
    std::fs::write(&path, "date-tz: pacific\n").unwrap();
    assert!(load_from(&path).is_err(), "unknown enum variant should fail to parse");
}

#[test]
fn xdg_config_dir_honors_env_and_falls_back() {
    let guard = ENV_LOCK.lock().unwrap();
    let prior = std::env::var("XDG_CONFIG_HOME").ok();

    let dir = tempfile::TempDir::new().unwrap();
    unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
    assert_eq!(xdg_config_dir().as_deref(), Some(dir.path()));

    unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
    assert!(xdg_config_dir().unwrap().ends_with(".config"));

    match prior {
        Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
    }
    drop(guard);
}
