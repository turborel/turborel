//! \[EXTENSION\] Referential constraints at the end of a statement [ISO/IEC
//! 9075:1992 §11.8]: whether a row's referencing values match [GR2, §8.10],
//! the rows that match a referenced row [GR3], the referential actions of
//! the rows deleted and updated [GR5, GR6], and NO ACTION. Split from
//! db/integrity.rs (RULES.md R-SIZE-2).
//!
//! With MATCH PARTIAL a row matches every referenced row its values not
//! null equal, so an action is done only to the rows it matches no other
//! row for: its unique matching rows [GR3b]. Which rows match which is
//! taken as it was before the statement [GR4]: the referenced table now,
//! with the rows the statement wrote as they were; the association of a
//! row with a referenced row deleted, or updated in a column it is not null
//! in, is dropped as that row is acted for, so the last of the rows a row
//! matched to go acts on it.

use std::collections::HashMap;
use std::sync::Arc;

use super::{Effect, Effects};
use crate::catalog::{ForeignKey, MatchType, QualifiedName, ReferentialAction, TableDesc};
use crate::db::defaults::default_row;
use crate::db::{State, api};
use crate::error::{Code, Result, SqlError, SqlState, Subject, fail};
use crate::index::Key;
use crate::storage::{Row, RowId, project};
use crate::txn::Txn;
use crate::value::{Value, compare};

/// [ISO/IEC 9075:1992 §11.8 GR7] The values given in a statement to each
/// column of each row, by the statement or its referential actions.
type Given = HashMap<(QualifiedName, RowId, usize), Value>;

/// What the referential actions of a statement keep while they run: the
/// values given [GR7], and, for MATCH PARTIAL, the referenced rows each
/// row of a foreign key is no longer associated with [GR4].
#[derive(Default)]
struct Acting {
    given: Given,
    dropped: HashMap<(QualifiedName, RowId), Vec<RowId>>,
}

