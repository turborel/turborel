//! \[DESIGN\] The history (RULES.md D211): each statement or command
//! typed is one entry, the lines of a statement typed over several lines
//! together. An entry that begins with a space is not kept, nor one the
//! same as the entry before. At most HISTORY_SIZE entries are kept, the
//! oldest dropped first. The entries are kept in a file, HISTORY_FILE,
//! else `TURBOREL_HISTORY`'s, else `.turborel_history` in the home
//! directory: one entry a line, a backslash written `\\` and a line break
//! `\n`, so an entry of several lines is one line of the file.

use std::fs;
use std::io;
use std::path::Path;

/// The entries kept unless HISTORY_SIZE says.
pub const DEFAULT_SIZE: usize = 500;

/// `entry` as a line of the file.
fn escape(entry: &str) -> String {
    entry.replace('\\', "\\\\").replace('\n', "\\n")
}

/// The entry a line of the file holds.
fn unescape(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// The entries kept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    entries: Vec<String>,
}

impl History {
    /// The history of the file at `path`; empty if it cannot be read.
    pub fn load(path: &Path) -> History {
        let entries = fs::read_to_string(path).map(|text| text.lines().map(unescape).collect()).unwrap_or_default();
        History { entries }
    }

    /// Writes it to the file at `path`.
    ///
    /// # Errors
    ///
    /// The file cannot be written.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text: String = self.entries.iter().map(|e| format!("{}\n", escape(e))).collect();
        fs::write(path, text)
    }

    /// Keeps `entry`, unless it begins with a space, is empty, or is the
    /// entry before; at most `size` kept.
    pub fn add(&mut self, entry: &str, size: usize) {
        let kept = !entry.starts_with(' ') && !entry.trim().is_empty() && self.entries.last().is_none_or(|last| last != entry);
        if kept {
            self.entries.push(entry.to_owned());
        }
        let over = self.entries.len().saturating_sub(size);
        self.entries.drain(..over);
    }

    /// The entries, oldest first.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// The newest entry before `before`, an index, that holds `query`.
    pub fn search(&self, query: &str, before: usize) -> Option<usize> {
        self.entries.iter().take(before).rposition(|e| e.contains(query))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D211: entries that begin with a space,
    /// empty ones and repeats are not kept; at most `size` are, the oldest
    /// dropped.
    #[test]
    fn entries_are_kept_bounded() {
        let mut history = History::default();
        for entry in ["a", " secret", "", "a", "b", "c"] {
            history.add(entry, 2);
        }
        assert_eq!(history.entries(), ["b", "c"]);
        history.add("d", 0);
        assert!(history.entries().is_empty());
    }

    /// Verifies: [DESIGN] RULES.md D211: the file holds an entry a line,
    /// backslashes and line breaks escaped, and reads back as written; a
    /// file that cannot be read is an empty history.
    #[test]
    fn the_file_reads_back_as_written() {
        let scratch = turborel_test_support::Scratch::new("cli-history");
        let path = scratch.path("history");
        let mut history = History::default();
        history.add("SELECT 1\nFROM T", 10);
        history.add("\\echo a\\nb", 10);
        history.save(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "SELECT 1\\nFROM T\n\\\\echo a\\\\nb\n");
        assert_eq!(History::load(&path), history);
        assert_eq!(History::load(&scratch.path("none")), History::default());
        assert_eq!(unescape("x\\"), "x\\");
    }

    /// Verifies: [DESIGN] RULES.md D211: a search finds the newest entry
    /// before the one given that holds the query.
    #[test]
    fn searches_go_backwards() {
        let mut history = History::default();
        for entry in ["select 1", "insert", "select 2"] {
            history.add(entry, 10);
        }
        assert_eq!(history.search("select", 3), Some(2));
        assert_eq!(history.search("select", 2), Some(0));
        assert_eq!(history.search("select", 0), None);
        assert_eq!(history.search("nothing", 3), None);
    }
}
