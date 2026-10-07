//! Measures an Anthropic Messages API request body.

use std::collections::HashMap;

use serde_json::Value;

use crate::measure::{
    ContextMeasure, ContextSource, IMAGE_CHARS, SliceKind, json_chars, text_chars,
};
use crate::rules::ContextRules;

/// Prefix of tool names provided by an MCP server: `mcp__<server>__<tool>`.
/// This is an API naming convention shared by every MCP client.
const MCP_PREFIX: &str = "mcp__";

/// What the model called a tool, and the file path in its input if any.
type ToolUses = HashMap<String, (String, Option<String>)>;

/// Measures the `system`, `tools` and `messages` of a request body in
/// characters. Other keys are ignored; a body that is not an object gives an
/// empty measure.
pub fn measure_request(body: &Value, rules: &ContextRules) -> ContextMeasure {
    let mut out = ContextMeasure::new(ContextSource::Captured);
    let Some(obj) = body.as_object() else {
        return out;
    };
    if let Some(system) = obj.get("system") {
        measure_system(system, &mut out);
    }
    if let Some(tools) = obj.get("tools").and_then(Value::as_array) {
        measure_tools(tools, &mut out);
    }
    if let Some(messages) = obj.get("messages").and_then(Value::as_array) {
        measure_messages(messages, rules, &mut out);
    }
    out
}

fn str_field<'a>(block: &'a Value, key: &str) -> &'a str {
    block.get(key).and_then(Value::as_str).unwrap_or("")
}

fn measure_system(system: &Value, out: &mut ContextMeasure) {
    match system {
        Value::String(s) => out.add(SliceKind::SystemPrompt, "block 1", text_chars(s)),
        Value::Array(blocks) => {
            for (i, block) in blocks.iter().enumerate() {
                let chars = match block.get("text").and_then(Value::as_str) {
                    Some(text) => text_chars(text),
                    None => json_chars(block),
                };
                let label = format!("block {}", i + 1);
                out.add(SliceKind::SystemPrompt, &label, chars);
            }
        }
        _ => {}
    }
}

fn measure_tools(tools: &[Value], out: &mut ContextMeasure) {
    for tool in tools {
        let name = str_field(tool, "name");
        let label = match name.strip_prefix(MCP_PREFIX) {
            Some(rest) => {
                let server = rest.find("__").map_or(rest, |i| &rest[..i]);
                format!("MCP: {server}")
            }
            None => "built-in".to_string(),
        };
        out.add_many(SliceKind::ToolDefinitions, &label, json_chars(tool), 1);
    }
}

fn measure_messages(messages: &[Value], rules: &ContextRules, out: &mut ContextMeasure) {
    let mut uses = ToolUses::new();
    for message in messages {
        let assistant = str_field(message, "role") == "assistant";
        let mut one = |block: &Value, out: &mut ContextMeasure| {
            if assistant {
                measure_assistant_block(block, rules, &mut uses, out);
            } else {
                measure_user_block(block, rules, &uses, out);
            }
        };
        match message.get("content") {
            Some(Value::String(text)) => one(&text_block(text), out),
            Some(Value::Array(blocks)) => blocks.iter().for_each(|b| one(b, out)),
            _ => {}
        }
    }
}

fn text_block(text: &str) -> Value {
    serde_json::json!({ "type": "text", "text": text })
}

fn measure_other(block: &Value, kind: &str, out: &mut ContextMeasure) {
    tracing::debug!(kind, "unknown content block measured as JSON");
    out.add(SliceKind::Conversation, "other", json_chars(block));
}

