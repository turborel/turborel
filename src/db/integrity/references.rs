//! \[EXTENSION\] Referential constraints at the end of a statement [ISO/IEC
//! 9075:1992 §11.8]: whether a row's referencing values match [GR2, §8.10],
//! the rows that match a referenced row [GR3], and NO ACTION; the
//! referential actions are `integrity/actions.rs`'s. Split from
//! db/integrity.rs (RULES.md R-SIZE-2).
//!
//! Each search reads a row at a time: a check stops at the first row it
//! needs, and the rows an action is for are held as a list that spills
//! (RULES.md D38).

use std::ops::ControlFlow;
use std::sync::Arc;

use crate::catalog::{ForeignKey, MatchType, QualifiedName, TableDesc};
use crate::db::{State, api};
use crate::error::{Code, Result, fail};
use crate::exec::HeldRows;
use crate::index::Key;
use crate::spill::ListSpace;
use crate::storage::{Row, RowId, project};
use crate::txn::Txn;
use crate::value::{Value, compare};

/// What is called with each row a search finds, with its id: whether to go
/// on to the next.
pub(super) type Visit<'a> = &'a mut dyn FnMut(RowId, Row) -> Result<ControlFlow<()>>;

impl State {
    /// [§11.8 GR2, §8.10 GR2-GR4] Whether `values`, of a foreign key's
    /// referencing columns, match a row of its referenced table: with no
    /// match type, any null matches; with FULL, all null do, some not;
    /// with PARTIAL, all null do, else those not null must equal a row's.
    pub(super) fn matches(&self, tx: &Txn, fk: &ForeignKey, values: &[Value]) -> Result<bool> {
        let nulls = values.iter().filter(|v| v.is_null()).count();
        if nulls == values.len() {
            return Ok(true);
        }
        if nulls > 0 && fk.match_type != MatchType::Partial {
            return Ok(fk.match_type == MatchType::Simple);
        }
        // Those not null, which with no match type or FULL are all of them.
        let (columns, wanted): (Vec<usize>, Vec<Value>) =
            fk.referenced.iter().zip(values).filter(|(_, v)| !v.is_null()).map(|(&c, v)| (c, v.clone())).unzip();
        self.has_row_with(tx, &fk.table, &columns, &wanted)
    }

    /// Whether a row of `table` has `columns` equal to `values` [§8.2],
    /// none null: the first found ends the search.
    fn has_row_with(&self, tx: &Txn, table: &QualifiedName, columns: &[usize], values: &[Value]) -> Result<bool> {
        let mut found = false;
        self.each_row_with(tx, table, (columns, values), &mut |_, _| {
            found = true;
            Ok(ControlFlow::Break(()))
        })?;
        Ok(found)
    }

    /// Calls `each` with the rows of `table`, with their ids, whose
    /// `columns` equal `values` [§8.2], none null, until it breaks: through
    /// an index of exactly those columns if there is one, else read in
    /// full, a row at a time.
    fn each_row_with(&self, tx: &Txn, table: &QualifiedName, (columns, values): (&[usize], &[Value]), each: Visit<'_>) -> Result<()> {
        let equal = |row: &Row| columns.iter().zip(values).all(|(&c, v)| row.get(c).is_some_and(|x| equals(x, v)));
        let index = self
            .catalog
            .indexes(table)
            .into_iter()
            .find(|ix| ix.columns.len() == columns.len() && ix.columns.iter().all(|c| columns.contains(c)));
        let Some(ix) = index else { return self.each_row_where(tx, table, &equal, each) };
        self.read_lock(tx, table)?;
        let storage = self.storage_of(table)?;
        let key: Option<Vec<Value>> =
            ix.columns.iter().map(|c| columns.iter().position(|x| x == c).and_then(|at| values.get(at)).cloned()).collect();
        let key = Key(key.ok_or_else(|| api("[DESIGN] an index key is not the values looked up"))?);
        for id in self.index_of(&ix.name)?.rows(&key)? {
            if let Some(row) = storage.get(tx.id, id)?.filter(|r| equal(r))
                && each(id, row)?.is_break()
            {
                break;
            }
        }
        Ok(())
    }

    /// Calls `each` with the rows of `table`, with their ids, that `keep`
    /// keeps, until it breaks: read in full, a row at a time.
    pub(super) fn each_row_where(&self, tx: &Txn, table: &QualifiedName, keep: &dyn Fn(&Row) -> bool, each: Visit<'_>) -> Result<()> {
        self.read_lock(tx, table)?;
        let width = self.catalog.table(table).map_or(0, |t| t.columns.len());
        let all: Vec<usize> = (0..width).collect();
        for read in Arc::clone(self.storage_of(table)?).scan(tx.id, &all) {
            let (id, row) = read?;
            if keep(&row) && each(id, row)?.is_break() {
                break;
            }
        }
        Ok(())
    }

