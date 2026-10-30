//! \[EXTENSION\] UNIQUE and PRIMARY KEY checked at the end of a statement
//! [X3.135-1986 §4.5, §6.4 GR2; ISO/IEC 9075:1992 §11.7 GR2], or when a
//! deferred one becomes immediate [§10.6]: the rows a statement wrote, of
//! one constraint, checked as they come, one at a time (`KeyCheck`), so
//! none is held for the check, however many there are (RULES.md D38).
//! Split from db/integrity.rs (RULES.md R-SIZE-2).

use std::sync::Arc;

use crate::catalog::{ConstraintKind, QualifiedName, TableDesc};
use crate::db::{State, api};
use crate::error::{Code, Result, SqlError, Subject, fail};
use crate::index::Key;
use crate::storage::{Newest, Row, RowId, TableStorage, project};
use crate::txn::Txn;
use crate::value::Value;

/// [X3.135-1986 §6.4 GR2] UNIQUE on some columns of a table, "effectively
/// checked after the execution of each `<SQL statement>`" [X3.135-1986
/// §4.5], or deferred [ISO/IEC 9075:1992 §10.6]: rows may pass through
/// duplicate keys meanwhile (as in SET ID = ID + 1), but not end with
/// them. The rows written are given to it one at a time (`row`), each
/// checked through the index of the constraint. Of a PRIMARY KEY, first,
/// none has a null in its columns [ISO/IEC 9075:1992 §11.7 SR3a]: so
/// another failure of one is told only once every row is (`finish`), a
/// null in a later row told before it. Each row is read once; a key whose
/// index holds only the row's own entry is no duplicate, and needs no
/// more.
///
/// \[DESIGN\] The rows counted are those whose newest version has the key,
/// whichever transaction committed it, as the transaction must not commit
/// a duplicate of a row it cannot see (RULES.md D29). A row that another
/// open transaction is changing, with the key before or after, is in
/// doubt: the statement is refused with SQLCODE -911, to be run again once
/// that transaction ends.
pub(in crate::db) struct KeyCheck<'s> {
    st: &'s State,
    txn: &'s Txn,
    desc: &'s TableDesc,
    storage: &'s Arc<dyn TableStorage>,
    /// The constraint's name, and its columns.
    name: &'s QualifiedName,
    columns: &'s [usize],
    /// Whether it is a PRIMARY KEY.
    primary: bool,
    /// The indexes of exactly its columns that are unique.
    unique: Vec<&'s str>,
    /// Of a PRIMARY KEY, the first failure of a row not for a null, told
    /// at its end.
    failed: Option<SqlError>,
}

