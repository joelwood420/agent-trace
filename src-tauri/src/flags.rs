//! The flags view: waste and problem flags for a whole session, found by the
//! harness-neutral `insights` crate with the Claude Code rules from the
//! adapter. The UI places each flag on the boxes named in its `trace_ids`.

use adapter_claude_code::flag_rules;
use serde::Serialize;
use trace_core::Trace;

/// The flags of one session, in session order.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionFlags {
    /// Every flag found, each with the trace ids it is about.
    pub flags: Vec<insights::Flag>,
}

/// Finds the flags of a session's trace.
pub fn session_flags(trace: &Trace) -> SessionFlags {
    SessionFlags {
        flags: insights::session_flags(trace, &flag_rules()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::load_trace;
    use insights::FlagKind;
    use std::path::{Path, PathBuf};

    fn fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("fixtures")
            .join("claude-code")
    }

    #[test]
    fn fixture_session_has_every_flag_kind() {
        let loaded = load_trace(&fixture_root(), "compaction", COMPACTION_SESSION).expect("trace");
        let view = session_flags(&loaded.trace);
        let kinds: Vec<FlagKind> = view.flags.iter().map(|f| f.kind).collect();
        assert_eq!(
            kinds,
            vec![
                FlagKind::RepeatedRead,
                FlagKind::FailedRetry,
                FlagKind::Loop,
                FlagKind::LargeResult,
                FlagKind::Compaction,
                FlagKind::RereadAfterCompaction,
            ]
        );
    }

    #[test]
    fn basic_fixture_flags_are_stable() {
        let loaded = load_trace(
            &fixture_root(),
            "basic",
            "00000000-0000-4000-8000-000000000002",
        )
        .expect("trace");
        let view = session_flags(&loaded.trace);
        assert_eq!(view, session_flags(&loaded.trace));
        for flag in &view.flags {
            for id in &flag.trace_ids {
                assert!(loaded.trace.get(id).is_some(), "unknown id {id}");
            }
        }
    }

    const COMPACTION_SESSION: &str = "00000000-0000-4000-8000-000000000003";
}
