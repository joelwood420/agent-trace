//! Prints a Claude Code session as an indented trace tree.
//!
//! ```powershell
//! cargo run -p adapter-claude-code --example print_tree -- <session.jsonl>
//! cargo run -p adapter-claude-code --example print_tree -- --stats <session.jsonl>...
//! ```
//!
//! `--stats` prints only node counts and skipped-line reasons, never content.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use adapter_claude_code::load_session;
use serde_json::Value;
use trace_core::{ContentBlock, Node, Trace, TraceEvent};

fn main() -> ExitCode {
    let mut stats = false;
    let mut paths = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--stats" => stats = true,
            "-h" | "--help" => {
                println!("usage: print_tree [--stats] <session.jsonl>...");
                return ExitCode::SUCCESS;
            }
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    if paths.is_empty() {
        eprintln!("usage: print_tree [--stats] <session.jsonl>...");
        return ExitCode::FAILURE;
    }

    let mut failed = false;
    for path in &paths {
        let session = match load_session(path) {
            Ok(s) => s,
            Err(err) => {
                eprintln!("error: {err}");
                failed = true;
                continue;
            }
        };
        let mut trace = Trace::new();
        let mut invalid = 0;
        for event in session.events {
            if let Err(err) = trace.apply(event) {
                eprintln!("invalid event: {err}");
                invalid += 1;
            }
        }
        if stats {
            print_stats(path, &trace, invalid, &session.skipped);
        } else {
            for root in trace.roots() {
                print_node(&trace, root, 0);
            }
            if !session.skipped.is_empty() {
                println!("\n{} line(s) skipped:", session.skipped.len());
                for s in &session.skipped {
                    println!("  {}:{}  {}", s.source, s.line, s.reason);
                }
            }
        }
        failed |= invalid > 0;
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn print_node(trace: &Trace, event: &TraceEvent, depth: usize) {
    let mut line = format!("{}{}", "  ".repeat(depth), label(trace, event));
    if let (Some(start), Some(end)) = (event.started_at_ms, event.ended_at_ms) {
        line.push_str(&format!("  [{}]", duration(end - start)));
    }
    println!("{line}");
    for child in trace.children(&event.id) {
        print_node(trace, child, depth + 1);
    }
}

fn label(trace: &Trace, event: &TraceEvent) -> String {
    match &event.node {
        Node::Run(run) => match &run.title {
            Some(title) => format!("Run: {}", short(title, 60)),
            None => "Run".to_string(),
        },
        Node::Turn(turn) => format!("Prompt: {}", short(&text_of(&turn.prompt), 70)),
        Node::ModelCall(call) => {
            let mut parts = vec![format!(
                "Model call ({})",
                call.model.as_deref().unwrap_or("unknown model")
            )];
            if let Some(reason) = &call.stop_reason {
                parts.push(format!("stop={}", stop_name(reason)));
            }
            if let Some(usage) = call.usage {
                if let Some(ctx) = usage.context_tokens() {
                    parts.push(format!("context={ctx}"));
                }
                if let Some(out) = usage.output_tokens {
                    parts.push(format!("out={out}"));
                }
            }
            let tools = trace
                .children(&event.id)
                .filter(|c| matches!(c.node, Node::ToolCall(_)))
                .count();
            if tools > 1 {
                parts.push(format!("{tools} tools in parallel"));
            }
            parts.join(" ")
        }
        Node::ToolCall(call) => {
            let status = match &call.result {
                None => "running",
                Some(r) if r.is_error => "error",
                Some(_) => "ok",
            };
            format!("{}({}) {status}", call.name, input_summary(&call.input))
        }
        Node::Marker(marker) => format!("[{}] {}", marker.kind, short(&marker.summary, 70)),
    }
}

fn print_stats(
    path: &std::path::Path,
    trace: &Trace,
    invalid: usize,
    skipped: &[adapter_claude_code::Skipped],
) {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut stack: Vec<&TraceEvent> = trace.roots().collect();
    while let Some(event) = stack.pop() {
        *counts.entry(event.node.kind_name()).or_default() += 1;
        stack.extend(trace.children(&event.id));
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let counts: Vec<String> = counts.iter().map(|(k, v)| format!("{k}={v}")).collect();
    println!(
        "{}  nodes={} {}  invalid={invalid} skipped={}",
        short(&name, 12),
        trace.len(),
        counts.join(" "),
        skipped.len()
    );
    let mut reasons: BTreeMap<&str, usize> = BTreeMap::new();
    for s in skipped {
        *reasons.entry(s.reason.as_str()).or_default() += 1;
    }
    for (reason, n) in reasons {
        println!("    skipped {n}x: {reason}");
    }
}

fn text_of(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The most telling string in the tool input, shortened.
fn input_summary(input: &Value) -> String {
    const PREFERRED: [&str; 8] = [
        "command",
        "file_path",
        "pattern",
        "query",
        "url",
        "skill",
        "path",
        "description",
    ];
    let Some(obj) = input.as_object() else {
        return String::new();
    };
    let found = PREFERRED
        .iter()
        .find_map(|k| obj.get(*k).and_then(Value::as_str))
        .or_else(|| obj.values().find_map(Value::as_str));
    short(found.unwrap_or(""), 40)
}

fn stop_name(reason: &trace_core::StopReason) -> String {
    match serde_json::to_value(reason) {
        Ok(Value::String(s)) => s,
        Ok(Value::Object(o)) => o
            .get("other")
            .and_then(Value::as_str)
            .unwrap_or("other")
            .to_string(),
        _ => "unknown".to_string(),
    }
}

/// One line, at most `max` characters, with an ellipsis if cut.
fn short(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max.saturating_sub(3)).collect();
        format!("{cut}...")
    }
}

fn duration(ms: i64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else {
        format!("{}m{:02}s", ms / 60_000, (ms % 60_000) / 1_000)
    }
}
