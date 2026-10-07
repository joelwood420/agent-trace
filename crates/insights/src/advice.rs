//! Plain-language advice about what fills a call's context.

use serde::Serialize;

use crate::breakdown::{ContextBreakdown, Item};
use crate::measure::{ContextSource, SliceKind};

/// How much attention a piece of advice deserves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdviceLevel {
    /// Worth acting on.
    Warn,
    /// Good to know.
    Info,
}

/// One piece of advice, tied to the slice it is about.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Advice {
    /// How much attention it deserves.
    pub level: AdviceLevel,
    /// The slice it is about.
    pub slice: SliceKind,
    /// The advice in plain words.
    pub text: String,
}

/// Tool definitions share of the call at or above which rule 1 fires.
pub const TOOLS_SHARE_WARN: f64 = 0.20;
/// Tokens for one MCP server at or above which rule 2 fires.
pub const MCP_SERVER_TOKENS_WARN: u64 = 2_000;
/// Tokens of instructions at or above which rule 3 fires.
pub const INSTRUCTIONS_TOKENS_WARN: u64 = 5_000;
/// Number of times a file is in context at or above which rule 4 fires.
pub const FILE_REPEAT_WARN: u32 = 3;
/// Share of the call for one result at or above which rule 5 fires.
pub const SINGLE_RESULT_SHARE_WARN: f64 = 0.10;
/// Conversation share of the call at or above which rule 6 fires.
pub const CONVERSATION_SHARE_INFO: f64 = 0.60;

const MCP_PREFIX: &str = "MCP: ";

/// Rounds a token count for prose: "about 9k" from 1,000 up, else the exact number.
pub fn about_tokens(n: u64) -> String {
    if n >= 1000 {
        format!("about {}k", (n + 500) / 1000)
    } else {
        n.to_string()
    }
}

fn pct(share: f64) -> u64 {
    (share * 100.0).round().max(0.0) as u64
}