/// Row `row` of a referenced table, `parent`, deleted, or (with its values
/// now) updated, from `old`.
struct Change<'a> {
    parent: &'a QualifiedName,
    row: RowId,
    old: &'a Row,
    new: Option<Row>,
}

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
        Ok(!self.rows_with(tx, &fk.table, &columns, &wanted)?.is_empty())
    }

    /// The rows of `table`, with their ids, whose `columns` equal `values`
    /// [§8.2], none null: through an index of exactly those columns if
    /// there is one, else read in full.
    fn rows_with(&self, tx: &Txn, table: &QualifiedName, columns: &[usize], values: &[Value]) -> Result<Vec<(RowId, Row)>> {
        self.read_lock(tx, table)?;
        let storage = self.storage_of(table)?;
        let equal = |row: &Row| columns.iter().zip(values).all(|(&c, v)| row.get(c).is_some_and(|x| equals(x, v)));
        let index = self
            .catalog
            .indexes(table)
            .into_iter()
            .find(|ix| ix.columns.len() == columns.len() && ix.columns.iter().all(|c| columns.contains(c)));
        if let Some(ix) = index {
            let key: Option<Vec<Value>> =
                ix.columns.iter().map(|c| columns.iter().position(|x| x == c).and_then(|at| values.get(at)).cloned()).collect();
            let key = Key(key.ok_or_else(|| api("[DESIGN] an index key is not the values looked up"))?);
            let mut found = Vec::new();
            for id in self.index_of(&ix.name)?.rows(&key)? {
                if let Some(row) = storage.get(tx.id, id)?.filter(|r| equal(r)) {
                    found.push((id, row));
                }
            }
            return Ok(found);
        }
        self.rows_where(tx, table, &equal)
    }

    /// The rows of `table`, with their ids, that `keep` keeps: read in full.
    fn rows_where(&self, tx: &Txn, table: &QualifiedName, keep: &dyn Fn(&Row) -> bool) -> Result<Vec<(RowId, Row)>> {
        self.read_lock(tx, table)?;
        let width = self.catalog.table(table).map_or(0, |t| t.columns.len());
        let all: Vec<usize> = (0..width).collect();
        Arc::clone(self.storage_of(table)?).scan(tx.id, &all).filter(|r| r.as_ref().map_or(true, |(_, row)| keep(row))).collect()
    }

    /// [§11.8 GR3] The rows of `child` that match a referenced row whose
    /// referenced values are `key`, by `fk`.
    fn matching_rows(&self, tx: &Txn, (child, fk): (&TableDesc, &ForeignKey), key: &[Value]) -> Result<Vec<(RowId, Row)>> {
        match fk.match_type {
            // Equal values: none null.
            MatchType::Simple | MatchType::Full => {
                if key.iter().any(Value::is_null) {
                    return Ok(Vec::new());
                }
                self.rows_with(tx, &child.name, &fk.columns, key)
            }
            MatchType::Partial => self.rows_where(tx, &child.name, &|row| project(row, &fk.columns).is_some_and(|v| partially(&v, key))),
        }
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
            for (_, row) in self.matching_rows(tx, (child, fk), &key)? {
                let values = project(&row, &fk.columns).ok_or_else(|| api(format!("[DESIGN] a row of {} is short", child.name)))?;
                if !self.matches(tx, fk, &values)? {
                    return Ok(true);
                }
            }
            return Ok(false);
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
        if !self.rows_with(tx, &fk.table, &fk.referenced, &key)?.is_empty() {
            return Ok(false);
        }
        Ok(!self.rows_with(tx, &child.name, &fk.columns, &key)?.is_empty())
    }

    /// [§11.8 GR5, GR6] The referential actions of the rows `effects`
    /// deleted and updated, each adding what it does to `effects`, until
    /// none is left to run: none, and what it did not read again, if no
    /// foreign key has an action. The values the statement gave are kept
    /// [GR7] only of the tables an action may give values to.
    pub(super) fn referential_actions(&self, tx: &Txn, effects: &mut Effects<'_>) -> Result<()> {
        let acted: Vec<&QualifiedName> = self.catalog.tables().into_iter().filter(|t| acts_on(t)).map(|t| &t.name).collect();
        if acted.is_empty() {
            return Ok(());
        }
        let mut acting = Acting::default();
        effects.done.visit(&mut |effect| {
            let Effect::Updated { table, id, columns, .. } = effect else { return Ok(()) };
            if !acted.contains(&table) {
                return Ok(());
            }
            let now = self.storage_of(table)?.get(tx.id, *id)?.unwrap_or_default();
            for &c in columns {
                acting.given.insert((table.clone(), *id, c), now.get(c).cloned().unwrap_or(Value::Null));
            }
            Ok(())
        })?;
        let mut at = 0_usize;
        while let Some(effect) = effects.done.get(at)? {
            at = at.saturating_add(1);
            let (parent, old, row, updated) = match &effect {
                Effect::Deleted { table, old, id } => (table, old, *id, false),
                Effect::Updated { table, old, id, .. } => (table, old, *id, true),
                Effect::Inserted { .. } => continue,
            };
            let new = if updated { Some(self.storage_of(parent)?.get(tx.id, row)?.unwrap_or_default()) } else { None };
            let change = Change { parent, row, old, new };
            for child in self.catalog.tables() {
                for (name, fk) in child.foreign_keys().filter(|(_, fk)| fk.table == *parent) {
                    self.act(tx, (child, name, fk), &change, (effects, &mut acting))?;
                }
            }
        }
        Ok(())
    }

    /// The action of `fk`, foreign key `name` of `child`, on the rows that
    /// match the row `change` deleted or updated: with MATCH PARTIAL, its
    /// unique matching rows [GR3b, GR5b, GR6b].
    fn act(
        &self,
        tx: &Txn,
        (child, name, fk): (&TableDesc, &QualifiedName, &ForeignKey),
        change: &Change<'_>,
        (effects, acting): (&mut Effects<'_>, &mut Acting),
    ) -> Result<()> {
        let updated = change.new.is_some();
        let action = if updated { fk.on_update } else { fk.on_delete };
        let key = project(change.old, &fk.referenced).ok_or_else(|| api(format!("[DESIGN] a row of {} is short", fk.table)))?;
        let new_key = change.new.as_ref().and_then(|new| project(new, &fk.referenced));
        // [GR6] an update acts only where a referenced value not null changed.
        let changed = new_key.as_ref().is_some_and(|k| key.iter().zip(k).any(|(was, now)| !was.is_null() && !same_value(was, now)));
        if (updated && !changed) || action == ReferentialAction::NoAction {
            return Ok(());
        }
        // [GR5a iii, GR6a iii] SET DEFAULT gives the child's defaults [§11.5 GR2].
        let defaults = if action == ReferentialAction::SetDefault { default_row(child, &effects.user, effects.now)? } else { Vec::new() };
        let partial = fk.match_type == MatchType::Partial;
        let before = if partial { self.before_statement(tx, change.parent, effects)? } else { Vec::new() };
        // None the statement deleted: the transaction reads its own changes.
        for (row_id, row) in self.matching_rows(tx, (child, fk), &key)? {
            let values = project(&row, &fk.columns).ok_or_else(|| api(format!("[DESIGN] a row of {} is short", child.name)))?;
            let associated: Vec<RowId> = before
                .iter()
                .filter(|(_, r)| project(r, &fk.referenced).is_some_and(|k| partially(&values, &k)))
                .map(|(id, _)| *id)
                .collect();
            if partial && !acting.unique((name, row_id), &associated, change.row, dropped_by(&values, &key, new_key.as_deref())) {
                continue;
            }
            // [GR8] a row a cursor changed: a cursor operation conflict.
            effects.cursors.acted_on(&child.name, row_id);
            if action == ReferentialAction::Cascade && !updated {
                // [GR5a i, GR5b i] the matching rows deleted.
                self.delete_row(tx, &child.name, row_id)?;
                effects.deleted(&child.name, row_id, row)?;
                continue;
            }
            if partial && action == ReferentialAction::Cascade {
                self.check_cascade(tx, (fk, change.parent), (&associated, effects), (&key, new_key.as_deref()))?;
            }
            let (next, columns) = acted((fk, action, &defaults), &row, (change.old, new_key.as_deref(), updated));
            give((&child.name, row_id), &next, &columns, &mut acting.given)?;
            self.update_row(tx, &child.name, row_id, &next)?;
            effects.updated(&child.name, row_id, row, &columns)?;
        }
        Ok(())
    }

    /// [§11.8 GR4] The rows of `table` as they were before the statement
    /// that did `effects`: as they are, but for the rows it inserted, gone,
    /// and those it updated or deleted, as they were first.
    fn before_statement(&self, tx: &Txn, table: &QualifiedName, effects: &mut Effects<'_>) -> Result<Vec<(RowId, Row)>> {
        let mut rows: HashMap<RowId, Option<Row>> =
            self.rows_where(tx, table, &|_| true)?.into_iter().map(|(id, r)| (id, Some(r))).collect();
        let mut seen = Vec::new();
        effects.done.visit(&mut |effect| {
            let (t, id, was) = match effect {
                Effect::Inserted { table: t, id } => (t, *id, None),
                Effect::Updated { table: t, id, old, .. } | Effect::Deleted { table: t, id, old } => (t, *id, Some(old.clone())),
            };
            if t == table && !seen.contains(&id) {
                seen.push(id);
                rows.insert(id, was);
            }
            Ok(())
        })?;
        let mut out: Vec<(RowId, Row)> = rows.into_iter().filter_map(|(id, r)| r.map(|r| (id, r))).collect();
        out.sort_by_key(|(id, _)| *id);
        Ok(out)
    }

    /// [§11.8 GR6b i] A CASCADE of MATCH PARTIAL gives a row the new value
    /// V of a referenced column only if every row of `parent` updated in
    /// the statement that the row matched before it, its `associated`, now
    /// has a value not distinct from V there; else a triggered data change
    /// violation.
    fn check_cascade(
        &self,
        tx: &Txn,
        (fk, parent): (&ForeignKey, &QualifiedName),
        (associated, effects): (&[RowId], &mut Effects<'_>),
        (key, new): (&[Value], Option<&[Value]>),
    ) -> Result<()> {
        let mut updated: Vec<RowId> = Vec::new();
        effects.done.visit(&mut |e| {
            if let Effect::Updated { table, id, .. } = e
                && table == parent
                && associated.contains(id)
            {
                updated.push(*id);
            }
            Ok(())
        })?;
        for id in updated {
            let now = self.storage_of(parent)?.get(tx.id, id)?.and_then(|r| project(&r, &fk.referenced));
            let agrees = now.as_ref().is_some_and(|now| {
                key.iter()
                    .zip(new.unwrap_or_default())
                    .zip(now)
                    .all(|((was, v), there)| was.is_null() || same_value(was, v) || same_value(there, v))
            });
            if !agrees {
                return Err(SqlError::new(
                    Code::Constraint,
                    format!("[ISO/IEC 9075:1992 §11.8 GR6b i] rows of {parent} a row matched are updated to distinct values"),
                )
                .with_state(SqlState::TRIGGERED_DATA_CHANGE_VIOLATION)
                .about(Subject::table(parent.parts())));
            }
        }
        Ok(())
    }
}

impl Acting {
    /// [§11.8 GR3b ii, GR4] Whether row `row` of a foreign key (`name`),
    /// which matched the referenced rows `associated` before the statement,
    /// is a unique matching row of `parent`, deleted or updated: no other
    /// of them is still associated with it. Its association with `parent`
    /// is then dropped if `drop`.
    fn unique(&mut self, (name, row): (&QualifiedName, RowId), associated: &[RowId], parent: RowId, drop: bool) -> bool {
        let dropped = self.dropped.entry((name.clone(), row)).or_default();
        let unique = !associated.iter().any(|a| *a != parent && !dropped.contains(a));
        if drop {
            dropped.push(parent);
        }
        unique
    }
}

/// [§11.8 GR4] Whether the association of a row whose referencing values
/// are `values` with a referenced row whose values were `key` is dropped:
/// the row deleted (`new` none), or updated to a distinct value in a
/// column whose referencing value is not null.
fn dropped_by(values: &[Value], key: &[Value], new: Option<&[Value]>) -> bool {
    let Some(new) = new else { return true };
    values.iter().zip(key).zip(new).any(|((v, was), now)| !v.is_null() && !same_value(was, now))
}

/// `row`, a row matching `old` by `fk`, with `action` done to its
/// referencing columns, `new`, if `updated`, being the referenced values
/// now, and `defaults` the defaults of its table's columns; and the
/// columns it gave values to.
fn acted(
    (fk, action, defaults): (&ForeignKey, ReferentialAction, &Row),
    row: &Row,
    (old, new, updated): (&Row, Option<&[Value]>, bool),
) -> (Row, Vec<usize>) {
    let mut changed = row.clone();
    let mut columns = Vec::new();
    for (j, (&c, &r)) in fk.columns.iter().zip(&fk.referenced).enumerate() {
        let new_value = new.and_then(|k| k.get(j)).cloned();
        let was = old.get(r).cloned().unwrap_or(Value::Null);
        let unchanged = new_value.as_ref().is_some_and(|v| same_value(v, &was));
        // [GR6b] with PARTIAL, an update acts on a column not null that
        // corresponds to one changed.
        let null_here = row.get(c).is_none_or(Value::is_null);
        if updated && fk.match_type == MatchType::Partial && (null_here || unchanged) {
            continue;
        }
        let value = match action {
            // [GR6a i, GR6b i] the new value of the referenced column.
            ReferentialAction::Cascade => new_value.unwrap_or(Value::Null),
            // [GR5a ii; GR6a ii 1] without MATCH FULL, only where it
            // changed; [GR6a ii 2] with it, each.
            ReferentialAction::SetNull => {
                if updated && fk.match_type == MatchType::Simple && unchanged {
                    continue;
                }
                Value::Null
            }
            // [GR5a iii; GR6a iii] the default, of the column that
            // corresponds to one changed.
            ReferentialAction::SetDefault => {
                if updated && unchanged {
                    continue;
                }
                defaults.get(c).cloned().unwrap_or(Value::Null)
            }
            ReferentialAction::NoAction => continue,
        };
        if let Some(slot) = changed.get_mut(c) {
            *slot = value;
        }
        columns.push(c);
    }
    (changed, columns)
}

/// [§11.8 GR7] Records the values `row` gives `columns` of row `id` of
/// `table`: refused if one was given another, distinct, before in the
/// statement.
fn give((table, id): (&QualifiedName, RowId), row: &Row, columns: &[usize], given: &mut Given) -> Result<()> {
    for &c in columns {
        let value = row.get(c).cloned().unwrap_or(Value::Null);
        if let Some(before) = given.get(&(table.clone(), id, c))
            && !same_value(before, &value)
        {
            return Err(SqlError::new(
                Code::Constraint,
                format!("[ISO/IEC 9075:1992 §11.8 GR7] a column of a row of {table} is given two distinct values in one statement"),
            )
            .with_state(SqlState::TRIGGERED_DATA_CHANGE_VIOLATION)
            .about(Subject::table(table.parts())));
        }
        given.insert((table.clone(), id, c), value);
    }
    Ok(())
}

/// [§11.8 GR3b i] Whether referencing `values` match referenced `key` with
/// MATCH PARTIAL: one at least not null, and each not null equal to its
/// referenced value.
fn partially(values: &[Value], key: &[Value]) -> bool {
    values.iter().any(|v| !v.is_null()) && values.iter().zip(key).all(|(v, k)| v.is_null() || equals(v, k))
}

/// Whether `a` equals `b` [§8.2]: neither null.
fn equals(a: &Value, b: &Value) -> bool {
    compare(a, b) == Some(std::cmp::Ordering::Equal)
}

/// Whether `a` and `b` are not distinct [ISO/IEC 9075:1992 §3.1]: equal,
/// or both null.
fn same_value(a: &Value, b: &Value) -> bool {
    (a.is_null() && b.is_null()) || equals(a, b)
}

/// Whether the values `a` and `b` are not distinct, each pair.
fn same(a: &[Value], b: &[Value]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_value(x, y))
}

/// Whether a foreign key of `child` has a referential action, by which a
/// change to the table it references may give values to `child`'s rows or
/// delete them [§11.8 GR5, GR6].
fn acts_on(child: &TableDesc) -> bool {
    child.foreign_keys().any(|(_, fk)| fk.on_delete != ReferentialAction::NoAction || fk.on_update != ReferentialAction::NoAction)
}
