//! Builds the diagram model from a trace.
//!
//! The rules are described in `docs/VIEW-MODEL.md`. In short:
//! - each top-level turn becomes one prompt diagram with the prompt as root;
//! - two or more tool calls under one model call are wrapped in a
//!   `parallel_group` box;
//! - a tool call that started a subagent gets a `subagent` child box,
//!   collapsed by default;
//! - `SUMMARY_MIN_CALLS` or more similar model calls in a row become one
//!   `summary` box, collapsed by default.

use trace_core::{Marker, ModelCall, Node, Run, ToolCall, Trace, TraceEvent, Turn};

use crate::model::{DiagramNode, NodeKind, PromptDiagram, PromptTotals, SessionDiagram, Status};
use crate::text::{
    MARKER_LABEL_CHARS, PROMPT_PREVIEW_CHARS, count, input_hint, prompt_preview, short,
    stop_reason_name,
};

/// Fewest similar model calls in a row that are collapsed into one
/// `summary` box. Two similar calls in a row are shown as they are.
pub const SUMMARY_MIN_CALLS: usize = 3;

/// Builds the diagram model for the first root `Run` in the trace.
///
/// If the trace has no root run, the result has no prompts and no markers.
/// The result depends only on the trace, so building the same trace twice
/// gives identical output.
pub fn build_session(trace: &Trace) -> SessionDiagram {
    let first_run = trace.roots().find_map(|event| match &event.node {
        Node::Run(run) => Some((event, run)),
        _ => None,
    });
    let Some((event, run)) = first_run else {
        return SessionDiagram {
            run_id: None,
            title: None,
            harness: None,
            started_at_ms: None,
            ended_at_ms: None,
            duration_ms: None,
            markers: Vec::new(),
            prompts: Vec::new(),
        };
    };

    let mut markers = Vec::new();
    let mut prompts = Vec::new();
    for child in trace.children(&event.id) {
        match &child.node {
            Node::Turn(turn) => {
                let index = prompts.len();
                prompts.push(prompt_diagram(trace, child, turn, index));
            }
            Node::Marker(marker) => markers.push(marker_node(child, marker)),
            _ => {}
        }
    }

    SessionDiagram {
        run_id: Some(event.id.clone()),
        title: run.title.clone(),
        harness: Some(run.harness.clone()),
        started_at_ms: event.started_at_ms,
        ended_at_ms: event.ended_at_ms,
        duration_ms: duration(event),
        markers,
        prompts,
    }
}

fn prompt_diagram(trace: &Trace, event: &TraceEvent, turn: &Turn, index: usize) -> PromptDiagram {
    let root = prompt_node(trace, event, turn);
    PromptDiagram {
        index,
        turn_id: event.id.clone(),
        prompt_preview: root.label.clone(),
        started_at_ms: event.started_at_ms,
        ended_at_ms: event.ended_at_ms,
        duration_ms: duration(event),
        totals: totals(trace, &event.id),
        root,
    }
}

/// A diagram box together with what the summary rule needs to know.
struct Item {
    node: DiagramNode,
    /// The tool name if this is a model call that may join a summary.
    similar_tool: Option<String>,
}

impl Item {
    fn plain(node: DiagramNode) -> Self {
        Item {
            node,
            similar_tool: None,
        }
    }
}

fn prompt_node(trace: &Trace, event: &TraceEvent, turn: &Turn) -> DiagramNode {
    let children = summarise(turn_items(trace, &event.id));
    let totals = totals(trace, &event.id);
    DiagramNode {
        id: node_id(NodeKind::Prompt, &event.id),
        kind: NodeKind::Prompt,
        label: prompt_preview(&turn.prompt),
        detail_label: Some(totals_label(&totals)),
        started_at_ms: event.started_at_ms,
        duration_ms: duration(event),
        context_tokens: totals.max_context_tokens,
        output_tokens: totals.output_tokens,
        status: worst_inside(&children),
        collapsed_by_default: false,
        trace_ids: vec![event.id.clone()],
        children,
    }
}

/// The model calls and markers of a turn, in order.
fn turn_items(trace: &Trace, turn_id: &str) -> Vec<Item> {
    trace
        .children(turn_id)
        .filter_map(|child| match &child.node {
            Node::ModelCall(call) => Some(Item {
                similar_tool: similar_tool(trace, child, call),
                node: model_call_node(trace, child, call),
            }),
            Node::Marker(marker) => Some(Item::plain(marker_node(child, marker))),
            _ => None,
        })
        .collect()
}

