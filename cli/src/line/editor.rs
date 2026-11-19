//! \[DESIGN\] One line being typed (RULES.md D211), the keys applied to
//! its buffer, Up and Down walking the history, Ctrl-R searching it, and
//! the line drawn again after each key. An entry of several lines is
//! edited as one line, each line break shown as a space and kept. A line
//! wider than the screen wraps, and is drawn again from its first row.

use super::history::History;
use super::{Buffer, Key};
use crate::format::width::Widths;
use crate::run::Completion;

/// What a key does to the line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Typing goes on.
    Go,
    /// The line is done, with its text.
    Submit(String),
    /// Input ended.
    End,
    /// The word before the cursor is to be completed.
    Complete,
}

/// A search of the history, under way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Search {
    query: String,
    /// The entry found, if any.
    found: Option<usize>,
}

/// The state of one line being typed.
pub struct Editor<'h> {
    buffer: Buffer,
    history: &'h History,
    /// The history entry shown, or the line typed at the end.
    at: usize,
    /// The line typed before the history was walked.
    typed: String,
    search: Option<Search>,
    widths: Widths,
    /// The row of the line the cursor was left on, when drawn.
    row: usize,
}

impl<'h> Editor<'h> {
    /// An empty line, of `history`.
    pub fn new(history: &'h History) -> Editor<'h> {
        Editor {
            buffer: Buffer::default(),
            history,
            at: history.entries().len(),
            typed: String::new(),
            search: None,
            widths: Widths::default(),
            row: 0,
        }
    }

    /// Its buffer.
    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    /// Applies `key`.
    pub fn key(&mut self, key: Key) -> Action {
        if self.search.is_some() {
            return self.search_key(key);
        }
        match key {
            Key::Enter => Action::Submit(self.buffer.text()),
            Key::EndOfInput => {
                if self.buffer.is_empty() {
                    return Action::End;
                }
                self.buffer.delete();
                Action::Go
            }
            Key::Interrupt => {
                self.buffer = Buffer::default();
                self.at = self.history.entries().len();
                Action::Submit(String::new())
            }
            Key::Tab => Action::Complete,
            Key::Up => self.walk(true),
            Key::Down => self.walk(false),
            Key::Search => {
                self.search = Some(Search::default());
                Action::Go
            }
            Key::Char(_)
            | Key::Backspace
            | Key::Delete
            | Key::Left
            | Key::Right
            | Key::Home
            | Key::End
            | Key::WordLeft
            | Key::WordRight
            | Key::CutStart
            | Key::CutEnd
            | Key::CutWord
            | Key::Redraw
            | Key::Cancel
            | Key::Other => {
                self.edit(key);
                Action::Go
            }
        }
    }

    /// Applies a key that edits the buffer.
    fn edit(&mut self, key: Key) {
        let buffer = &mut self.buffer;
        match key {
            Key::Char(c) => buffer.insert(c),
            Key::Backspace => buffer.backspace(),
            Key::Delete => buffer.delete(),
            Key::Left => buffer.left(),
            Key::Right => buffer.right(),
            Key::Home => buffer.home(),
            Key::End => buffer.end(),
            Key::WordLeft => buffer.word_left(),
            Key::WordRight => buffer.word_right(),
            Key::CutStart => buffer.cut_start(),
            Key::CutEnd => buffer.cut_end(),
            Key::CutWord => buffer.cut_word(),
            Key::Enter
            | Key::Up
            | Key::Down
            | Key::Tab
            | Key::Search
            | Key::Interrupt
            | Key::EndOfInput
            | Key::Redraw
            | Key::Cancel
            | Key::Other => {}
        }
    }

    /// Shows the history entry before, if `back`, or after.
    fn walk(&mut self, back: bool) -> Action {
        let end = self.history.entries().len();
        if self.at == end {
            self.typed = self.buffer.text();
        }
        self.at = if back { self.at.saturating_sub(1) } else { self.at.saturating_add(1).min(end) };
        let shown = self.history.entries().get(self.at).map_or(self.typed.as_str(), String::as_str);
        self.buffer = Buffer::of(shown);
        Action::Go
    }

    /// Applies `key` to the search under way.
    fn search_key(&mut self, key: Key) -> Action {
        let Some(search) = &mut self.search else { return Action::Go };
        let found = |query: &str, before: usize| self.history.search(query, before);
        match key {
            Key::Char(c) => {
                search.query.push(c);
                search.found = found(&search.query, search.found.map_or(self.history.entries().len(), |f| f.saturating_add(1)));
            }
            Key::Backspace => {
                search.query.pop();
                search.found = found(&search.query, self.history.entries().len());
            }
            Key::Search => search.found = found(&search.query, search.found.unwrap_or(self.history.entries().len())).or(search.found),
            Key::Cancel | Key::Interrupt => self.search = None,
            Key::Enter
            | Key::Delete
            | Key::Left
            | Key::Right
            | Key::Home
            | Key::End
            | Key::Up
            | Key::Down
            | Key::WordLeft
            | Key::WordRight
            | Key::Tab
            | Key::EndOfInput
            | Key::CutStart
            | Key::CutEnd
            | Key::CutWord
            | Key::Redraw
            | Key::Other => return self.accept(key),
        }
        Action::Go
    }

    /// Ends the search, the entry found in the buffer, then applies `key`.
    fn accept(&mut self, key: Key) -> Action {
        if let Some(index) = self.search.take().and_then(|s| s.found) {
            self.at = index;
            self.buffer = Buffer::of(self.history.entries().get(index).map_or("", String::as_str));
        }
        self.key(key)
    }

    /// Applies `completion` of the text before the cursor, one candidate
    /// put in place of the word, several their common start. It returns
    /// the candidates to list, if it did not add to the word and there are
    /// several.
    pub fn complete(&mut self, completion: &Completion) -> Option<Vec<String>> {
        let before = self.buffer.before();
        let start = before.get(..completion.start).map_or(0, |b| b.chars().count());
        let word: String = before.chars().skip(start).collect();
        match completion.candidates.as_slice() {
            [] => None,
            [one] => {
                self.buffer.replace_before(start, one);
                None
            }
            many => {
                let common = many.iter().skip(1).fold(many.first().cloned().unwrap_or_default(), |c, n| common_start(&c, n));
                if common.chars().count() > word.chars().count() {
                    self.buffer.replace_before(start, &common);
                    return None;
                }
                Some(many.to_vec())
            }
        }
    }

    /// The bytes that draw the line again after `prompt`, on a screen
    /// `width` columns wide, the cursor where it is.
    pub fn render(&mut self, prompt: &str, width: usize) -> String {
        let width = width.max(1);
        let (lead, text, cursor) = match &self.search {
            Some(search) => {
                let shown = search.found.and_then(|i| self.history.entries().get(i)).map_or("", String::as_str);
                let lead = format!("(search) '{}': ", search.query);
                (lead, shown.replace('\n', " "), None)
            }
            None => (prompt.to_owned(), self.buffer.text().replace('\n', " "), Some(self.buffer.cursor())),
        };
        let mut out = if self.row > 0 { format!("\x1b[{}A", self.row) } else { String::new() };
        out.push_str("\r\x1b[J");
        out.push_str(&lead);
        out.push_str(&text);
        let total = self.widths.of(&lead).saturating_add(self.widths.of(&text));
        if total > 0 && total.is_multiple_of(width) {
            out.push_str("\r\n");
        }
        let before: String = cursor.map_or_else(|| text.clone(), |c| text.chars().take(c).collect());
        let at = self.widths.of(&lead).saturating_add(self.widths.of(&before));
        let (end_row, row, column) =
            (total.checked_div(width).unwrap_or(0), at.checked_div(width).unwrap_or(0), at.checked_rem(width).unwrap_or(0));
        let up = end_row.saturating_sub(row);
        if up > 0 {
            out.extend(format!("\x1b[{up}A").chars());
        }
        out.push('\r');
        if column > 0 {
            out.extend(format!("\x1b[{column}C").chars());
        }
        self.row = row;
        out
    }

    /// The bytes that leave the line drawn, the cursor after its end.
    pub fn leave(&mut self, width: usize) -> String {
        let text = self.buffer.text().replace('\n', " ");
        let after: String = text.chars().skip(self.buffer.cursor()).collect();
        let rows_after = self.widths.of(&after).checked_div(width.max(1)).unwrap_or(0);
        self.row = 0;
        if rows_after > 0 { format!("\x1b[{rows_after}B\r\n") } else { "\r\n".to_owned() }
    }
}

/// The longest start `a` and `b` share.
fn common_start(a: &str, b: &str) -> String {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).map(|(x, _)| x).collect()
}

#[cfg(test)]
mod tests;
