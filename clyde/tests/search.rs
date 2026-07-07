//! Integration tests for `clyde session search`'s piped JSON shape.
//!
//! Phase 2 (AND->OR fallback) changed the piped JSON output from a bare `Vec<SearchHit>` array to
//! the `SearchResults` object (`count`, `results`, `fallback`, `unenriched`) — a disclosed
//! breaking change for scripted consumers (design doc, Resolved Decisions). Driven through the
//! real `clyde` binary so the JSON asserted here is exactly what a piped consumer receives.

use std::fs;
use std::io::Write;
use std::path::Path;
use tempfile::TempDir;

const SID_KUBERNETES: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";
const SID_TERRAFORM: &str = "8b21c34d-1e22-4f5a-b91c-1234567890ab";

fn write_jsonl(path: &Path, lines: &[&str]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let mut f = fs::File::create(path).unwrap();
    for line in lines {
        writeln!(f, "{}", line).unwrap();
    }
}

/// Reindex a fresh catalog from `projects` into `db_path`, then run `session search` with the
/// given terms and return the parsed piped-JSON body.
fn run_search(db_path: &Path, projects: &Path, terms: &[&str]) -> serde_json::Value {
    let bin = env!("CARGO_BIN_EXE_clyde");

    let reindex = std::process::Command::new(bin)
        .arg("--db")
        .arg(db_path)
        .args(["session", "reindex", "--projects-dir"])
        .arg(projects)
        .output()
        .expect("clyde session reindex should run");
    assert!(reindex.status.success(), "reindex failed: {:?}", reindex);

    let mut args = vec!["--db".to_string(), db_path.to_string_lossy().into_owned()];
    args.extend(["session".to_string(), "search".to_string()]);
    args.extend(terms.iter().map(|t| t.to_string()));
    args.push("--no-reindex".to_string());

    let search = std::process::Command::new(bin)
        .args(&args)
        .output()
        .expect("clyde session search should run");
    assert!(search.status.success(), "search failed: {:?}", search);

    let stdout = String::from_utf8(search.stdout).expect("stdout is valid utf8");
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("stdout is not valid JSON: {e}\nstdout: {stdout:?}"))
}

fn seed_projects(projects: &Path) {
    write_jsonl(
        &projects
            .join("-home-saidler-repos-tatari-tv-marquee")
            .join(format!("{SID_KUBERNETES}.jsonl")),
        &[
            r#"{"type":"user","cwd":"/home/saidler/repos/tatari-tv/marquee","gitBranch":"main","timestamp":"2026-06-21T10:00:00Z","message":{"content":"look into the kubernetes networking issue"}}"#,
            r#"{"type":"ai-title","aiTitle":"Kubernetes networking debug","sessionId":"x"}"#,
        ],
    );
    write_jsonl(
        &projects
            .join("-home-saidler-repos-tatari-tv-loopr")
            .join(format!("{SID_TERRAFORM}.jsonl")),
        &[
            r#"{"type":"user","cwd":"/home/saidler/repos/tatari-tv/loopr","gitBranch":"main","timestamp":"2026-06-22T10:00:00Z","message":{"content":"migrate the terraform state bucket"}}"#,
            r#"{"type":"ai-title","aiTitle":"Terraform state migration","sessionId":"y"}"#,
        ],
    );
}

#[test]
fn piped_search_emits_search_results_object_not_a_bare_array() {
    let tmp = TempDir::new().expect("tempdir");
    let projects = tmp.path().join("projects");
    let db_path = tmp.path().join("sessions.db");
    seed_projects(&projects);

    // A normal AND query that a single session satisfies: the shape is the `SearchResults`
    // object (NOT a bare JSON array -- the disclosed breaking change), `fallback` is absent, and
    // `unenriched` carries the real gap counts (Phase 4). Neither seeded session was ever
    // enriched (reindex alone never sets `summary`), so the one returned hit is un-enriched
    // (`in-results: 1`) and so is the whole two-session catalog (`in-catalog: 2`).
    let v = run_search(&db_path, &projects, &["kubernetes"]);
    assert!(v.is_object(), "piped output must be an object, not a bare array: {v}");
    assert_eq!(v["count"], 1);
    assert_eq!(v["results"].as_array().unwrap().len(), 1);
    assert_eq!(v["results"][0]["record"]["session-id"], SID_KUBERNETES);
    assert!(
        v.get("fallback").is_none(),
        "an AND-satisfied query must carry no fallback key: {v}"
    );
    assert_eq!(v["unenriched"]["in-results"], 1, "the one hit is un-enriched: {v}");
    assert_eq!(
        v["unenriched"]["in-catalog"], 2,
        "both seeded sessions are un-enriched: {v}"
    );
}

#[test]
fn piped_search_flags_or_fallback_when_and_terms_never_co_occur() {
    let tmp = TempDir::new().expect("tempdir");
    let projects = tmp.path().join("projects");
    let db_path = tmp.path().join("sessions.db");
    seed_projects(&projects);

    // Neither seeded session mentions BOTH "kubernetes" and "terraform", so the strict AND pass
    // must return zero hits and the CLI must fall back to OR-joined matching.
    let v = run_search(&db_path, &projects, &["kubernetes", "terraform"]);
    assert_eq!(v["fallback"], "or", "OR fallback must be flagged: {v}");
    assert_eq!(v["count"], 2, "both sessions match on OR (one term each): {v}");
    let ids: Vec<&str> = v["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["record"]["session-id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&SID_KUBERNETES));
    assert!(ids.contains(&SID_TERRAFORM));
}
