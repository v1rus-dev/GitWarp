//! The pure single-line editing model behind [`super::TextInput`].

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

/// Text, selection and marked range of a single-line input.
///
/// Offsets are UTF-8 byte offsets; the selection is always normalised so that
/// `start <= end`, with `reversed` recording which end holds the cursor.
#[derive(Clone, Debug, Default)]
pub(crate) struct EditModel {
    pub(super) text: String,
    pub(super) selection: Range<usize>,
    pub(super) reversed: bool,
    pub(super) marked: Option<Range<usize>>,
    pub(super) max_len: Option<usize>,
}

impl EditModel {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        let end = self.text.len();
        self.selection = end..end;
        self.reversed = false;
        self.marked = None;
    }

    pub(super) fn cursor(&self) -> usize {
        if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }

    pub(super) fn move_to(&mut self, offset: usize) {
        let offset = offset.min(self.text.len());
        self.selection = offset..offset;
        self.reversed = false;
    }

    pub(super) fn select_to(&mut self, offset: usize) {
        let offset = offset.min(self.text.len());
        if self.reversed {
            self.selection.start = offset;
        } else {
            self.selection.end = offset;
        }
        if self.selection.end < self.selection.start {
            self.reversed = !self.reversed;
            self.selection = self.selection.end..self.selection.start;
        }
    }

    pub(super) fn select_all(&mut self) {
        self.selection = 0..self.text.len();
        self.reversed = false;
    }

    /// Replaces `range` with `text`, enforcing `max_len`; returns the inserted range.
    pub(super) fn replace_range(&mut self, range: Range<usize>, text: &str) -> Range<usize> {
        let start = range.start.min(self.text.len());
        let end = range.end.min(self.text.len()).max(start);
        let range = start..end;

        let existing = self.text.chars().count() - self.text[range.clone()].chars().count();
        let available = self
            .max_len
            .map_or(usize::MAX, |max_len| max_len.saturating_sub(existing));
        let limited: String = if text.chars().count() > available {
            text.chars().take(available).collect()
        } else {
            text.to_owned()
        };

        self.text.replace_range(range.clone(), &limited);
        let inserted = range.start..range.start + limited.len();
        self.selection = inserted.end..inserted.end;
        self.reversed = false;
        self.marked = None;
        inserted
    }

    pub(super) fn insert(&mut self, text: &str) -> Range<usize> {
        self.replace_range(self.selection.clone(), text)
    }

    pub(super) fn set_marked(&mut self, range: Option<Range<usize>>) {
        self.marked = range.map(|range| {
            let start = range.start.min(self.text.len());
            let end = range.end.min(self.text.len()).max(start);
            start..end
        });
    }

    pub(super) fn backspace(&mut self) {
        if self.selection.is_empty() {
            let end = self.cursor();
            let start = self.previous_grapheme(end);
            self.replace_range(start..end, "");
        } else {
            self.replace_range(self.selection.clone(), "");
        }
    }

    pub(super) fn delete(&mut self) {
        if self.selection.is_empty() {
            let start = self.cursor();
            let end = self.next_grapheme(start);
            self.replace_range(start..end, "");
        } else {
            self.replace_range(self.selection.clone(), "");
        }
    }

    pub(super) fn delete_word_left(&mut self) {
        if self.selection.is_empty() {
            let end = self.cursor();
            let start = self.previous_word(end);
            self.replace_range(start..end, "");
        } else {
            self.replace_range(self.selection.clone(), "");
        }
    }

    pub(super) fn delete_word_right(&mut self) {
        if self.selection.is_empty() {
            let start = self.cursor();
            let end = self.next_word(start);
            self.replace_range(start..end, "");
        } else {
            self.replace_range(self.selection.clone(), "");
        }
    }

    pub(super) fn move_left(&mut self) {
        if self.selection.is_empty() {
            self.move_to(self.previous_grapheme(self.cursor()));
        } else {
            self.move_to(self.selection.start);
        }
    }

    pub(super) fn move_right(&mut self) {
        if self.selection.is_empty() {
            self.move_to(self.next_grapheme(self.cursor()));
        } else {
            self.move_to(self.selection.end);
        }
    }

    pub(super) fn move_word_left(&mut self) {
        if self.selection.is_empty() {
            self.move_to(self.previous_word(self.cursor()));
        } else {
            self.move_to(self.selection.start);
        }
    }

    pub(super) fn move_word_right(&mut self) {
        if self.selection.is_empty() {
            self.move_to(self.next_word(self.cursor()));
        } else {
            self.move_to(self.selection.end);
        }
    }

    pub(super) fn move_home(&mut self) {
        self.move_to(0);
    }

    pub(super) fn move_end(&mut self) {
        self.move_to(self.text.len());
    }

    pub(super) fn select_left(&mut self) {
        self.select_to(self.previous_grapheme(self.cursor()));
    }

    pub(super) fn select_right(&mut self) {
        self.select_to(self.next_grapheme(self.cursor()));
    }

    pub(super) fn select_word_left(&mut self) {
        self.select_to(self.previous_word(self.cursor()));
    }

    pub(super) fn select_word_right(&mut self) {
        self.select_to(self.next_word(self.cursor()));
    }

    pub(super) fn select_home(&mut self) {
        self.select_to(0);
    }

    pub(super) fn select_end(&mut self) {
        self.select_to(self.text.len());
    }

    pub(super) fn previous_grapheme(&self, offset: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    pub(super) fn next_grapheme(&self, offset: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.text.len())
    }

    /// Next non-whitespace word segment start, or the end of the text.
    pub(super) fn next_word(&self, offset: usize) -> usize {
        self.word_starts()
            .into_iter()
            .find(|start| *start > offset)
            .unwrap_or(self.text.len())
    }

    /// Previous non-whitespace word segment start, or the beginning.
    pub(super) fn previous_word(&self, offset: usize) -> usize {
        self.word_starts()
            .into_iter()
            .rev()
            .find(|start| *start < offset)
            .unwrap_or(0)
    }

    fn word_starts(&self) -> Vec<usize> {
        self.text
            .split_word_bound_indices()
            .filter(|(index, word)| {
                !word.trim().is_empty()
                    && self.text[..*index]
                        .chars()
                        .next_back()
                        .is_none_or(char::is_whitespace)
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// The word range around `offset`, used by double-click.
    pub(super) fn word_range(&self, offset: usize) -> Range<usize> {
        let starts = self.word_starts();
        let start = starts
            .iter()
            .copied()
            .take_while(|start| *start <= offset)
            .last()
            .unwrap_or(0);
        let end = self.text[start..]
            .find(char::is_whitespace)
            .map(|rel| start + rel)
            .unwrap_or(self.text.len());
        start..end
    }

    pub(super) fn offset_to_utf16(&self, offset: usize) -> usize {
        let offset = offset.min(self.text.len());
        self.text[..offset].chars().map(char::len_utf16).sum()
    }

    pub(super) fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf16 = 0;
        let mut utf8 = 0;
        for ch in self.text.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }

    pub(super) fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    pub(super) fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(text: &str) -> EditModel {
        let mut model = EditModel::new();
        model.set_text(text);
        model
    }

    #[test]
    fn ascii_editing_and_selection() {
        let mut m = model("hello");
        m.move_left();
        m.move_left();
        m.backspace();
        assert_eq!(m.text, "helo");
        assert_eq!(m.cursor(), 2);

        m.delete();
        assert_eq!(m.text, "heo");

        m.select_all();
        m.insert("world");
        assert_eq!(m.text, "world");
        assert_eq!(m.selection, 5..5);
    }

    #[test]
    fn cyrillic_moves_by_grapheme() {
        let mut m = model("привет");
        m.move_left();
        assert_eq!(m.cursor(), 10);
        m.backspace();
        assert_eq!(m.text, "привт");
    }

    #[test]
    fn combining_marks_are_one_grapheme() {
        let mut m = model("e\u{301}");
        assert_eq!(m.text.len(), 3);
        m.move_left();
        assert_eq!(m.cursor(), 0);
        m.move_end();
        m.backspace();
        assert!(m.text.is_empty());
    }

    #[test]
    fn emoji_zwj_sequence_is_one_grapheme() {
        let mut m = model("👩\u{200d}💻");
        assert_eq!(m.text.len(), 11);
        m.move_left();
        assert_eq!(m.cursor(), 0);
        m.move_end();
        m.backspace();
        assert!(m.text.is_empty());
    }

    #[test]
    fn word_moves_skip_whitespace() {
        let mut m = model("git push --force origin");
        m.move_home();
        m.move_word_right();
        assert_eq!(m.cursor(), 4);
        m.move_word_right();
        assert_eq!(m.cursor(), 9);
        m.move_word_right();
        assert_eq!(m.cursor(), 17);
        m.move_word_right();
        assert_eq!(m.cursor(), 23);

        m.move_word_left();
        assert_eq!(m.cursor(), 17);
        m.move_word_left();
        assert_eq!(m.cursor(), 9);
        m.move_word_left();
        assert_eq!(m.cursor(), 4);
        m.move_word_left();
        assert_eq!(m.cursor(), 0);
    }

    #[test]
    fn delete_word_left_and_right() {
        let mut m = model("git push --force origin");
        m.move_end();
        m.delete_word_left();
        assert_eq!(m.text, "git push --force ");
        m.delete_word_left();
        assert_eq!(m.text, "git push ");

        m.move_home();
        m.delete_word_right();
        assert_eq!(m.text, "push ");
    }

    #[test]
    fn utf16_round_trip() {
        let m = model("aé👩\u{200d}💻z");
        for (offset, _) in m.text.char_indices().chain([(m.text.len(), ' ')]) {
            let utf16 = m.offset_to_utf16(offset);
            assert_eq!(m.offset_from_utf16(utf16), offset);
        }
    }

    #[test]
    fn max_len_counts_chars_and_truncates() {
        let mut m = model("");
        m.max_len = Some(5);
        m.insert("hello world");
        assert_eq!(m.text, "hello");

        m.select_all();
        m.insert("αβγδεζη");
        assert_eq!(m.text, "αβγδε");
    }

    #[test]
    fn replacing_a_selection_keeps_max_len() {
        let mut m = model("hello world");
        m.max_len = Some(11);
        m.selection = 6..11;
        m.insert("there");
        assert_eq!(m.text, "hello there");
        assert_eq!(m.selection, 11..11);
    }

    #[test]
    fn word_range_for_double_click() {
        let m = model("git push --force origin");
        assert_eq!(m.word_range(6), 4..8);
        assert_eq!(m.word_range(13), 9..16);
    }
}
