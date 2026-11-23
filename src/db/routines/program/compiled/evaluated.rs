//! \[DESIGN\] The values of a compiled program evaluated, and what assigns
//! them (RULES.md D841): the assignment statement [ISO/IEC 9075-4:2006
//! draft §14.5], SELECT INTO [ISO/IEC 9075-2:2023 §14.7], and the items of
//! SIGNAL and RESIGNAL [§17.2, §17.3], as the interpreter evaluates and
//! assigns them (`exec/program/leaves.rs`).

use std::sync::Arc;

use super::super::super::kept::point_of;
use super::code::{Assign, Into, Signal};
use super::run::{Exec, Stop};
use super::valued::{Queried, RowOf, Store, Stores, Valued, stored};
use crate::ast::ParameterSpecification;
use crate::db::{Completion, assign_targets};
use crate::error::{Code, Result, SqlError, fail};
use crate::exec::{Framed, Host, assign_target, eval, item_text, raised};
use crate::logical::Target;
use crate::storage::Row;
use crate::value::Value;

impl Exec<'_> {
    /// The rows `v` gives over the frame, at most `limit` if given.
    ///
    /// # Errors
    ///
    /// The leaf's, or a lock's.
    pub(super) fn rows(&self, v: &Valued, limit: Option<usize>) -> Result<Vec<Row>> {
        match v {
            Valued::Row(r) => self.row_of(r).map(|row| vec![row]),
            Valued::Query(q) => self.queried(q, limit),
            Valued::General(leaf) => self.host.rows(leaf, self.frame, limit),
        }
    }

    /// The value of `v`: of the first column of its row, the null value if
    /// it has none.
    pub(super) fn value(&self, v: &Valued) -> Result<Value> {
        Ok(self.rows(v, Some(1))?.into_iter().next().and_then(|r| r.into_iter().next()).unwrap_or(Value::Null))
    }

    /// Whether `v`, a condition, is true: neither false nor unknown.
    pub(super) fn truth(&self, v: &Valued) -> Result<bool> {
        Ok(matches!(self.value(v)?, Value::Bool(true)))
    }

    /// The values the leaf's outer expressions `outer` take over the frame.
    fn outer_values(&self, outer: &[crate::logical::Expr]) -> Result<Vec<Value>> {
        let framed = Framed { inner: &self.hosted, frame: self.frame.dynamic_values(), outer: &[] };
        let none = Row::new();
        outer.iter().map(|e| eval(e, &none, &framed)).collect()
    }

    /// The row of values `r`, evaluated where it is.
    fn row_of(&self, r: &RowOf) -> Result<Row> {
        let evaluated = || {
            let outer = self.outer_values(&r.outer)?;
            let framed = Framed { inner: &self.hosted, frame: self.frame.dynamic_values(), outer: &outer };
            let none = Row::new();
            r.row.iter().map(|e| eval(e, &none, &framed)).collect::<Result<Row>>()
        };
        evaluated().map_err(|e| e.or_at(r.at))
    }

    /// The rows of the query `q`: by its point read, once it plans to one,
    /// its tables locked and its snapshot taken as the session takes them
    /// for a leaf; else as the session runs it.
    fn queried(&self, q: &Queried, limit: Option<usize>) -> Result<Vec<Row>> {
        let st = self.cx.st;
        let point = q.point.get_or_init(|| point_of(st, &q.leaf));
        let Some(read) = point.as_ref().and_then(|p| p.point(st)) else { return self.host.rows(&q.leaf, self.frame, limit) };
        self.host.fresh(self.cx.tx)?;
        for (t, needed) in &q.reads {
            st.read_lock_of(self.cx.tx, (t, *needed))?;
        }
        let found = self.outer_values(&q.leaf.query.outer).and_then(|outer| read.read(st, self.cx.tx.id, &outer));
        Ok(found.map_err(|e| e.or_at(q.leaf.at))?.into_iter().take(limit.unwrap_or(usize::MAX)).collect())
    }

    /// [ISO/IEC 9075-4:2006 draft §14.5] The assignment statement `a`: its
    /// source's row, the first a query gives and of none the null values,
    /// each value assigned to its target in turn.
    ///
    /// # Errors
    ///
    /// SQLSTATE 21000 if its subquery gives more than one row; the store
    /// assignment's.
    pub(super) fn assignment(&mut self, a: &Assign) -> Result<Completion> {
        let row = match &a.source {
            None => vec![Value::Null; a.targets.len()],
            Some(Valued::Row(r)) => self.row_of(r)?,
            Some(source) => match self.rows(source, Some(2))?.as_mut_slice() {
                [] => vec![Value::Null; a.targets.len()],
                [row] => std::mem::take(row),
                [_, _, ..] => {
                    return fail(
                        Code::Cardinality,
                        "[ISO/IEC 9075:1992 §7.11 GR1; ISO/IEC 9075-4:2006 draft §14.5 SR1f] the assigned row's subquery gives more than one row",
                    );
                }
            },
        };
        self.frame.set_routine(Arc::clone(&a.scope));
        for ((target, store), value) in a.targets.iter().zip(&a.stores.0).zip(row) {
            self.assign_one(target, store, value).map_err(|e| e.or_at(target.spec.at).or_at(a.at))?;
        }
        Ok(Completion::Success)
    }

    /// `value` assigned to `target`, in its slot if `store` is one.
    fn assign_one(&mut self, target: &Target, store: &Store, value: Value) -> Result<()> {
        match store {
            Store::Slot { at, data_type, written } => {
                let value = stored(value, (data_type, *written))?;
                self.frame.set_slot(*at, value)
            }
            Store::General => assign_target(target, value, self.frame),
        }
    }

    /// [ISO/IEC 9075-2:2023 §14.7] SELECT INTO `i`: of no row, no data, its
    /// targets not assigned [GR2]; of one, its values assigned to them.
    ///
    /// # Errors
    ///
    /// SQLSTATE 21000 of more than one row; the assignment's.
    pub(super) fn select_into(&mut self, i: &Into) -> Result<Completion> {
        let mut found = self.rows(&i.query, Some(2))?;
        match found.as_mut_slice() {
            [] => Ok(Completion::NoData),
            [row] => {
                self.host.present(row);
                self.frame.set_routine(Arc::clone(&i.scope));
                let row = std::mem::take(row);
                store_row(self.frame, row, (&i.targets, &i.stores))
            }
            [_, _, ..] => fail(Code::Cardinality, "[X3.135-1986 §8.10 GR2] the result has more than one row"),
        }
    }

    /// [§17.2, §17.3] The condition SIGNAL or RESIGNAL `s` raises.
    pub(super) fn signal(&mut self, s: &Signal) -> std::result::Result<SqlError, Stop> {
        let mut items = Vec::with_capacity(s.items.len());
        for ((item, _), value) in s.signal.items.iter().zip(&s.items) {
            let value = self.value(value).map_err(Stop::Raised)?;
            items.push((item.name(), item_text(value)));
        }
        raised(&s.signal, items, self.handling.last()).map_err(Stop::Raised)
    }
}

/// [X3.135-1986 §8.10 GR8, GR9] `row`, a SELECT INTO's, assigned to its
/// targets `specs`, in their slots where `stores` has them: the warning
/// the assignment raised, if one.
///
/// # Errors
///
/// The assignment's, at the target.
pub(super) fn store_row(
    frame: &mut crate::params::Params,
    row: Row,
    (specs, stores): (&[ParameterSpecification], &Stores),
) -> Result<Completion> {
    if !stores.all_slots() {
        return assign_targets(&row, specs, frame, "X3.135-1986 §8.10 GR8", "X3.135-1986 §8.10 GR9");
    }
    for (value, store) in row.into_iter().zip(&stores.0) {
        if let Store::Slot { at, data_type, written } = store {
            let value = stored(value, (data_type, *written))?;
            frame.set_slot(*at, value)?;
        }
    }
    Ok(Completion::Success)
}
