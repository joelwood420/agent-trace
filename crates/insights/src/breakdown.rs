//! Scaling a character measure to tokens, so slices add up to the total.

use serde::Serialize;

use crate::advice::Advice;
use crate::measure::{ContextMeasure, ContextSource, SliceKind};

/// Characters per token used when no reported total is known.
pub const CHARS_PER_TOKEN: f64 = 4.0;

/// What fills one call's context, in tokens.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContextBreakdown {
    /// Where the measure came from.
    pub source: ContextSource,
    /// Total input tokens.
    pub total_tokens: u64,
    /// True when the total is the API's reported number, not an estimate.
    pub total_is_reported: bool,
    /// Non-empty slices, in display order.
    pub slices: Vec<Slice>,
    /// Advice about the biggest costs.
    pub advice: Vec<Advice>,
}

/// One slice of the context.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Slice {
    /// Which kind of context this is.
    pub kind: SliceKind,
    /// Display name.
    pub label: String,
    /// Tokens in this slice.
    pub tokens: u64,
    /// Fraction of the total (0 to 1).
    pub share: f64,
    /// Named parts of the slice, biggest first.
    pub items: Vec<Item>,
}

/// One named part of a slice, in tokens.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Item {
    /// Name of the item.
    pub label: String,
    /// Tokens over all occurrences.
    pub tokens: u64,
    /// Number of occurrences.
    pub count: u32,
    /// Tokens of the biggest single occurrence.
    pub largest_tokens: u64,
}

/// A breakdown reduced to what a bar needs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContextBar {
    /// Where the measure came from.
    pub source: ContextSource,
    /// Total input tokens.
    pub total_tokens: u64,
    /// True when the total is the API's reported number.
    pub total_is_reported: bool,
    /// Slices in display order.
    pub slices: Vec<BarSlice>,
}

/// One segment of a context bar.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BarSlice {
    /// Which kind of context this is.
    pub kind: SliceKind,
    /// Tokens in this segment.
    pub tokens: u64,
}

fn to_tokens(chars: u64, ratio: f64) -> u64 {
    (chars as f64 * ratio).round().max(0.0) as u64
}

/// Scales a measure to tokens. `reported_total` is the model call's input
/// context as the API reported it, if known.
pub fn breakdown(measure: &ContextMeasure, reported_total: Option<u64>) -> ContextBreakdown {
    let chars = measure.total_chars();
    let estimate = 1.0 / CHARS_PER_TOKEN;
    let captured = measure.source == ContextSource::Captured;

    // (ratio, total, reported, extra not-captured tokens)
    let (ratio, total, reported, not_captured) = match (captured, reported_total) {
        (true, Some(t)) if chars > 0 => (t as f64 / chars as f64, t, true, 0),
        (true, Some(t)) => (estimate, t, true, 0),
        (true, None) => (estimate, to_tokens(chars, estimate), false, 0),
        (false, Some(t)) => {
            if chars as f64 * estimate <= t as f64 {
                let nc = t - to_tokens(chars, estimate).min(t);
                (estimate, t, true, nc)
            } else {
                (t as f64 / chars as f64, t, true, 0)
            }
        }
        (false, None) => (estimate, to_tokens(chars, estimate), false, 0),
    };

    let mut slices: Vec<Slice> = Vec::new();
    for kind in SliceKind::ALL {
        let slice_chars = measure.slice_chars(kind);
        let mut tokens = to_tokens(slice_chars, ratio);
        if kind == SliceKind::NotCaptured {
            tokens += not_captured;
        } else if slice_chars == 0 {
            continue;
        }
        if kind == SliceKind::NotCaptured && tokens == 0 && slice_chars == 0 {
            continue;
        }
        let mut items: Vec<Item> = measure
            .items(kind)
            .iter()
            .map(|i| Item {
                label: i.label.clone(),
                tokens: to_tokens(i.chars, ratio),
                count: i.count,
                largest_tokens: to_tokens(i.largest_chars, ratio),
            })
            .collect();
        items.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.label.cmp(&b.label)));
        slices.push(Slice {
            kind,
            label: kind.label().to_string(),
            tokens,
            share: 0.0,
            items,
        });
    }

    // Make the slices add up to the total: the largest slice absorbs the gap.
    if !slices.is_empty() {
        let sum: i128 = slices.iter().map(|s| i128::from(s.tokens)).sum();
        let gap = i128::from(total) - sum;
        let mut largest = 0;
        for (i, s) in slices.iter().enumerate() {
            if s.tokens > slices[largest].tokens {
                largest = i;
            }
        }
        let fixed = (i128::from(slices[largest].tokens) + gap).max(0);
        slices[largest].tokens = u64::try_from(fixed).unwrap_or(0);
    }
    slices.retain(|s| s.tokens > 0);
    for s in &mut slices {
        s.share = if total == 0 {
            0.0
        } else {
            s.tokens as f64 / total as f64
        };
    }

    ContextBreakdown {
        source: measure.source,
        total_tokens: total,
        total_is_reported: reported,
        slices,
        advice: Vec::new(),
    }
}

