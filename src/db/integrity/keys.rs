//! \[EXTENSION\] UNIQUE and PRIMARY KEY checked at the end of a statement
//! [X3.135-1986 §4.5, §6.4 GR2; ISO/IEC 9075:1992 §11.7 GR2], or when a
//! deferred one becomes immediate [§10.6]: the rows a statement wrote, of
//! one constraint, checked as they come, one at a time (`KeyCheck`), so
//! none is held for the check, however many there are (RULES.md D38).

use std::collections::HashSet;
use std::sync::Arc;

use crate::catalog::{ConstraintKind, IndexDesc, QualifiedName, TableDesc, global_index_name};
use crate::db::partitions::dispatch::Dispatched;
use crate::db::{State, api};
use crate::error::{Code, Result, SqlError, Subject, fail};
use crate::index::{Key, KeyOrder, Probe};
use crate::storage::{Near, Newest, Row, RowId, TableStorage, project};
use crate::txn::Txn;
use crate::value::CompOp;
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
    /// The key of its index: its columns, and of one WITHOUT OVERLAPS, the
    /// period's start and end columns after them (`overlaps`).
    key: Vec<usize>,
    overlaps: bool,
    /// Whether it is a PRIMARY KEY.
    primary: bool,
    /// Whether NULLS DISTINCT [ISO/IEC 9075-2:2023 §11.7 SR5]: a key with
    /// a null then a duplicate of none.
    nulls_distinct: bool,
    /// The indexes of exactly its columns that are unique.
    unique: Vec<&'s IndexDesc>,
    /// Of a PRIMARY KEY, the first failure of a row not for a null, told
    /// at its end.
    failed: Option<SqlError>,
    /// [ISO/IEC 9075-2:2023 §4.17.9, §4.25.1] Of a constraint of a typed
    /// table with proper subtables, every table of its family but
    /// `desc`'s, whose rows are the constraint's too; and the unique index
    /// of its columns, of the table whose constraint it is, which gives
    /// each row's key. Empty of another.
    family: Vec<QualifiedName>,
    keyed: Option<&'s IndexDesc>,
    /// \[EXTENSION\] Of a constraint of a partitioned table a global index
    /// holds, the index and its rows, read from their partitions (RULES.md
    /// D157).
    global: Option<(&'s IndexDesc, Dispatched)>,
    /// What its table keeps from one row's read to the next.
    near: Near,
}

