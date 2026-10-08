//! Maps Claude Code `attachment` lines to hidden-context parts.
//!
//! Each mapped attachment type gives zero or more [`ContextPart`]s. A missing
//! or wrongly typed field gives no part (or a part from the fields that
//! exist); nothing here panics on odd input.

use serde_json::Value;
use trace_core::{ContextPart, ContextPartKind};

/// Key of the system prompt part.
pub(crate) const SYSTEM_KEY: &str = "system";
/// Key of the tool definitions part.
pub(crate) const TOOLS_KEY: &str = "tools";
/// Prefix of the keys of reminder parts. Compaction removes these.
pub(crate) const REMINDER_PREFIX: &str = "reminder:";

/// The parts one attachment gives, or `None` when the attachment type is not
/// one that carries hidden context. `line_id` is the line's uuid, or
/// `line-<n>` when it has none; it makes reminder keys unique.
pub(crate) fn parts_for_attachment(
    kind: &str,
    attachment: &Value,
    line_id: &str,
) -> Option<Vec<ContextPart>> {
    let reminder = |label: &str, text: Option<String>| -> Vec<ContextPart> {
        match text {
            Some(text) => vec![ContextPart {
                key: format!("{REMINDER_PREFIX}{line_id}"),
                kind: ContextPartKind::Reminder,
                label: label.to_string(),
                text,
            }],
            None => Vec::new(),
        }
    };
    let parts = match kind {
        "prompt_snapshot" => prompt_snapshot(attachment),
        "instructions" => instructions(attachment),
        "skill_listing" => reminder("skills list", string_at(attachment, "content")),
        "mcp_instructions_delta" => reminder("MCP server instructions", mcp_text(attachment)),
        "agent_listing_delta" => {
            reminder("agent list", joined_strings(attachment, "addedLines", "\n"))
        }
        "deferred_tools_delta" => reminder(
            "deferred tools list",
            joined_strings(attachment, "addedLines", "\n"),
        ),
        "hook_additional_context" => reminder("hook output", hook_text(attachment)),
        "environment" => reminder("environment", environment_text(attachment)),
        "date" => reminder("date", string_at(attachment, "date")),
        "model" => reminder("model", string_at(attachment, "text")),
        "session_context" => reminder(
            "session context",
            attachment.get("context").and_then(compact_json),
        ),
        _ => return None,
    };
    Some(parts)
}

fn prompt_snapshot(attachment: &Value) -> Vec<ContextPart> {
    let mut parts = Vec::new();
    let system = match attachment.get("systemPrompt") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(_)) => joined_strings(attachment, "systemPrompt", "\n\n"),
        _ => None,
    };
    if let Some(text) = system {
        parts.push(ContextPart {
            key: SYSTEM_KEY.to_string(),
            kind: ContextPartKind::SystemPrompt,
            label: "System prompt".to_string(),
            text,
        });
    }
    if let Some(tools) = attachment.get("tools").filter(|t| t.is_array()) {
        if let Some(text) = compact_json(tools) {
            parts.push(ContextPart {
                key: TOOLS_KEY.to_string(),
                kind: ContextPartKind::ToolDefinitions,
                label: "Tool definitions".to_string(),
                text,
            });
        }
    }
    parts
}

fn instructions(attachment: &Value) -> Vec<ContextPart> {
    let Some(files) = attachment.get("files").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut parts = Vec::new();
    for (index, file) in files.iter().enumerate() {
        let Some(text) = string_at(file, "content") else {
            continue;
        };
        let (key, label) = match string_at(file, "path") {
            Some(path) => (
                format!("instructions:{path}"),
                format!("{}: {path}", file_name(&path)),
            ),
            None => (
                format!("instructions:{index}"),
                string_at(file, "type").unwrap_or_else(|| "instructions".to_string()),
            ),
        };
        parts.push(ContextPart {
            key,
            kind: ContextPartKind::Instructions,
            label,
            text,
        });
    }
    parts
}

/// The last component of a path written with either separator.
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(path)
}

