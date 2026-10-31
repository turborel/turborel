//! \[EXTENSION\] `turboreld --dir PATH reseal AUTH.TABLE` (RULES.md D48): a
//! table's rows sealed again, through the encodings its option ENCODINGS
//! names now, on the database's directory while no server holds it
//! (`Session::reseal_table`). Committed, then the database closed, which
//! checkpoints it: nothing of the table is read through the keys it named
//! before, so the operator may retire them.

use std::io::Write;

use crate::{Config, Store, within};

/// The table `words`, after `reseal`, names.
///
/// # Errors
///
/// What is wrong with them, for the operator: not one table, written
/// AUTH.TABLE.
pub fn parse_reseal(words: &[String]) -> Result<String, String> {
    let wrong = || format!("reseal takes one table, AUTH.TABLE, not {}", words.join(" "));
    let [table] = words else { return Err(wrong()) };
    if !table.contains('.') {
        return Err(wrong());
    }
    Ok(table.clone())
}

/// Seals the rows of `table`, of the database `config` names, kept in a
/// directory, again, as its schema's owner; what it did written to `out`;
/// the database closed after.
///
/// # Errors
///
/// What failed, for the operator: the database not kept in a directory,
/// or held by a server; the table not one; a key it names not in
/// `--keys`.
pub fn run_reseal(config: &Config, table: &str, out: &mut dyn Write) -> Result<(), String> {
    if config.store == Store::Memory {
        return Err("reseal works on a database kept in a directory: name it with --dir".into());
    }
    let owner = table.split_once('.').map_or(table, |(auth, _)| auth);
    within(config, None, |server| {
        let session = server.database().session(owner);
        let failed = |e: turborel::error::SqlError| format!("cannot reseal {table}: {e}");
        session.reseal_table(table).map_err(failed)?;
        session.commit().map_err(failed)?;
        writeln!(out, "{table} sealed again").map_err(|e| format!("cannot write: {e}"))
    })
}
