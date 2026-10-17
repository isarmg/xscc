use std::ops::Range;

#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct State {
    pub text: String,
    pub selection: Range<usize>,
    pub composing: Option<Range<usize>>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Replacement {
    pub range: Range<usize>,
    pub text: String,
    pub result: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Edit {
    Mark(Replacement),
    Commit(Replacement),
    Unmark,
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

    pub fn edits(&self, incoming: &Self) -> Vec<Edit> {
        if self == incoming {
            return Vec::new();
        }
        let replacement = self.replacement(incoming);
        if let Some(marked) = &incoming.composing {
            let old_len = self.text.encode_utf16().count();
            let new_len = incoming.text.encode_utf16().count();
            let old_end = marked
                .end
                .checked_add(old_len)
                .and_then(|end| end.checked_sub(new_len));
            let contains_edit = replacement.as_ref().is_none_or(|edit| {
                marked.start <= edit.range.start
                    && marked.end >= edit.range.start + edit.text.encode_utf16().count()
            });
            if let Some(old_end) = old_end.filter(|end| *end >= marked.start && *end <= old_len)
                && contains_edit
                && self
                    .composing
                    .as_ref()
                    .is_none_or(|old| old.start >= marked.start && old.end <= old_end)
            {
                // No committed text changed outside the incoming composition.
                return vec![Edit::Mark(Replacement {
                    range: marked.start..old_end,
                    text: incoming.composing_text().unwrap_or_default().to_owned(),
                    result: incoming.text.clone(),
                })];
            }
            // One snapshot can commit a word and start the next composition. Apply only
            // its committed document first; never expose the next preedit as a commit.
            let mut committed = incoming.clone();
            committed.text.replace_range(
                utf16_byte_offset(&incoming.text, marked.start)
                    ..utf16_byte_offset(&incoming.text, marked.end),
                "",
            );
            committed.composing = None;
            let mut edits = match self.commit_replacement(&committed) {
                Some(edit) => vec![Edit::Commit(edit)],
                None if self.composing.is_some() => vec![Edit::Unmark],
                None => Vec::new(),
            };
            edits.push(Edit::Mark(Replacement {
                range: marked.start..marked.start,
                text: incoming.composing_text().unwrap_or_default().to_owned(),
                result: incoming.text.clone(),
            }));
            return edits;
        }
        if replacement.is_some() {
            if let Some(marked) = &self.composing {
                let mut without_preedit = self.text.clone();
                without_preedit.replace_range(
                    utf16_byte_offset(&self.text, marked.start)
                        ..utf16_byte_offset(&self.text, marked.end),
                    "",
                );
                if without_preedit == incoming.text {
                    return vec![
                        Edit::Mark(Replacement {
                            range: marked.clone(),
                            text: String::new(),
                            result: incoming.text.clone(),
                        }),
                        Edit::Unmark,
                    ];
                }
            }
            self.commit_replacement(incoming)
                .map(Edit::Commit)
                .into_iter()
                .collect()
        } else if self.composing.is_some() {
            vec![Edit::Unmark]
        } else {
            Vec::new()
        }
    }

    fn commit_replacement(&self, incoming: &Self) -> Option<Replacement> {
        let mut edit = self.replacement(incoming)?;
        if let Some(marked) = &self.composing {
            // A commit must finalize the entire old preedit, including unchanged characters.
            let start = edit.range.start.min(marked.start);
            let end = edit.range.end.max(marked.end);
            let new_end =
                end - (edit.range.end - edit.range.start) + edit.text.encode_utf16().count();
            edit.range = start..end;
            edit.text = incoming.text[utf16_byte_offset(&incoming.text, start)
                ..utf16_byte_offset(&incoming.text, new_end)]
                .to_owned();
        }
        Some(edit)
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
            result: incoming.text.clone(),
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
    use super::{Edit, State};

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
    #[test]
    fn preedit_updates_never_issue_commit_and_final_text_commits_once() {
        let empty = State::default();
        let n = State::new("n".into(), 1..1, Some(0..1));
        let ni = State::new("ni".into(), 2..2, Some(0..2));
        for (old, new) in [(&empty, &n), (&n, &ni)] {
            let [Edit::Mark(edit)]: [Edit; 1] = old.edits(new).try_into().unwrap() else {
                panic!("preedit must only mark");
            };
            assert_eq!(edit.text, new.text);
            assert_eq!(edit.result, new.text);
        }
        let final_text = State::new("你".into(), 1..1, None);
        let [Edit::Commit(edit)]: [Edit; 1] = ni.edits(&final_text).try_into().unwrap() else {
            panic!("commit expected");
        };
        assert_eq!(edit.range, 0..2);
        assert_eq!(edit.text, "你");
        assert!(final_text.edits(&final_text).is_empty());
        assert_eq!(
            n.edits(&State::new("n".into(), 1..1, None)),
            vec![Edit::Unmark]
        );
    }

    #[test]
    fn cancelled_and_mixed_composition_never_commit_pending_text() {
        let old = State::new("aa".into(), 1..1, Some(0..1));
        let cancelled = State::new("a".into(), 0..0, None);
        let [Edit::Mark(edit), Edit::Unmark]: [Edit; 2] = old.edits(&cancelled).try_into().unwrap()
        else {
            panic!("cancellation must erase preedit");
        };
        assert_eq!(edit.range, 0..1);
        assert!(edit.text.is_empty());
        let old = State::new("ni".into(), 2..2, Some(0..2));
        let mixed = State::new("你h".into(), 2..2, Some(1..2));
        let [Edit::Commit(commit), Edit::Mark(mark)]: [Edit; 2] =
            old.edits(&mixed).try_into().unwrap()
        else {
            panic!("commit then new preedit");
        };
        assert_eq!(
            (commit.range, commit.text, commit.result),
            (0..2, "你".into(), "你".into())
        );
        assert_eq!(
            (mark.range, mark.text, mark.result),
            (1..1, "h".into(), "你h".into())
        );
    }

    #[test]
    fn marked_emoji_and_selection_only_changes_preserve_utf16_contract() {
        let old = State::new("A😀Z".into(), 3..3, Some(1..3));
        let selected = State::new("A😀Z".into(), 1..3, Some(1..3));
        let [Edit::Mark(edit)]: [Edit; 1] = old.edits(&selected).try_into().unwrap() else {
            panic!("mark expected");
        };
        assert_eq!(edit.range, 1..3);
        assert_eq!(edit.text, "😀");
        let deleted = State::new("AZ".into(), 1..1, None);
        let [Edit::Mark(edit), Edit::Unmark]: [Edit; 2] = old.edits(&deleted).try_into().unwrap()
        else {
            panic!("cancel expected");
        };
        assert_eq!(edit.range, 1..3);
        assert!(edit.text.is_empty());
        let plain = State::new("A😀Z".into(), 3..3, None);
        let [Edit::Commit(edit)]: [Edit; 1] = plain.edits(&deleted).try_into().unwrap() else {
            panic!("committed deletion expected");
        };
        assert_eq!(edit.range, 1..3);
        assert!(
            plain
                .edits(&State::new("A😀Z".into(), 1..3, None))
                .is_empty()
        );
    }
}
