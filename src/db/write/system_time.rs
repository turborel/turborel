//! System-time periods [ISO/IEC 9075-2:2023 §4.17.2.3, §4.44]: each row a
//! statement writes to a table with one stamped, its start the transaction
//! timestamp, its end the highest value of its type [§15.11 GR3b, GR3c;
//! §15.14 GR10a iii 1 C II]; a row changed whose start is later refused
//! [§9.17 GR16a iii 1; §15.14 GR10a iii 1 A]; and of a system-versioned
//! table, a row updated or deleted whose start is earlier kept as a
//! historical system row, its end the transaction timestamp [§9.17
//! GR16a iii 3; §15.14 GR10a iii 1 C I].
//!
//! \[DESIGN\] The historical system rows of a table are kept in a table of
//! their own, its history, in DEFINITION_SCHEMA, of its columns and engine
//! (RULES.md D82): a read of the table's current rows, an index of it and
//! a check of its constraints find none of them.

use crate::catalog::{ColumnDesc, QualifiedName, TableDesc};
use crate::db::State;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::storage::Row;
use crate::system::DEFINITION_SCHEMA;
use crate::txn::Txn;
use crate::value::datetime::Now;
use crate::value::types::datetime::{cast_at, current, highest};
use crate::value::{Value, compare};

/// [DESIGN] The words a history's name begins with (RULES.md D82).
const HISTORY: &str = "SYSTEM HISTORY";

/// [DESIGN] The history of base table `table`: in DEFINITION_SCHEMA, its
/// schema's length first, so no two tables' names meet (RULES.md D82).
pub(crate) fn history_name(table: &QualifiedName) -> QualifiedName {
    let ident = format!("{HISTORY} {} {}{}", table.auth.chars().count(), table.auth, table.ident);
    QualifiedName { auth: DEFINITION_SCHEMA.to_owned(), ident }
}

/// [ISO/IEC 9075-2:2023 §4.17.2.3] What a statement of a transaction writes
/// to the system-time period columns of a table's rows.
#[derive(Debug, Clone)]
pub(crate) struct Stamp {
    /// The positions of the start and end columns.
    columns: [usize; 2],
    /// The transaction timestamp, CAST to the start column's type.
    start: Value,
    /// The transaction timestamp, CAST to the end column's type: the end
    /// of a historical system row.
    ended: Value,
    /// The highest value of the end column's type: the end of a current
    /// system row.
    highest: Value,
    /// Of a system-versioned table, its history.
    history: Option<QualifiedName>,
}

/// What `Stamp::judge` finds of a row changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Changed {
    /// [§15.14 GR10a iii 1 B] Changed in place: begun by this transaction.
    InPlace,
    /// [GR10a iii 1 C] Kept as a historical system row, of a
    /// system-versioned table.
    Kept,
}

impl Stamp {
    /// `row` given the start and end of a current system row [§15.11
    /// GR3b, GR3c; §15.14 GR10a iii 1 C II].
    pub(crate) fn put(&self, row: &mut Row) {
        let [start, end] = self.columns;
        if let Some(slot) = row.get_mut(start) {
            *slot = self.start.clone();
        }
        if let Some(slot) = row.get_mut(end) {
            *slot = self.highest.clone();
        }
    }

    /// [ISO/IEC 9075-2:2023 §9.17 GR16a iii, §15.14 GR10a iii 1] Of `old`,
    /// a row updated or deleted: whether it is changed in place, begun by
    /// this transaction, or kept as a historical system row, of a
    /// system-versioned table, its history named.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2201H if it begins later than the transaction timestamp.
    pub(crate) fn judge(&self, old: &Row) -> Result<(Changed, Option<&QualifiedName>)> {
        let begun = old.get(self.columns[0]).unwrap_or(&Value::Null);
        match compare(&self.start, begun) {
            Some(std::cmp::Ordering::Less) => Err(SqlError::new(
                Code::Data,
                "[ISO/IEC 9075-2:2023 §9.17 GR16a iii 1, §15.14 GR10a iii 1 A] a row changed begins later than the transaction timestamp",
            )
            .with_state(SqlState::INVALID_ROW_VERSION)),
            Some(std::cmp::Ordering::Equal) => Ok((Changed::InPlace, None)),
            Some(std::cmp::Ordering::Greater) | None => Ok((Changed::Kept, self.history.as_ref())),
        }
    }

