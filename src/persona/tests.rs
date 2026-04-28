#![allow(clippy::unwrap_used)]

use super::*;
use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::time::Instant;
use tempfile::TempDir;

fn write_executable(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut f = fs::File::create(&path).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    f.flush().unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

#[test]
fn map_raw_combines_supervisor_fields_and_renames_keys() {
    let raw = RawPersona {
        preferred_full_name: Some("Scott Idler".into()),
        business_title: Some("Director, Engineering".into()),
        team_org: Some("Platform".into()),
        organization_org: Some("Engineering".into()),
        department_org: Some("Platform".into()),
        supervisor_name: Some("Mark Weiler".into()),
        supervisor_email: Some("mark.weiler@tatari.tv".into()),
        work_email: Some("scott.idler@tatari.tv".into()),
        github_username: Some("escote-tatari".into()),
        primary_home_address_state: Some("Oregon".into()),
    };
    let block = map_raw(raw);
    assert_eq!(block.name.as_deref(), Some("Scott Idler"));
    assert_eq!(block.title.as_deref(), Some("Director, Engineering"));
    assert_eq!(block.team.as_deref(), Some("Platform"));
    assert_eq!(block.organization.as_deref(), Some("Engineering"));
    assert_eq!(block.manager.as_deref(), Some("Mark Weiler (mark.weiler@tatari.tv)"));
    assert_eq!(block.email.as_deref(), Some("scott.idler@tatari.tv"));
    assert_eq!(block.github.as_deref(), Some("escote-tatari"));
    assert_eq!(block.location.as_deref(), Some("Oregon"));
}

#[test]
fn map_raw_handles_missing_fields() {
    let raw = RawPersona {
        preferred_full_name: None,
        business_title: None,
        team_org: None,
        organization_org: None,
        department_org: None,
        supervisor_name: None,
        supervisor_email: None,
        work_email: Some("scott.idler@tatari.tv".into()),
        github_username: None,
        primary_home_address_state: None,
    };
    let block = map_raw(raw);
    assert_eq!(block.name, None);
    assert_eq!(block.manager, None);
    assert_eq!(block.email.as_deref(), Some("scott.idler@tatari.tv"));

    let yaml = serde_yaml::to_string(&block).unwrap();
    assert!(!yaml.contains("name:"));
    assert!(!yaml.contains("manager:"));
    assert!(yaml.contains("email: scott.idler@tatari.tv"));
}

#[test]
fn whoami_via_returns_none_when_binary_missing() {
    let result = whoami_via("/nonexistent/persona-binary-xyz");
    assert_eq!(result, None);
}

#[test]
fn whoami_via_parses_happy_path_json() {
    let tmp = TempDir::new().unwrap();
    let body = r#"#!/bin/sh
cat <<'EOF'
{
  "preferred_full_name": "Test User",
  "business_title": "SWE",
  "team_org": "Platform",
  "organization_org": "Engineering",
  "department_org": "Platform",
  "supervisor_name": "Boss",
  "supervisor_email": "boss@example.com",
  "work_email": "test@example.com",
  "github_username": "testuser",
  "primary_home_address_state": "Oregon"
}
EOF
"#;
    let path = write_executable(tmp.path(), "persona", body);
    let block = whoami_via(path.to_str().unwrap()).expect("happy path must succeed");
    assert_eq!(block.name.as_deref(), Some("Test User"));
    assert_eq!(block.email.as_deref(), Some("test@example.com"));
    assert_eq!(block.manager.as_deref(), Some("Boss (boss@example.com)"));
}

#[test]
fn whoami_via_returns_none_on_timeout() {
    let tmp = TempDir::new().unwrap();
    let body = "#!/bin/sh\nsleep 30\n";
    let path = write_executable(tmp.path(), "persona", body);
    let start = Instant::now();
    let result = whoami_via(path.to_str().unwrap());
    let elapsed = start.elapsed();
    assert_eq!(result, None);
    assert!(
        elapsed < Duration::from_secs(10),
        "timeout must fire well under 10s; took {:?}",
        elapsed
    );
}

#[test]
fn whoami_via_returns_none_on_invalid_json() {
    let tmp = TempDir::new().unwrap();
    let body = "#!/bin/sh\necho 'not json'\n";
    let path = write_executable(tmp.path(), "persona", body);
    let result = whoami_via(path.to_str().unwrap());
    assert_eq!(result, None);
}
