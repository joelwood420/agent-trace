//! Prints the diagram model of a Claude Code session, to check by eye what
//! the UI will be asked to draw.
//!
//! ```powershell
//! cargo run -p trace-view --example print_diagram -- <session.jsonl>
//! cargo run -p trace-view --example print_diagram -- --json <session.jsonl>
//! cargo run -p trace-view --example print_diagram -- --counts <session.jsonl>...
//! ```
//!
//! `--json` prints the diagram as JSON. `--counts` prints only box counts per
//! kind, never content, so it is safe to run on real sessions.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::process::ExitCode;

use adapter_claude_code::load_session;
use trace_core::Trace;
use trace_view::{DiagramNode, SessionDiagram, Status, build_session};

enum Mode {
    Tree,
    Json,
    Counts,
}

fn main() -> ExitCode {
    let mut mode = Mode::Tree;
    let mut paths = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--json" => mode = Mode::Json,
            "--counts" => mode = Mode::Counts,
            "-h" | "--help" => {
                println!("usage: print_diagram [--json | --counts] <session.jsonl>...");
                return ExitCode::SUCCESS;
            }
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    if paths.is_empty() {
        eprintln!("usage: print_diagram [--json | --counts] <session.jsonl>...");
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
            if trace.apply(event).is_err() {
                invalid += 1;
            }
        }
        failed |= invalid > 0;
        let diagram = build_session(&trace);
        match mode {
            Mode::Tree => print_tree(&diagram),
            Mode::Json => match serde_json::to_string_pretty(&diagram) {
                Ok(json) => println!("{json}"),
                Err(err) => {
                    eprintln!("error: {err}");
                    failed = true;
                }
            },
            Mode::Counts => print_counts(&diagram, invalid),
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn print_tree(diagram: &SessionDiagram) {
    println!(
        "Session: {}  harness={}{}",
        diagram.title.as_deref().unwrap_or("(untitled)"),
        diagram.harness.as_deref().unwrap_or("?"),
        timing(diagram.duration_ms)
    );
    for marker in &diagram.markers {
        print_node(marker, 1);
    }
    for prompt in &diagram.prompts {
        let t = &prompt.totals;
        println!(
            "\nPrompt {} (model_calls={} tool_calls={} errors={} max_context={} output={}){}",
            prompt.index,
            t.model_calls,
            t.tool_calls,
            t.errors,
            number(t.max_context_tokens),
            number(t.output_tokens),
            timing(prompt.duration_ms)
        );
        print_node(&prompt.root, 1);
    }
}

fn print_node(node: &DiagramNode, depth: usize) {
    let mut line = format!(
        "{}[{}] {}",
        "  ".repeat(depth),
        node.kind.as_str(),
        node.label
    );
    if let Some(detail) = &node.detail_label {
        line.push_str(&format!(" | {detail}"));
    }
    if let Some(ctx) = node.context_tokens {
        line.push_str(&format!(" ctx={ctx}"));
    }
    if let Some(out) = node.output_tokens {
        line.push_str(&format!(" out={out}"));
    }
    match node.status {
        Status::Ok => line.push_str(" ok"),
        Status::Error => line.push_str(" ERROR"),
        Status::Running => line.push_str(" running"),
        Status::None => {}
    }
    if node.collapsed_by_default {
        line.push_str(" (collapsed)");
    }
    line.push_str(&timing(node.duration_ms));
    println!("{line}");
    for child in &node.children {
        print_node(child, depth + 1);
    }
}

fn print_counts(diagram: &SessionDiagram, invalid: usize) {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut seen = HashSet::new();
    let mut duplicate_ids = 0;
    let mut stack: Vec<&DiagramNode> = diagram.markers.iter().collect();
    stack.extend(diagram.prompts.iter().map(|p| &p.root));
    while let Some(node) = stack.pop() {
        *counts.entry(node.kind.as_str()).or_default() += 1;
        if !seen.insert(node.id.as_str()) {
            duplicate_ids += 1;
        }
        stack.extend(&node.children);
    }
    let counts: Vec<String> = counts.iter().map(|(k, v)| format!("{k}={v}")).collect();
    println!(
        "prompts={} {} invalid_events={invalid} duplicate_ids={duplicate_ids}",
        diagram.prompts.len(),
        counts.join(" ")
    );
}

fn number(value: Option<u64>) -> String {
    value.map_or_else(|| "?".to_string(), |v| v.to_string())
}

fn timing(ms: Option<i64>) -> String {
    match ms {
        None => String::new(),
        Some(ms) if ms < 1_000 => format!("  [{ms}ms]"),
        Some(ms) if ms < 60_000 => format!("  [{:.1}s]", ms as f64 / 1_000.0),
        Some(ms) => format!("  [{}m{:02}s]", ms / 60_000, (ms % 60_000) / 1_000),
    }
}