impl<'s> KeyCheck<'s> {
    /// The check of constraint `name`, UNIQUE or PRIMARY KEY as `kind`
    /// says, of the table `desc`, in transaction `txn`; none of another
    /// kind.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the table has no storage.
    pub(in crate::db) fn new(
        (st, txn): (&'s State, &'s Txn),
        desc: &'s TableDesc,
        (name, kind): (&'s QualifiedName, &'s ConstraintKind),
    ) -> Result<Option<KeyCheck<'s>>> {
        let ConstraintKind::Unique { columns, primary, nulls_distinct, period } = kind else { return Ok(None) };
        let storage = st.storage_of(&desc.name)?;
        let (key, overlaps) = (desc.key_columns(columns, period.as_deref()), period.is_some());
        let indexes = st.catalog.indexes(&desc.name).into_iter();
        let unique: Vec<&IndexDesc> = indexes.filter(|ix| ix.unique && ix.columns == key).collect();
        let (primary, nulls_distinct) = (*primary, *nulls_distinct);
        // The table whose constraint it is: `desc`'s, or a supertable's.
        let owner = std::iter::once(desc.name.clone())
            .chain(st.above(&desc.name))
            .find(|t| st.catalog.table(t).is_some_and(|d| d.constraints.iter().any(|c| c.name == *name)))
            .unwrap_or_else(|| desc.name.clone());
        let mut family: Vec<QualifiedName> = st.family(&owner).into_iter().filter(|t| *t != desc.name).collect();
        // [EXTENSION] a key a partition's own unique index enforces for the
        // whole tree, its rows' key in no other partition (RULES.md D157).
        let partition = st.catalog.partitioning().of(&desc.name).is_some();
        if !unique.is_empty() && partition {
            family.clear();
        }
        // [EXTENSION] a key a global index holds for the tree, checked
        // through it, not in each partition (RULES.md D157).
        // Its name made only of a partition's (RULES.md D415).
        let template = partition.then(|| global_index_name(&format!("{}.{}.{}", owner.auth, owner.ident, name.ident)));
        let global = template.and_then(|t| st.catalog.index(&t)).map(|ix| (ix, Dispatched::of(st, &ix.table)));
        if global.is_some() {
            family.clear();
        }
        let keyed = st.catalog.indexes(&owner).into_iter().find(|ix| ix.unique && ix.columns == key).or_else(|| unique.first().copied());
        let failed = None;
        let check = KeyCheck {
            st,
            txn,
            desc,
            storage,
            name,
            columns,
            key,
            overlaps,
            primary,
            nulls_distinct,
            unique,
            failed,
            family,
            keyed,
            global,
            near: Near::default(),
        };
        Ok(Some(check))
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
        // (`Txn::alone`): its index holds its key, and a row of its key
        // inserted after was not, and is read itself.
        // Of one WITHOUT OVERLAPS, a row whose key is its own may still
        // overlap another's period.
        if !self.overlaps && self.family.is_empty() && self.unique.iter().any(|ix| self.txn.alone(&ix.name, id)) {
            return Ok(());
        }
        let Some(row) = self.storage.get_near(self.txn.id, id, None, &mut self.near)? else { return Ok(()) };
        let key = self.key_of(&row)?;
        if self.primary {
            let null = key.0.iter().position(Value::is_null);
            if let Some(column) = null.and_then(|at| self.columns.get(at)).and_then(|&c| self.desc.columns.get(c)) {
                let (column, table) = (&column.name, &self.desc.name);
                return fail(Code::Constraint, format!("[ISO/IEC 9075:1992 §11.7 SR3a] PRIMARY KEY column {column} of {table} is null"));
            }
        }
        // A key with a null is never a duplicate [X3.135-1986 §6.4 GR2],
        // but of NULLS NOT DISTINCT, whose index holds it, a null there
        // equal to a null [ISO/IEC 9075-2:2023 §4.25.3.2].
        if self.failed.is_some() || self.nulls_distinct && key.0.iter().any(Value::is_null) {
            return Ok(());
        }
        // [DESIGN] the key's values, for a client's structured failure (RULES.md D186).
        let duplicate = self.duplicate(&key).map_err(|e| self.about(e).with_detail(held_by_another(&key)));
        match (duplicate, self.primary) {
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
        self.failed.map_or(Ok(()), |e| Err(if e.code == Code::Constraint && e.about_nothing() { e.about(subject) } else { e }))
    }

    /// `e`, about the constraint and its table if it is about nothing.
    fn about(&self, e: SqlError) -> SqlError {
        if e.code == Code::Constraint && e.about_nothing() {
            return e.about(Subject::constraint(self.name.parts(), self.desc.name.parts()));
        }
        e
    }

    /// The key of `row`.
    fn key_of(&self, row: &Row) -> Result<Key> {
        let key = match self.keyed {
            Some(ix) => ix.key_of(row),
            None => project(row, &self.key).map(Key),
        };
        key.ok_or_else(|| api(format!("[DESIGN] a row of {} is shorter than its columns", self.desc.name)))
    }

    /// Whether `key`, of a row written, is held by another row too, through
    /// each unique index: the row's own entry alone, no other row may hold
    /// it.
    fn duplicate(&self, key: &Key) -> Result<()> {
        if self.overlaps {
            return self.overlapping(key);
        }
        if let Some((ix, rows)) = &self.global {
            return self.global_duplicate(ix, rows, key);
        }
        if !self.family.is_empty() {
            return self.family_duplicate(key);
        }
        for ix in &self.unique {
            let found = self.st.index_of(&ix.name)?.rows(key)?;
            if found.len() < 2 {
                continue;
            }
            let mut holders = 0_usize;
            for other in found {
                let newest = self.storage.newest(self.txn.id, other)?;
                // [DESIGN] RULES.md D441: a check of the newest state rests on it.
                self.txn.depend(newest.stamp);
                holders = holders.saturating_add(usize::from(holds(&newest, &|row| Ok(self.key_of(row)? == *key))?));
            }
            if holders > 1 {
                return fail(Code::Constraint, duplicated(self.desc, self.name, self.columns));
            }
        }
        Ok(())
    }
}

