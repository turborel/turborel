//! \[DESIGN\] The pager (RULES.md D213): output to a screen taller than it
//! goes through a program that shows it a screen at a time. The program
//! is `TURBOREL_PAGER`'s, else `PAGER`'s, else `less -S` if a `less` is
//! found in the directories of `PATH`, else none; an empty value names
//! none. Output is held until it fills the screen, so a result shorter
//! than the screen is written as it is, and a pager for a streamed result
//! starts once the first screen is full. A pager that cannot be started
//! leaves the output written as it would be without one.

use std::io::{self, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};

use crate::args::Environment;

/// The pager found in `PATH`, unless one is named.
const LESS: &str = "less";
/// Its option to cut long lines rather than fold them, as aligned output
/// reads best.
const LESS_CHOP: &str = "-S";

/// The program and arguments of the pager `environment` names, if any.
pub fn command(environment: &Environment) -> Option<Vec<String>> {
    if let Some(named) = &environment.pager {
        let words: Vec<String> = named.split_whitespace().map(str::to_owned).collect();
        return (!words.is_empty()).then_some(words);
    }
    let path = environment.path.as_deref()?;
    let file = format!("{LESS}{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(path).any(|dir| Path::new(&dir).join(&file).is_file()).then(|| vec![LESS.to_owned(), LESS_CHOP.to_owned()])
}

/// Output held until it is taller than a screen, then given to a pager.
pub struct Paged {
    program: Vec<String>,
    /// How many lines are written before the pager starts.
    lines: usize,
    held: Vec<u8>,
    seen: usize,
    started: Option<(Child, ChildStdin)>,
    /// Whether the pager was given up on, the output written directly.
    direct: bool,
}

impl Paged {
    /// Output paged by `program` once more than `lines` lines are written.
    pub fn new(program: Vec<String>, lines: usize) -> Paged {
        Paged { program, lines, held: Vec::new(), seen: 0, started: None, direct: false }
    }

    /// Writes `bytes`: held, given to the pager, or to `main` if none
    /// could be started.
    ///
    /// # Errors
    ///
    /// `main` or the pager cannot be written.
    pub fn write(&mut self, bytes: &[u8], main: &mut dyn Write) -> io::Result<()> {
        if self.direct {
            return main.write_all(bytes);
        }
        if let Some((_, stdin)) = &mut self.started {
            return stdin.write_all(bytes);
        }
        self.held.extend_from_slice(bytes);
        self.seen = self.seen.saturating_add(bytes.iter().filter(|b| **b == b'\n').count());
        if self.seen > self.lines {
            self.start(main)?;
        }
        Ok(())
    }

    /// Starts the pager, giving it what was held; or writes that to
    /// `main` if it cannot start.
    fn start(&mut self, main: &mut dyn Write) -> io::Result<()> {
        let held = std::mem::take(&mut self.held);
        let Some((program, args)) = self.program.split_first() else {
            self.direct = true;
            return main.write_all(&held);
        };
        let spawned = Command::new(program).args(args).stdin(Stdio::piped()).spawn();
        let Some((child, mut stdin)) = spawned.ok().and_then(|mut c| c.stdin.take().map(|s| (c, s))) else {
            self.direct = true;
            return main.write_all(&held);
        };
        stdin.write_all(&held)?;
        self.started = Some((child, stdin));
        Ok(())
    }

    /// Ends the output: what was held written to `main`, or the pager
    /// waited for once its input is closed.
    ///
    /// # Errors
    ///
    /// `main` cannot be written, or the pager waited for.
    pub fn finish(self, main: &mut dyn Write) -> io::Result<()> {
        match self.started {
            Some((mut child, stdin)) => {
                drop(stdin);
                child.wait().map(drop)
            }
            None => main.write_all(&self.held),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D213: output no taller than the screen
    /// is written as it is, when it ends; a pager that cannot be started
    /// leaves output written directly, once it is taller.
    #[test]
    fn short_output_is_written_and_a_missing_pager_is_passed_by() {
        let mut main = Vec::new();
        let mut short = Paged::new(vec!["no-such-pager-anywhere".into()], 2);
        short.write(b"a\nb\n", &mut main).unwrap();
        assert!(main.is_empty(), "held");
        short.finish(&mut main).unwrap();
        assert_eq!(main, b"a\nb\n");
        let mut tall_main = Vec::new();
        let mut tall = Paged::new(vec!["no-such-pager-anywhere".into()], 1);
        tall.write(b"a\nb\n", &mut tall_main).unwrap();
        tall.write(b"c\n", &mut tall_main).unwrap();
        tall.finish(&mut tall_main).unwrap();
        assert_eq!(tall_main, b"a\nb\nc\n");
        let mut none_main = Vec::new();
        let mut none = Paged::new(Vec::new(), 0);
        none.write(b"x\n", &mut none_main).unwrap();
        assert_eq!(none_main, b"x\n");
    }

    /// Verifies: [DESIGN] RULES.md D213: the pager is the one named, its
    /// words split, an empty name naming none; else `less -S` if `PATH`
    /// has it; else none.
    #[test]
    fn the_pager_is_named_or_found() {
        let named = Environment { pager: Some("more -d".into()), ..Environment::default() };
        assert_eq!(command(&named), Some(vec!["more".to_owned(), "-d".to_owned()]));
        let empty = Environment { pager: Some(" ".into()), path: Some("/bin".into()), ..Environment::default() };
        assert_eq!(command(&empty), None);
        let scratch = turborel_test_support::Scratch::new("cli-pager-path");
        std::fs::write(scratch.path(&format!("less{}", std::env::consts::EXE_SUFFIX)), "").unwrap();
        let found = Environment { path: Some(scratch.dir.display().to_string()), ..Environment::default() };
        assert_eq!(command(&found), Some(vec!["less".to_owned(), "-S".to_owned()]));
        assert_eq!(command(&Environment::default()), None);
    }
}
