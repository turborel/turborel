//! \[DESIGN\] The line editor (RULES.md D211), the client's own, as it
//! takes no dependency for one: the line typed is held as characters
//! with a cursor, edited by keys decoded from the bytes a terminal sends,
//! its history kept in a file and searched backwards. This module holds
//! what is pure, so it is tested without a terminal: the keys, the bytes
//! they are sent as, and the buffer. `editor.rs` is the state of one line
//! being typed, `history.rs` the entries kept, and `terminal.rs` what
//! puts the terminal in raw mode by `stty` and reads it.
//!
//! The keys are the ones a terminal sends without being told more: Left,
//! Right, Home, End, Delete, Up and Down by their ANSI escape sequences,
//! Ctrl or Alt with Left and Right, and Alt-B and Alt-F, moving by words;
//! Ctrl-A, Ctrl-E, Ctrl-B, Ctrl-F, Ctrl-P and Ctrl-N as Home, End, Left,
//! Right, Up and Down; Ctrl-U, Ctrl-K and Ctrl-W cutting before the
//! cursor, after it and the word before it; Ctrl-R searching the history;
//! Ctrl-L redrawing; Tab completing; Ctrl-C throwing the line away and
//! Ctrl-D, on an empty line, ending input.

pub mod editor;
pub mod history;
pub mod terminal;

/// A key typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A character to insert.
    Char(char),
    /// Enter.
    Enter,
    /// Backspace.
    Backspace,
    /// Delete.
    Delete,
    /// The cursor one character left.
    Left,
    /// One character right.
    Right,
    /// To the start.
    Home,
    /// To the end.
    End,
    /// The history entry before.
    Up,
    /// The history entry after.
    Down,
    /// One word left.
    WordLeft,
    /// One word right.
    WordRight,
    /// Tab: completes.
    Tab,
    /// Ctrl-R: searches the history backwards.
    Search,
    /// Ctrl-C: throws the line away.
    Interrupt,
    /// Ctrl-D: ends input, on an empty line; else deletes.
    EndOfInput,
    /// Ctrl-U: cuts before the cursor.
    CutStart,
    /// Ctrl-K: cuts after it.
    CutEnd,
    /// Ctrl-W: cuts the word before it.
    CutWord,
    /// Ctrl-L: redraws.
    Redraw,
    /// Escape, or Ctrl-G: leaves a search.
    Cancel,
    /// A key the editor does nothing with.
    Other,
}

/// The escape character.
const ESC: u8 = 0x1b;
/// The most bytes after an escape a sequence the editor knows has.
const MAX_ESCAPE: usize = 6;
/// The byte of Ctrl and `letter`.
const fn ctrl(letter: u8) -> u8 {
    letter & 0x1f
}

/// What a decoder holds of a key whose bytes are not all read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Partial {
    /// Nothing.
    #[default]
    None,
    /// An escape, and the bytes after it.
    Escape(Vec<u8>),
    /// The first bytes of a UTF-8 character, and how many it has.
    Utf8(Vec<u8>, usize),
}

/// What turns the bytes a terminal sends into keys.
#[derive(Debug, Clone, Default)]
pub struct Decoder {
    partial: Partial,
}

/// The key of a byte by itself, below 0x80.
fn plain(byte: u8) -> Key {
    match byte {
        b'\r' | b'\n' => Key::Enter,
        0x7f | 0x08 => Key::Backspace,
        b'\t' => Key::Tab,
        _ => controlled(byte),
    }
}

/// The key of a control byte other than those `plain` reads.
fn controlled(byte: u8) -> Key {
    const KEYS: [(u8, Key); 15] = [
        (ctrl(b'a'), Key::Home),
        (ctrl(b'e'), Key::End),
        (ctrl(b'b'), Key::Left),
        (ctrl(b'f'), Key::Right),
        (ctrl(b'p'), Key::Up),
        (ctrl(b'n'), Key::Down),
        (ctrl(b'r'), Key::Search),
        (ctrl(b'c'), Key::Interrupt),
        (ctrl(b'd'), Key::EndOfInput),
        (ctrl(b'u'), Key::CutStart),
        (ctrl(b'k'), Key::CutEnd),
        (ctrl(b'w'), Key::CutWord),
        (ctrl(b'l'), Key::Redraw),
        (ctrl(b'g'), Key::Cancel),
        (ctrl(b'h'), Key::Backspace),
    ];
    if let Some((_, key)) = KEYS.iter().find(|(b, _)| *b == byte) {
        return *key;
    }
    if byte < b' ' { Key::Other } else { Key::Char(char::from(byte)) }
}

/// The key an escape sequence `after` its escape is, if it is whole: none
/// if more bytes are to come.
fn escaped(after: &[u8]) -> Option<Key> {
    let whole = match after {
        [] | [b'[' | b'O'] => false,
        [b'[', ..] => after.len() > MAX_ESCAPE || after.last().is_some_and(|b| b.is_ascii_alphabetic() || *b == b'~'),
        _ => true,
    };
    if !whole {
        return None;
    }
    let key = match after {
        [b'[' | b'O', b'A'] => Key::Up,
        [b'[' | b'O', b'B'] => Key::Down,
        [b'[' | b'O', b'C'] => Key::Right,
        [b'[' | b'O', b'D'] => Key::Left,
        [b'[' | b'O', b'H'] | [b'[', b'1' | b'7', b'~'] => Key::Home,
        [b'[' | b'O', b'F'] | [b'[', b'4' | b'8', b'~'] => Key::End,
        [b'[', b'3', b'~'] => Key::Delete,
        [b'f'] | [b'[', b'1', b';', b'3' | b'5', b'C'] => Key::WordRight,
        [b'b'] | [b'[', b'1', b';', b'3' | b'5', b'D'] => Key::WordLeft,
        [ESC] => Key::Cancel,
        _ => Key::Other,
    };
    Some(key)
}

