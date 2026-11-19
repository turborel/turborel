//! \[DESIGN\] Files a run reads and writes (RULES.md D59, D215, D216):
//! `-f` and `\i` run a file's statements, `\ir` one named from the current
//! file's directory; `--import` and `\import` insert a file's rows into a
//! table; `\export` writes a table's or a query's rows to a file.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Duration;

use turborel_client::import::{Cause, DEFAULT_BATCH, Delimited, Dialect, Import, Rejected, rows};

use super::{MAX_INCLUDES, Next, Outcome, Shell, Statements, cancelled};
use crate::export::{self, Export};

impl<S: Statements + ?Sized, C: FnMut() -> Duration> Shell<'_, S, C> {
    /// Runs the statements of the file at `path`: unreadable, the run
    /// stops.
    pub(super) fn file(&mut self, path: &Path) -> Next {
        if self.files.len() >= MAX_INCLUDES {
            return self.tell(&format!("cannot read {}: files are included at most {MAX_INCLUDES} deep", path.display()), Next::Stop);
        }
        let file = match File::open(path) {
            Ok(file) => file,
            Err(e) => return self.stop(&format!("cannot read {}: {e}", path.display()), Outcome::Failed),
        };
        self.files.push(path.to_path_buf());
        let next = self.text(&mut BufReader::new(file), false);
        self.files.pop();
        next
    }

    /// `\i` and `\ir`: the statements of `path` run, named from the
    /// directory of the current file if `relative`, else from the
    /// current directory.
    pub(super) fn include(&mut self, path: &Path, relative: bool) -> Next {
        let base = self.files.last().and_then(|f| f.parent()).filter(|_| relative && path.is_relative());
        let full: PathBuf = base.map_or_else(|| path.to_path_buf(), |dir| dir.join(path));
        self.file(&full)
    }

    /// Imports the rows of the file at `path` (RULES.md D59), as the
    /// options say.
    pub(super) fn import(&mut self, path: &Path) -> Next {
        let Some(importing) = &self.options.import else { return self.tell("[DESIGN] --import needs --table", Next::Stop) };
        let (into, dialect) = (importing.into.clone(), importing.dialect.clone());
        self.import_rows(path, &into, dialect)
    }

    /// Imports the rows of the file at `path` into the table `into`
    /// names, read as `dialect` says: each row skipped told as a failure,
    /// and what was imported said.
    ///
    /// The rows go through one prepared INSERT, a batch to a transaction
    /// (`turborel_client::import`); a bulk copy of the protocol, once it
    /// has one, takes its place here.
    fn import_rows(&mut self, path: &Path, into: &Import, dialect: Dialect) -> Next {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(e) => return self.tell(&format!("cannot read {}: {e}", path.display()), Next::Stop),
        };
        let mut source = Delimited::new(BufReader::new(file), dialect);
        let shown = path.display();
        let err = &mut *self.err;
        // Nothing is left to tell a failure to write a failure to.
        let mut told = |row: Rejected| drop(writeln!(err, "turborel: {shown}: {row}"));
        let done = self.connection.import(&mut source, into, &mut told);
        match done {
            Ok(imported) => {
                // A row skipped is a failure, told as it was skipped.
                self.failed |= imported.rejected > 0;
                let skipped = if imported.rejected > 0 { format!(", {} skipped", imported.rejected) } else { String::new() };
                let said = format!("OK, {} imported{skipped}", rows(imported.rows));
                self.result_line(&said).map_or_else(|e| self.tell(&format!("cannot write: {e}"), Next::Stop), |()| Next::Go)
            }
            Err(e) => {
                let gave_up = match &e.cause {
                    Cause::Sql(f) => cancelled(f),
                    Cause::Input(_) => false,
                };
                let msg = format!("{shown}: {e}");
                if self.connection.is_lost() {
                    return self.stop(&msg, Outcome::Lost);
                }
                if self.vars.on(crate::vars::STOP_ON_ERROR) && !self.typing {
                    return self.stop(&msg, Outcome::Stopped);
                }
                self.tell(&msg, if gave_up { Next::Stop } else { Next::Go })
            }
        }
    }

    /// `\import TABLE FILE [OPTION...]`.
    pub(super) fn import_command(&mut self, args: &[String]) -> Next {
        let read = || -> Result<(Import, PathBuf, Dialect), String> {
            let [table, file, options @ ..] = args else { return Err("\\import TABLE FILE [OPTION...]".to_owned()) };
            let mut each = options.iter().map(String::as_str);
            let (mut dialect, mut into) = (Dialect::csv(), Import { batch: DEFAULT_BATCH, ..Import::into(table) });
            while let Some(option) = each.next() {
                let mut value = || each.next().ok_or_else(|| format!("\\import: {option} needs a value"));
                dialect = match option {
                    "csv" => Dialect::csv(),
                    "tsv" => Dialect::tsv(),
                    "header" => dialect.with_header(),
                    "delimiter" => dialect.with_delimiter(export::one_char(value()?)?)?,
                    "null" => dialect.with_null(value()?),
                    "batch" => {
                        into.batch = value()?.parse().map_err(|_| "\\import: batch takes a number of rows".to_owned())?;
                        dialect
                    }
                    "skip" => {
                        into.skip = true;
                        dialect
                    }
                    other => return Err(format!("\\import: {other} is no option")),
                };
            }
            Ok((into, PathBuf::from(file), dialect))
        };
        match read() {
            Ok((into, path, dialect)) => self.import_rows(&path, &into, dialect),
            Err(e) => self.tell(&e, Next::Go),
        }
    }

    /// `\export {TABLE | (QUERY)} FILE [OPTION...]`: the rows written,
    /// and how many said.
    pub(super) fn export(&mut self, args: &[String]) -> Next {
        let how = match Export::parse(args) {
            Ok(how) => how,
            Err(e) => return self.tell(&e, Next::Go),
        };
        match export::export(&mut *self.connection, &how) {
            Ok(count) => {
                let said = format!("OK, {} exported", rows(count));
                self.result_line(&said).map_or_else(|e| self.tell(&format!("cannot write: {e}"), Next::Stop), |()| Next::Go)
            }
            Err(e) => self.tell(&format!("\\export: {e}"), Next::Go),
        }
    }
}