fn model_call_node(trace: &Trace, event: &TraceEvent, call: &ModelCall) -> DiagramNode {
    let mut tools = Vec::new();
    let mut names = Vec::new();
    let mut others = Vec::new();
    for child in trace.children(&event.id) {
        match &child.node {
            Node::ToolCall(tool) => {
                names.push(tool.name.as_str());
                tools.push(tool_call_node(trace, child, tool));
            }
            Node::Marker(marker) => others.push(marker_node(child, marker)),
            _ => {}
        }
    }
    let mut children = if tools.len() > 1 {
        vec![parallel_group_node(&event.id, &names, tools)]
    } else {
        tools
    };
    children.extend(others);

    let mut detail = Vec::new();
    if let Some(model) = &call.model {
        detail.push(model.clone());
    }
    if let Some(reason) = &call.stop_reason {
        detail.push(format!("stop: {}", stop_reason_name(reason)));
    }

    DiagramNode {
        id: node_id(NodeKind::ModelCall, &event.id),
        kind: NodeKind::ModelCall,
        label: "Model call".to_string(),
        detail_label: (!detail.is_empty()).then(|| detail.join(", ")),
        started_at_ms: event.started_at_ms,
        duration_ms: duration(event),
        context_tokens: call.usage.and_then(|u| u.context_tokens()),
        output_tokens: call.usage.and_then(|u| u.output_tokens),
        status: model_call_status(event, call),
        collapsed_by_default: false,
        trace_ids: vec![event.id.clone()],
        children,
    }
}

/// A model call is running until it has a stop reason or an end time.
fn model_call_status(event: &TraceEvent, call: &ModelCall) -> Status {
    if call.stop_reason.is_some() || event.ended_at_ms.is_some() {
        Status::Ok
    } else {
        Status::Running
    }
}

fn tool_call_node(trace: &Trace, event: &TraceEvent, tool: &ToolCall) -> DiagramNode {
    let children: Vec<DiagramNode> = trace
        .children(&event.id)
        .filter_map(|child| match &child.node {
            Node::Run(run) => Some(subagent_node(trace, child, run)),
            _ => None,
        })
        .collect();
    let label = match input_hint(&tool.input) {
        Some(hint) => format!("{}({hint})", tool.name),
        None => tool.name.clone(),
    };
    DiagramNode {
        id: node_id(NodeKind::ToolCall, &event.id),
        kind: NodeKind::ToolCall,
        label,
        detail_label: None,
        started_at_ms: event.started_at_ms,
        duration_ms: duration(event),
        context_tokens: None,
        output_tokens: None,
        status: tool_status(tool),
        collapsed_by_default: false,
        trace_ids: vec![event.id.clone()],
        children,
    }
}

fn tool_status(tool: &ToolCall) -> Status {
    match &tool.result {
        None => Status::Running,
        Some(result) if result.is_error => Status::Error,
        Some(_) => Status::Ok,
    }
}

/// A subagent run. If it has exactly one turn, that turn's model calls and
/// markers are its children directly, and the turn id is added to
/// `trace_ids` so the UI can still show the subagent's task. With several
/// turns, each becomes a `prompt` child.
fn subagent_node(trace: &Trace, event: &TraceEvent, run: &Run) -> DiagramNode {
    let turn_count = trace
        .children(&event.id)
        .filter(|c| matches!(c.node, Node::Turn(_)))
        .count();
    let mut trace_ids = vec![event.id.clone()];
    let mut items = Vec::new();
    for child in trace.children(&event.id) {
        match &child.node {
            Node::Turn(turn) if turn_count == 1 => {
                trace_ids.push(child.id.clone());
                items.extend(turn_items(trace, &child.id));
            }
            Node::Turn(turn) => items.push(Item::plain(prompt_node(trace, child, turn))),
            Node::Marker(marker) => items.push(Item::plain(marker_node(child, marker))),
            _ => {}
        }
    }
    let children = summarise(items);
    let totals = totals(trace, &event.id);
    let label = match &run.title {
        Some(title) if !title.trim().is_empty() => short(title, PROMPT_PREVIEW_CHARS),
        _ => "Subagent".to_string(),
    };
    DiagramNode {
        id: node_id(NodeKind::Subagent, &event.id),
        kind: NodeKind::Subagent,
        label,
        detail_label: Some(totals_label(&totals)),
        started_at_ms: event.started_at_ms,
        duration_ms: duration(event),
        context_tokens: totals.max_context_tokens,
        output_tokens: totals.output_tokens,
        status: worst_inside(&children),
        collapsed_by_default: true,
        trace_ids,
        children,
    }
}

