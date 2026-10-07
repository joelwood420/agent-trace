//! What Claude Code's injected reminders and file reads look like, as plain
//! data for the harness neutral `insights` crate.

use insights::{ContextRules, FileReadRule, LabelRule};

/// The rules that let `insights` recognise Claude Code's system reminders
/// (CLAUDE.md, memory, skill and tool lists) and its `Read` tool.
pub fn context_rules() -> ContextRules {
    ContextRules {
        reminder_open: "<system-reminder>".into(),
        reminder_close: "</system-reminder>".into(),
        section_marker: Some("Contents of ".into()),
        labels: vec![
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
        ],
        file_reads: vec![FileReadRule {
            tool: "Read".into(),
            path_field: "file_path".into(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use insights::{SliceKind, measure_request};
    use serde_json::json;

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
}