fn measure_assistant_block(
    block: &Value,
    rules: &ContextRules,
    uses: &mut ToolUses,
    out: &mut ContextMeasure,
) {
    let reply = |out: &mut ContextMeasure, text: &str| {
        out.add(SliceKind::Conversation, "model replies", text_chars(text))
    };
    match str_field(block, "type") {
        "text" => reply(out, str_field(block, "text")),
        "thinking" => reply(out, str_field(block, "thinking")),
        "redacted_thinking" => reply(out, str_field(block, "data")),
        "tool_use" => {
            let input = block.get("input");
            out.add(
                SliceKind::Conversation,
                "tool inputs",
                input.map_or(0, json_chars),
            );
            let name = str_field(block, "name");
            let path = rules
                .file_reads
                .iter()
                .find(|r| r.tool == name)
                .and_then(|r| input?.get(&r.path_field)?.as_str())
                .map(str::to_string);
            uses.insert(str_field(block, "id").to_string(), (name.to_string(), path));
        }
        other => measure_other(block, other, out),
    }
}

fn measure_user_block(
    block: &Value,
    rules: &ContextRules,
    uses: &ToolUses,
    out: &mut ContextMeasure,
) {
    match str_field(block, "type") {
        "text" => {
            let outside = split_reminders(str_field(block, "text"), rules, out);
            if outside > 0 {
                out.add(SliceKind::Conversation, "your prompts", outside);
            }
        }
        "image" => out.add(SliceKind::Conversation, "images", IMAGE_CHARS),
        "tool_result" => measure_tool_result(block, rules, uses, out),
        other => measure_other(block, other, out),
    }
}

fn measure_tool_result(
    block: &Value,
    rules: &ContextRules,
    uses: &ToolUses,
    out: &mut ContextMeasure,
) {
    let (kind, label) = match uses.get(str_field(block, "tool_use_id")) {
        Some((_, Some(path))) => (SliceKind::FilesRead, path.as_str()),
        Some((name, None)) => (SliceKind::ToolResults, name.as_str()),
        None => (SliceKind::ToolResults, "unknown tool"),
    };
    let mut chars = 0;
    match block.get("content") {
        Some(Value::String(text)) => chars += split_reminders(text, rules, out),
        Some(Value::Array(parts)) => {
            for part in parts {
                chars += match str_field(part, "type") {
                    "text" => split_reminders(str_field(part, "text"), rules, out),
                    "image" => IMAGE_CHARS,
                    other => {
                        tracing::debug!(kind = other, "unknown content block measured as JSON");
                        json_chars(part)
                    }
                };
            }
        }
        _ => {}
    }
    out.add(kind, label, chars);
}

/// Adds each reminder section of `text` to `Instructions` and returns the
/// number of characters outside any reminder, for the caller to add to its
/// own slice.
pub(crate) fn split_reminders(text: &str, rules: &ContextRules, out: &mut ContextMeasure) -> u64 {
    let open = rules.reminder_open.as_str();
    if open.is_empty() {
        return text_chars(text);
    }
    let close = rules.reminder_close.as_str();
    let mut outside = 0;
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        outside += text_chars(&rest[..start]);
        let after = &rest[start + open.len()..];
        let end = if close.is_empty() {
            None
        } else {
            after.find(close)
        };
        match end {
            Some(end) => {
                add_sections(&after[..end], rules, out);
                rest = &after[end + close.len()..];
            }
            None => {
                add_sections(after, rules, out);
                rest = "";
            }
        }
    }
    outside + text_chars(rest)
}

fn add_sections(body: &str, rules: &ContextRules, out: &mut ContextMeasure) {
    let marker = rules.section_marker.as_deref().filter(|m| !m.is_empty());
    let Some(marker) = marker else {
        if !body.is_empty() {
            add_section(body, None, rules, out);
        }
        return;
    };
    let first = body.find(marker).unwrap_or(body.len());
    if !body[..first].trim().is_empty() {
        add_section(&body[..first], None, rules, out);
    }
    let mut rest = &body[first..];
    while !rest.is_empty() {
        let next = rest[marker.len()..]
            .find(marker)
            .map_or(rest.len(), |i| i + marker.len());
        add_section(&rest[..next], Some(marker), rules, out);
        rest = &rest[next..];
    }
}

