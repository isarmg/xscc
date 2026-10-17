use std::ops::Range;

#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct State {
    pub text: String,
    pub selection: Range<usize>,
    pub composing: Option<Range<usize>>,
}

pub(crate) struct Replacement {
    pub range: Range<usize>,
    pub text: String,
}

impl State {
    pub fn new(text: String, selection: Range<usize>, composing: Option<Range<usize>>) -> Self {
        let selection = utf16_boundary(&text, selection.start, false)
            ..utf16_boundary(&text, selection.end, false);
        let composing = composing.and_then(|range| {
            if range.start == range.end {
                return None;
            }
            // Expand a malformed span around a surrogate pair instead of splitting it.
            let start = utf16_boundary(&text, range.start.min(range.end), false);
            let end = utf16_boundary(&text, range.start.max(range.end), true);
            (start < end).then_some(start..end)
        });
        Self {
            text,
            selection,
            composing,
        }
    }

    pub fn composing_text(&self) -> Option<&str> {
        let range = self.composing.as_ref()?;
        self.text.get(
            utf16_byte_offset(&self.text, range.start)..utf16_byte_offset(&self.text, range.end),
        )
    }

    // A composition commit can change only the spans, so compare States as well.
    pub fn replacement(&self, incoming: &Self) -> Option<Replacement> {
        if self.text == incoming.text {
            return None;
        }

        let (mut prefix_bytes, mut prefix_utf16) = (0, 0);
        for (old, new) in self.text.chars().zip(incoming.text.chars()) {
            if old != new {
                break;
            }
            prefix_bytes += old.len_utf8();
            prefix_utf16 += old.len_utf16();
        }

        let (mut suffix_bytes, mut suffix_utf16) = (0, 0);
        for (old, new) in self.text[prefix_bytes..]
            .chars()
            .rev()
            .zip(incoming.text[prefix_bytes..].chars().rev())
        {
            if old != new {
                break;
            }
            suffix_bytes += old.len_utf8();
            suffix_utf16 += old.len_utf16();
        }

        Some(Replacement {
            range: prefix_utf16..self.text.encode_utf16().count() - suffix_utf16,
            text: incoming.text[prefix_bytes..incoming.text.len() - suffix_bytes].to_owned(),
        })
    }
}

fn utf16_boundary(text: &str, index: usize, round_up: bool) -> usize {
    let mut offset = 0;
    for character in text.chars() {
        let next = offset + character.len_utf16();
        if index < next {
            return if round_up && index > offset {
                next
            } else {
                offset
            };
        }
        offset = next;
    }
    offset
}

fn utf16_byte_offset(text: &str, index: usize) -> usize {
    let mut offset = 0;
    for (byte, character) in text.char_indices() {
        offset += character.len_utf16();
        if index < offset {
            return byte;
        }
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::State;

    #[test]
    fn chinese_composition_commit_preserves_text_and_region() {
        let previous = State::new("我n好".into(), 2..2, Some(1..2));
        let composing = State::new("我你好".into(), 2..2, Some(1..2));
        let replacement = previous.replacement(&composing).unwrap();
        assert_eq!(replacement.range, 1..2);
        assert_eq!(replacement.text, "你");
        assert_eq!(composing.composing, Some(1..2));
        assert_eq!(composing.composing_text(), Some("你"));

        let committed = State::new("我你好".into(), 2..2, None);
        assert!(composing != committed);
        assert!(composing.replacement(&committed).is_none());
        assert_eq!(committed.composing_text(), None);
        assert!(committed == State::new("我你好".into(), 2..2, None));
        assert!(committed.replacement(&committed).is_none());
    }

    #[test]
    fn emoji_insertion_deletion_and_selection_use_utf16() {
        let initial = State::new("A😀Z".into(), 3..3, None);
        let inserted = State::new("A😀🦀Z".into(), 5..5, None);
        let insertion = initial.replacement(&inserted).unwrap();
        assert_eq!(insertion.range, 3..3);
        assert_eq!(insertion.text, "🦀");
        assert_eq!(inserted.selection, 5..5);

        let deleted = State::new("A🦀Z".into(), 1..1, None);
        let deletion = inserted.replacement(&deleted).unwrap();
        assert_eq!(deletion.range, 1..3);
        assert_eq!(deletion.text, "");
        assert_eq!(deleted.selection, 1..1);
    }

    #[test]
    fn malformed_spans_clamp_without_splitting_surrogates() {
        let state = State::new("a😀b".into(), usize::MAX..2, Some(2..1));
        assert_eq!(state.selection, 4..1);
        assert_eq!(state.composing, Some(1..3));
        assert_eq!(state.composing_text(), Some("😀"));
        let clamped = State::new("a😀b".into(), 2..usize::MAX, Some(2..usize::MAX));
        assert_eq!(clamped.selection, 1..4);
        assert_eq!(clamped.composing, Some(1..4));
        let empty = State::new("a😀b".into(), 2..2, Some(2..2));
        assert_eq!(empty.selection, 1..1);
        assert_eq!(empty.composing, None);
        let outside = State::new("a😀b".into(), 5..6, Some(5..usize::MAX));
        assert_eq!(outside.selection, 4..4);
        assert_eq!(outside.composing, None);
    }
    #[test]
    fn protected_document_uses_actual_text_not_display_mask() {
        let old = State::new("ab".into(), 1..1, None);
        let deleted = State::new("b".into(), 0..0, None);
        assert_eq!(old.replacement(&deleted).unwrap().range, 0..1);
        let emoji = State::new("a😀b".into(), 3..3, Some(1..3));
        let edit = old.replacement(&emoji).unwrap();
        assert_eq!(edit.range, 1..1);
        assert_eq!(edit.text, "😀");
        assert_eq!(emoji.composing_text(), Some("😀"));
        let selected_delete = emoji.replacement(&deleted).unwrap();
        assert_eq!(selected_delete.range, 0..3);
        assert_eq!(selected_delete.text, "");
    }
}