impl KeyCheck<'_> {
    /// [ISO/IEC 9075-2:2023 §4.25.3.2, §11.7 SR6d i] `duplicate`, of a
    /// constraint WITHOUT OVERLAPS: no other row of `key`'s values in its
    /// columns has a period that overlaps `key`'s, whose start and end
    /// follow them. Through its index, of those rows whose period starts
    /// before `key`'s ends, if its method finds them by their first
    /// columns; else read whole.
    fn overlapping(&self, key: &Key) -> Result<()> {
        let n = self.columns.len();
        let (Some(values), Some(end)) = (key.0.get(..n), key.0.get(n.saturating_add(1))) else { return Ok(()) };
        let width = self.key.len();
        let method = |ix: &&&IndexDesc| self.st.methods.get(&ix.method).is_some_and(|m| m.supports_prefix(width, n, Some(CompOp::Lt)));
        // A null equal to a null, of NULLS NOT DISTINCT, is equal to none
        // as a probe compares.
        let found = self.unique.iter().find(method).filter(|_| !values.iter().any(Value::is_null));
        let Some(ix) = found else { return self.overlapping_read(key) };
        let probe = Probe::Prefix { equal: values.to_vec(), then: Some((CompOp::Lt, end.clone())) };
        let (mut holders, mut seen) = (0_usize, HashSet::new());
        for entry in Arc::clone(self.st.index_of(&ix.name)?).search(&probe, KeyOrder::Ascending) {
            // A row the transaction changed has an entry of each key it had.
            let id = entry?.1;
            if !seen.insert(id) {
                continue;
            }
            let newest = self.storage.newest(self.txn.id, id)?;
            // [DESIGN] RULES.md D441: a check of the newest state rests on it.
            self.txn.depend(newest.stamp);
            holders = holders.saturating_add(usize::from(holds(&newest, &|row| Ok(overlap(&self.key_of(row)?, key, n)))?));
            if holders > 1 {
                return fail(Code::Constraint, duplicated(self.desc, self.name, self.columns));
            }
        }
        Ok(())
    }

    /// `overlapping`, the table read whole.
    fn overlapping_read(&self, key: &Key) -> Result<()> {
        self.st.read_lock(self.txn, &self.desc.name)?;
        let all: Vec<usize> = (0..self.desc.columns.len()).collect();
        let mut holders = 0_usize;
        for read in Arc::clone(self.storage).scan(self.txn.id, &all) {
            let (_, row) = read?;
            holders = holders.saturating_add(usize::from(overlap(&self.key_of(&row)?, key, self.columns.len())));
            if holders > 1 {
                return fail(Code::Constraint, duplicated(self.desc, self.name, self.columns));
            }
        }
        Ok(())
    }

    /// \[EXTENSION\] `duplicate`, of a constraint a global index `ix`
    /// holds. No two rows its entries name hold `key`, each read from its
    /// partition in `rows` (RULES.md D157).
    fn global_duplicate(&self, ix: &IndexDesc, rows: &Dispatched, key: &Key) -> Result<()> {
        let mut holders = 0_usize;
        for other in self.st.index_of(&ix.name)?.rows(key)? {
            let newest = rows.newest(self.txn.id, other)?;
            // [DESIGN] RULES.md D441: a check of the newest state rests on it.
            self.txn.depend(newest.stamp);
            holders = holders.saturating_add(usize::from(holds(&newest, &|row| Ok(self.key_of(row)? == *key))?));
            if holders > 1 {
                return fail(Code::Constraint, duplicated(self.desc, self.name, self.columns));
            }
        }
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §4.25.1] `duplicate`, of a constraint of a
    /// typed table with proper subtables: `key` is held by no two rows of
    /// the tables of its family, the row's own table first.
    fn family_duplicate(&self, key: &Key) -> Result<()> {
        let mut holders = 0_usize;
        for table in std::iter::once(&self.desc.name).chain(&self.family) {
            holders = holders.saturating_add(self.holders_in(table, key)?);
            if holders > 1 {
                return fail(Code::Constraint, duplicated(self.desc, self.name, self.columns));
            }
        }
        Ok(())
    }

    /// How many rows of `table` hold `key`: through an index of exactly
    /// the constraint's columns, if it has one, else read whole.
    fn holders_in(&self, table: &QualifiedName, key: &Key) -> Result<usize> {
        let storage = self.st.storage_of(table)?;
        let indexed = self.st.catalog.indexes(table).into_iter().find(|ix| ix.columns == self.key);
        let mut holders = 0_usize;
        if let Some(ix) = indexed {
            for other in self.st.index_of(&ix.name)?.rows(key)? {
                let newest = storage.newest(self.txn.id, other)?;
                // [DESIGN] RULES.md D441: a check of the newest state rests on it.
                self.txn.depend(newest.stamp);
                holders = holders.saturating_add(usize::from(holds(&newest, &|row| Ok(self.key_of(row)? == *key))?));
            }
            return Ok(holders);
        }
        self.st.read_lock(self.txn, table)?;
        let width = self.st.catalog.table(table).map_or(0, |d| d.columns.len());
        let all: Vec<usize> = (0..width).collect();
        for read in Arc::clone(storage).scan(self.txn.id, &all) {
            let (_, row) = read?;
            holders = holders.saturating_add(usize::from(self.key_of(&row)? == *key));
        }
        Ok(holders)
    }
}

