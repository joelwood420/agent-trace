//! The compare view: two runs side by side. Each side is a whole session or
//! one prompt of it; the numbers come from the harness-neutral `insights`
//! crate, with the Claude Code flag rules from the adapter.

use adapter_claude_code::flag_rules;
use serde::{Deserialize, Serialize};
use trace_core::Trace;

use crate::captures::SessionRecords;
use crate::context::{self, ContextFacts};

/// One side of a comparison, as the UI asks for it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RunRef {
    /// The encoded project folder name.
    pub project: String,
    /// The session id.
    pub session_id: String,
    /// A main-run turn to limit the side to, or none for the whole session.
    #[serde(default)]
    pub prompt_trace_id: Option<String>,
}

/// A prompt the UI can limit a side to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PromptChoice {
    /// The turn's trace id.
    pub turn_id: String,
    /// Its position, from 0, as in the diagram.
    pub index: usize,
    /// A one-line preview of the prompt.
    pub preview: String,
}

/// What the UI needs about one side besides its numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompareSide {
    /// Every prompt of the session, for the scope drop-down.
    pub prompts: Vec<PromptChoice>,
    /// True when the asked-for prompt was not found and the whole session
    /// is shown instead.
    pub scope_missing: bool,
}

/// The response of `compare_runs`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompareView {
    /// The two summaries and the rows.
    pub comparison: insights::Comparison,
    /// Run A's prompts and scope note.
    pub a: CompareSide,
    /// Run B's prompts and scope note.
    pub b: CompareSide,
}

/// One side summarised, ready to compare.
#[derive(Debug, Clone, PartialEq)]
pub struct SideSummary {
    /// The numbers.
    pub summary: insights::RunSummary,
    /// The prompts and scope note.
    pub side: CompareSide,
}

/// Summarises one side. `captures` must be summarised, as for the context
/// views. A prompt that is not a main-run turn falls back to the whole
/// session.
pub fn side_summary(trace: &Trace, prompt: Option<&str>, captures: &SessionRecords) -> SideSummary {
    let (scope, scope_missing) = match prompt {
        Some(id) if insights::prompt_in_main_run(trace, id) => {
            (insights::Scope::Prompt(id.to_string()), false)
        }
        Some(_) => (insights::Scope::Session, true),
        None => (insights::Scope::Session, false),
    };
    let first = insights::first_main_call(trace, &scope).and_then(|id| {
        let facts = ContextFacts::of_call(trace, &id);
        context::call_context(&facts, captures, &id)
    });
    let flags = insights::session_flags(trace, &flag_rules());
    let summary = insights::run_summary(trace, &scope, first.as_ref(), &flags);
    let prompts = trace_view::build_session(trace)
        .prompts
        .into_iter()
        .map(|p| PromptChoice {
            turn_id: p.turn_id,
            index: p.index,
            preview: p.prompt_preview,
        })
        .collect();
    SideSummary {
        summary,
        side: CompareSide {
            prompts,
            scope_missing,
        },
    }
}

/// Puts two sides together.
pub fn compare_view(a: SideSummary, b: SideSummary) -> CompareView {
    CompareView {
        comparison: insights::compare(a.summary, b.summary),
        a: a.side,
        b: b.side,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::load_trace;
    use insights::Higher;
    use std::path::{Path, PathBuf};

    const BASIC: &str = "00000000-0000-4000-8000-000000000002";
    const COMPACTION: &str = "00000000-0000-4000-8000-000000000003";

    fn fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("fixtures")
            .join("claude-code")
    }

    fn side(project: &str, session: &str, prompt: Option<&str>) -> SideSummary {
        let loaded = load_trace(&fixture_root(), project, session).expect("trace");
        side_summary(&loaded.trace, prompt, &SessionRecords::default())
    }

    #[test]
    fn a_session_against_itself_is_equal() {
        let view = compare_view(side("basic", BASIC, None), side("basic", BASIC, None));
        for sec in &view.comparison.sections {
            for row in &sec.rows {
                assert!(
                    matches!(row.higher, Higher::Equal | Higher::Unknown),
                    "{}",
                    row.label
                );
            }
        }
        assert!(!view.a.scope_missing);
        assert!(!view.a.prompts.is_empty());
    }

    #[test]
    fn prompts_match_the_diagram() {
        let loaded = load_trace(&fixture_root(), "compaction", COMPACTION).expect("trace");
        let diagram = trace_view::build_session(&loaded.trace);
        let s = side("compaction", COMPACTION, None);
        let ids: Vec<&str> = s.side.prompts.iter().map(|p| p.turn_id.as_str()).collect();
        let expected: Vec<&str> = diagram.prompts.iter().map(|p| p.turn_id.as_str()).collect();
        assert_eq!(ids, expected);
        assert_eq!(s.summary.prompts as usize, expected.len());
    }

    #[test]
    fn two_prompts_of_one_session_count_separately() {
        let whole = side("compaction", COMPACTION, None);
        let prompts = &whole.side.prompts;
        assert!(
            prompts.len() >= 2,
            "the compaction fixture has several prompts"
        );
        let first = side("compaction", COMPACTION, Some(&prompts[0].turn_id));
        let second = side("compaction", COMPACTION, Some(&prompts[1].turn_id));
        assert_eq!(first.summary.prompts, 1);
        assert_eq!(second.summary.prompts, 1);
        assert!(
            first.summary.model_calls + second.summary.model_calls <= whole.summary.model_calls
        );
        assert!(!first.side.scope_missing);
    }

    #[test]
    fn a_missing_prompt_falls_back_to_the_session() {
        let whole = side("basic", BASIC, None);
        let missing = side("basic", BASIC, Some("no-such-turn"));
        assert!(missing.side.scope_missing);
        assert_eq!(missing.summary, whole.summary);
    }
}
