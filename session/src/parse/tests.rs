#![allow(clippy::unwrap_used)]

use super::*;
use crate::model::SessionFileKind;
use std::fs;
use tempfile::TempDir;

const UUID_A: &str = "9d4c1f28-7a3b-4a9c-93b1-6e2a90d1f042";

fn write(path: &Path, lines: &[&str]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, lines.join("\n")).unwrap();
}

fn parent_file(path: PathBuf) -> SessionFile {
    SessionFile {
        path,
        group_id: UUID_A.to_string(),
        kind: SessionFileKind::Parent,
    }
}

#[test]
fn parses_title_prompt_model_and_counts() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("-home-saidler-repos-foo");
    let path = proj.join(format!("{UUID_A}.jsonl"));
    write(
        &path,
        &[
            r#"{"type":"user","cwd":"/home/saidler/repos/foo","gitBranch":"main","timestamp":"2026-06-21T10:00:00.000Z","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#,
            r#"{"type":"user","timestamp":"2026-06-21T10:00:05.000Z","message":{"role":"user","content":"set up the terraform marquee bucket"}}"#,
            r#"{"type":"ai-title","aiTitle":"Terraform Marquee bucket setup","sessionId":"x"}"#,
            r#"{"type":"assistant","timestamp":"2026-06-21T10:00:10.000Z","message":{"model":"claude-opus-4-8","content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"Creating the S3 bucket now"}]}}"#,
            r#"not even json"#,
            r#"{"type":"ai-title","aiTitle":"Terraform Marquee bucket setup","sessionId":"x"}"#,
        ],
    );

    let sessions = parse_sessions(&[parent_file(path)]);
    assert_eq!(sessions.len(), 1);
    let s = &sessions[0];
    assert_eq!(s.session_id, UUID_A);
    assert_eq!(s.ai_title.as_deref(), Some("Terraform Marquee bucket setup"));
    assert_eq!(s.first_prompt.as_deref(), Some("set up the terraform marquee bucket"));
    assert_eq!(s.title(), Some("Terraform Marquee bucket setup"));
    assert_eq!(s.git_branch.as_deref(), Some("main"));
    assert_eq!(s.cwd.as_deref(), Some(Path::new("/home/saidler/repos/foo")));
    assert_eq!(s.model.as_deref(), Some("claude-opus-4-8"));
    assert_eq!(s.n_msgs, 3, "two user lines (incl. the /clear wrapper) + one assistant");
    assert_eq!(s.project_dir, proj);
    // Body holds content text (for recall) but not the command-noise wrapper or thinking.
    assert!(s.body.contains("terraform marquee bucket"));
    assert!(s.body.contains("Creating the S3 bucket now"));
    assert!(!s.body.contains("/clear"));
    assert!(!s.body.contains("hmm"));
    // created = earliest timestamp.
    assert_eq!(s.created.unwrap().to_rfc3339(), "2026-06-21T10:00:00+00:00");
}

#[test]
fn title_falls_back_to_first_prompt_when_no_ai_title() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("proj").join(format!("{UUID_A}.jsonl"));
    write(
        &path,
        &[r#"{"type":"user","timestamp":"2026-06-21T10:00:00Z","message":{"content":"first real prompt here"}}"#],
    );
    let sessions = parse_sessions(&[parent_file(path)]);
    assert_eq!(sessions[0].ai_title, None);
    assert_eq!(sessions[0].title(), Some("first real prompt here"));
}

#[test]
fn rolls_up_subagent_messages_into_parent() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("proj");
    let parent = proj.join(format!("{UUID_A}.jsonl"));
    let sub = proj.join(UUID_A).join("subagents").join("agent-1.jsonl");
    write(
        &parent,
        &[
            r#"{"type":"assistant","timestamp":"2026-06-21T10:00:00Z","message":{"model":"claude-opus-4-8","content":[{"type":"text","text":"parent says hi"}]}}"#,
        ],
    );
    write(
        &sub,
        &[
            r#"{"type":"assistant","timestamp":"2026-06-21T10:01:00Z","message":{"model":"claude-haiku-4-5","content":[{"type":"text","text":"subagent grep result"}]}}"#,
        ],
    );

    let files = vec![
        parent_file(parent),
        SessionFile {
            path: sub,
            group_id: UUID_A.to_string(),
            kind: SessionFileKind::Subagent,
        },
    ];
    let sessions = parse_sessions(&files);
    assert_eq!(sessions.len(), 1, "subagent rolled into one parent record");
    let s = &sessions[0];
    assert_eq!(s.n_msgs, 2, "parent + subagent messages counted together");
    assert!(s.body.contains("parent says hi"));
    assert!(s.body.contains("subagent grep result"));
    assert_eq!(s.jsonl_paths.len(), 2);
}