/// Whether keys `a` and `b`, each `n` values then a period's start and
/// end, are of equal values whose periods overlap [ISO/IEC 9075-2:2023
/// §4.16.2]: each starts before the other ends.
fn overlap(a: &Key, b: &Key, n: usize) -> bool {
    let one = |k: &Key, at: usize| Key(k.0.get(at).cloned().into_iter().collect());
    let (a_start, a_end, b_start, b_end) = (one(a, n), one(a, n.saturating_add(1)), one(b, n), one(b, n.saturating_add(1)));
    a.0.get(..n).map(|v| Key(v.to_vec())) == b.0.get(..n).map(|v| Key(v.to_vec())) && a_start < b_end && b_start < a_end
}

/// Whether a row whose newest state is `newest` holds a key, as `has_key`
/// finds it; refused with SQLCODE -911 while another transaction's change
/// to it leaves it in doubt.
fn holds(newest: &Newest, has_key: &dyn Fn(&Row) -> Result<bool>) -> Result<bool> {
    let has = |row: &Option<Row>| row.as_ref().map_or(Ok(false), has_key);
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

/// \[DESIGN\] What a key another row holds says, as a structured failure's
/// detail (RULES.md D186): its values as SQL writes them.
fn held_by_another(key: &Key) -> String {
    let values: Vec<String> = key.0.iter().map(Value::to_literal).collect();
    format!("the key ({}) is held by another row", values.join(", "))
}

/// The message of a duplicate of `columns` of the table `desc`: its
/// UNIQUE or PRIMARY KEY constraint `name`, as GET DIAGNOSTICS names it,
/// and of one WITHOUT OVERLAPS, its period.
fn duplicated(desc: &TableDesc, name: &QualifiedName, columns: &[usize]) -> String {
    let names: Vec<&str> = columns.iter().filter_map(|&c| desc.columns.get(c)).map(|c| c.name.as_str()).collect();
    let constraint = desc.constraints.iter().find_map(|c| match &c.kind {
        ConstraintKind::Unique { primary, period, .. } => {
            (c.name == *name).then_some((if *primary { "PRIMARY KEY" } else { "UNIQUE" }, &c.name, period))
        }
        ConstraintKind::NotNull(_) | ConstraintKind::ForeignKey(_) | ConstraintKind::Check { .. } | ConstraintKind::Connection(_) => None,
    });
    let Some((kind, constraint, Some(period))) = constraint else {
        let (kind, constraint) = constraint.map_or(("UNIQUE", String::new()), |(k, n, _)| (k, format!(" {n}")));
        return format!(
            "[X3.135-1986 §6.4 GR2] {kind}{constraint} ({}) of {} would be violated: a row of these values is there",
            names.join(", "),
            desc.name
        );
    };
    format!(
        "[ISO/IEC 9075-2:2023 §11.7 SR6d i] {kind} {constraint} ({}, {period} WITHOUT OVERLAPS) of {} would be violated: a row of these values is there for a time in its period",
        names.join(", "),
        desc.name
    )
}