fn plural(n: u32, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn sorted_by<F: Fn(&Item) -> u64>(items: &[Item], key: F) -> Vec<&Item> {
    let mut v: Vec<&Item> = items.iter().collect();
    v.sort_by(|a, b| key(b).cmp(&key(a)).then_with(|| a.label.cmp(&b.label)));
    v
}

/// Turns a breakdown into plain advice, in a fixed rule order.
pub fn advise(b: &ContextBreakdown) -> Vec<Advice> {
    let mut out = Vec::new();
    let slice = |k: SliceKind| b.slices.iter().find(|s| s.kind == k);
    let mut push = |level: AdviceLevel, slice: SliceKind, text: String| {
        out.push(Advice { level, slice, text });
    };

    if let Some(s) = slice(SliceKind::ToolDefinitions) {
        if s.share >= TOOLS_SHARE_WARN {
            let n: u32 = s.items.iter().map(|i| i.count).sum();
            let m: u32 = s
                .items
                .iter()
                .filter(|i| i.label.starts_with(MCP_PREFIX))
                .map(|i| i.count)
                .sum();
            let mcp = if m > 0 {
                format!(", {m} from MCP servers")
            } else {
                String::new()
            };
            push(
                AdviceLevel::Warn,
                SliceKind::ToolDefinitions,
                format!(
                    "Tool definitions are {}% of this call ({} tokens): {n} tools{mcp}.",
                    pct(s.share),
                    about_tokens(s.tokens)
                ),
            );
        }
        for i in sorted_by(&s.items, |i| i.tokens) {
            if let Some(server) = i.label.strip_prefix(MCP_PREFIX) {
                if i.tokens >= MCP_SERVER_TOKENS_WARN {
                    push(
                        AdviceLevel::Warn,
                        SliceKind::ToolDefinitions,
                        format!(
                            "MCP server `{server}` adds {} tokens to every call ({}). Turn it off in projects that do not use it.",
                            about_tokens(i.tokens),
                            plural(i.count, "tool")
                        ),
                    );
                }
            }
        }
    }

    if let Some(s) = slice(SliceKind::Instructions) {
        if s.tokens >= INSTRUCTIONS_TOKENS_WARN {
            if let Some(top) = sorted_by(&s.items, |i| i.tokens).first() {
                push(
                    AdviceLevel::Warn,
                    SliceKind::Instructions,
                    format!(
                        "Instructions and reminders add {} tokens to every call. Largest: {} ({}).",
                        about_tokens(s.tokens),
                        top.label,
                        about_tokens(top.tokens)
                    ),
                );
            }
        }
    }

    if let Some(s) = slice(SliceKind::FilesRead) {
        for i in sorted_by(&s.items, |i| i.tokens) {
            if i.count >= FILE_REPEAT_WARN {
                push(
                    AdviceLevel::Warn,
                    SliceKind::FilesRead,
                    format!(
                        "`{}` is in context {} times ({} tokens). Each read adds the full file again.",
                        i.label,
                        i.count,
                        about_tokens(i.tokens)
                    ),
                );
            }
        }
    }

    for kind in [SliceKind::FilesRead, SliceKind::ToolResults] {
        let Some(s) = slice(kind) else { continue };
        if b.total_tokens == 0 {
            continue;
        }
        for i in sorted_by(&s.items, |i| i.largest_tokens) {
            let share = i.largest_tokens as f64 / b.total_tokens as f64;
            if share >= SINGLE_RESULT_SHARE_WARN {
                let text = if kind == SliceKind::FilesRead {
                    format!(
                        "One read of `{}` is {}% of this call ({} tokens).",
                        i.label,
                        pct(share),
                        about_tokens(i.largest_tokens)
                    )
                } else {
                    format!(
                        "One `{}` result is {}% of this call ({} tokens).",
                        i.label,
                        pct(share),
                        about_tokens(i.largest_tokens)
                    )
                };
                push(AdviceLevel::Warn, kind, text);
            }
        }
    }

    if let Some(s) = slice(SliceKind::Conversation) {
        if s.share >= CONVERSATION_SHARE_INFO {
            push(
                AdviceLevel::Info,
                SliceKind::Conversation,
                format!(
                    "Conversation history is {}% of this call. /compact or a fresh session would shrink it.",
                    pct(s.share)
                ),
            );
        }
    }

    if b.source == ContextSource::Transcript {
        push(
            AdviceLevel::Info,
            SliceKind::NotCaptured,
            "The system prompt, tool definitions and instructions are not visible for this call. The grey part is everything in the reported total that the transcript does not show. Run the session through the capture proxy to see it.".to_string(),
        );
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::breakdown::Slice;

    fn item(label: &str, tokens: u64, count: u32, largest: u64) -> Item {
        Item {
            label: label.to_string(),
            tokens,
            count,
            largest_tokens: largest,
        }
    }

    fn slice(kind: SliceKind, tokens: u64, total: u64, items: Vec<Item>) -> Slice {
        Slice {
            kind,
            label: kind.label().to_string(),
            tokens,
            share: tokens as f64 / total as f64,
            items,
        }
    }

    fn bd(total: u64, slices: Vec<Slice>) -> ContextBreakdown {
        ContextBreakdown {
            source: ContextSource::Captured,
            total_tokens: total,
            total_is_reported: true,
            slices,
            advice: Vec::new(),
        }
    }

    fn texts(b: &ContextBreakdown) -> Vec<String> {
        advise(b).into_iter().map(|a| a.text).collect()
    }

    #[test]
    fn about_tokens_cases() {
        assert_eq!(about_tokens(0), "0");
        assert_eq!(about_tokens(999), "999");
        assert_eq!(about_tokens(1000), "about 1k");
        assert_eq!(about_tokens(1499), "about 1k");
        assert_eq!(about_tokens(1500), "about 2k");
        assert_eq!(about_tokens(41_234), "about 41k");
    }

    #[test]
    fn rule1_tool_definitions_share() {
        let mk = |t| {
            bd(
                1000,
                vec![slice(
                    SliceKind::ToolDefinitions,
                    t,
                    1000,
                    vec![item("Bash", t - 50, 3, 10), item("MCP: git", 50, 2, 10)],
                )],
            )
        };
        assert!(texts(&mk(199)).is_empty());
        assert_eq!(
            texts(&mk(200)),
            vec![
                "Tool definitions are 20% of this call (200 tokens): 5 tools, 2 from MCP servers."
            ]
        );
        assert_eq!(texts(&mk(201)).len(), 1);
        let b = bd(
            1000,
            vec![slice(
                SliceKind::ToolDefinitions,
                300,
                1000,
                vec![item("Bash", 300, 1, 10)],
            )],
        );
        assert_eq!(
            texts(&b),
            vec!["Tool definitions are 30% of this call (300 tokens): 1 tools."]
        );
        let a = &advise(&mk(200))[0];
        assert_eq!(a.level, AdviceLevel::Warn);
        assert_eq!(a.slice, SliceKind::ToolDefinitions);
    }

    #[test]
    fn rule2_mcp_server() {
        let mk = |t, c| {
            bd(
                100_000,
                vec![slice(
                    SliceKind::ToolDefinitions,
                    t,
                    100_000,
                    vec![item("MCP: db", t, c, 10)],
                )],
            )
        };
        assert!(texts(&mk(1999, 4)).is_empty());
        assert_eq!(
            texts(&mk(2000, 4)),
            vec![
                "MCP server `db` adds about 2k tokens to every call (4 tools). Turn it off in projects that do not use it."
            ]
        );
        assert_eq!(
            texts(&mk(2001, 1)),
            vec![
                "MCP server `db` adds about 2k tokens to every call (1 tool). Turn it off in projects that do not use it."
            ]
        );
    }

    #[test]
    fn rule2_largest_first() {
        let b = bd(
            100_000,
            vec![slice(
                SliceKind::ToolDefinitions,
                9000,
                100_000,
                vec![item("MCP: a", 3000, 1, 1), item("MCP: b", 6000, 2, 1)],
            )],
        );
        let t = texts(&b);
        assert!(t[0].contains("`b`"));
        assert!(t[1].contains("`a`"));
    }

    #[test]
    fn rule3_instructions() {
        let mk = |t| {
            bd(
                100_000,
                vec![slice(
                    SliceKind::Instructions,
                    t,
                    100_000,
                    vec![item("CLAUDE.md", t - 100, 1, 1), item("memory", 100, 1, 1)],
                )],
            )
        };
        assert!(texts(&mk(4999)).is_empty());
        assert_eq!(
            texts(&mk(5000)),
            vec![
                "Instructions and reminders add about 5k tokens to every call. Largest: CLAUDE.md (about 5k)."
            ]
        );
        assert_eq!(texts(&mk(5001)).len(), 1);
    }

    #[test]
    fn rule4_repeated_file() {
        let mk = |c| {
            bd(
                100_000,
                vec![slice(
                    SliceKind::FilesRead,
                    1500,
                    100_000,
                    vec![item("src/a.rs", 1500, c, 10)],
                )],
            )
        };
        assert!(texts(&mk(2)).is_empty());
        assert_eq!(
            texts(&mk(3)),
            vec![
                "`src/a.rs` is in context 3 times (about 2k tokens). Each read adds the full file again."
            ]
        );
        assert_eq!(texts(&mk(4)).len(), 1);
    }

    #[test]
    fn rule5_single_result() {
        let mk = |kind, l| {
            bd(
                1000,
                vec![slice(kind, 500, 1000, vec![item("x", 500, 1, l)])],
            )
        };
        assert!(texts(&mk(SliceKind::FilesRead, 99)).is_empty());
        assert_eq!(
            texts(&mk(SliceKind::FilesRead, 100)),
            vec!["One read of `x` is 10% of this call (100 tokens)."]
        );
        assert_eq!(texts(&mk(SliceKind::FilesRead, 101)).len(), 1);
        assert!(texts(&mk(SliceKind::ToolResults, 99)).is_empty());
        assert_eq!(
            texts(&mk(SliceKind::ToolResults, 100)),
            vec!["One `x` result is 10% of this call (100 tokens)."]
        );
        assert_eq!(texts(&mk(SliceKind::ToolResults, 101)).len(), 1);
    }

    #[test]
    fn rule6_conversation() {
        let mk = |t| {
            bd(
                1000,
                vec![slice(SliceKind::Conversation, t, 1000, Vec::new())],
            )
        };
        assert!(texts(&mk(599)).is_empty());
        let a = advise(&mk(600));
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].level, AdviceLevel::Info);
        assert_eq!(
            a[0].text,
            "Conversation history is 60% of this call. /compact or a fresh session would shrink it."
        );
        assert_eq!(advise(&mk(601)).len(), 1);
    }

    #[test]
    fn rule7_transcript_only() {
        let mut b = bd(1000, Vec::new());
        assert!(advise(&b).is_empty());
        b.source = ContextSource::Transcript;
        let a = advise(&b);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].level, AdviceLevel::Info);
        assert_eq!(a[0].slice, SliceKind::NotCaptured);
        assert_eq!(
            a[0].text,
            "The system prompt, tool definitions and instructions are not visible for this call. The grey part is everything in the reported total that the transcript does not show. Run the session through the capture proxy to see it."
        );
    }

    #[test]
    fn breakdown_includes_advice() {
        let mut m = crate::measure::ContextMeasure::new(ContextSource::Transcript);
        m.add(SliceKind::Conversation, "x", 4000);
        let b = crate::breakdown::breakdown(&m, Some(5000));
        assert!(b.advice.iter().any(|a| a.slice == SliceKind::NotCaptured));
    }
}
