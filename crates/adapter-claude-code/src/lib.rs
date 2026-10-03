//! Reads Claude Code JSONL transcripts and converts them into `trace-core`
//! events. Parsing is defensive: unknown fields and event types are skipped
//! and logged, never a panic.