    /// [§11.8 GR3] The rows of `child` that match a referenced row whose
    /// referenced values are `key`, by `fk`: all found before any is acted
    /// on, held in `space` as a statement's object rows are (RULES.md D38).
    pub(super) fn matching_rows(
        &self,
        tx: &Txn,
        (child, fk): (&TableDesc, &ForeignKey),
        key: &[Value],
        space: ListSpace,
    ) -> Result<HeldRows<(RowId, Row)>> {
        let mut found = HeldRows::new(space);
        let mut keep = |id, row| found.push((id, row)).map(|()| ControlFlow::Continue(()));
        match fk.match_type {
            // Equal values: none null.
            MatchType::Simple | MatchType::Full => {
                if !key.iter().any(Value::is_null) {
                    self.each_row_with(tx, &child.name, (&fk.columns, key), &mut keep)?;
                }
            }
            MatchType::Partial => {
                let matching = |row: &Row| project(row, &fk.columns).is_some_and(|v| partially(&v, key));
                self.each_row_where(tx, &child.name, &matching, &mut keep)?;
            }
        }
        Ok(found)
    }

    /// [§11.8 GR2] NO ACTION: once `old`, a row of the table foreign key
    /// `fk` (`name`) of `child` references, is deleted, or row `id` updated
    /// from it, every row that matched it still matches.
    pub(super) fn check_referenced(
        &self,
        tx: &Txn,
        (child, name, fk): (&TableDesc, &QualifiedName, &ForeignKey),
        old: &Row,
        id: Option<RowId>,
    ) -> Result<()> {
        if self.orphans(tx, (child, fk), old, id)? {
            return fail(
                Code::Constraint,
                format!(
                    "[ISO/IEC 9075:1992 §11.8 GR2] a row of {} references a row of {} no longer there: FOREIGN KEY {name}",
                    child.name, fk.table
                ),
            );
        }
        Ok(())
    }

    /// Whether rows of `child` that matched what `old`, a row of `fk`'s
    /// referenced table, was, by `fk`, match no row now that it was deleted
    /// or row `id` was updated from it.
    fn orphans(&self, tx: &Txn, (child, fk): (&TableDesc, &ForeignKey), old: &Row, id: Option<RowId>) -> Result<bool> {
        let key = project(old, &fk.referenced).ok_or_else(|| api(format!("[DESIGN] a row of {} is short", fk.table)))?;
        if fk.match_type == MatchType::Partial {
            // Each matching row in turn, until one matches no row now.
            let mut orphaned = false;
            let matching = |row: &Row| project(row, &fk.columns).is_some_and(|v| partially(&v, &key));
            self.each_row_where(tx, &child.name, &matching, &mut |_, row| {
                let values = project(&row, &fk.columns).ok_or_else(|| api(format!("[DESIGN] a row of {} is short", child.name)))?;
                orphaned = !self.matches(tx, fk, &values)?;
                Ok(if orphaned { ControlFlow::Break(()) } else { ControlFlow::Continue(()) })
            })?;
            return Ok(orphaned);
        }
        if key.iter().any(Value::is_null) {
            return Ok(false);
        }
        if let Some(id) = id {
            let now = self.storage_of(&fk.table)?.get(tx.id, id)?;
            if now.as_ref().and_then(|r| project(r, &fk.referenced)).is_some_and(|k| same(&k, &key)) {
                return Ok(false);
            }
        }
        if self.has_row_with(tx, &fk.table, &fk.referenced, &key)? {
            return Ok(false);
        }
        self.has_row_with(tx, &child.name, &fk.columns, &key)
    }
}

/// [§11.8 GR3b i] Whether referencing `values` match referenced `key` with
/// MATCH PARTIAL: one at least not null, and each not null equal to its
/// referenced value.
pub(super) fn partially(values: &[Value], key: &[Value]) -> bool {
    values.iter().any(|v| !v.is_null()) && values.iter().zip(key).all(|(v, k)| v.is_null() || equals(v, k))
}

/// Whether `a` equals `b` [§8.2]: neither null.
fn equals(a: &Value, b: &Value) -> bool {
    compare(a, b) == Some(std::cmp::Ordering::Equal)
}

/// Whether `a` and `b` are not distinct [ISO/IEC 9075:1992 §3.1]: equal,
/// or both null.
pub(super) fn same_value(a: &Value, b: &Value) -> bool {
    (a.is_null() && b.is_null()) || equals(a, b)
}

/// Whether the values `a` and `b` are not distinct, each pair.
fn same(a: &[Value], b: &[Value]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_value(x, y))
}
