//! \[DESIGN\] Bulk copy in (RULES.md D195): CopyIn names a table and its
//! columns; CopyData sends rows, as many as the client likes at a time,
//! with no reply; CopyEnd ends it, its reply a Complete of the rows
//! inserted or the copy's Error. The rows go in `BATCH` at a time, or a
//! group's rows at a time once its load writes them straight to groups
//! (RULES.md D533), each batch by one execution of a prepared INSERT of one row, given them all
//! (`Session::execute_prepared_rows`), so the server inserts them together
//! (RULES.md D530); each value is assigned to its column as INSERT assigns,
//! so a string too long is refused, not cut [ISO/IEC 9075:1992 §13.8 GR4b
//! iii].
//!
//! A copy is all or nothing: under a savepoint of its own while the
//! transaction goes on after it, released once it is done, rolled back to
//! if it fails or the client abandons it; or, of a copy that ends the
//! transaction, committed once done and rolled back if not. A failure is
//! kept until CopyEnd, the rows sent after it thrown away; a batch that
//! fails is tried again a row at a time, so the Error names the row that
//! failed in its detail.

use turborel::db::{Prepared, Session};
use turborel::error::{Code, Result, SqlError, SqlState};
use turborel::lexer::{Tok, Token, delim, lex};
use turborel::params::Load;
use turborel::storage::Row;
use turborel::value::{DataType, Value, assign};
use turborel_protocol::{Completed, broken};

/// \[DESIGN\] The rows an execution of a copy's INSERT takes at once: many
/// enough that a statement's cost is shared by many rows and they go in
/// together (RULES.md D530), few enough that a batch tried again a row at
/// a time is soon done.
const BATCH: usize = 4096;
/// The savepoint of a copy within a transaction that goes on: a name no
/// client's statement is likely to take, as SQL writes it.
const SAVEPOINT: &str = "\"turborel copy\"";
/// The most parts of a `<table name>` [ISO/IEC 9075:1992 §5.4].
const MAX_NAME_PARTS: usize = 3;
/// The rule by which INSERT assigns a value to its column.
const INSERT_ASSIGNS: &str = "ISO/IEC 9075:1992 §13.8 GR4b iii";

/// A copy under way.
pub(super) struct Copy {
    /// The INSERTs, or why the copy failed.
    insert: std::result::Result<Insert, SqlError>,
    /// Whether it took a savepoint, to undo it by.
    saved: bool,
    /// Whether the transaction ends with it.
    ends: bool,
}

/// The INSERT of a copy, and the rows waiting for a batch.
struct Insert {
    one: Prepared,
    /// The types of its parameters, each its column's.
    types: Vec<DataType>,
    waiting: Vec<Row>,
    /// The rows inserted, or waiting, so far.
    rows: u64,
    /// The rows an execution takes: `BATCH`, or a group's once the load
    /// writes its rows straight to groups, so each is whole (RULES.md
    /// D533).
    batch: usize,
}

/// `name` as a `<delimited identifier>` [ISO/IEC 9075:1992 §5.2], a quote
/// in it written twice.
fn delimited(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// [ISO/IEC 9075:1992 §5.4] `text`, a `<table name>`, as SQL text of
/// delimited identifiers, so nothing else is taken for one.
fn table_name(text: &str) -> Result<String> {
    let wrong = || SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 9075:1992 §5.4] {text} is not a <table name>"));
    let tokens = lex(text).map_err(|_| wrong())?;
    let mut rest = tokens.iter();
    let mut parts = Vec::new();
    loop {
        let Some(Token { tok: Tok::Identifier(name), .. }) = rest.next() else { return Err(wrong()) };
        parts.push(delimited(name));
        let next = rest.next();
        if matches!(next, None | Some(Token { tok: Tok::End, .. })) {
            return Ok(parts.join("."));
        }
        if !matches!(next, Some(Token { tok: Tok::Delimiter(delim::PERIOD), .. })) || parts.len() == MAX_NAME_PARTS {
            return Err(wrong());
        }
    }
}