fn add_section(
    section: &str,
    marker: Option<&str>,
    rules: &ContextRules,
    out: &mut ContextMeasure,
) {
    let label = match rules.labels.iter().find(|r| section.contains(&r.needle)) {
        None => "other reminders".to_string(),
        Some(rule) => match marker {
            Some(m) if rule.with_detail => {
                let after = &section[m.len()..];
                let stop = [after.find(" ("), after.find('\n')]
                    .into_iter()
                    .flatten()
                    .min()
                    .unwrap_or(after.len());
                format!("{}: {}", rule.label, after[..stop].trim())
            }
            _ => rule.label.clone(),
        },
    };
    out.add(SliceKind::Instructions, &label, text_chars(section));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{FileReadRule, LabelRule};
    use serde_json::json;

    fn rules() -> ContextRules {
        ContextRules {
            reminder_open: "<r>".into(),
            reminder_close: "</r>".into(),
            section_marker: Some("Contents of ".into()),
            labels: vec![
                LabelRule {
                    needle: "NOTES.md".into(),
                    label: "memory".into(),
                    with_detail: true,
                },
                LabelRule {
                    needle: "GUIDE.md".into(),
                    label: "guide".into(),
                    with_detail: true,
                },
                LabelRule {
                    needle: "skills".into(),
                    label: "skills list".into(),
                    with_detail: false,
                },
            ],
            file_reads: vec![FileReadRule {
                tool: "ReadFile".into(),
                path_field: "path".into(),
            }],
        }
    }

    /// (chars, count, largest) of an item, or (u64::MAX, 0, 0) if missing.
    fn item(m: &ContextMeasure, kind: SliceKind, label: &str) -> (u64, u32, u64) {
        m.items(kind)
            .iter()
            .find(|i| i.label == label)
            .map(|i| (i.chars, i.count, i.largest_chars))
            .unwrap_or((u64::MAX, 0, 0))
    }

    fn user(text: &str) -> Value {
        json!({"messages": [{"role": "user", "content": [{"type": "text", "text": text}]}]})
    }

    #[test]
    fn system_string_and_blocks() {
        let m = measure_request(&json!({"system": "hello"}), &rules());
        assert_eq!(m.items(SliceKind::SystemPrompt).len(), 1);
        assert_eq!(item(&m, SliceKind::SystemPrompt, "block 1"), (5, 1, 5));
        let body =
            json!({"system": [{"type": "text", "text": "abc"}, {"type": "text", "text": "de"}]});
        let m = measure_request(&body, &rules());
        assert_eq!(item(&m, SliceKind::SystemPrompt, "block 1").0, 3);
        assert_eq!(item(&m, SliceKind::SystemPrompt, "block 2").0, 2);
    }

    #[test]
    fn tools_group_by_mcp_server() {
        let tools = vec![
            json!({"name": "Bash"}),
            json!({"name": "mcp__docs__search"}),
            json!({"name": "mcp__docs__fetch"}),
            json!({"name": "mcp__odd"}),
        ];
        let lens: Vec<u64> = tools.iter().map(json_chars).collect();
        let m = measure_request(&json!({ "tools": tools }), &rules());
        let k = SliceKind::ToolDefinitions;
        assert_eq!(item(&m, k, "built-in"), (lens[0], 1, lens[0]));
        assert_eq!(
            item(&m, k, "MCP: docs"),
            (lens[1] + lens[2], 2, lens[1].max(lens[2]))
        );
        assert_eq!(item(&m, k, "MCP: odd"), (lens[3], 1, lens[3]));
    }

    #[test]
    fn assistant_blocks() {
        let body = json!({"messages": [{"role": "assistant", "content": [
            {"type": "text", "text": "abcd"},
            {"type": "thinking", "thinking": "xyz", "signature": "s"},
            {"type": "redacted_thinking", "data": "12"},
            {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"a": 1}},
        ]}]});
        let m = measure_request(&body, &rules());
        let c = SliceKind::Conversation;
        assert_eq!(item(&m, c, "model replies"), (9, 3, 4));
        assert_eq!(item(&m, c, "tool inputs"), (7, 1, 7));
    }

    #[test]
    fn reminders_split_into_sections() {
        let text = "hi <r>intro Contents of /p/GUIDE.md (project):\nbe kind Contents of /p/NOTES.md (memory):\nx</r> bye";
        let m = measure_request(&user(text), &rules());
        let i = SliceKind::Instructions;
        assert_eq!(m.items(i).len(), 3);
        assert_eq!(item(&m, i, "other reminders").0, 6);
        assert_eq!(
            item(&m, i, "guide: /p/GUIDE.md").0,
            text_chars("Contents of /p/GUIDE.md (project):\nbe kind ")
        );
        assert_eq!(
            item(&m, i, "memory: /p/NOTES.md").0,
            text_chars("Contents of /p/NOTES.md (memory):\nx")
        );
        assert_eq!(
            item(&m, SliceKind::Conversation, "your prompts").0,
            text_chars("hi ") + text_chars(" bye")
        );
    }

    #[test]
    fn unclosed_and_stray_tags() {
        let m = measure_request(&user("a </r> b <r>rest"), &rules());
        assert_eq!(
            item(&m, SliceKind::Conversation, "your prompts").0,
            text_chars("a </r> b ")
        );
        assert_eq!(m.items(SliceKind::Instructions).len(), 1);
        assert_eq!(item(&m, SliceKind::Instructions, "other reminders").0, 4);
    }

    #[test]
    fn file_reads_match_by_tool_use_id() {
        let body = json!({"messages": [
            {"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "ReadFile", "input": {"path": "src/a.rs"}},
                {"type": "tool_use", "id": "t2", "name": "ReadFile", "input": {"path": "src/a.rs"}},
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "12345"},
                {"type": "tool_result", "tool_use_id": "t2", "content": [
                    {"type": "text", "text": "abc"}, {"type": "text", "text": "defgh"}]},
            ]},
        ]});
        let m = measure_request(&body, &rules());
        assert_eq!(item(&m, SliceKind::FilesRead, "src/a.rs"), (13, 2, 8));
        assert!(m.items(SliceKind::ToolResults).is_empty());
    }

    #[test]
    fn other_results_unknown_ids_and_missing_content() {
        let body = json!({"messages": [
            {"role": "assistant", "content": [
                {"type": "tool_use", "id": "g1", "name": "Grep", "input": {}},
                {"type": "tool_use", "id": "g2", "name": "Grep", "input": {}},
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "g1", "content": "abcdef"},
                {"type": "tool_result", "tool_use_id": "zzz", "content": "xy"},
                {"type": "tool_result", "tool_use_id": "g2"},
            ]},
        ]});
        let m = measure_request(&body, &rules());
        let k = SliceKind::ToolResults;
        assert_eq!(item(&m, k, "Grep"), (6, 2, 6));
        assert_eq!(item(&m, k, "unknown tool"), (2, 1, 2));
    }

    #[test]
    fn reminder_inside_tool_result_counts_as_instructions() {
        let body = json!({"messages": [
            {"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "ReadFile", "input": {"path": "f.rs"}}]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "line1<r>skills: a, b</r>"}]},
        ]});
        let m = measure_request(&body, &rules());
        assert_eq!(item(&m, SliceKind::FilesRead, "f.rs"), (5, 1, 5));
        assert_eq!(
            item(&m, SliceKind::Instructions, "skills list").0,
            text_chars("skills: a, b")
        );
    }

    #[test]
    fn images_and_unknown_blocks() {
        let mystery = json!({"type": "mystery", "x": 1});
        let body = json!({"messages": [{"role": "user", "content": [
            {"type": "image", "source": {}}, mystery]}]});
        let m = measure_request(&body, &rules());
        let c = SliceKind::Conversation;
        assert_eq!(item(&m, c, "images").0, IMAGE_CHARS);
        assert_eq!(item(&m, c, "other").0, json_chars(&mystery));
    }

    #[test]
    fn not_an_object_is_empty() {
        assert_eq!(measure_request(&json!([1, 2]), &rules()).total_chars(), 0);
    }
}
