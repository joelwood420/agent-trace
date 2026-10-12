//! What Claude Code's injected reminders and file reads look like, as plain
//! data for the harness neutral `insights` crate.

use insights::{
    AlignRules, ContextRules, FileEditRule, FileReadRule, FlagRules, LabelRule, TargetRule,
};

/// The rules that let `insights` recognise Claude Code's system reminders
/// and its `Read` tool. Specific content rules (SessionStart hook, MCP
/// instructions, agent types, environment, skills, deferred tools) are
/// checked first so they match before broader filename rules (AGENTS.md,
/// MEMORY.md, CLAUDE.md).
pub fn context_rules() -> ContextRules {
    ContextRules {
        reminder_open: "<system-reminder>".into(),
        reminder_close: "</system-reminder>".into(),
        section_marker: Some("Contents of ".into()),
        labels: vec![
            LabelRule {
                needle: "SessionStart hook".into(),
                label: "hook output".into(),
                with_detail: false,
            },
            LabelRule {
                needle: "MCP Server Instructions".into(),
                label: "MCP server instructions".into(),
                with_detail: false,
            },
            LabelRule {
                needle: "agent types".into(),
                label: "agent list".into(),
                with_detail: false,
            },
            LabelRule {
                needle: "# Environment".into(),
                label: "environment".into(),
                with_detail: false,
            },
            LabelRule {
                needle: "skills are available".into(),
                label: "skills list".into(),
                with_detail: false,
            },
            LabelRule {
                needle: "deferred tools".into(),
                label: "deferred tools list".into(),
                with_detail: false,
            },
            LabelRule {
                needle: "AGENTS.md".into(),
                label: "AGENTS.md".into(),
                with_detail: true,
            },
            LabelRule {
                needle: "MEMORY.md".into(),
                label: "memory".into(),
                with_detail: true,
            },
            LabelRule {
                needle: "CLAUDE.md".into(),
                label: "CLAUDE.md".into(),
                with_detail: true,
            },
        ],
        file_reads: vec![FileReadRule {
            tool: "Read".into(),
            path_field: "file_path".into(),
        }],
    }
}

/// The rules the waste and problem flags use for Claude Code. `Read`, `Edit`,
/// `MultiEdit` (older transcripts), `Write` and `NotebookEdit` are Claude
/// Code's built-in tool names, and the field names are their input fields.
/// `pages` is the range a PDF read gives, so page chunks of one PDF are
/// different reads.
pub fn flag_rules() -> FlagRules {
    let edit = |tool: &str, fields: &[&str]| FileEditRule {
        tool: tool.into(),
        path_fields: fields.iter().map(|f| f.to_string()).collect(),
    };
    FlagRules {
        file_reads: vec![FileReadRule {
            tool: "Read".into(),
            path_field: "file_path".into(),
        }],
        range_fields: vec!["offset".into(), "limit".into(), "pages".into()],
        file_edits: vec![
            edit("Edit", &["file_path"]),
            edit("MultiEdit", &["file_path"]),
            edit("Write", &["file_path"]),
            edit("NotebookEdit", &["notebook_path", "file_path"]),
        ],
    }
}

