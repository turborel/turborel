//! \[EXTENSION\] The referential actions of the rows a statement deleted
//! and updated, at its end [ISO/IEC 9075:1992 §11.8 GR5, GR6], each adding
//! what it does to what the statement did. Split from
//! `integrity/references.rs` (RULES.md R-SIZE-2).
//!
//! With MATCH PARTIAL a row matches every referenced row its values not
//! null equal, so an action is done only to the rows it matches no other
//! row for: its unique matching rows [GR3b]. Which rows match which is
//! taken as it was before the statement [GR4]: the referenced table now,
//! with the rows the statement wrote as they were; the association of a
//! row with a referenced row deleted, or updated in a column it is not null
//! in, is dropped as that row is acted for, so the last of the rows a row
//! matched to go acts on it.
//!
//! What they keep is bounded as the statement's effects are (RULES.md
//! D38): the rows matching each row acted for, gathered before any is
//! acted on; for MATCH PARTIAL, the referenced table as it was before the
//! statement, made once for each table, and the referenced rows each row
//! matched then: lists that spill. The values the statement gave [GR7],
//! and the associations dropped [GR4], are maps that spill
//! (`spill/map.rs`).

use std::ops::ControlFlow;

use super::references::{partially, same_value};
use super::{Effect, Effects};
use crate::bytes::{Put, Reader};
use crate::catalog::{ForeignKey, MatchType, QualifiedName, ReferentialAction, TableDesc};
use crate::db::defaults::default_row;
use crate::db::{State, api};
use crate::error::{Code, Result, SqlError, SqlState, Subject};
use crate::exec::HeldRows;
use crate::spill::ListSpace;
use crate::spill::map::SpillMap;
use crate::storage::{Row, RowId, project};
use crate::txn::Txn;
use crate::value::Value;
use crate::value::codec::{put_value, value};
use crate::value::types::TypeRegistry;

/// What the referential actions of a statement keep while they run, in
/// the statement's space: the values given [GR7], each row's column's
/// under its table, row and column; for MATCH PARTIAL, the associations
/// dropped [GR4], each a row of a foreign key with a referenced row, under
/// the foreign key's name and the two rows; and each referenced table as
/// it was before the statement, made when first needed.
struct Acting {
    space: ListSpace,
    given: SpillMap,
    dropped: SpillMap,
    before: Vec<(QualifiedName, HeldRows<(RowId, Row)>)>,
}

/// Row `row` of a referenced table, `parent`, deleted, or (with its values
/// now) updated, from `old`.
struct Change<'a> {
    parent: &'a QualifiedName,
    row: RowId,
    old: &'a Row,
    new: Option<Row>,
}

/// The action of a foreign key, `fk`, named `name`, of `child`, for the
/// row `change` deleted or updated: `rule`, whose referenced values were
/// `key` and are now `new_key`, if updated; `defaults`, of SET DEFAULT.
struct Action<'a> {
    child: &'a TableDesc,
    name: &'a QualifiedName,
    fk: &'a ForeignKey,
    change: &'a Change<'a>,
    rule: ReferentialAction,
    key: Vec<Value>,
    new_key: Option<Vec<Value>>,
    defaults: Row,
}