impl<'s> KeyCheck<'s> {
    /// The check of constraint `name`, UNIQUE or PRIMARY KEY as `kind`
    /// says, of the table `desc`, in transaction `txn`; none of another
    /// kind.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the table has no storage.
    pub(in crate::db) fn new(
        (st, txn): (&'s State, &'s Txn),
        desc: &'s TableDesc,
        (name, kind): (&'s QualifiedName, &'s ConstraintKind),
    ) -> Result<Option<KeyCheck<'s>>> {
        let ConstraintKind::Unique { columns, primary } = kind else { return Ok(None) };
        let storage = st.storage_of(&desc.name)?;
        let indexes = st.catalog.indexes(&desc.name).into_iter();
        let unique = indexes.filter(|ix| ix.unique && ix.columns == *columns).map(|ix| ix.name.as_str()).collect();
        Ok(Some(KeyCheck { st, txn, desc, storage, name, columns, primary: *primary, unique, failed: None }))
    }

    /// Whether it checks constraint `name`.
    pub(in crate::db) fn checks(&self, name: &QualifiedName) -> bool {
        self.name == name
    }

    /// Row `id`, as the transaction now sees it, written.
    ///
    /// # Errors
    ///
    /// SQLCODE -201, SQLSTATE 23000, for a null in a PRIMARY KEY, or a
    /// duplicate, but of a PRIMARY KEY; -911 for a row in doubt, but of a
    /// PRIMARY KEY; the storage's and the index's.
    pub(in crate::db) fn row(&mut self, id: RowId) -> Result<()> {
        // None whose entry was its key's only one as it was inserted
        // (`Txn::alone`): its key has no null, and a row of its key
        // inserted after was not, and is read itself.
        if self.unique.iter().any(|ix| self.txn.alone(ix, id)) {
            return Ok(());
        }
        let Some(row) = self.storage.get(self.txn.id, id)? else { return Ok(()) };
        let key = self.key_of(&row)?;
        if self.primary {
            let null = key.0.iter().position(Value::is_null);
            if let Some(column) = null.and_then(|at| self.columns.get(at)).and_then(|&c| self.desc.columns.get(c)) {
                let (column, table) = (&column.name, &self.desc.name);
                return fail(Code::Constraint, format!("[ISO/IEC 9075:1992 §11.7 SR3a] PRIMARY KEY column {column} of {table} is null"));
            }
        }
        // A key with a null is never a duplicate [X3.135-1986 §6.4 GR2].
        if self.failed.is_some() || key.0.iter().any(Value::is_null) {
            return Ok(());
        }
        match (self.duplicate(&key), self.primary) {
            (Ok(()), _) => Ok(()),
            (Err(e), true) => {
                self.failed = Some(e);
                Ok(())
            }
            (Err(e), false) => Err(e),
        }
    }

    /// The end of the rows written: the failure held back, if any.
    ///
    /// # Errors
    ///
    /// That failure, about the constraint and its table.
    pub(in crate::db) fn finish(self) -> Result<()> {
        let subject = Subject::constraint(self.name.parts(), self.desc.name.parts());
        self.failed.map_or(Ok(()), |e| Err(if e.code == Code::Constraint && e.subject.is_none() { e.about(subject) } else { e }))
    }

    /// The key of `row`.
    fn key_of(&self, row: &Row) -> Result<Key> {
        project(row, self.columns).map(Key).ok_or_else(|| api(format!("[DESIGN] a row of {} is shorter than its columns", self.desc.name)))
    }

    /// Whether `key`, of a row written, is held by another row too, through
    /// each unique index: the row's own entry alone, no other row may hold
    /// it.
    fn duplicate(&self, key: &Key) -> Result<()> {
        for ix in &self.unique {
            let found = self.st.index_of(ix)?.rows(key)?;
            if found.len() < 2 {
                continue;
            }
            let mut holders = 0_usize;
            for other in found {
                let newest = self.storage.newest(self.txn.id, other)?;
                holders = holders.saturating_add(usize::from(holds(&newest, key, &|row| self.key_of(row))?));
            }
            if holders > 1 {
                return fail(Code::Constraint, duplicated(self.desc, self.columns));
            }
        }
        Ok(())
    }
}

/// Whether a row whose newest state is `newest` holds `key`, as `key_of`
/// finds it; refused with SQLCODE -911 while another transaction's change
/// to it leaves it in doubt.
fn holds(newest: &Newest, key: &Key, key_of: &dyn Fn(&Row) -> Result<Key>) -> Result<bool> {
    let has = |row: &Option<Row>| row.as_ref().map(key_of).transpose().map(|k| k.as_ref() == Some(key));
    let doubt = match &newest.pending {
        Some(pending) => has(&newest.row)? || has(pending)?,
        None => false,
    };
    if doubt {
        return fail(
            Code::Conflict,
            "[DESIGN] another transaction is changing a row with the same key; run the statement again".to_owned(),
        );
    }
    has(&newest.row)
}

/// The message of a duplicate of `columns` of the table `desc`: its
/// UNIQUE or PRIMARY KEY constraint named, as GET DIAGNOSTICS names it.
fn duplicated(desc: &TableDesc, columns: &[usize]) -> String {
    let names: Vec<&str> = columns.iter().filter_map(|&c| desc.columns.get(c)).map(|c| c.name.as_str()).collect();
    let constraint = desc.constraints.iter().find_map(|c| match &c.kind {
        ConstraintKind::Unique { columns: on, primary } => {
            (on == columns).then_some((if *primary { "PRIMARY KEY" } else { "UNIQUE" }, &c.name))
        }
        ConstraintKind::NotNull(_) | ConstraintKind::ForeignKey(_) | ConstraintKind::Check { .. } => None,
    });
    let (kind, constraint_name) = constraint.map_or(("UNIQUE", String::new()), |(k, n)| (k, format!(" {n}")));
    format!(
        "[X3.135-1986 §6.4 GR2] {kind}{constraint_name} ({}) of {} would be violated: a row of these values is there",
        names.join(", "),
        desc.name
    )
}
