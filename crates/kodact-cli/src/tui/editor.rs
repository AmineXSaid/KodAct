//! The prompt's text editor: a multiline buffer with a cursor, word moves,
//! kill commands and prompt history.

use unicode_width::UnicodeWidthChar;

const MAX_UNDO: usize = 200;

/// Split one logical line of displayed characters into visual rows of at most `w`
/// display columns. Returns each row's `(start, end)` character index range.
pub fn char_rows(chars: &[char], w: usize) -> Vec<(usize, usize)> {
    let w = w.max(1);
    let mut rows = vec![];
    let mut start = 0;
    let mut used = 0;
    for (i, c) in chars.iter().enumerate() {
        let cw = c.width().unwrap_or(0);
        if used + cw > w && i > start {
            rows.push((start, i));
            start = i;
            used = 0;
        }
        used += cw;
    }
    rows.push((start, chars.len()));
    rows
}

/// Text being typed, as characters, with the cursor between two of them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Editor {
    text: Vec<char>,
    /// Index into `text` (0..=len).
    cursor: usize,
    /// Earlier prompts, oldest first.
    history: Vec<String>,
    /// While browsing history: the entry shown, and what was typed before browsing.
    browsing: Option<(usize, String)>,
    /// Undo/redo snapshots `(text, cursor)`.
    undo_stack: Vec<(Vec<char>, usize)>,
    redo_stack: Vec<(Vec<char>, usize)>,
    /// Last killed text (`Ctrl+K`, `Ctrl+U`, `Ctrl+W`) for `Ctrl+Y` yank.
    kill_ring: String,
    /// Preserved visual display column across consecutive `up_wrapped` / `down_wrapped` moves.
    preferred_col: Option<usize>,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Editor {
    pub fn with_history(history: Vec<String>) -> Self {
        Editor { history, ..Default::default() }
    }

    fn save_undo(&mut self) {
        self.preferred_col = None;
        if self.undo_stack.last().is_some_and(|(t, c)| t == &self.text && *c == self.cursor) {
            return;
        }
        if self.undo_stack.len() >= MAX_UNDO {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push((self.text.clone(), self.cursor));
        self.redo_stack.clear();
    }

    /// Undo the last edit (`Ctrl+Z`).
    pub fn undo(&mut self) -> bool {
        let Some((text, cursor)) = self.undo_stack.pop() else { return false };
        self.redo_stack.push((std::mem::replace(&mut self.text, text), self.cursor));
        self.cursor = cursor.min(self.text.len());
        self.browsing = None;
        true
    }

    /// Redo the last undone edit (`Ctrl+Y` when redo history exists, or `Ctrl+Shift+Z`).
    pub fn redo(&mut self) -> bool {
        let Some((text, cursor)) = self.redo_stack.pop() else { return false };
        self.undo_stack.push((std::mem::replace(&mut self.text, text), self.cursor));
        self.cursor = cursor.min(self.text.len());
        self.browsing = None;
        true
    }

    /// Paste the last killed text (`Ctrl+K` / `Ctrl+U` / `Ctrl+W`).
    pub fn yank(&mut self) -> bool {
        if self.kill_ring.is_empty() {
            return false;
        }
        let killed = self.kill_ring.clone();
        self.insert(&killed);
        true
    }

    pub fn text(&self) -> String {
        self.text.iter().collect()
    }

    /// The text with `▏` marking the cursor position.
    pub fn text_with_cursor(&self) -> String {
        let at = self.cursor.min(self.text.len());
        let before: String = self.text[..at].iter().collect();
        let after: String = self.text[at..].iter().collect();
        format!("{before}▏{after}")
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    #[cfg(test)]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn set(&mut self, text: &str) {
        let next: Vec<char> = text.chars().collect();
        if next != self.text {
            self.save_undo();
        }
        self.text = next;
        self.cursor = self.text.len();
        self.browsing = None;
    }

    pub fn clear(&mut self) {
        self.set("");
    }

    /// Take the text (Enter): it joins the history unless it repeats the last entry.
    pub fn take(&mut self) -> String {
        let t = self.text();
        if !t.trim().is_empty() && self.history.last() != Some(&t) {
            self.history.push(t.clone());
        }
        self.text.clear();
        self.cursor = 0;
        self.browsing = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        t
    }

    pub fn history(&self) -> &[String] {
        &self.history
    }

    pub fn insert(&mut self, s: &str) {
        // Pasted text may carry carriage returns.
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        if s.is_empty() {
            return;
        }
        self.save_undo();
        for c in s.chars() {
            self.text.insert(self.cursor, c);
            self.cursor += 1;
        }
        self.browsing = None;
    }

    /// Replace the `n` characters before the cursor with `with` (a completion).
    pub fn replace_back(&mut self, n: usize, with: &str) {
        self.save_undo();
        let from = self.cursor.saturating_sub(n);
        self.text.drain(from..self.cursor);
        self.cursor = from;
        let s = with.replace("\r\n", "\n").replace('\r', "\n");
        for c in s.chars() {
            self.text.insert(self.cursor, c);
            self.cursor += 1;
        }
        self.browsing = None;
    }

    /// The word before the cursor: its start and its text (up to whitespace).
    pub fn word_before_cursor(&self) -> (usize, String) {
        let start = self.text[..self.cursor].iter().rposition(|c| c.is_whitespace()).map(|i| i + 1).unwrap_or(0);
        (start, self.text[start..self.cursor].iter().collect())
    }

    /// The character immediately after the cursor, if any.
    pub fn char_after_cursor(&self) -> Option<char> {
        self.text.get(self.cursor).copied()
    }

    /// Whether the word before the cursor is the only non-whitespace token in the buffer.
    pub fn word_is_sole_prompt(&self) -> bool {
        let (start, _) = self.word_before_cursor();
        self.text[..start].iter().all(|c| c.is_whitespace())
            && self.text[self.cursor..].iter().all(|c| c.is_whitespace())
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.save_undo();
            self.cursor -= 1;
            self.text.remove(self.cursor);
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.text.len() {
            self.save_undo();
            self.text.remove(self.cursor);
        }
    }

    pub fn left(&mut self) {
        self.preferred_col = None;
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.preferred_col = None;
        self.cursor = (self.cursor + 1).min(self.text.len());
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].iter().rposition(|c| *c == '\n').map(|i| i + 1).unwrap_or(0)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..].iter().position(|c| *c == '\n').map(|i| self.cursor + i).unwrap_or(self.text.len())
    }

    pub fn home(&mut self) {
        self.preferred_col = None;
        self.cursor = self.line_start();
    }

    pub fn end(&mut self) {
        self.preferred_col = None;
        self.cursor = self.line_end();
    }

    fn word_left_index(&self) -> usize {
        let mut i = self.cursor;
        while i > 0 && !is_word(self.text[i - 1]) {
            i -= 1;
        }
        while i > 0 && is_word(self.text[i - 1]) {
            i -= 1;
        }
        i
    }

    pub fn word_left(&mut self) {
        self.preferred_col = None;
        self.cursor = self.word_left_index();
    }

    pub fn word_right(&mut self) {
        self.preferred_col = None;
        let n = self.text.len();
        let mut i = self.cursor;
        while i < n && !is_word(self.text[i]) {
            i += 1;
        }
        while i < n && is_word(self.text[i]) {
            i += 1;
        }
        self.cursor = i;
    }

    /// Ctrl+W: delete the word before the cursor.
    pub fn delete_word(&mut self) {
        let from = self.word_left_index();
        if from < self.cursor {
            self.save_undo();
            self.kill_ring = self.text.drain(from..self.cursor).collect();
            self.cursor = from;
        }
    }

    /// Ctrl+U: delete to the start of the line.
    pub fn kill_to_start(&mut self) {
        let from = self.line_start();
        if from < self.cursor {
            self.save_undo();
            self.kill_ring = self.text.drain(from..self.cursor).collect();
            self.cursor = from;
        }
    }

    /// Ctrl+K: delete to the end of the line.
    pub fn kill_to_end(&mut self) {
        let to = self.line_end();
        if self.cursor < to {
            self.save_undo();
            self.kill_ring = self.text.drain(self.cursor..to).collect();
        }
    }

    /// The cursor's line and column (in characters).
    pub fn position(&self) -> (usize, usize) {
        let before = &self.text[..self.cursor];
        let row = before.iter().filter(|c| **c == '\n').count();
        (row, self.cursor - self.line_start())
    }

    pub fn lines(&self) -> usize {
        self.text.iter().filter(|c| **c == '\n').count() + 1
    }

    pub fn is_browsing(&self) -> bool {
        self.browsing.is_some()
    }

    pub fn has_line_above(&self) -> bool {
        self.position().0 > 0
    }

    pub fn has_line_below(&self) -> bool {
        self.position().0 + 1 < self.lines()
    }

    /// Visual row slices `(start_char_idx, end_char_idx)` into `self.text` for a given
    /// inner display column width. Each `\n`-delimited logical line is wrapped using
    /// [`char_rows`] on `super::text::shown` characters so navigation matches rendering.
    pub fn visual_rows(&self, width: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut line_start = 0;
        while line_start <= self.text.len() {
            let line_end = self.text[line_start..]
                .iter()
                .position(|c| *c == '\n')
                .map(|i| line_start + i)
                .unwrap_or(self.text.len());
            let shown: Vec<char> = self.text[line_start..line_end].iter().map(|c| super::text::shown(*c)).collect();
            for (a, b) in char_rows(&shown, width) {
                out.push((line_start + a, line_start + b));
            }
            if line_end == self.text.len() {
                break;
            }
            line_start = line_end + 1;
        }
        out
    }

    /// Display width of `self.text[start..end]` as drawn in the input box.
    fn display_width(&self, start: usize, end: usize) -> usize {
        self.text[start..end].iter().map(|c| super::text::shown(*c).width().unwrap_or(0)).sum()
    }

    /// The cursor's `(visual_row, display_col)` at `width`.
    #[cfg(test)]
    pub fn visual_position(&self, width: usize) -> (usize, usize) {
        let rows = self.visual_rows(width);
        let row_idx = self.visual_row_index(&rows);
        let (a, _) = rows[row_idx];
        (row_idx, self.display_width(a, self.cursor))
    }

    fn visual_row_index(&self, rows: &[(usize, usize)]) -> usize {
        for (i, (a, b)) in rows.iter().enumerate() {
            let next_same_line = rows.get(i + 1).is_some_and(|(na, _)| *na == *b);
            if self.cursor >= *a && (self.cursor < *b || (self.cursor == *b && !next_same_line)) {
                return i;
            }
        }
        rows.len().saturating_sub(1)
    }

    /// Place `self.cursor` on visual row `(a, b)` at the character closest to `target_col`
    /// without exceeding it.
    fn cursor_at_col(&self, a: usize, b: usize, target_col: usize, is_last_in_line: bool) -> usize {
        let mut col = 0;
        let limit = if is_last_in_line { b } else { b.saturating_sub(1).max(a) };
        for (offset, &ch) in self.text[a..limit].iter().enumerate() {
            let cw = super::text::shown(ch).width().unwrap_or(0);
            if col + cw > target_col {
                return a + offset;
            }
            col += cw;
        }
        limit
    }

    /// Up: move to the visual row above (at `width` columns), or the previous prompt
    /// from history when already on the first visual row.
    pub fn up_wrapped(&mut self, width: usize) -> bool {
        let rows = self.visual_rows(width);
        let row_idx = self.visual_row_index(&rows);
        if row_idx > 0 {
            let (cur_a, _) = rows[row_idx];
            let target_col = self.preferred_col.unwrap_or_else(|| self.display_width(cur_a, self.cursor));
            self.preferred_col = Some(target_col);
            let (prev_a, prev_b) = rows[row_idx - 1];
            let prev_is_last_in_line = rows[row_idx].0 != prev_b;
            self.cursor = self.cursor_at_col(prev_a, prev_b, target_col, prev_is_last_in_line);
            return true;
        }
        self.preferred_col = None;
        let next = match &self.browsing {
            Some((i, _)) if *i > 0 => i - 1,
            Some(_) => return false,
            None if self.history.is_empty() => return false,
            None => self.history.len() - 1,
        };
        let draft = self.browsing.take().map(|(_, d)| d).unwrap_or_else(|| self.text());
        self.text = self.history[next].chars().collect();
        self.cursor = self.text.len();
        self.browsing = Some((next, draft));
        true
    }

    /// Down: move to the visual row below (at `width` columns), or the next prompt
    /// from history when already on the last visual row.
    pub fn down_wrapped(&mut self, width: usize) -> bool {
        let rows = self.visual_rows(width);
        let row_idx = self.visual_row_index(&rows);
        if row_idx + 1 < rows.len() {
            let (cur_a, _) = rows[row_idx];
            let target_col = self.preferred_col.unwrap_or_else(|| self.display_width(cur_a, self.cursor));
            self.preferred_col = Some(target_col);
            let (next_a, next_b) = rows[row_idx + 1];
            let next_is_last_in_line = rows.get(row_idx + 2).is_none_or(|(na, _)| *na != next_b);
            self.cursor = self.cursor_at_col(next_a, next_b, target_col, next_is_last_in_line);
            return true;
        }
        self.preferred_col = None;
        let Some((i, draft)) = self.browsing.take() else { return false };
        let shown = if i + 1 < self.history.len() {
            self.browsing = Some((i + 1, draft));
            self.history[i + 1].clone()
        } else {
            draft
        };
        self.text = shown.chars().collect();
        self.cursor = self.text.len();
        true
    }

    /// Up: the line above, or the previous prompt from the first line.
    /// Returns false when nothing changed.
    #[cfg(test)]
    pub fn up(&mut self) -> bool {
        self.up_wrapped(usize::MAX)
    }

    /// Down: the line below, or the next prompt from history (then what was being typed).
    #[cfg(test)]
    pub fn down(&mut self) -> bool {
        self.down_wrapped(usize::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_and_moves_by_words_and_lines() {
        let mut e = Editor::default();
        e.insert("hello world");
        e.word_left();
        assert_eq!(e.cursor(), 6);
        e.insert("big ");
        assert_eq!(e.text(), "hello big world");
        e.end();
        e.delete_word();
        assert_eq!(e.text(), "hello big ");
        e.home();
        e.kill_to_end();
        assert!(e.is_empty());
        e.insert("one\r\ntwo");
        assert_eq!(e.text(), "one\ntwo");
        assert_eq!(e.position(), (1, 3));
        assert!(e.up());
        assert_eq!(e.position(), (0, 3));
        e.left();
        e.kill_to_start();
        assert_eq!(e.text(), "e\ntwo");
        e.word_right();
        assert_eq!(e.cursor(), 1);
        e.backspace();
        e.delete();
        assert_eq!(e.text(), "two");
    }

    #[test]
    fn history_comes_back_with_up_and_down() {
        let mut e = Editor::with_history(vec!["first".into(), "second".into()]);
        e.insert("draft");
        assert!(e.up());
        assert_eq!(e.text(), "second");
        assert!(e.up());
        assert_eq!(e.text(), "first");
        assert!(!e.up(), "nothing older");
        assert!(e.down());
        assert_eq!(e.text(), "second");
        assert!(e.down());
        assert_eq!(e.text(), "draft", "back to what was being typed");
        assert!(!e.down());
        assert_eq!(e.take(), "draft");
        assert_eq!(e.history(), ["first", "second", "draft"]);
        e.insert("draft");
        e.take();
        assert_eq!(e.history().len(), 3, "a repeat isn't stored twice");
    }

    #[test]
    fn undo_redo_and_yank_restore_edits() {
        let mut e = Editor::default();
        e.insert("hello ");
        e.insert("world");
        assert_eq!(e.text(), "hello world");
        assert!(e.undo());
        assert_eq!(e.text(), "hello ");
        assert!(e.redo());
        assert_eq!(e.text(), "hello world");
        assert!(!e.redo());

        // Kill-ring yank restores killed text.
        e.home();
        e.kill_to_end();
        assert!(e.is_empty());
        assert!(e.undo());
        assert_eq!(e.text(), "hello world");
        e.end();
        e.word_left();
        e.kill_to_end();
        assert_eq!(e.text(), "hello ");
        assert!(e.yank());
        assert_eq!(e.text(), "hello world");
    }

    #[test]
    fn sole_prompt_and_char_after_cursor_helpers() {
        let mut e = Editor::default();
        e.insert("  /stat");
        assert!(e.word_is_sole_prompt());
        assert_eq!(e.char_after_cursor(), None);

        e.set("check /sim and more");
        e.cursor = 10; // right after `/sim`
        assert!(!e.word_is_sole_prompt());
        assert_eq!(e.char_after_cursor(), Some(' '));

        e.set("line1\n/sim");
        assert!(!e.word_is_sole_prompt());
    }

    #[test]
    fn wrapped_up_down_traverses_visual_rows_before_history() {
        let mut e = Editor::with_history(vec!["older prompt".into()]);
        // 15 chars at width 5 -> 3 visual rows: "01234", "56789", "abcde"
        e.set("0123456789abcde");
        assert_eq!(e.visual_rows(5), vec![(0, 5), (5, 10), (10, 15)]);
        assert_eq!(e.visual_position(5), (2, 5));

        // Up from end of row 2 moves to row 1, NOT history.
        assert!(e.up_wrapped(5));
        assert_eq!(e.text(), "0123456789abcde");
        assert_eq!(e.visual_position(5).0, 1);

        // Up again moves to row 0.
        assert!(e.up_wrapped(5));
        assert_eq!(e.visual_position(5).0, 0);

        // Up from row 0 now falls back to history.
        assert!(e.up_wrapped(5));
        assert_eq!(e.text(), "older prompt");

        // Down restores draft.
        assert!(e.down_wrapped(5));
        assert_eq!(e.text(), "0123456789abcde");

        // Preferred column is preserved across a short line.
        e.set("0123456789\nx\nabcdefghij");
        e.cursor = 21; // col 8 on "abcdefghij"
        assert_eq!(e.visual_position(20), (2, 8));
        assert!(e.up_wrapped(20)); // lands on "x" (col 1)
        assert_eq!(e.visual_position(20), (1, 1));
        assert!(e.up_wrapped(20)); // snaps back to col 8 on "0123456789"
        assert_eq!(e.visual_position(20), (0, 8));
    }
}
