//! \[DESIGN\] An index join whose probe fixes a whole unique key, run as
//! nested point reads (RULES.md D932). Each row of its left operand finds
//! one row of the table at most, so the probe is a point read (D752,
//! D881): the row's values of the key are placed as that read's, its
//! index is probed by the whole key with nothing boxed, and the one row
//! found is read of the columns the statement reads alone. The pair is
//! made in a row kept for the run, the left row's values moved into it,
//! and the rest of the condition judged of it, as the join judges it
//! (`indexed.rs`).
//!
//! The point read records its read, counts its entries and looks for a
//! cancel as a probe of the join does, and a left row with a null in its
//! key finds nothing and reads nothing, as there. It is taken only of an
//! inner join by the columns of its left operand, each of the type of the
//! key column it is compared with, or a constant that column's type takes
//! unchanged, so its value is placed as it is; any other join is the
//! executor's.

use crate::db::State;
use crate::error::Result;
use crate::exec::{PointRead, RangeAt, eval_cond};
use crate::index::Probe;
use crate::logical::{Cond, Expr, Slot, Truth};
use crate::physical::{JoinProbe, Place, Plan, Preserved, ProbeSlot, placed};
use crate::storage::Row;
use crate::value::{CompOp, DataType, Value};

/// An index join by a whole unique key, as nested point reads.
#[derive(Debug, Clone)]
pub(in crate::exec) struct Direct {
    /// The read of the table's row of a key, giving the columns `needed`.
    point: PointRead,
    /// The left row's columns that are the key's values, in its order.
    keys: Vec<usize>,
    /// The table's columns the statement reads, as the read gives them.
    needed: Vec<usize>,
    /// The table's degree, the width of its part of a pair.
    width: usize,
    /// The rest of the condition, over the pair's columns.
    cond: Option<Cond>,
}

/// What a column of a join's left operand gives: a value of a type, or a
/// constant.
#[derive(Debug, Clone)]
pub(in crate::exec) enum Given {
    Typed(DataType),
    Constant(Value),
}

impl Given {
    /// Whether a key column of `data_type` takes what it gives as it is,
    /// as a point read places it: a value of that type, or a constant the
    /// type's cast leaves unchanged.
    fn placed_as_is(&self, data_type: &DataType) -> bool {
        let constant = match self {
            Given::Typed(given) => return given == data_type,
            Given::Constant(constant) => constant,
        };
        let slot = ProbeSlot::of(Place::Key(0), Slot { index: 0, data_type: data_type.clone(), at: 0 });
        let probe = Probe::Compare { op: CompOp::Eq, value: Value::Null };
        let Ok(cast) = placed(&probe, &[slot], std::slice::from_ref(constant)) else { return false };
        let Probe::Compare { value, .. } = &*cast else { return false };
        format!("{value:?}") == format!("{constant:?}")
    }
}

/// What one run of such a join keeps from pair to pair: the key's values
/// and the pair.
#[derive(Default)]
pub(in crate::exec) struct Pairing {
    key: Vec<Value>,
    pair: Row,
}

impl Direct {
    /// The join `plan` is, in `st`, if it is an inner index join whose
    /// probe is the whole key of a unique index, by columns of its left
    /// operand the key columns take as they are, `left` what that
    /// operand's columns give.
    pub(in crate::exec) fn of(plan: &Plan, left: &[Given], st: &State) -> Option<Direct> {
        let Plan::IndexJoin {
            outer: Preserved::Inner,
            spatial: None,
            table,
            index,
            method,
            probe: JoinProbe::Prefix(keys),
            cond,
            width,
            columns,
            located: false,
            ..
        } = plan
        else {
            return None;
        };
        let types = &st.catalog.table(table)?.columns;
        let ix = st.catalog.index(index)?;
        let same =
            |(&key, &column): (&usize, &usize)| left.get(key).zip(types.get(column)).is_some_and(|(l, r)| l.placed_as_is(&r.data_type));
        if keys.len() != ix.columns.len() || !keys.iter().zip(&ix.columns).all(same) {
            return None;
        }
        let slot = |(at, &column): (usize, &usize)| {
            Some(ProbeSlot::of(Place::Key(at), Slot { index: at, data_type: types.get(column)?.data_type.clone(), at: 0 }))
        };
        let slots = ix.columns.iter().enumerate().map(slot).collect::<Option<Vec<ProbeSlot>>>()?;
        // The probe of a whole key, as the planner makes one (`physical/kept.rs`).
        let probe = match slots.len() {
            1 => Probe::Compare { op: CompOp::Eq, value: Value::Null },
            n => Probe::Prefix { equal: vec![Value::Null; n], then: None },
        };
        let scan = Plan::IndexScan {
            table: table.clone(),
            index: index.to_string(),
            method: method.to_string(),
            probe,
            slots,
            reverse: false,
            columns: columns.to_vec(),
            detail: String::new(),
            est: 1.0_f64,
            cost: 0.0_f64,
            covering: false,
            located: false,
            decided: None,
        };
        let exprs = columns.iter().map(|&c| Expr::Column(c)).collect();
        let point = PointRead::of(&Plan::Project { exprs, detail: String::new(), input: Box::new(scan) }, st)?;
        Some(Direct { point, keys: keys.to_vec(), needed: columns.to_vec(), width: *width, cond: cond.clone() })
    }

    /// The pair of the left row `left`, whose values it takes, and the
    /// table's row of its key, in `pairing`, if `at`'s transaction sees
    /// one and the rest of the condition is true of the pair: whether
    /// there is one.
    ///
    /// # Errors
    ///
    /// The point read's, and the condition's.
    pub(in crate::exec) fn pair(&self, at: &RangeAt<'_>, left: &mut Row, pairing: &mut Pairing) -> Result<bool> {
        let Pairing { key, pair } = pairing;
        key.clear();
        for &k in &self.keys {
            // A null compared finds no entry, and is no read.
            let Some(value) = left.get(k).filter(|v| !v.is_null()) else { return Ok(false) };
            key.push(value.clone());
        }
        let Some(found) = self.point.read(at.st, at.tx, key)? else { return Ok(false) };
        pair.clear();
        pair.append(left);
        let from = pair.len();
        pair.resize(from.saturating_add(self.width), Value::Null);
        for (&column, value) in self.needed.iter().zip(found) {
            if let Some(slot) = pair.get_mut(from.saturating_add(column)) {
                *slot = value;
            }
        }
        match &self.cond {
            Some(cond) => Ok(eval_cond(cond, pair, at.subqueries)? == Truth::True),
            None => Ok(true),
        }
    }
}

impl Pairing {
    /// The pair made, lent.
    pub(in crate::exec) fn row(&mut self) -> &mut Row {
        &mut self.pair
    }
}
