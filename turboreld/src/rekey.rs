//! \[EXTENSION\] `turboreld --dir PATH [--keys DIR] rekey` (RULES.md D48):
//! the database's files written again in the formats STORAGE.ENCODINGS
//! and STORAGE.CHECKSUM_KEY name now, on the database's directory while
//! no server holds it (`Database::rekey`), at once rather than at the
//! pace of its job `turborel-rekey`; then the database closed, which
//! checkpoints it. Every file is then in those formats, so the operator
//! may retire the keys and encodings named before.

use std::io::Write;

use crate::{Config, Store, within};

/// Does `rekey` for `config`'s database, telling `out` what was done.
///
/// # Errors
///
/// What failed, for the operator: the database not kept in a directory,
/// or held by a server; a key or encoding its files name not given.
pub fn run_rekey(config: &Config, out: &mut dyn Write) -> Result<(), String> {
    if config.store == Store::Memory {
        return Err("rekey works on a database kept in a directory: name it with --dir".into());
    }
    within(config, None, |server| {
        let db = server.database();
        let failed = |e: turborel::error::SqlError| format!("cannot write the files again: {e}");
        let value = |name: &str| db.setting(name).map(|v| v.to_string()).map_err(failed);
        let (encodings, key) = (value("STORAGE.ENCODINGS")?, value("STORAGE.CHECKSUM_KEY")?);
        db.rekey().map_err(failed)?;
        let formats = match (encodings.is_empty(), key.is_empty()) {
            (true, true) => "no encoding, the checksum's own key".to_owned(),
            (true, false) => format!("no encoding, the checksum keyed by {key}"),
            (false, true) => format!("the encodings {encodings}, the checksum's own key"),
            (false, false) => format!("the encodings {encodings}, the checksum keyed by {key}"),
        };
        writeln!(out, "the files are written in {formats}").map_err(|e| format!("cannot write: {e}"))
    })
}
