//! One editable text field: its text, its selection, and every edit as a method on that
//! value.
//!
//! The dialog paints fields, places a caret in them and feeds them keys. Each of those
//! paths used to reach into a shared store, and an edit helper that reached back in while
//! the store was already borrowed aborted the whole app in release builds. Taking the field
//! as a value means an edit happens once, in place, and it can be tested without a window.
//!
//! Caret and anchor are char indices, never byte offsets, so an edit cannot split a
//! surrogate pair.

#[derive(Clone, Debug, Default)]
pub struct Field {
    pub text: String,
    pub caret: usize,
    pub anchor: usize,
}

impl Field {
    /// Selection as (start, end), caret-first order.
    pub fn selection(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    /// Selected text; `None` while the selection is collapsed.
    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection();
        (start != end).then(|| self.text.chars().skip(start).take(end - start).collect())
    }

    /// Replace the text, caret at the end, selection collapsed.
    pub fn set_text(&mut self, text: String) {
        self.text = text;
        self.caret = self.char_len();
        self.anchor = self.caret;
    }

    pub fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.char_len();
    }

    /// Select the word under a char index, for a double click.
    pub fn select_word_at(&mut self, at: usize) {
        let chars: Vec<char> = self.text.chars().collect();
        let mut start = at.min(chars.len());
        let mut end = start;
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while end < chars.len() && !chars[end].is_whitespace() {
            end += 1;
        }
        self.anchor = start;
        self.caret = end;
    }

    /// Replace the selection (or, collapsed, insert at the caret) with `insert`.
    pub fn insert(&mut self, insert: &str) {
        let (start, end) = self.selection();
        let (a, b) = (self.byte_index(start), self.byte_index(end));
        self.text.replace_range(a..b, insert);
        self.caret = start + insert.chars().count();
        self.anchor = self.caret;
    }

    /// Delete the selection, else `[a, b)`; the caret lands on the start. True when the
    /// text changed.
    pub fn delete_span(&mut self, a: usize, b: usize) -> bool {
        let (start, end) = self.selection();
        let (lo, hi) = if start != end { (start, end) } else { (a, b) };
        if lo >= hi {
            return false;
        }
        let (x, y) = (self.byte_index(lo), self.byte_index(hi));
        self.text.replace_range(x..y, "");
        self.caret = lo;
        self.anchor = lo;
        true
    }

    pub fn delete_selection(&mut self) -> bool {
        let (start, end) = self.selection();
        self.delete_span(start, end)
    }

    /// Backspace: the selection if there is one, else one char - or, with `word`, the word
    /// left of the caret.
    pub fn backspace(&mut self, word: bool) -> bool {
        let caret = self.caret;
        let from = if word {
            self.word_left(caret)
        } else {
            caret.saturating_sub(1)
        };
        self.delete_span(from, caret)
    }

    /// Delete forward: the selection if there is one, else one char - or, with `word`, the
    /// word right of the caret.
    pub fn delete_forward(&mut self, word: bool) -> bool {
        let caret = self.caret;
        let to = if word {
            self.word_right(caret)
        } else {
            (caret + 1).min(self.char_len())
        };
        self.delete_span(caret, to)
    }

    /// Start of the word left of `caret`: skips spaces, then the run of non-spaces.
    pub fn word_left(&self, caret: usize) -> usize {
        let head: Vec<char> = self.text.chars().take(caret).collect();
        let mut i = head.len();
        while i > 0 && head[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !head[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    /// End of the word right of `caret`, the way a native edit box lands it.
    pub fn word_right(&self, caret: usize) -> usize {
        let tail: Vec<char> = self.text.chars().skip(caret).collect();
        let mut i = 0;
        while i < tail.len() && tail[i].is_whitespace() {
            i += 1;
        }
        while i < tail.len() && !tail[i].is_whitespace() {
            i += 1;
        }
        caret + i
    }

    /// Byte offset of a char index; the end of the text when it is past the last char.
    fn byte_index(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map_or(self.text.len(), |(i, _)| i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A field holding `text`, caret at the end, selection collapsed.
    fn field(text: &str) -> Field {
        let mut field = Field::default();
        field.set_text(text.to_string());
        field
    }

    /// The caret at `at`, selection collapsed.
    fn caret_at(mut field: Field, at: usize) -> Field {
        field.caret = at;
        field.anchor = at;
        field
    }

    #[test]
    fn typing_inserts_at_the_caret_and_lands_after_it() {
        let mut field = caret_at(field("ab"), 1);
        field.insert("X");
        assert_eq!(field.text, "aXb");
        assert_eq!(field.caret, 2);
        assert_eq!(field.anchor, 2);
    }

    #[test]
    fn typing_replaces_the_selection() {
        let mut field = field("hello world");
        field.anchor = 6;
        field.caret = 11;
        field.insert("there");
        assert_eq!(field.text, "hello there");
        assert_eq!(field.selection(), (11, 11));
        assert_eq!(field.selected_text(), None);
    }

    #[test]
    fn backspace_and_delete_take_one_char_or_a_word() {
        let mut back = field("one two");
        assert!(back.backspace(false));
        assert_eq!(back.text, "one tw");
        assert!(back.backspace(true));
        assert_eq!(back.text, "one ");

        let mut forward = caret_at(field("one two"), 4);
        assert!(forward.delete_forward(true));
        assert_eq!(forward.text, "one ");

        // Nothing to delete at either end.
        let mut empty = caret_at(field("one"), 0);
        assert!(!empty.backspace(false));
        let mut end = field("one");
        assert!(!end.delete_forward(false));
    }

    /// Char indices, not bytes: a surrogate pair is one caret step and cannot be split.
    #[test]
    fn a_surrogate_pair_is_one_char() {
        let mut field = caret_at(field("a\u{1F3B5}b"), 2);
        assert_eq!(field.char_len(), 3);
        assert!(field.backspace(false));
        assert_eq!(field.text, "ab");
    }

    #[test]
    fn a_double_click_selects_the_word_under_the_pointer() {
        let mut field = field("alpha beta gamma");
        field.select_word_at(8);
        assert_eq!(field.selected_text().as_deref(), Some("beta"));
        field.select_all();
        assert_eq!(field.selected_text().as_deref(), Some("alpha beta gamma"));
    }

    #[test]
    fn word_moves_skip_spaces_and_then_the_word() {
        let field = field("alpha beta  ");
        assert_eq!(field.word_left(10), 6);
        assert_eq!(field.word_left(6), 0);
        assert_eq!(field.word_right(0), 5);
        assert_eq!(field.word_right(5), 10);
    }
}