impl ContextBreakdown {
    /// The bar form: kinds and token counts only.
    pub fn bar(&self) -> ContextBar {
        ContextBar {
            source: self.source,
            total_tokens: self.total_tokens,
            total_is_reported: self.total_is_reported,
            slices: self
                .slices
                .iter()
                .map(|s| BarSlice {
                    kind: s.kind,
                    tokens: s.tokens,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn captured(parts: &[(SliceKind, u64)]) -> ContextMeasure {
        let mut m = ContextMeasure::new(ContextSource::Captured);
        for (k, c) in parts {
            m.add(*k, "x", *c);
        }
        m
    }

    fn tokens_of(b: &ContextBreakdown, k: SliceKind) -> Option<u64> {
        b.slices.iter().find(|s| s.kind == k).map(|s| s.tokens)
    }

    #[test]
    fn captured_slices_add_up_to_the_reported_total() {
        let m = captured(&[
            (SliceKind::SystemPrompt, 1000),
            (SliceKind::ToolDefinitions, 3000),
            (SliceKind::Conversation, 1),
        ]);
        let b = breakdown(&m, Some(1001));
        assert_eq!(tokens_of(&b, SliceKind::SystemPrompt), Some(250));
        assert_eq!(tokens_of(&b, SliceKind::ToolDefinitions), Some(751));
        assert_eq!(tokens_of(&b, SliceKind::Conversation), None);
        assert_eq!(b.slices.iter().map(|s| s.tokens).sum::<u64>(), 1001);
        assert!(b.total_is_reported);

        let m = captured(&[
            (SliceKind::SystemPrompt, 3),
            (SliceKind::ToolDefinitions, 3),
        ]);
        let b = breakdown(&m, Some(5));
        assert_eq!(tokens_of(&b, SliceKind::SystemPrompt), Some(2));
        assert_eq!(tokens_of(&b, SliceKind::ToolDefinitions), Some(3));
    }

    #[test]
    fn captured_without_total_uses_four_chars_per_token() {
        let m = captured(&[(SliceKind::Conversation, 4000)]);
        let b = breakdown(&m, None);
        assert_eq!(b.total_tokens, 1000);
        assert_eq!(tokens_of(&b, SliceKind::Conversation), Some(1000));
        assert!(!b.total_is_reported);
    }

    #[test]
    fn transcript_adds_not_captured() {
        let mut m = ContextMeasure::new(ContextSource::Transcript);
        m.add(SliceKind::Conversation, "x", 4000);
        let b = breakdown(&m, Some(5000));
        assert_eq!(tokens_of(&b, SliceKind::Conversation), Some(1000));
        assert_eq!(tokens_of(&b, SliceKind::NotCaptured), Some(4000));
        assert_eq!(b.total_tokens, 5000);
    }

    #[test]
    fn transcript_estimate_larger_than_total_is_scaled_down() {
        let mut m = ContextMeasure::new(ContextSource::Transcript);
        m.add(SliceKind::Conversation, "x", 8000);
        m.add(SliceKind::FilesRead, "y", 8000);
        let b = breakdown(&m, Some(1000));
        assert_eq!(b.slices.iter().map(|s| s.tokens).sum::<u64>(), 1000);
        assert_eq!(tokens_of(&b, SliceKind::NotCaptured), None);
    }

    #[test]
    fn transcript_without_total_has_no_not_captured() {
        let mut m = ContextMeasure::new(ContextSource::Transcript);
        m.add(SliceKind::Conversation, "x", 400);
        let b = breakdown(&m, None);
        assert_eq!(b.total_tokens, 100);
        assert!(!b.total_is_reported);
        assert_eq!(tokens_of(&b, SliceKind::NotCaptured), None);
    }

    #[test]
    fn items_are_sorted_largest_first_with_largest_tokens() {
        let mut m = ContextMeasure::new(ContextSource::Captured);
        m.add(SliceKind::FilesRead, "small", 400);
        m.add(SliceKind::FilesRead, "big", 400);
        m.add(SliceKind::FilesRead, "big", 1200);
        let b = breakdown(&m, None);
        let items = &b.slices[0].items;
        assert_eq!(items[0].label, "big");
        assert_eq!(items[0].tokens, 400);
        assert_eq!(items[0].count, 2);
        assert_eq!(items[0].largest_tokens, 300);
        assert_eq!(items[1].label, "small");
        assert_eq!(b.slices[0].label, "Files read");
    }

    #[test]
    fn bar_keeps_kinds_and_tokens_only() {
        let m = captured(&[
            (SliceKind::SystemPrompt, 400),
            (SliceKind::Conversation, 800),
        ]);
        let b = breakdown(&m, Some(300));
        let bar = b.bar();
        assert_eq!(bar.total_tokens, 300);
        assert!(bar.total_is_reported);
        assert_eq!(bar.slices.len(), 2);
        assert_eq!(bar.slices[0].kind, SliceKind::SystemPrompt);
        assert_eq!(bar.slices[0].tokens, 100);
        assert_eq!(bar.slices[1].tokens, 200);
    }

    #[test]
    fn zero_measure_with_total() {
        let m = ContextMeasure::new(ContextSource::Captured);
        let b = breakdown(&m, Some(500));
        assert!(b.slices.is_empty());
        assert_eq!(b.total_tokens, 500);
        assert!(b.total_is_reported);
    }
}