fn mcp_text(attachment: &Value) -> Option<String> {
    let added = joined_strings(attachment, "addedBlocks", "\n\n");
    let removed = joined_strings(attachment, "removedNames", ", ")
        .map(|names| format!("Removed MCP servers: {names}"));
    match (added, removed) {
        (Some(a), Some(r)) => Some(format!("{a}\n\n{r}")),
        (a, r) => a.or(r),
    }
}

fn hook_text(attachment: &Value) -> Option<String> {
    match attachment.get("content") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(_)) => joined_strings(attachment, "content", "\n\n"),
        _ => None,
    }
}

fn environment_text(attachment: &Value) -> Option<String> {
    let snapshot = attachment.get("snapshot");
    let changes = attachment.get("changes");
    match (snapshot, changes) {
        (Some(s), None) => compact_json(s),
        (None, None) => None,
        (s, c) => {
            let mut object = serde_json::Map::new();
            if let Some(s) = s {
                object.insert("snapshot".to_string(), s.clone());
            }
            if let Some(c) = c {
                object.insert("changes".to_string(), c.clone());
            }
            compact_json(&Value::Object(object))
        }
    }
}

fn string_at(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

/// The string items of an array field joined by `sep`. Non-string items are
/// dropped; `None` when the field is not an array or no item is a string.
fn joined_strings(value: &Value, key: &str, sep: &str) -> Option<String> {
    let items: Vec<&str> = value
        .get(key)?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    if items.is_empty() {
        None
    } else {
        Some(items.join(sep))
    }
}

fn compact_json(value: &Value) -> Option<String> {
    serde_json::to_string(value).ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn one(kind: &str, attachment: Value) -> ContextPart {
        let mut parts = parts_for_attachment(kind, &attachment, "u1").expect("mapped");
        assert_eq!(parts.len(), 1, "{parts:#?}");
        parts.remove(0)
    }

    fn check(part: &ContextPart, key: &str, kind: ContextPartKind, label: &str, text: &str) {
        assert_eq!(part.key, key);
        assert_eq!(part.kind, kind);
        assert_eq!(part.label, label);
        assert_eq!(part.text, text);
    }

    #[test]
    fn prompt_snapshot_without_tools_gives_only_the_system_prompt() {
        let parts = parts_for_attachment(
            "prompt_snapshot",
            &json!({"systemPrompt": ["Example one.", "Example two."]}),
            "u1",
        )
        .expect("mapped");
        assert_eq!(parts.len(), 1);
        check(
            &parts[0],
            "system",
            ContextPartKind::SystemPrompt,
            "System prompt",
            "Example one.\n\nExample two.",
        );
    }

    #[test]
    fn prompt_snapshot_with_tools_gives_a_tools_part() {
        let parts = parts_for_attachment(
            "prompt_snapshot",
            &json!({"systemPrompt": ["Example."], "tools": [{"name": "Example"}]}),
            "u1",
        )
        .expect("mapped");
        assert_eq!(parts.len(), 2);
        check(
            &parts[1],
            "tools",
            ContextPartKind::ToolDefinitions,
            "Tool definitions",
            r#"[{"name":"Example"}]"#,
        );
    }

    #[test]
    fn instructions_give_one_part_per_file() {
        let parts = parts_for_attachment(
            "instructions",
            &json!({"files": [
                {"path": "C:\\work\\example\\CLAUDE.md", "type": "Project", "content": "Example rules."},
                {"path": "C:\\work\\example\\notes.txt", "type": "User", "content": "Example notes."},
            ]}),
            "u1",
        )
        .expect("mapped");
        assert_eq!(parts.len(), 2);
        check(
            &parts[0],
            "instructions:C:\\work\\example\\CLAUDE.md",
            ContextPartKind::Instructions,
            "CLAUDE.md: C:\\work\\example\\CLAUDE.md",
            "Example rules.",
        );
        assert_eq!(parts[1].label, "notes.txt: C:\\work\\example\\notes.txt");
    }

    #[test]
    fn an_instructions_file_without_a_path_uses_its_index_and_type() {
        let parts = parts_for_attachment(
            "instructions",
            &json!({"files": [
                {"path": "/work/example/CLAUDE.md", "content": "A."},
                {"type": "AutoMem", "content": "Example memory."},
            ]}),
            "u1",
        )
        .expect("mapped");
        assert_eq!(parts[0].label, "CLAUDE.md: /work/example/CLAUDE.md");
        check(
            &parts[1],
            "instructions:1",
            ContextPartKind::Instructions,
            "AutoMem",
            "Example memory.",
        );
    }

    #[test]
    fn reminder_attachments_map_to_keys_labels_and_text() {
        let r = ContextPartKind::Reminder;
        check(
            &one("skill_listing", json!({"content": "Example skills."})),
            "reminder:u1",
            r.clone(),
            "skills list",
            "Example skills.",
        );
        check(
            &one(
                "mcp_instructions_delta",
                json!({"addedBlocks": ["A.", "B."], "removedNames": ["old", "older"]}),
            ),
            "reminder:u1",
            r.clone(),
            "MCP server instructions",
            "A.\n\nB.\n\nRemoved MCP servers: old, older",
        );
        check(
            &one("agent_listing_delta", json!({"addedLines": ["L1", "L2"]})),
            "reminder:u1",
            r.clone(),
            "agent list",
            "L1\nL2",
        );
        check(
            &one("deferred_tools_delta", json!({"addedLines": ["T1", "T2"]})),
            "reminder:u1",
            r.clone(),
            "deferred tools list",
            "T1\nT2",
        );
        check(
            &one("hook_additional_context", json!({"content": ["H1", "H2"]})),
            "reminder:u1",
            r.clone(),
            "hook output",
            "H1\n\nH2",
        );
        check(
            &one("environment", json!({"snapshot": {"shell": "example"}})),
            "reminder:u1",
            r.clone(),
            "environment",
            r#"{"shell":"example"}"#,
        );
        check(
            &one(
                "environment",
                json!({"snapshot": {"shell": "example"}, "changes": [{"field": "shell"}]}),
            ),
            "reminder:u1",
            r.clone(),
            "environment",
            r#"{"snapshot":{"shell":"example"},"changes":[{"field":"shell"}]}"#,
        );
        check(
            &one("date", json!({"date": "Example date."})),
            "reminder:u1",
            r.clone(),
            "date",
            "Example date.",
        );
        check(
            &one("model", json!({"text": "Example model."})),
            "reminder:u1",
            r.clone(),
            "model",
            "Example model.",
        );
        check(
            &one(
                "session_context",
                json!({"context": {"gitStatus": "clean"}}),
            ),
            "reminder:u1",
            r,
            "session context",
            r#"{"gitStatus":"clean"}"#,
        );
    }

    #[test]
    fn wrong_field_types_give_no_part() {
        for (kind, attachment) in [
            ("prompt_snapshot", json!({"systemPrompt": 5, "tools": "x"})),
            ("instructions", json!({"files": "nope"})),
            ("skill_listing", json!({"content": 7})),
            ("mcp_instructions_delta", json!({"addedBlocks": {"a": 1}})),
            ("agent_listing_delta", json!({"addedLines": [1, 2]})),
            ("hook_additional_context", json!({"content": null})),
            ("environment", json!({})),
            ("date", json!({"date": ["x"]})),
            ("model", json!(null)),
            ("session_context", json!({})),
        ] {
            let parts = parts_for_attachment(kind, &attachment, "u1").expect("mapped type");
            assert!(parts.is_empty(), "{kind}: {parts:#?}");
        }
    }

    #[test]
    fn unknown_attachment_types_are_not_mapped() {
        assert!(
            parts_for_attachment("total_tokens_reminder", &json!({"text": "x"}), "u1").is_none()
        );
        assert!(parts_for_attachment("deferred_tools_record", &json!({}), "u1").is_none());
    }
}
