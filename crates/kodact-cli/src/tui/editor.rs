//! The prompt's text editor: a multiline buffer with a cursor, word moves,
//! kill commands and prompt history.

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
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Editor {
    pub fn with_history(history: Vec<String>) -> Self {
        Editor { history, ..Default::default() }
    }

    pub fn text(&self) -> String {
        self.text.iter().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    #[cfg(test)]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn set(&mut self, text: &str) {
        self.text = text.chars().collect();
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
        self.clear();
        t
    }

    pub fn history(&self) -> &[String] {
        &self.history
    }

    pub fn insert(&mut self, s: &str) {
        // Pasted text may carry carriage returns.
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        for c in s.chars() {
            self.text.insert(self.cursor, c);
            self.cursor += 1;
        }
        self.browsing = None;
    }

    /// Replace the `n` characters before the cursor with `with` (a completion).
    pub fn replace_back(&mut self, n: usize, with: &str) {
        let from = self.cursor.saturating_sub(n);
        self.text.drain(from..self.cursor);
        self.cursor = from;
        self.insert(with);
    }

    /// The word before the cursor: its start and its text (up to whitespace).
    pub fn word_before_cursor(&self) -> (usize, String) {
        let start = self.text[..self.cursor].iter().rposition(|c| c.is_whitespace()).map(|i| i + 1).unwrap_or(0);
        (start, self.text[start..self.cursor].iter().collect())
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.text.remove(self.cursor);
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.text.len() {
            self.text.remove(self.cursor);
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.text.len());
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].iter().rposition(|c| *c == '\n').map(|i| i + 1).unwrap_or(0)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..].iter().position(|c| *c == '\n').map(|i| self.cursor + i).unwrap_or(self.text.len())
    }

    pub fn home(&mut self) {
        self.cursor = self.line_start();
    }

    pub fn end(&mut self) {
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
        self.cursor = self.word_left_index();
    }

    pub fn word_right(&mut self) {
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
        self.text.drain(from..self.cursor);
        self.cursor = from;
    }

    /// Ctrl+U: delete to the start of the line.
    pub fn kill_to_start(&mut self) {
        let from = self.line_start();
        self.text.drain(from..self.cursor);
        self.cursor = from;
    }

    /// Ctrl+K: delete to the end of the line.
    pub fn kill_to_end(&mut self) {
        let to = self.line_end();
        self.text.drain(self.cursor..to);
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

    /// Up: the line above, or the previous prompt from the first line.
    /// Returns false when nothing changed.
    pub fn up(&mut self) -> bool {
        let (row, col) = self.position();
        if row > 0 {
            let start = self.line_start();
            let prev_start = self.text[..start - 1].iter().rposition(|c| *c == '\n').map(|i| i + 1).unwrap_or(0);
            let prev_len = start - 1 - prev_start;
            self.cursor = prev_start + col.min(prev_len);
            return true;
        }
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

    /// Down: the line below, or the next prompt from history (then what was being typed).
    pub fn down(&mut self) -> bool {
        let (row, col) = self.position();
        if row + 1 < self.lines() {
            let end = self.line_end();
            let next_start = end + 1;
            let next_end = self.text[next_start..]
                .iter()
                .position(|c| *c == '\n')
                .map(|i| next_start + i)
                .unwrap_or(self.text.len());
            self.cursor = next_start + col.min(next_end - next_start);
            return true;
        }
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
}