#[test]
fn first_prompt_is_capped() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("proj").join(format!("{UUID_A}.jsonl"));
    let huge = "x".repeat(MAX_FIRST_PROMPT_CHARS + 500);
    let line = format!(r#"{{"type":"user","timestamp":"2026-06-21T10:00:00Z","message":{{"content":"{huge}"}}}}"#);
    write(&path, &[&line]);
    let sessions = parse_sessions(&[parent_file(path)]);
    assert_eq!(
        sessions[0].first_prompt.as_deref().map(str::len),
        Some(MAX_FIRST_PROMPT_CHARS)
    );
}

#[test]
fn command_opened_session_titles_from_skill_name() {
    // A /clear-then-skill session: no ai-title, no typed prompt, skill body injected. Title
    // should fall back to the invoked skill name, not the "Base directory for this skill:" dump.
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("proj").join(format!("{UUID_A}.jsonl"));
    write(
        &path,
        &[
            r#"{"type":"user","timestamp":"2026-06-21T10:00:00Z","message":{"content":"<local-command-caveat>Caveat: the messages below were generated while running local commands</local-command-caveat>"}}"#,
            r#"{"type":"user","timestamp":"2026-06-21T10:00:01Z","message":{"content":"<command-name>/clear</command-name>"}}"#,
            r#"{"type":"user","timestamp":"2026-06-21T10:00:02Z","message":{"content":"<command-name>/how-to-execute-a-plan</command-name>"}}"#,
            r#"{"type":"user","timestamp":"2026-06-21T10:00:03Z","message":{"content":"Base directory for this skill: /home/saidler/.claude/skills/how-to-execute-a-plan\n\n# How to Execute a Plan"}}"#,
            r#"{"type":"assistant","timestamp":"2026-06-21T10:00:04Z","message":{"model":"claude-opus-4-8","content":[{"type":"text","text":"starting phase 1"}]}}"#,
        ],
    );
    let s = &parse_sessions(&[parent_file(path)])[0];
    assert_eq!(s.ai_title, None);
    assert_eq!(s.first_prompt, None, "skill-body injection is not a prompt");
    assert_eq!(
        s.command_name.as_deref(),
        Some("how-to-execute-a-plan"),
        "/clear excluded, skill wins"
    );
    assert_eq!(s.title(), Some("how-to-execute-a-plan"));
    // Skill boilerplate stays out of the body; real assistant content stays in.
    assert!(!s.body.contains("Base directory for this skill"));
    assert!(s.body.contains("starting phase 1"));
}

#[test]
fn typed_prompt_after_clear_wins_over_command_name() {
    // The common "clear and keep working" case: a real typed prompt follows /clear.
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("proj").join(format!("{UUID_A}.jsonl"));
    write(
        &path,
        &[
            r#"{"type":"user","timestamp":"2026-06-21T10:00:00Z","message":{"content":"<command-name>/clear</command-name>"}}"#,
            r#"{"type":"user","timestamp":"2026-06-21T10:00:01Z","message":{"content":"now wire up the retry logic"}}"#,
        ],
    );
    let s = &parse_sessions(&[parent_file(path)])[0];
    assert_eq!(s.title(), Some("now wire up the retry logic"));
}

#[test]
fn local_command_stdout_is_noise() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("proj").join(format!("{UUID_A}.jsonl"));
    write(
        &path,
        &[
            r#"{"type":"user","timestamp":"2026-06-21T10:00:00Z","message":{"content":"<command-name>/model</command-name>"}}"#,
            r#"{"type":"user","timestamp":"2026-06-21T10:00:01Z","message":{"content":"<local-command-stdout>Set model to Fable 5</local-command-stdout>"}}"#,
        ],
    );
    let s = &parse_sessions(&[parent_file(path)])[0];
    assert_eq!(s.first_prompt, None, "local-command-stdout is not a prompt");
    assert_eq!(s.title(), Some("model"));
}

#[test]
fn extract_command_name_edges() {
    assert_eq!(
        extract_command_name("<command-name>/how-to-execute-a-plan</command-name>").as_deref(),
        Some("how-to-execute-a-plan")
    );
    assert_eq!(extract_command_name("  <command-name>/clear</command-name>  "), None);
    assert_eq!(extract_command_name("not a command"), None);
    assert_eq!(extract_command_name("<command-name></command-name>"), None);
}

