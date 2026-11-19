//! \[DESIGN\] The terminal the line editor reads (RULES.md D211). It is
//! put in raw mode, its characters read one at a time and not echoed, by
//! the `stty` program, run with the terminal as its input, while a line
//! is typed, and put back as it was once the line is read, a guard doing
//! so even as a panic unwinds. The editor is used only where standard
//! input and output are terminals and `stty` saves and sets their modes;
//! elsewhere lines are read whole, as the terminal's own line discipline
//! edits them. The screen's size is what `stty size` says.
//!
//! Between lines the terminal is as it was, so Ctrl-C cancels a statement
//! running as a signal does (RULES.md D44).

use std::io::{self, IsTerminal, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use super::editor::{Action, Editor};
use super::history::History;
use super::{Decoder, Key};
use crate::run::{Completion, Lines, Screen};

/// The program that sets the terminal's modes.
const STTY: &str = "stty";
/// The modes of raw input, each byte as typed, not echoed, Ctrl-C and
/// Ctrl-Z and flow control read as keys.
const RAW: [&str; 6] = ["-icanon", "-echo", "-isig", "-ixon", "min", "1"];
/// The columns of a screen whose size is not known.
const DEFAULT_WIDTH: usize = 80;

/// Runs `stty` with `args`, the terminal its input, and returns what it
/// wrote, if it succeeded.
fn stty(args: &[&str]) -> Option<String> {
    let out = Command::new(STTY).args(args).stdin(Stdio::inherit()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// The screen's size, if standard output is a terminal whose size `stty`
/// tells.
pub fn screen() -> Option<Screen> {
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        return None;
    }
    let size = stty(&["size"])?;
    let (rows, columns) = size.split_once(' ')?;
    Some(Screen { width: columns.trim().parse().ok()?, height: rows.trim().parse().ok()? })
}

/// The terminal in raw mode; put back as it was when dropped.
struct Raw {
    saved: String,
}

impl Raw {
    /// Puts the terminal in raw mode, if `stty` can.
    fn enter() -> Option<Raw> {
        let saved = stty(&["-g"])?;
        stty(&RAW)?;
        Some(Raw { saved })
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        // The terminal is put back as well as can be; a failure has no one
        // left to be told to, as the program is ending or the line done.
        drop(stty(&[self.saved.as_str()]));
    }
}

/// The line editor over the process's terminal.
pub struct LineEditor {
    history: History,
}

impl LineEditor {
    /// The editor, its history read from `file`, if standard input and
    /// output are terminals and `stty` can set their modes.
    pub fn open(file: Option<&Path>) -> Option<LineEditor> {
        let terminals = io::stdin().is_terminal() && io::stdout().is_terminal();
        if !terminals || stty(&["-g"]).is_none() {
            return None;
        }
        let history = file.map(History::load).unwrap_or_default();
        Some(LineEditor { history })
    }

    /// Reads one line in raw mode.
    fn raw_line(&self, prompt: &str, complete: &mut dyn FnMut(&str) -> Completion) -> io::Result<Option<String>> {
        let width = screen().map_or(DEFAULT_WIDTH, |s| s.width);
        let mut out = io::stdout().lock();
        let mut editor = Editor::new(&self.history);
        let mut decoder = Decoder::default();
        out.write_all(editor.render(prompt, width).as_bytes())?;
        out.flush()?;
        let mut byte = [0_u8; 1];
        loop {
            if io::stdin().lock().read(&mut byte)? == 0 {
                return Ok(None);
            }
            let Some(key) = decoder.feed(byte[0]) else { continue };
            let action = editor.key(key);
            match action {
                Action::Go => {}
                Action::Submit(text) => {
                    out.write_all(editor.leave(width).as_bytes())?;
                    return Ok(Some(text));
                }
                Action::End => {
                    out.write_all(b"\r\n")?;
                    return Ok(None);
                }
                Action::Complete => {
                    let completion = complete(&editor.buffer().before());
                    if let Some(listed) = editor.complete(&completion) {
                        write!(out, "{}{}\r\n", editor.leave(width), listed.join("  "))?;
                    }
                }
            }
            if key == Key::Redraw {
                out.write_all(b"\x1b[H\x1b[2J")?;
            }
            out.write_all(editor.render(prompt, width).as_bytes())?;
            out.flush()?;
        }
    }
}

impl Lines for LineEditor {
    fn read_line(&mut self, prompt: &str, complete: &mut dyn FnMut(&str) -> Completion) -> io::Result<Option<String>> {
        let Some(raw) = Raw::enter() else {
            let mut out = io::stdout().lock();
            write!(out, "{prompt}")?;
            out.flush()?;
            let mut line = String::new();
            let read = io::stdin().read_line(&mut line)?;
            return Ok((read > 0).then(|| line.trim_end_matches(['\r', '\n']).to_owned()));
        };
        let line = self.raw_line(prompt, complete);
        drop(raw);
        line
    }

    fn remember(&mut self, entry: &str, size: usize, file: Option<&Path>) {
        self.history.add(entry, size);
        if let Some(file) = file {
            // The history is the user's convenience, so a file that cannot
            // be written leaves it kept for this session alone.
            drop(self.history.save(file));
        }
    }
}
