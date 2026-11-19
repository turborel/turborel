//! \[DESIGN\] Where results go (RULES.md D213, D215, D219), to the
//! output, or a file or program `\o` or `\g` names; copied to the file
//! `\tee` names; through the pager when the output is a screen.

use std::fs::File;
use std::io::{self, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::Duration;

use super::{Next, Shell, Statements};
use crate::command::Target;
use crate::format::Paging;
use crate::pager::{self, Paged};

/// Where results go in place of the output.
pub(super) enum Redirect {
    /// A file, written anew.
    File(File),
    /// A program's standard input.
    Program(Child, ChildStdin),
}

impl Redirect {
    /// What `target` names, opened, or none for the default.
    ///
    /// # Errors
    ///
    /// The file cannot be made, or the program started.
    pub(super) fn open(target: &Target) -> io::Result<Option<Redirect>> {
        match target {
            Target::Default => Ok(None),
            Target::File(path) => File::create(path).map(|f| Some(Redirect::File(f))),
            Target::Program(words) => {
                let (program, args) = words.split_first().ok_or_else(|| io::Error::other("| names no program"))?;
                let mut child = Command::new(program).args(args).stdin(Stdio::piped()).spawn()?;
                let stdin = child.stdin.take().ok_or_else(|| io::Error::other("the program takes no input"))?;
                Ok(Some(Redirect::Program(child, stdin)))
            }
        }
    }

    /// What it is written by.
    pub(super) fn writer(&mut self) -> &mut dyn Write {
        match self {
            Redirect::File(file) => file,
            Redirect::Program(_, stdin) => stdin,
        }
    }

    /// Ends it, the file flushed, or the program waited for once its input
    /// is closed.
    ///
    /// # Errors
    ///
    /// Either fails.
    pub(super) fn close(self) -> io::Result<()> {
        match self {
            Redirect::File(mut file) => file.flush(),
            Redirect::Program(mut child, stdin) => {
                drop(stdin);
                child.wait().map(drop)
            }
        }
    }
}

/// The writer of one result, its target, copied to the tee, through the
/// pager if one is wanted.
pub(super) struct Out<'w> {
    pub(super) main: &'w mut dyn Write,
    pub(super) tee: Option<&'w mut File>,
    pub(super) pager: Option<Paged>,
}

impl Write for Out<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match &mut self.pager {
            Some(paged) => paged.write(bytes, self.main)?,
            None => self.main.write_all(bytes)?,
        }
        if let Some(tee) = &mut self.tee {
            tee.write_all(bytes)?;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.pager.is_none() {
            self.main.flush()?;
        }
        Ok(())
    }
}

impl Out<'_> {
    /// Ends the result, what the pager held written, or the pager waited
    /// for.
    ///
    /// # Errors
    ///
    /// Either fails.
    pub(super) fn finish(mut self) -> io::Result<()> {
        if let Some(paged) = self.pager.take() {
            paged.finish(self.main)?;
        }
        self.main.flush()
    }
}

impl<S: Statements + ?Sized, C: FnMut() -> Duration> Shell<'_, S, C> {
    /// The pager a result to the output wants. There is one if output is
    /// a screen and paging is on, a pager is named or found, and neither
    /// `\o` nor a target of its own sends it elsewhere.
    pub(super) fn pager(&self, elsewhere: bool) -> Option<Paged> {
        let screen = self.terminal.screen?;
        let lines = match self.layout.paging {
            Paging::Off => return None,
            Paging::On => screen.height.saturating_sub(1),
            Paging::Always => 0,
        };
        if elsewhere || self.redirect.is_some() {
            return None;
        }
        pager::command(&self.options.settings.environment).map(|program| Paged::new(program, lines))
    }

    /// `\o` sends results to `target` from now on, the target before
    /// closed.
    pub(super) fn redirect(&mut self, target: &Target) -> Next {
        self.close_output();
        match Redirect::open(target) {
            Ok(redirect) => {
                self.redirect = redirect;
                Next::Go
            }
            Err(e) => self.tell(&format!("\\o: cannot write to {target:?}: {e}"), Next::Go),
        }
    }

    /// Closes the target `\o` named, if any.
    pub(super) fn close_output(&mut self) {
        if let Some(redirect) = self.redirect.take()
            && let Err(e) = redirect.close()
        {
            self.tell(&format!("cannot close the output: {e}"), Next::Go);
        }
    }

    /// `\tee FILE` copies output to the file from now on.
    pub(super) fn tee(&mut self, path: &std::path::Path) -> Next {
        match File::create(path) {
            Ok(file) => {
                self.tee = Some(file);
                Next::Go
            }
            Err(e) => self.tell(&format!("\\tee: cannot write {}: {e}", path.display()), Next::Go),
        }
    }

    /// Writes `line` to where results go now, `\o`'s target or the
    /// output, copied to the tee.
    pub(super) fn result_line(&mut self, line: &str) -> io::Result<()> {
        let main: &mut dyn Write = match &mut self.redirect {
            Some(r) => r.writer(),
            None => &mut *self.out,
        };
        writeln!(main, "{line}").and_then(|()| main.flush())?;
        if let Some(tee) = &mut self.tee {
            writeln!(tee, "{line}")?;
        }
        Ok(())
    }
}