/// The column `name` names: the `<identifier>` it is, a regular one in
/// upper case [ISO/IEC 9075:1992 §5.2 SR10], or a key word so read; else
/// as written; as a delimited identifier.
fn column_name(name: &str) -> String {
    match lex(name).as_deref() {
        Ok([Token { tok: Tok::Identifier(read), .. }, Token { tok: Tok::End, .. }]) => delimited(read),
        Ok([Token { tok: Tok::KeyWord(word), .. }, Token { tok: Tok::End, .. }]) => delimited(word),
        Ok(_) | Err(_) => delimited(name),
    }
}

/// The INSERT into `table` of one row of `columns`.
fn insert_text(table: &str, columns: &[String]) -> String {
    format!("INSERT INTO {table} ({}) VALUES ({})", columns.join(", "), vec!["?"; columns.len()].join(", "))
}

/// The prepared INSERTs of `session` into the table `table` names, of the
/// columns `columns` name, all if none.
fn prepared(session: &Session, table: &str, columns: &[String]) -> Result<Insert> {
    let table = table_name(table)?;
    let columns: Vec<String> = if columns.is_empty() {
        let all = session.prepare(&format!("SELECT * FROM {table}"))?;
        all.columns().iter().map(|c| delimited(c.name.as_deref().unwrap_or_default())).collect()
    } else {
        columns.iter().map(|c| column_name(c)).collect()
    };
    let one = session.prepare(&insert_text(&table, &columns))?;
    let types = one.parameters().iter().map(|t| t.clone().data_type()).collect();
    Ok(Insert { one, types, waiting: Vec::with_capacity(BATCH), rows: 0, batch: BATCH })
}

/// `row`, the copy's `n`th from 1, its values assigned to `types`: refused
/// if it has too many or too few.
fn assigned(row: Row, types: &[DataType], n: u64) -> Result<Row> {
    if row.len() != types.len() {
        let msg = format!("[DESIGN] row {n} of the copy has {} values, for {} columns", row.len(), types.len());
        return Err(SqlError::new(Code::Dynamic, msg).with_state(SqlState::USING_CLAUSE_MISMATCH));
    }
    row.into_iter()
        .zip(types)
        .map(|(v, to)| if matches!(v, Value::Char(_)) && to.is_character() { assign(v, to, INSERT_ASSIGNS) } else { Ok(v) })
        .collect()
}

impl Copy {
    /// A copy into `table`'s `columns` for `session`, the transaction
    /// ended with it if `ends`: its savepoint taken, of one that does not
    /// end it, and its INSERTs prepared. A savepoint that cannot be taken
    /// fails the copy before it inserts anything, so the transaction it is
    /// within loses nothing.
    pub(super) fn begin(session: &Session, (table, columns): (&str, &[String]), ends: bool) -> Copy {
        let saving = if ends { Ok(()) } else { session.direct(&format!("SAVEPOINT {SAVEPOINT}")).map(drop) };
        let saved = !ends && saving.is_ok();
        Copy { insert: saving.and_then(|()| prepared(session, table, columns)), saved, ends }
    }

    /// Whether the transaction ends with it.
    pub(super) fn ends(&self) -> bool {
        self.ends
    }

    /// Takes `rows`, inserted a batch at a time; thrown away once the copy
    /// failed.
    pub(super) fn rows(&mut self, session: &Session, rows: Vec<Row>) {
        let Ok(insert) = &mut self.insert else { return };
        let failed = insert.take(session, rows).err();
        if let Some(e) = failed {
            self.insert = Err(e);
        }
    }

