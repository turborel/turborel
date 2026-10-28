//! \[EXTENSION\] [ISO/IEC 9075:1992 §21] System tables: tables whose rows
//! the database makes from its definitions when a statement reads them, not
//! from storage, and the views over them.
//!
//! * The Information Schema [§21.2], the schema INFORMATION_SCHEMA
//!   (`system/information.rs`): its views, each the standard's own
//!   definition over the Definition Schema, and its table
//!   INFORMATION_SCHEMA_CATALOG_NAME. SELECT on them is PUBLIC's, WITH GRANT
//!   OPTION, and no other privilege is granted, "so they cannot be
//!   updated" \[§21.1\].
//! * The Definition Schema [§21.3], DEFINITION_SCHEMA
//!   (`system/definition.rs`): "An implementation need do no more than
//!   simulate the existence of the Definition Schema, as viewed through the
//!   Information Schema views" \[§21.1\]; its tables are system tables
//!   that only those views read, as "the Definition Schema cannot be
//!   accessed in an SQL-statement" \[§21.1\].
//! * Tables of this implementation's: "An implementation may define
//!   objects that are associated with INFORMATION_SCHEMA that are not
//!   defined in this Clause" \[§21.1\]: the settings (`SETTINGS`, RULES.md
//!   D36, `system/settings.rs`).
//!
//! A system table is an extension point (`SystemTable`): a plug-in may add
//! one (`Database::register_system_table`), an engine's statistics, say.
//! Its rows are made as the statement runs, from the snapshot of the
//! definitions it runs against, as its user sees them, and as its
//! session's own settings are: a cursor over one keeps the rows of its OPEN.
//! Made then, not as the statement is bound, binding a view never makes
//! them: the Definition Schema's rows of a view's columns are found by
//! binding it.

mod definition;
mod information;
mod memory;
mod settings;
mod users;

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::catalog::{ColumnDesc, QualifiedName, TableDesc};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::lexer::MAX_IDENTIFIER_LENGTH;
use crate::settings::Own;
use crate::storage::Row;
use crate::value::{DataType, Decimal, INTEGER_PRECISION, Value, assign};

pub use information::{SystemView, view};
pub use settings::DESCRIPTION_LENGTH;

/// [ISO/IEC 9075:1992 §21.2.1] The schema of the Information Schema, and
/// its owner: CREATE SCHEMA INFORMATION_SCHEMA AUTHORIZATION
/// INFORMATION_SCHEMA.
pub const INFORMATION_SCHEMA: &str = "INFORMATION_SCHEMA";

/// [ISO/IEC 9075:1992 §21.3.2] The schema of the Definition Schema.
pub const DEFINITION_SCHEMA: &str = "DEFINITION_SCHEMA";

/// [ISO/IEC 9075:1992 §4.26] "Privilege descriptors that represent
/// privileges for the owner of an object have a special grantor value,
/// "_SYSTEM"."
pub const SYSTEM_GRANTOR: &str = "_SYSTEM";

/// What a system table's rows are made from: the snapshot of the
/// definitions the statement runs against, its user, and its session's own
/// settings.
pub struct Seen<'a> {
    /// The definitions, and what the database has registered.
    pub state: &'a State,
    /// The current authorization identifier of the statement.
    pub auth: &'a str,
    /// The session's own settings (`Session::set_setting`).
    pub own: &'a Own,
}

/// \[DESIGN\] A system table: an extension point (RULES.md D30).
///
/// Contract (RULES.md R-SAFE-3):
/// * `schema` and `name` are `<identifier>`s, upper case, unique together
///   among system tables, and constant; `columns` are constant, and named
///   as `<identifier>`s.
/// * Each row `rows` gives has a value for each column, of its type (a
///   character string no longer than its column, which pads it): the
///   executor checks, and refuses one that is not (SQLCODE -910).
/// * `rows` only reads what `Seen` gives; it may be called from several
///   threads at once, and never calls back into the `Database` or its
///   sessions (RULES.md R-ERR-4).
pub trait SystemTable: Send + Sync {
    /// Its schema: INFORMATION_SCHEMA, unless it says otherwise.
    fn schema(&self) -> &'static str {
        INFORMATION_SCHEMA
    }
    /// Its name, in its schema.
    fn name(&self) -> &'static str;
    /// Its columns, in order.
    fn columns(&self) -> Vec<ColumnDesc>;
    /// Its rows, as `seen` sees them.
    ///
    /// # Errors
    ///
    /// Any the table has making them.
    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>>;
}

/// The system tables, by name.
pub(crate) type SystemTables = HashMap<QualifiedName, Arc<dyn SystemTable>>;

/// The name of system table `table`.
pub(crate) fn name_of(table: &dyn SystemTable) -> QualifiedName {
    QualifiedName { auth: table.schema().to_owned(), ident: table.name().to_owned() }
}

/// The built-in system tables.
pub(crate) fn builtin() -> SystemTables {
    let mut tables: Vec<Arc<dyn SystemTable>> = vec![
        Arc::new(settings::Settings),
        Arc::new(users::Users),
        Arc::new(memory::MemoryReservations),
        Arc::new(information::CatalogName),
    ];
    tables.extend(definition::tables());
    tables.into_iter().map(|t| (name_of(&*t), t)).collect()
}