    /// [§9.17 GR16a iii 3, §15.14 GR10a iii 1 C I] `old`, a row replaced
    /// or deleted, as its historical system row: its end the transaction
    /// timestamp.
    pub(crate) fn ended(&self, old: &Row) -> Row {
        let mut row = old.clone();
        if let Some(slot) = row.get_mut(self.columns[1]) {
            *slot = self.ended.clone();
        }
        row
    }
}

impl State {
    /// [ISO/IEC 9075-2:2023 §11.9 SR1, §11.47 SR5, §11.49 SR12] That of
    /// `uses`, the columns `what` references, by their tables, none is a
    /// system-time period start or end column, of a system-versioned table
    /// alone if `versioned`, as `cited` says.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if one is.
    pub(in crate::db) fn no_system_time(
        &self,
        uses: &[(QualifiedName, String)],
        (what, cited): (&str, &str),
        versioned: bool,
    ) -> Result<()> {
        for (table, column) in uses {
            let Some(desc) = self.catalog.table(table) else { continue };
            let system = desc.column(column).and_then(|at| desc.columns.get(at)).and_then(ColumnDesc::system_time).is_some();
            if system && (!versioned || self.catalog.table(&history_name(table)).is_some()) {
                let msg = format!("[ISO/IEC 9075-2:2023 {cited}] {what} references {table}.{column}, a system-time period column");
                return Err(SqlError::new(Code::SyntaxRule, msg));
            }
        }
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §4.44] The transaction timestamp of `tx`: the
    /// clock's instant as it first changes data, the same for the rest of
    /// it, of TIMESTAMP(6) WITH TIME ZONE at +00:00 (IV072), so every
    /// session's rows are stamped in one time zone.
    pub(crate) fn transaction_timestamp(&self, tx: &Txn) -> Value {
        let utc = tx.stamp(|| crate::value::datetime::utc_of_unix(self.clock.now()));
        current(crate::value::datetime::Moment::Timestamp, 6, Now { utc, zone: 0 })
    }

    /// [ISO/IEC 9075-2:2023 §4.17.2.3] What `tx` writes to the system-time
    /// period columns of the rows of `table`, if it has a system-time
    /// period.
    ///
    /// # Errors
    ///
    /// The transaction timestamp CAST to a column's type fails.
    pub(crate) fn stamp_of(&self, tx: &Txn, table: &QualifiedName) -> Result<Option<Stamp>> {
        let Some(desc) = self.catalog.table(table) else { return Ok(None) };
        self.stamp_for(tx, desc)
    }

    /// `stamp_of` the table `desc`.
    ///
    /// # Errors
    ///
    /// As `stamp_of`.
    pub(in crate::db) fn stamp_for(&self, tx: &Txn, desc: &TableDesc) -> Result<Option<Stamp>> {
        let Some(columns) = system_time(desc) else { return Ok(None) };
        let types = columns.map(|at| desc.columns.get(at).map(|c| c.data_type.clone()));
        let [Some(start_type), Some(end_type)] = types else { return Ok(None) };
        let now = self.transaction_timestamp(tx);
        let utc = Now::default();
        let start = cast_at(now.clone(), &start_type, utc)?;
        let ended = cast_at(now, &end_type, utc)?;
        let highest = highest(&end_type).ok_or_else(|| crate::db::api(format!("[DESIGN] {} has a period of no datetime", desc.name)))?;
        let history = Some(history_name(&desc.name)).filter(|h| self.catalog.table(h).is_some());
        Ok(Some(Stamp { columns, start, ended, highest, history }))
    }
}

/// [ISO/IEC 9075-2:2023 §4.16.1] The positions of the start and end columns
/// of `desc`'s system-time period, if it has one.
pub(in crate::db) fn system_time(desc: &TableDesc) -> Option<[usize; 2]> {
    desc.period_columns(crate::ast::SYSTEM_TIME)
}