impl Decoder {
    /// The key `byte` ends, if it ends one.
    pub fn feed(&mut self, byte: u8) -> Option<Key> {
        match std::mem::take(&mut self.partial) {
            Partial::None => self.first(byte),
            Partial::Escape(mut after) => {
                after.push(byte);
                let key = escaped(&after);
                if key.is_none() {
                    self.partial = Partial::Escape(after);
                }
                key
            }
            Partial::Utf8(mut bytes, length) => {
                bytes.push(byte);
                if bytes.len() < length {
                    self.partial = Partial::Utf8(bytes, length);
                    return None;
                }
                Some(std::str::from_utf8(&bytes).ok().and_then(|s| s.chars().next()).map_or(Key::Other, Key::Char))
            }
        }
    }

    /// The key a first byte is, or none if more are to come.
    fn first(&mut self, byte: u8) -> Option<Key> {
        if byte == ESC {
            self.partial = Partial::Escape(Vec::new());
            return None;
        }
        let length = match byte.leading_ones() {
            0 => return Some(plain(byte)),
            n @ 2..=4 => usize::try_from(n).unwrap_or(1),
            _ => return Some(Key::Other),
        };
        self.partial = Partial::Utf8(vec![byte], length);
        None
    }

    /// Ends an escape that no byte followed, as a terminal sends Escape:
    /// the key it is, if one is held.
    pub fn flush(&mut self) -> Option<Key> {
        match std::mem::take(&mut self.partial) {
            Partial::Escape(after) => after.is_empty().then_some(Key::Cancel),
            Partial::None | Partial::Utf8(..) => None,
        }
    }
}

/// Whether `c` is of a word, for moving and cutting by words.
fn in_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The text of a line typed, and the cursor in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Buffer {
    chars: Vec<char>,
    cursor: usize,
}

impl Buffer {
    /// A buffer holding `text`, the cursor at its end.
    pub fn of(text: &str) -> Buffer {
        let chars: Vec<char> = text.chars().collect();
        Buffer { cursor: chars.len(), chars }
    }

    /// Its text.
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    /// Its text before the cursor.
    pub fn before(&self) -> String {
        self.chars.iter().take(self.cursor).collect()
    }

    /// The cursor, in characters.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Whether it holds nothing.
    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// Inserts `c` at the cursor.
    pub fn insert(&mut self, c: char) {
        self.chars.insert(self.cursor, c);
        self.cursor = self.cursor.saturating_add(1);
    }

    /// Deletes the character before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor = self.cursor.saturating_sub(1);
            self.chars.remove(self.cursor);
        }
    }

    /// Deletes the character at the cursor.
    pub fn delete(&mut self) {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
        }
    }

    /// Moves the cursor one character left.
    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// One character right.
    pub fn right(&mut self) {
        self.cursor = self.cursor.saturating_add(1).min(self.chars.len());
    }

    /// To the start.
    pub fn home(&mut self) {
        self.cursor = 0;
    }

    /// To the end.
    pub fn end(&mut self) {
        self.cursor = self.chars.len();
    }

    /// To the start of the word before.
    pub fn word_left(&mut self) {
        let not_word = self.chars.iter().take(self.cursor).rev().take_while(|c| !in_word(**c)).count();
        let word = self.chars.iter().take(self.cursor.saturating_sub(not_word)).rev().take_while(|c| in_word(**c)).count();
        self.cursor = self.cursor.saturating_sub(not_word.saturating_add(word));
    }

    /// To the end of the word after.
    pub fn word_right(&mut self) {
        let not_word = self.chars.iter().skip(self.cursor).take_while(|c| !in_word(**c)).count();
        let word = self.chars.iter().skip(self.cursor.saturating_add(not_word)).take_while(|c| in_word(**c)).count();
        self.cursor = self.cursor.saturating_add(not_word).saturating_add(word).min(self.chars.len());
    }

    /// Cuts the text before the cursor.
    pub fn cut_start(&mut self) {
        self.chars.drain(..self.cursor);
        self.cursor = 0;
    }

    /// Cuts the text after the cursor.
    pub fn cut_end(&mut self) {
        self.chars.truncate(self.cursor);
    }

    /// Cuts the word before the cursor.
    pub fn cut_word(&mut self) {
        let end = self.cursor;
        self.word_left();
        self.chars.drain(self.cursor..end);
    }

    /// Puts `text` in place of the characters from `start` to the cursor.
    pub fn replace_before(&mut self, start: usize, text: &str) {
        let from = start.min(self.cursor);
        self.chars.splice(from..self.cursor, text.chars());
        self.cursor = from.saturating_add(text.chars().count());
    }
}

#[cfg(test)]
mod tests;