fn marker_node(event: &TraceEvent, marker: &Marker) -> DiagramNode {
    DiagramNode {
        id: node_id(NodeKind::Marker, &event.id),
        kind: NodeKind::Marker,
        label: short(&marker.summary, MARKER_LABEL_CHARS),
        detail_label: Some(marker.kind.clone()),
        started_at_ms: event.started_at_ms,
        duration_ms: duration(event),
        context_tokens: None,
        output_tokens: None,
        status: Status::None,
        collapsed_by_default: false,
        trace_ids: vec![event.id.clone()],
        children: Vec::new(),
    }
}

fn parallel_group_node(
    model_call_id: &str,
    names: &[&str],
    tools: Vec<DiagramNode>,
) -> DiagramNode {
    let (started_at_ms, duration_ms) = span(&tools);
    DiagramNode {
        id: node_id(NodeKind::ParallelGroup, model_call_id),
        kind: NodeKind::ParallelGroup,
        label: format!("{} tool calls in parallel", tools.len()),
        detail_label: Some(tool_names(names)),
        started_at_ms,
        duration_ms,
        context_tokens: None,
        output_tokens: None,
        status: worst_inside(&tools),
        collapsed_by_default: false,
        trace_ids: tools.iter().flat_map(|t| t.trace_ids.clone()).collect(),
        children: tools,
    }
}