#[test]
fn parse_messages_excludes_noise_and_labels_roles() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("proj");
    let parent = proj.join(format!("{UUID_A}.jsonl"));
    write(
        &parent,
        &[
            r#"{"type":"user","timestamp":"2026-06-21T10:00:00Z","message":{"content":"<command-name>/clear</command-name>"}}"#,
            r#"{"type":"user","timestamp":"2026-06-21T10:00:01Z","message":{"content":"set up the terraform marquee bucket"}}"#,
            r#"{"type":"assistant","timestamp":"2026-06-21T10:00:02Z","message":{"model":"claude-opus-4-8","content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"Creating the S3 bucket now"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-06-21T10:00:03Z","message":{"model":"claude-opus-4-8","content":[]}}"#,
        ],
    );
    let subagents_dir = proj.join(UUID_A).join("subagents");

    let messages = parse_messages(UUID_A, &parent, &subagents_dir);

    // The noise-wrapped `/clear` and the empty assistant turn are excluded; exactly the two
    // real turns remain, in file order, with correct roles and `subagent: false`.
    assert_eq!(
        messages.len(),
        2,
        "noise-wrapped user msg and empty assistant msg excluded"
    );
    assert_eq!(messages[0].role, Role::User);
    assert_eq!(messages[0].text, "set up the terraform marquee bucket");
    assert!(!messages[0].subagent);
    assert_eq!(messages[1].role, Role::Assistant);
    assert_eq!(messages[1].text, "Creating the S3 bucket now");
    assert!(!messages[1].subagent);
}

#[test]
fn parse_messages_includes_subagent_turns_flagged_and_ordered_after_parent() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("proj");
    let parent = proj.join(format!("{UUID_A}.jsonl"));
    let subagents_dir = proj.join(UUID_A).join("subagents");
    let sub_a = subagents_dir.join("agent-a.jsonl");
    let sub_b = subagents_dir.join("agent-b.jsonl");
    write(
        &parent,
        &[
            r#"{"type":"assistant","timestamp":"2026-06-21T10:00:00Z","message":{"model":"claude-opus-4-8","content":[{"type":"text","text":"parent says hi"}]}}"#,
        ],
    );
    // Written out of alphabetical order -- the served sequence must still sort subagents by path.
    write(
        &sub_b,
        &[r#"{"type":"user","timestamp":"2026-06-21T10:01:00Z","message":{"content":"agent b prompt"}}"#],
    );
    write(
        &sub_a,
        &[
            r#"{"type":"assistant","timestamp":"2026-06-21T10:00:30Z","message":{"model":"claude-haiku-4-5","content":[{"type":"text","text":"agent a result"}]}}"#,
        ],
    );

    let messages = parse_messages(UUID_A, &parent, &subagents_dir);

    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].text, "parent says hi");
    assert!(!messages[0].subagent, "parent transcript comes first");
    assert_eq!(
        messages[1].text, "agent a result",
        "subagents ordered by path (agent-a before agent-b)"
    );
    assert!(messages[1].subagent);
    assert_eq!(messages[2].text, "agent b prompt");
    assert!(messages[2].subagent);
}

#[test]
fn parse_messages_returns_empty_for_a_missing_transcript() {
    let tmp = TempDir::new().unwrap();
    let parent = tmp.path().join("proj").join(format!("{UUID_A}.jsonl"));
    let subagents_dir = tmp.path().join("proj").join(UUID_A).join("subagents");
    assert!(parse_messages(UUID_A, &parent, &subagents_dir).is_empty());
}

#[test]
fn helpers_handle_edges() {
    assert!(is_command_noise("   "));
    assert!(is_command_noise("<system-reminder>foo"));
    assert!(is_command_noise(
        "<task-notification><status>failed</status></task-notification>"
    ));
    assert!(is_command_noise("[Request interrupted by user]"));
    assert!(is_command_noise("[Request interrupted by user for tool use]"));
    assert!(!is_command_noise("real prompt"));
    assert_eq!(cap_chars("héllo", 2), "hé");
    assert_eq!(
        extract_text(Some(&serde_json::json!([
            {"type":"text","text":"a"},
            {"type":"tool_result","content":"ignored"},
            {"type":"text","text":"b"}
        ]))),
        "a\nb"
    );
}
