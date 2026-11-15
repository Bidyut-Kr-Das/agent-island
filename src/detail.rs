use serde_json::Value;

const MAX: usize = 60;

pub fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_owned();
    }
    s.chars().take(max_chars - 3).chain("...".chars()).collect()
}

// (verb phrase, untruncated detail)
pub fn full_line(tool: &str, input: &Value, mcp_server: Option<&str>) -> (String, String) {
    let field = |k: &str| input.get(k).and_then(Value::as_str);
    let (verb, detail) = match tool {
        "Bash" | "PowerShell" => ("Wants to run a command".to_owned(), field("command")),
        "Edit" | "Write" | "MultiEdit" => ("Wants to edit a file".to_owned(), field("file_path")),
        "NotebookEdit" => ("Wants to edit a file".to_owned(), field("notebook_path")),
        "WebFetch" => ("Wants to fetch a URL".to_owned(), field("url")),
        _ if tool.starts_with("mcp__") => {
            let mut parts = tool["mcp__".len()..].splitn(2, "__");
            let (server, name) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
            let server = mcp_server.unwrap_or(server);
            return ("Wants to use a tool".to_owned(), format!("{server} / {name}"));
        }
        _ => {
            let first = input
                .as_object()
                .and_then(|o| o.values().filter_map(Value::as_str).find(|s| s.chars().count() <= MAX));
            (format!("Wants to use {tool}"), first)
        }
    };
    (verb, detail.unwrap_or(tool).to_owned())
}

pub fn detail_line(tool: &str, input: &Value, mcp_server: Option<&str>) -> (String, String) {
    let (verb, full) = full_line(tool, input, mcp_server);
    let detail = match full.split_once('\n') {
        Some((first, _)) => truncate(&format!("{} ...", first.trim_end()), MAX),
        None => truncate(&full, MAX),
    };
    (verb, detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn d(tool: &str, input: Value) -> (String, String) {
        detail_line(tool, &input, None)
    }

    fn pair(a: &str, b: &str) -> (String, String) {
        (a.to_owned(), b.to_owned())
    }

    #[test]
    fn bash_shows_command() {
        assert_eq!(
            d("Bash", json!({"command": "npm install zod@3.23.8"})),
            pair("Wants to run a command", "npm install zod@3.23.8")
        );
    }

    #[test]
    fn powershell_shows_command() {
        assert_eq!(
            d("PowerShell", json!({"command": "mkdir probe1", "description": "x"})),
            pair("Wants to run a command", "mkdir probe1")
        );
    }

    #[test]
    fn edit_shows_path() {
        for (t, k) in [("Edit", "file_path"), ("Write", "file_path"), ("MultiEdit", "file_path"), ("NotebookEdit", "notebook_path")] {
            assert_eq!(d(t, json!({ k: "src/a.rs" })), pair("Wants to edit a file", "src/a.rs"), "{t}");
        }
    }

    #[test]
    fn webfetch_shows_url() {
        assert_eq!(
            d("WebFetch", json!({"url": "https://example.com"})),
            pair("Wants to fetch a URL", "https://example.com")
        );
    }

    #[test]
    fn mcp_shows_server_and_tool() {
        assert_eq!(
            d("mcp__github__create_issue", json!({})),
            pair("Wants to use a tool", "github / create_issue")
        );
        assert_eq!(
            detail_line("mcp__github__create_issue", &json!({}), Some("gh")),
            pair("Wants to use a tool", "gh / create_issue")
        );
    }

    #[test]
    fn other_tool_uses_first_short_string_field() {
        let long = "x".repeat(80);
        assert_eq!(
            d("Grep", json!({"a_long": long, "pattern": "foo", "n": 3})),
            pair("Wants to use Grep", "foo")
        );
    }

    #[test]
    fn other_tool_without_strings_shows_tool_name() {
        assert_eq!(d("Grep", json!({"n": 3})), pair("Wants to use Grep", "Grep"));
    }

    #[test]
    fn multiline_command_uses_first_line_with_ellipsis() {
        let cmd = "cat <<EOF\nabc\nEOF";
        assert_eq!(d("Bash", json!({ "command": cmd })).1, "cat <<EOF ...");
        assert_eq!(full_line("Bash", &json!({ "command": cmd }), None).1, cmd);
    }

    #[test]
    fn truncate_counts_chars_not_bytes() {
        let s = "é".repeat(100);
        let t = truncate(&s, 60);
        assert_eq!(t.chars().count(), 60);
        assert!(t.ends_with("..."));
        assert_eq!(truncate("short", 60), "short");
    }
}