/// Which input fields name each Claude Code tool's target, for lining up two
/// runs, and the prefix of background-agent notification prompts.
pub fn align_rules() -> AlignRules {
    let target = |tool: &str, fields: &[&str]| TargetRule {
        tool: tool.into(),
        fields: fields.iter().map(|f| f.to_string()).collect(),
    };
    AlignRules {
        targets: vec![
            target("Read", &["file_path"]),
            target("Edit", &["file_path"]),
            target("MultiEdit", &["file_path"]),
            target("Write", &["file_path"]),
            target("NotebookEdit", &["notebook_path"]),
            target("Bash", &["command"]),
            target("PowerShell", &["command"]),
            // `path` is left out: it is optional and defaults to the working
            // folder, so the same search with and without it is the same step.
            target("Grep", &["pattern", "glob"]),
            target("Glob", &["pattern"]),
            target("WebFetch", &["url"]),
            target("WebSearch", &["query"]),
            target("Agent", &["subagent_type", "description"]),
            target("Task", &["subagent_type", "description"]),
        ],
        fallback_fields: ["command", "file_path", "pattern", "query", "url", "path"]
            .iter()
            .map(|f| f.to_string())
            .collect(),
        notification_prefixes: vec!["<task-notification>".to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use insights::{SliceKind, measure_request};
    use serde_json::{Value, json};

    const CLAUDE_MD: &str = r"C:\work\example\CLAUDE.md";
    const MEMORY_MD: &str = r"C:\work\example\memory\MEMORY.md";
    const NOTES: &str = r"C:\work\example\notes.txt";

    #[test]
    fn reminders_and_reads_are_recognised() {
        let reminder = format!(
            "<system-reminder>\nContents of {CLAUDE_MD} (project instructions):\nBe brief.\nContents of {MEMORY_MD} (memory):\n- a note\n</system-reminder>"
        );
        let body = json!({
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": reminder }] },
                { "role": "assistant", "content": [{
                    "type": "tool_use", "id": "t1", "name": "Read",
                    "input": { "file_path": NOTES }
                }] },
                { "role": "user", "content": [{
                    "type": "tool_result", "tool_use_id": "t1", "content": "hello notes"
                }] },
            ]
        });
        let measure = measure_request(&body, &context_rules());
        let labels: Vec<&str> = measure
            .items(SliceKind::Instructions)
            .iter()
            .map(|i| i.label.as_str())
            .collect();
        let claude = format!("CLAUDE.md: {CLAUDE_MD}");
        let memory = format!("memory: {MEMORY_MD}");
        assert!(labels.contains(&claude.as_str()), "{labels:?}");
        assert!(labels.contains(&memory.as_str()), "{labels:?}");
        let reads = measure.items(SliceKind::FilesRead);
        assert!(reads.iter().any(|i| i.label == NOTES), "{reads:?}");
    }

    fn instruction_labels(text: &str) -> Vec<String> {
        let body = json!({
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": text }] }]
        });
        measure_request(&body, &context_rules())
            .items(SliceKind::Instructions)
            .iter()
            .map(|i| i.label.clone())
            .collect()
    }

    #[test]
    fn claude_md_mentioning_memory_keeps_its_name() {
        let text = format!(
            "<system-reminder>\nContents of {CLAUDE_MD} (project instructions):\nKeep MEMORY.md short.\n</system-reminder>"
        );
        assert_eq!(
            instruction_labels(&text),
            vec![format!("CLAUDE.md: {CLAUDE_MD}")]
        );
    }

    #[test]
    fn agents_md_mentioning_claude_md_is_agents_md() {
        let agents = r"C:\work\example\AGENTS.md";
        let text = format!(
            "<system-reminder>\nContents of {agents} (project instructions):\nSee CLAUDE.md as well.\n</system-reminder>"
        );
        assert_eq!(
            instruction_labels(&text),
            vec![format!("AGENTS.md: {agents}")]
        );
    }

    #[test]
    fn other_claude_code_reminders_are_named() {
        let cases = [
            ("SessionStart hook additional context: ok", "hook output"),
            (
                "# MCP Server Instructions\nUse the tools.",
                "MCP server instructions",
            ),
            (
                "Available agent types for the Agent tool:\n- a",
                "agent list",
            ),
            ("# Environment\nPlatform: test", "environment"),
        ];
        for (body, label) in cases {
            let text = format!("<system-reminder>\n{body}\n</system-reminder>");
            assert_eq!(instruction_labels(&text), vec![label.to_string()], "{body}");
        }
    }

    #[test]
    fn hook_mentioning_claude_md_is_labeled_as_hook() {
        let text = "<system-reminder>\nSessionStart hook additional context: remember to read CLAUDE.md first.\n</system-reminder>";
        assert_eq!(instruction_labels(text), vec!["hook output".to_string()]);
    }

    /// Parses one prompt whose model calls each make one tool call, and
    /// returns how many `RepeatedRead` flags the flag rules give.
    fn repeated_reads(calls: &[(&str, Value)]) -> usize {
        let mut parser = crate::Parser::for_session("flags.jsonl", "s1");
        let mut lines =
            vec![r#"{"type":"user","uuid":"u1","message":{"content":"Go."}}"#.to_string()];
        for (i, (tool, input)) in calls.iter().enumerate() {
            let line = json!({
                "type": "assistant",
                "uuid": format!("a{i}"),
                "message": { "id": format!("m{i}"), "content": [{
                    "type": "tool_use", "id": format!("t{i}"), "name": tool, "input": input
                }] }
            });
            lines.push(line.to_string());
        }
        let mut trace = trace_core::Trace::new();
        let mut events = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            events.extend(parser.push_line(i as u64 + 1, line));
        }
        events.extend(parser.finish());
        for event in events {
            trace.apply(event).expect("valid tree");
        }
        insights::session_flags(&trace, &flag_rules())
            .iter()
            .filter(|f| f.kind == insights::FlagKind::RepeatedRead)
            .count()
    }

    #[test]
    fn page_chunks_of_one_pdf_are_not_a_repeated_read() {
        let read = |pages: &str| {
            (
                "Read",
                json!({ "file_path": r"C:\x\doc.pdf", "pages": pages }),
            )
        };
        assert_eq!(
            repeated_reads(&[read("1-20"), read("21-40"), read("41-60")]),
            0
        );
        assert_eq!(
            repeated_reads(&[read("1-20"), read("1-20"), read("1-20")]),
            1
        );
    }

    #[test]
    fn multi_edit_between_reads_resets_the_count() {
        let read = ("Read", json!({ "file_path": NOTES }));
        let edit = ("MultiEdit", json!({ "file_path": NOTES, "edits": [] }));
        assert_eq!(repeated_reads(&[read.clone(), edit, read.clone(), read]), 0);
    }

    #[test]
    fn align_rules_name_each_tools_target() {
        let rules = align_rules();
        let fields = |tool: &str| {
            rules
                .targets
                .iter()
                .find(|r| r.tool == tool)
                .map(|r| r.fields.clone())
                .unwrap_or_default()
        };
        for tool in ["Read", "Edit", "MultiEdit", "Write"] {
            assert_eq!(fields(tool), vec!["file_path".to_string()], "{tool}");
        }
        assert_eq!(fields("NotebookEdit"), vec!["notebook_path".to_string()]);
        for tool in ["Bash", "PowerShell"] {
            assert_eq!(fields(tool), vec!["command".to_string()], "{tool}");
        }
        assert_eq!(fields("Grep"), vec!["pattern".to_string(), "glob".into()]);
        assert_eq!(fields("Glob"), vec!["pattern".to_string()]);
        assert_eq!(fields("WebFetch"), vec!["url".to_string()]);
        assert_eq!(fields("WebSearch"), vec!["query".to_string()]);
        for tool in ["Agent", "Task"] {
            assert_eq!(
                fields(tool),
                vec!["subagent_type".to_string(), "description".into()],
                "{tool}"
            );
        }
        assert_eq!(
            rules.fallback_fields,
            vec!["command", "file_path", "pattern", "query", "url", "path"]
        );
        assert_eq!(rules.notification_prefixes, vec!["<task-notification>"]);
    }
}