impl State {
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
        let mut acting = Acting::new(&effects.space);
        effects.done.visit(&mut |effect| {
            let Effect::Updated { table, id, columns, .. } = effect else { return Ok(()) };
            if !acted.contains(&table) {
                return Ok(());
            }
            let now = self.storage_of(table)?.get(tx.id, *id)?.unwrap_or_default();
            for &c in columns {
                acting.given.insert(&given_key(table, *id, c), &value_record(now.get(c).unwrap_or(&Value::Null)))?;
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
    /// unique matching rows [GR3b, GR5b, GR6b]. The rows that match are
    /// all found before any is acted on.
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
        // None the statement deleted: the transaction reads its own changes.
        let matching = self.matching_rows(tx, (child, fk), &key, effects.space.clone())?;
        let done = Action { child, name, fk, change, rule: action, key, new_key, defaults };
        for found in matching {
            self.act_on(tx, &done, found?, (effects, acting))?;
        }
        Ok(())
    }

    /// `done`'s action on row `row_id` of its foreign key's table, `row`,
    /// which matches the row changed: if it is a unique matching row, with
    /// MATCH PARTIAL [GR3b].
    fn act_on(
        &self,
        tx: &Txn,
        done: &Action<'_>,
        (row_id, row): (RowId, Row),
        (effects, acting): (&mut Effects<'_>, &mut Acting),
    ) -> Result<()> {
        let (child, fk, change) = (&done.child.name, done.fk, done.change);
        let updated = change.new.is_some();
        let values = project(&row, &fk.columns).ok_or_else(|| api(format!("[DESIGN] a row of {child} is short")))?;
        let mut associated =
            if fk.match_type == MatchType::Partial { Some(acting.associated(self, tx, (done, &values), effects)?) } else { None };
        let unique = match &mut associated {
            Some(rows) => acting.unique((done.name, row_id), rows, change.row, dropped_by(&values, &done.key, done.new_key.as_deref()))?,
            None => true,
        };
        if !unique {
            return Ok(());
        }
        // [GR8] a row a cursor changed: a cursor operation conflict.
        effects.cursors.acted_on(child, row_id);
        if done.rule == ReferentialAction::Cascade && !updated {
            // [GR5a i, GR5b i] the matching rows deleted.
            self.delete_row(tx, child, row_id)?;
            return effects.deleted(child, row_id, row);
        }
        if let Some(rows) = &mut associated
            && done.rule == ReferentialAction::Cascade
        {
            self.check_cascade(tx, (fk, change.parent), (rows, effects), (&done.key, done.new_key.as_deref()))?;
        }
        let (next, columns) = acted((fk, done.rule, &done.defaults), &row, (change.old, done.new_key.as_deref(), updated));
        acting.give((child, row_id), &next, &columns)?;
        self.update_row(tx, child, row_id, &next)?;
        effects.updated(child, row_id, row, &columns)
    }

    /// [§11.8 GR4] The rows of `table` as they were before the statement
    /// that did `effects`, held as a list that spills: as they are, but for
    /// the rows it changed, each as it was first, none of one it inserted.
    /// The ids of the rows it changed are kept in a map that spills.
    fn before_statement(&self, tx: &Txn, table: &QualifiedName, effects: &mut Effects<'_>) -> Result<HeldRows<(RowId, Row)>> {
        let mut changed = SpillMap::new(effects.space.clone());
        effects.done.visit(&mut |effect| {
            let (t, id, _) = effect_row(effect);
            if t == table { changed.insert(&id.to_le_bytes(), &[]) } else { Ok(()) }
        })?;
        let mut rows = HeldRows::new(effects.space.clone());
        self.each_row_where(tx, table, &|_| true, &mut |id, row| {
            if !changed.contains(&id.to_le_bytes())? {
                rows.push((id, row))?;
            }
            Ok(ControlFlow::Continue(()))
        })?;
        effects.done.visit(&mut |effect| {
            let (t, id, was) = effect_row(effect);
            if t != table {
                return Ok(());
            }
            // Its first effect, taken from those to come: what it was.
            let first = changed.remove(&id.to_le_bytes())?;
            match (first, was) {
                (true, Some(old)) => rows.push((id, old.clone())),
                (true, None) | (false, _) => Ok(()),
            }
        })?;
        Ok(rows)
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
        (associated, effects): (&mut HeldRows<RowId>, &mut Effects<'_>),
        (key, new): (&[Value], Option<&[Value]>),
    ) -> Result<()> {
        let mut of = SpillMap::new(effects.space.clone());
        associated.visit(&mut |id| of.insert(&id.to_le_bytes(), &[]))?;
        effects.done.visit(&mut |e| {
            let Effect::Updated { table, id, .. } = e else { return Ok(()) };
            let theirs = table == parent && of.contains(&id.to_le_bytes())?;
            if !theirs {
                return Ok(());
            }
            let now = self.storage_of(parent)?.get(tx.id, *id)?.and_then(|r| project(&r, &fk.referenced));
            let agrees = now.as_ref().is_some_and(|now| {
                key.iter()
                    .zip(new.unwrap_or_default())
                    .zip(now)
                    .all(|((was, v), there)| was.is_null() || same_value(was, v) || same_value(there, v))
            });
            if agrees {
                return Ok(());
            }
            Err(SqlError::new(
                Code::Constraint,
                format!("[ISO/IEC 9075:1992 §11.8 GR6b i] rows of {parent} a row matched are updated to distinct values"),
            )
            .with_state(SqlState::TRIGGERED_DATA_CHANGE_VIOLATION)
            .about(Subject::table(parent.parts())))
        })
    }
}

impl Acting {
    /// Nothing kept yet, what it will keep held in `space`.
    fn new(space: &ListSpace) -> Acting {
        Acting { space: space.clone(), given: SpillMap::new(space.clone()), dropped: SpillMap::new(space.clone()), before: Vec::new() }
    }

    /// [§11.8 GR3b, GR4] The rows of the table `done` acts for a change
    /// to that a row whose referencing values are `values` matched before
    /// the statement that did `effects`: of that table as it was then,
    /// made the first time.
    fn associated(
        &mut self,
        db: &State,
        tx: &Txn,
        (done, values): (&Action<'_>, &[Value]),
        effects: &mut Effects<'_>,
    ) -> Result<HeldRows<RowId>> {
        let parent = done.change.parent;
        if !self.before.iter().any(|(t, _)| t == parent) {
            let rows = db.before_statement(tx, parent, effects)?;
            self.before.push((parent.clone(), rows));
        }
        let mut out = HeldRows::new(self.space.clone());
        let Some((_, before)) = self.before.iter_mut().find(|(t, _)| t == parent) else { return Ok(out) };
        before.visit(&mut |(id, row)| {
            if project(row, &done.fk.referenced).is_some_and(|k| partially(values, &k)) { out.push(*id) } else { Ok(()) }
        })?;
        Ok(out)
    }

    /// [§11.8 GR3b ii, GR4] Whether row `row` of a foreign key (`name`),
    /// which matched the referenced rows `associated` before the statement,
    /// is a unique matching row of `parent`, deleted or updated: no other
    /// of them is still associated with it. Its association with `parent`
    /// is then dropped if `drop`.
    fn unique(
        &mut self,
        (name, row): (&QualifiedName, RowId),
        associated: &mut HeldRows<RowId>,
        parent: RowId,
        drop: bool,
    ) -> Result<bool> {
        let mut unique = true;
        let dropped = &self.dropped;
        associated.visit(&mut |a| {
            if unique && *a != parent {
                unique = dropped.contains(&association(name, row, *a))?;
            }
            Ok(())
        })?;
        if drop {
            self.dropped.insert(&association(name, row, parent), &[])?;
        }
        Ok(unique)
    }

    /// [§11.8 GR7] Records the values `row` gives `columns` of row `id` of
    /// `table`: refused if one was given another, distinct, before in the
    /// statement.
    fn give(&mut self, (table, id): (&QualifiedName, RowId), row: &Row, columns: &[usize]) -> Result<()> {
        for &c in columns {
            let given = row.get(c).unwrap_or(&Value::Null);
            let at = given_key(table, id, c);
            if let Some(record) = self.given.get(&at)?
                && !same_value(&record_value(&record, &self.space.types)?, given)
            {
                return Err(SqlError::new(
                    Code::Constraint,
                    format!("[ISO/IEC 9075:1992 §11.8 GR7] a column of a row of {table} is given two distinct values in one statement"),
                )
                .with_state(SqlState::TRIGGERED_DATA_CHANGE_VIOLATION)
                .about(Subject::table(table.parts())));
            }
            self.given.insert(&at, &value_record(given))?;
        }
        Ok(())
    }
}

/// The table, row and old values of what `effect` did: none of a row
/// inserted.
fn effect_row(effect: &Effect) -> (&QualifiedName, RowId, Option<&Row>) {
    match effect {
        Effect::Inserted { table, id } => (table, *id, None),
        Effect::Updated { table, id, old, .. } | Effect::Deleted { table, id, old } => (table, *id, Some(old)),
    }
}

/// The key of the value given column `column` of row `id` of `table`
/// [GR7]: the table's name, the row's id, the column.
fn given_key(table: &QualifiedName, id: RowId, column: usize) -> Vec<u8> {
    let mut out = Vec::new();
    out.put_str(&table.auth);
    out.put_str(&table.ident);
    out.put_u64(id);
    out.put_u64(u64::try_from(column).unwrap_or(u64::MAX));
    out
}

/// The key of the association of row `row` of foreign key `name` with
/// referenced row `parent` [GR4].
fn association(name: &QualifiedName, row: RowId, parent: RowId) -> Vec<u8> {
    let mut out = Vec::new();
    out.put_str(&name.auth);
    out.put_str(&name.ident);
    out.put_u64(row);
    out.put_u64(parent);
    out
}

/// `v` as a map holds it.
fn value_record(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    put_value(&mut out, v);
    out
}

/// The value `record` holds, its type found among `types` if beyond the
/// standard.
fn record_value(record: &[u8], types: &TypeRegistry) -> Result<Value> {
    let mut r = Reader::new(record, "given value").with_types(types);
    let found = value(&mut r)?;
    r.finish()?;
    Ok(found)
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

/// Whether a foreign key of `child` has a referential action, by which a
/// change to the table it references may give values to `child`'s rows or
/// delete them [§11.8 GR5, GR6].
fn acts_on(child: &TableDesc) -> bool {
    child.foreign_keys().any(|(_, fk)| fk.on_delete != ReferentialAction::NoAction || fk.on_update != ReferentialAction::NoAction)
}