    /// Ends the copy: its last rows inserted and its savepoint released, a
    /// Complete of its rows; else, or if `abandon`, everything it did
    /// undone, its failure, or a Complete of none.
    ///
    /// # Errors
    ///
    /// The copy's failure.
    pub(super) fn end(self, session: &Session, abandon: bool) -> Result<Completed> {
        let Copy { insert, saved, ends } = self;
        let done = insert.and_then(|mut i| i.flush(session).map(|()| i.rows)).map(|rows| if abandon { 0 } else { rows });
        let undo = abandon || done.is_err();
        // A copy that ends the transaction and succeeds is committed with
        // the request (`Held::ended`); one undone is rolled back here, so a
        // copy abandoned, whose reply is a Complete, commits nothing.
        if saved {
            let how = if undo { format!("ROLLBACK TO SAVEPOINT {SAVEPOINT}") } else { format!("RELEASE SAVEPOINT {SAVEPOINT}") };
            session.direct(&how)?;
        } else if undo && ends {
            session.rollback()?;
        }
        done.map(|rows| Completed { rows: Some(rows), ..Completed::of(0, SqlState::SUCCESSFUL_COMPLETION.as_str()) })
    }
}

impl Insert {
    /// Takes `rows`: each full batch inserted.
    fn take(&mut self, session: &Session, rows: Vec<Row>) -> Result<()> {
        for row in rows {
            self.rows = self.rows.saturating_add(1);
            let n = self.rows;
            self.waiting.push(assigned(row, &self.types, n).map_err(|e| e.with_detail(format!("row {n} of the copy")))?);
            if self.waiting.len() >= self.batch {
                self.insert_waiting(session, Load::Going)?;
            }
        }
        Ok(())
    }

    /// Inserts the rows waiting, part `load` of the copy's load: together
    /// by one execution, tried again a row at a time if it fails, to name
    /// the row that fails, and the load then ended alone if that ends it.
    fn insert_waiting(&mut self, session: &Session, load: Load) -> Result<()> {
        let rows = std::mem::replace(&mut self.waiting, Vec::with_capacity(self.batch));
        let first = self.rows.saturating_sub(u64::try_from(rows.len()).unwrap_or(0));
        if session.execute_prepared_rows(&self.one, &rows, load).is_ok() {
            self.batch = session.load_rows().map_or(BATCH, |grouped| grouped.max(BATCH));
            return Ok(());
        }
        for (at, row) in (1_u64..).zip(&rows) {
            let n = first.saturating_add(at);
            session.execute_prepared(&self.one, row).map_err(|e| e.with_detail(format!("row {n} of the copy")))?;
        }
        if load == Load::Ending { session.execute_prepared_rows(&self.one, &[], load).map(drop) } else { Ok(()) }
    }

    /// Inserts the rows still waiting, ending the copy's load (RULES.md
    /// D532): by an execution of none, if none is.
    fn flush(&mut self, session: &Session) -> Result<()> {
        self.insert_waiting(session, Load::Ending)
    }
}

/// A request other than CopyData or CopyEnd while a copy is under way, or
/// one of those while none is: a connection failure.
pub(super) fn out_of_turn() -> SqlError {
    broken("[DESIGN] a copy takes CopyData and CopyEnd alone, and those come only within a copy")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] [ISO/IEC 9075:1992 §5.2, §5.4] RULES.md D195: a
    /// copy's table name is read as delimited identifiers, at most three,
    /// regular ones in upper case; anything else is refused; a column is
    /// read as the identifier or key word it is, else as written.
    #[test]
    fn a_copy_names_its_table_and_columns_safely() {
        assert_eq!(table_name("s.t").unwrap(), "\"S\".\"T\"");
        assert_eq!(table_name("\"a\"\"b\"").unwrap(), "\"a\"\"b\"");
        assert!(table_name("t; DROP TABLE t").is_err());
        assert!(table_name("a.b.c.d").is_err());
        assert_eq!(column_name("k"), "\"K\"");
        assert_eq!(column_name("VALUE"), "\"VALUE\"");
        assert_eq!(column_name("a b"), "\"a b\"");
        assert_eq!(insert_text("T", &["\"K\"".into(), "V".into()]), "INSERT INTO T (\"K\", V) VALUES (?, ?)");
    }
}