/// The description of system table `table`: no constraints, no engine.
pub(crate) fn desc_of(table: &dyn SystemTable) -> TableDesc {
    TableDesc {
        name: name_of(table),
        columns: table.columns(),
        constraints: Vec::new(),
        engine: String::new(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    }
}

/// \[DESIGN\] A read of a system table, as a plan carries it to the
/// executor: the table, and the user and settings its rows are made for.
#[derive(Clone)]
pub struct SystemRead {
    /// The table.
    pub(crate) table: Arc<dyn SystemTable>,
    /// The current authorization identifier of the statement.
    pub(crate) user: String,
    /// Its session's own settings.
    pub(crate) own: Own,
}

impl fmt::Debug for SystemRead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SystemRead({})", name_of(&*self.table))
    }
}

impl SystemRead {
    /// The table's name.
    pub(crate) fn name(&self) -> QualifiedName {
        name_of(&*self.table)
    }

    /// Its rows, made from the definitions `state`, each checked against
    /// the table's columns.
    ///
    /// # Errors
    ///
    /// The table's, or SQLCODE -910 if a row breaks its contract.
    pub(crate) fn rows(&self, state: &State) -> Result<Vec<Row>> {
        // [ISO/IEC 9075:1992 §13.11 GR5] a declared local temporary table is
        // in no view of the Information Schema.
        let shown = state.without_declared();
        let state = shown.as_ref().unwrap_or(state);
        let rows = self.table.rows(&Seen { state, auth: &self.user, own: &self.own })?;
        checked(&desc_of(&*self.table), rows)
    }
}

/// The rows `rows` of a system table of `desc`, each of its degree, its
/// values of their columns' types, a character string padded to its
/// column's length, none null in a NOT NULL column.
///
/// # Errors
///
/// SQLCODE -910 if one is not: the table breaks its contract.
fn checked(desc: &TableDesc, rows: Vec<Row>) -> Result<Vec<Row>> {
    let broken = |what: &str| SqlError::new(Code::Api, format!("[DESIGN] system table {} {what}", desc.name));
    rows.into_iter()
        .map(|row| {
            if row.len() != desc.columns.len() {
                return Err(broken(&format!("gave a row of {} values for {} columns", row.len(), desc.columns.len())));
            }
            row.into_iter()
                .zip(&desc.columns)
                .map(|(v, c)| {
                    if v.is_null() && c.not_null {
                        return Err(broken(&format!("gave a null for {}, which is NOT NULL", c.name)));
                    }
                    assign(v, &c.data_type, "[DESIGN] a system table's row")
                        .map_err(|e| broken(&format!("gave {}: {}", c.name, e.described())))
                })
                .collect()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Columns and values of the system tables
// ---------------------------------------------------------------------------

/// A column of CHARACTER(`length`), NOT NULL if `not_null`.
pub(crate) fn text_column((name, length): (&str, u32), not_null: bool) -> ColumnDesc {
    ColumnDesc { name: name.to_owned(), data_type: DataType::Character { length }, not_null, default: None, domain: None }
}

/// [ISO/IEC 9075:1992 §21.2.27] A column of SQL_IDENTIFIER, CHARACTER
/// VARYING(L), L the longest `<identifier>`: CHARACTER(L) here, as there is
/// no CHARACTER VARYING.
pub(crate) fn identifier(name: &str, not_null: bool) -> ColumnDesc {
    text_column((name, u32::try_from(MAX_IDENTIFIER_LENGTH).unwrap_or(u32::MAX)), not_null)
}

/// [ISO/IEC 9075:1992 §21.2.29] A column of CARDINAL_NUMBER: INTEGER, not
/// negative.
pub(crate) fn cardinal(name: &str, not_null: bool) -> ColumnDesc {
    ColumnDesc {
        name: name.to_owned(),
        data_type: DataType::Exact { precision: INTEGER_PRECISION, scale: 0 },
        not_null,
        default: None,
        domain: None,
    }
}

/// A character string value.
pub(crate) fn text(s: &str) -> Value {
    Value::Char(s.to_owned())
}

/// A whole number value, of a CARDINAL_NUMBER column.
pub(crate) fn number(n: usize) -> Value {
    Value::Exact(Decimal::new(i128::try_from(n).unwrap_or(i128::MAX), 0))
}

/// YES or NO, as the Information Schema says whether one is.
pub(crate) fn yes(is: bool) -> Value {
    text(if is { "YES" } else { "NO" })
}

/// [ISO/IEC 9075:1992 §21.2.2] The name of the catalog: the setting
/// CATALOG_NAME's.
pub(crate) fn catalog_name(seen: &Seen<'_>) -> Value {
    seen.state.settings.get(crate::settings::CATALOG_NAME).clone()
}

/// A system table its functions define: of `schema`, named `name`, with
/// the columns `columns` gives and the rows `rows` makes.
pub(crate) struct Defined {
    pub(crate) schema: &'static str,
    pub(crate) name: &'static str,
    pub(crate) columns: fn() -> Vec<ColumnDesc>,
    pub(crate) rows: Rows,
}

/// How a `Defined` table makes its rows.
#[derive(Clone, Copy)]
pub(crate) enum Rows {
    /// Always.
    Infallible(fn(&Seen<'_>) -> Vec<Row>),
    /// Or fails, binding a query to find them, say.
    Fallible(fn(&Seen<'_>) -> Result<Vec<Row>>),
}

impl SystemTable for Defined {
    fn schema(&self) -> &'static str {
        self.schema
    }
    fn name(&self) -> &'static str {
        self.name
    }
    fn columns(&self) -> Vec<ColumnDesc> {
        (self.columns)()
    }
    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        match self.rows {
            Rows::Infallible(made) => Ok(made(seen)),
            Rows::Fallible(made) => made(seen),
        }
    }
}
