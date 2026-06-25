use serde::Deserialize;
use serde_json::Value;

/// The JSON payload piped to hook commands on stdin by Claude Code.
#[derive(Debug, Deserialize)]
pub struct HookPayload {
    pub tool_name: String,
    pub tool_input: Value,
    #[serde(default)]
    pub session_id: Option<String>,
    /// Capture any extra fields we don't know about yet.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// Extract a single normalized string from tool_input based on tool type.
///
/// For Bash -> command, Edit/Write/Read -> file_path, WebFetch -> url,
/// Glob/Grep -> pattern, MCP tools -> compact JSON of full input.
pub fn normalize_tool_input(tool_name: &str, tool_input: &Value) -> String {
    match tool_name {
        "Bash" => tool_input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "Edit" | "Write" | "Read" => tool_input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "WebFetch" => tool_input.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        "Glob" | "Grep" => tool_input
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "WebSearch" => tool_input
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        // MCP tools and anything else: compact JSON
        _ => serde_json::to_string(tool_input).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_bash_payload() {
        let json = r#"{
            "tool_name": "Bash",
            "tool_input": {"command": "git status", "description": "Show status"},
            "session_id": "abc123"
        }"#;
        let payload: HookPayload = serde_json::from_str(json).expect("parse");
        assert_eq!(payload.tool_name, "Bash");
        assert_eq!(payload.session_id.as_deref(), Some("abc123"));
    }

    #[test]
    fn parse_payload_without_session_id() {
        let json = r#"{"tool_name": "Read", "tool_input": {"file_path": "/tmp/foo.rs"}}"#;
        let payload: HookPayload = serde_json::from_str(json).expect("parse");
        assert_eq!(payload.tool_name, "Read");
        assert!(payload.session_id.is_none());
    }

    #[test]
    fn parse_payload_with_extra_fields() {
        let json = r#"{
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "some_future_field": 42
        }"#;
        let payload: HookPayload = serde_json::from_str(json).expect("parse");
        assert_eq!(
            payload.extra.get("some_future_field").and_then(|v| v.as_i64()),
            Some(42)
        );
    }

    #[test]
    fn normalize_bash_command() {
        let input = json!({"command": "git status --short", "description": "desc"});
        assert_eq!(normalize_tool_input("Bash", &input), "git status --short");
    }

    #[test]
    fn normalize_edit_file_path() {
        let input = json!({"file_path": "/home/user/foo.rs", "old_string": "a", "new_string": "b"});
        assert_eq!(normalize_tool_input("Edit", &input), "/home/user/foo.rs");
    }

    #[test]
    fn normalize_write_file_path() {
        let input = json!({"file_path": "/tmp/bar.rs", "content": "fn main() {}"});
        assert_eq!(normalize_tool_input("Write", &input), "/tmp/bar.rs");
    }

    #[test]
    fn normalize_read_file_path() {
        let input = json!({"file_path": "/tmp/baz.rs"});
        assert_eq!(normalize_tool_input("Read", &input), "/tmp/baz.rs");
    }

    #[test]
    fn normalize_webfetch_url() {
        let input = json!({"url": "https://docs.rs/clap"});
        assert_eq!(normalize_tool_input("WebFetch", &input), "https://docs.rs/clap");
    }

    #[test]
    fn normalize_glob_pattern() {
        let input = json!({"pattern": "**/*.rs", "path": "/home"});
        assert_eq!(normalize_tool_input("Glob", &input), "**/*.rs");
    }

    #[test]
    fn normalize_mcp_tool() {
        let input = json!({"account": "home", "owner": "foo"});
        let result = normalize_tool_input("mcp__multi-account-github__get_repo", &input);
        assert!(result.contains("account"));
        assert!(result.contains("home"));
    }

    #[test]
    fn normalize_missing_field_returns_empty() {
        let input = json!({"unexpected": "data"});
        assert_eq!(normalize_tool_input("Bash", &input), "");
    }
}