/// Distinct tool names in order of first use, with a count when repeated,
/// for example `"Grep x2, Read"`.
fn tool_names(names: &[&str]) -> String {
    let mut counted: Vec<(&str, usize)> = Vec::new();
    for name in names {
        match counted.iter_mut().find(|(n, _)| n == name) {
            Some((_, seen)) => *seen += 1,
            None => counted.push((name, 1)),
        }
    }
    counted
        .into_iter()
        .map(|(name, n)| {
            if n > 1 {
                format!("{name} x{n}")
            } else {
                name.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The tool name if this model call can join a summary: it has finished,
/// its only child is one finished tool call without error, and that tool
/// call did not start a subagent.
fn similar_tool(trace: &Trace, event: &TraceEvent, call: &ModelCall) -> Option<String> {
    if model_call_status(event, call) != Status::Ok {
        return None;
    }
    let children: Vec<&TraceEvent> = trace.children(&event.id).collect();
    let [only] = children.as_slice() else {
        return None;
    };
    let Node::ToolCall(tool) = &only.node else {
        return None;
    };
    if tool_status(tool) != Status::Ok || trace.children(&only.id).next().is_some() {
        return None;
    }
    Some(tool.name.clone())
}

/// Replaces each run of similar model calls with one summary box.
fn summarise(items: Vec<Item>) -> Vec<DiagramNode> {
    let ranges = summary_ranges(&items);
    let mut out = Vec::new();
    let mut members = Vec::new();
    let mut next = 0;
    for (i, item) in items.into_iter().enumerate() {
        match ranges.get(next) {
            Some((start, end, tool)) if i >= *start && i < *end => {
                members.push(item.node);
                if i + 1 == *end {
                    out.push(summary_node(tool, std::mem::take(&mut members)));
                    next += 1;
                }
            }
            _ => out.push(item.node),
        }
    }
    out
}

/// Finds runs to summarise as `(start, end, tool)` with `end` exclusive.
///
/// A run starts at a model call that may join a summary and continues over
/// model calls with the same tool name. Markers in between do not break the
/// run and are kept inside it, but a run never starts or ends with a
/// marker. Anything else ends the run. A run is kept only if it has at
/// least `SUMMARY_MIN_CALLS` model calls.
fn summary_ranges(items: &[Item]) -> Vec<(usize, usize, String)> {
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < items.len() {
        let Some(tool) = &items[i].similar_tool else {
            i += 1;
            continue;
        };
        let mut end = i + 1;
        let mut calls = 1;
        for (j, item) in items.iter().enumerate().skip(i + 1) {
            match &item.similar_tool {
                Some(other) if other == tool => {
                    calls += 1;
                    end = j + 1;
                }
                None if item.node.kind == NodeKind::Marker => {}
                _ => break,
            }
        }
        if calls >= SUMMARY_MIN_CALLS {
            ranges.push((i, end, tool.clone()));
        }
        i = end;
    }
    ranges
}

fn summary_node(tool: &str, members: Vec<DiagramNode>) -> DiagramNode {
    let calls = members
        .iter()
        .filter(|m| m.kind == NodeKind::ModelCall)
        .count();
    let markers = members.len() - calls;
    let mut detail = count(calls, "model call", "model calls");
    if markers > 0 {
        detail = format!("{detail}, {}", count(markers, "marker", "markers"));
    }
    let first_id = members.first().map(|m| m.id.clone()).unwrap_or_default();
    let (started_at_ms, duration_ms) = span(&members);
    DiagramNode {
        id: format!("{}:{first_id}", NodeKind::Summary.as_str()),
        kind: NodeKind::Summary,
        label: format!("{calls} x {tool}"),
        detail_label: Some(detail),
        started_at_ms,
        duration_ms,
        context_tokens: members.iter().filter_map(|m| m.context_tokens).max(),
        output_tokens: sum_known(members.iter().map(|m| m.output_tokens)),
        status: worst_inside(&members),
        collapsed_by_default: true,
        trace_ids: members.iter().flat_map(|m| m.trace_ids.clone()).collect(),
        children: members,
    }
}

/// The worst status of these boxes and everything inside them. Container
/// boxes use this, so an error deep inside a collapsed box still shows.
fn worst_inside(nodes: &[DiagramNode]) -> Status {
    let mut statuses = Vec::new();
    collect_statuses(nodes, &mut statuses);
    Status::worst(&statuses)
}

fn collect_statuses(nodes: &[DiagramNode], out: &mut Vec<Status>) {
    for node in nodes {
        out.push(node.status);
        collect_statuses(&node.children, out);
    }
}

/// Start and duration covering these boxes and everything inside them.
/// The duration is `None` if anything inside is still running.
fn span(nodes: &[DiagramNode]) -> (Option<i64>, Option<i64>) {
    let start = nodes.iter().filter_map(|n| n.started_at_ms).min();
    if nodes.iter().any(|n| n.status == Status::Running) {
        return (start, None);
    }
    let end = nodes.iter().filter_map(span_end).max();
    let duration = match (start, end) {
        (Some(s), Some(e)) if e >= s => Some(e - s),
        _ => None,
    };
    (start, duration)
}

/// The latest known end time of a box or anything inside it.
fn span_end(node: &DiagramNode) -> Option<i64> {
    let own = node
        .started_at_ms
        .zip(node.duration_ms)
        .map(|(start, duration)| start + duration);
    node.children.iter().filter_map(span_end).chain(own).max()
}

fn sum_known(values: impl Iterator<Item = Option<u64>>) -> Option<u64> {
    values
        .flatten()
        .fold(None, |sum, v| Some(sum.unwrap_or(0) + v))
}

/// Summary numbers for everything under the node with this id.
fn totals(trace: &Trace, id: &str) -> PromptTotals {
    let mut totals = PromptTotals::default();
    add_totals(trace, id, false, &mut totals);
    totals
}

/// `in_subagent` is true below a nested `Run`, whose model calls do not
/// count toward the largest context size.
fn add_totals(trace: &Trace, id: &str, in_subagent: bool, totals: &mut PromptTotals) {
    for child in trace.children(id) {
        let mut nested = in_subagent;
        match &child.node {
            Node::ModelCall(call) => {
                totals.model_calls += 1;
                if let Some(usage) = call.usage {
                    if !in_subagent {
                        totals.max_context_tokens =
                            totals.max_context_tokens.max(usage.context_tokens());
                    }
                    if let Some(out) = usage.output_tokens {
                        totals.output_tokens = Some(totals.output_tokens.unwrap_or(0) + out);
                    }
                }
            }
            Node::ToolCall(tool) => {
                totals.tool_calls += 1;
                if tool_status(tool) == Status::Error {
                    totals.errors += 1;
                }
            }
            Node::Run(_) => nested = true,
            Node::Turn(_) | Node::Marker(_) => {}
        }
        add_totals(trace, &child.id, nested, totals);
    }
}

/// For example `"3 model calls, 2 tool calls, 1 error"`.
fn totals_label(totals: &PromptTotals) -> String {
    let mut parts = vec![
        count(totals.model_calls, "model call", "model calls"),
        count(totals.tool_calls, "tool call", "tool calls"),
    ];
    if totals.errors > 0 {
        parts.push(count(totals.errors, "error", "errors"));
    }
    parts.join(", ")
}

fn node_id(kind: NodeKind, trace_id: &str) -> String {
    format!("{}:{trace_id}", kind.as_str())
}

/// `ended_at_ms - started_at_ms`, when both are known and in order.
pub(crate) fn duration(event: &TraceEvent) -> Option<i64> {
    match (event.started_at_ms, event.ended_at_ms) {
        (Some(start), Some(end)) if end >= start => Some(end - start),
        _ => None,
    }
}
