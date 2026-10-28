//! \[EXTENSION\] Table settings (RULES.md D41): the options a storage engine
//! declares for its tables (`StorageEngine::table_options`), and the values
//! a table sets of them, written `CREATE TABLE ... USING engine WITH (name
//! = value, ...)` and changed by ALTER TABLE. A table keeps only the values
//! it sets (`TableDesc::options`); the rest are their defaults. An option
//! changes in place, the table's storage told (`TableStorage::reconfigure`),
//! or by a rebuild, the rows copied to new storage made with it. Split from
//! storage.rs (RULES.md R-SIZE-2).

use crate::error::{Code, Result, SqlError};
use crate::settings::{Kind, accepted};
use crate::value::Value;

/// How a change of a table option is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rebuild {
    /// In place: the table's storage is told (`TableStorage::reconfigure`).
    No,
    /// By a rebuild: new storage, made with the option, the rows copied to it.
    Yes,
}

/// \[EXTENSION\] An option of an engine's tables, as the engine declares it.
#[derive(Debug, Clone)]
pub struct TableOption {
    /// Its name: an `<identifier>`, upper case, its parts joined by periods.
    pub name: String,
    /// The values it takes.
    pub kind: Kind,
    /// Its value when a table does not set it, of its kind.
    pub default: Value,
    /// How a change of it is made.
    pub rebuild: Rebuild,
    /// What it does, one sentence.
    pub doc: String,
}

impl TableOption {
    /// The option `name`, of `kind`, `default` until a table sets it.
    pub fn new((name, kind, default): (&str, Kind, Value), rebuild: Rebuild, doc: &str) -> TableOption {
        TableOption { name: name.trim().to_ascii_uppercase(), kind, default, rebuild, doc: doc.to_owned() }
    }
}

/// \[EXTENSION\] The database's own option of every table, of any engine
/// (RULES.md D41, D48): the registered encodings, by commas, each with its
/// argument after a colon, that the table's rows pass through: in its
/// engine's storage, for an engine that seals it
/// (`StorageEngine::seals`), and in the log.
pub const ENCODINGS: &str = "ENCODINGS";

/// The option `ENCODINGS`, as the database declares it.
pub fn encodings_option() -> TableOption {
    TableOption::new(
        (ENCODINGS, Kind::Encodings, Value::Char(String::new())),
        Rebuild::No,
        "The registered encodings, by commas, such as a compression or an encryption with its key (AES-256-GCM:KEY), that the table's rows pass through as they are written, in its storage and the log; rows written before a change are read through their own.",
    )
}

/// The encodings `options`, a table's, set it to pass its rows through.
pub fn encodings_of(options: &[(String, Value)]) -> &str {
    match options.iter().find(|(n, _)| n == ENCODINGS) {
        Some((_, Value::Char(names))) => names,
        Some(_) | None => "",
    }
}

/// The value of option `name` for a table that sets `set`, of those
/// `declared`: the table's, else the option's default; none if it is not
/// declared.
pub fn option_value<'a>(declared: &'a [TableOption], set: &'a [(String, Value)], name: &str) -> Option<&'a Value> {
    let option = declared.iter().find(|o| o.name == name)?;
    Some(set.iter().find(|(n, _)| *n == name).map_or(&option.default, |(_, v)| v))
}

/// `set`, a table's values, with option `name` of those `declared` by
/// engine `engine` given `value`, or its default again if none: the value
/// taken of the option's kind, the values kept in order of name.
///
/// # Errors
///
/// SQLCODE -103 for an option `engine` does not declare, or a value not of
/// its option's kind or range.
pub fn chosen(
    engine: &str,
    (declared, set): (&[TableOption], &[(String, Value)]),
    (name, value): (&str, Option<&Value>),
) -> Result<Vec<(String, Value)>> {
    let name = name.trim().to_ascii_uppercase();
    let option = declared.iter().find(|o| o.name == name).ok_or_else(|| refused(&format!("{engine} takes no option {name}")))?;
    let mut out: Vec<(String, Value)> = set.iter().filter(|(n, _)| *n != name).cloned().collect();
    if let Some(value) = value {
        let taken = accepted(&name, &option.kind, value).map_err(|e| refused(&e.msg))?;
        out.push((name, taken));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Whether changing a table's values from `before` to `after`, of the
/// options `declared`, needs a rebuild.
pub fn needs_rebuild(declared: &[TableOption], before: &[(String, Value)], after: &[(String, Value)]) -> bool {
    declared.iter().filter(|o| o.rebuild == Rebuild::Yes).any(|o| {
        let shown = |set: &[(String, Value)]| option_value(declared, set, &o.name).map(Value::to_literal);
        shown(before) != shown(after)
    })
}

/// A table option refused: a Syntax Rule of this extension (SQLCODE -103).
fn refused(msg: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[EXTENSION] {msg}"))
}
