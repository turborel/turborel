//! \[DESIGN\] (RULES.md D360) The conjuncts of a filter a column store's
//! scan is told, judged by the scan itself, a run of a frozen group at a
//! time, before it decodes the columns the filter does not read. This is
//! late materialization in the scan (Abadi, Myers, DeWitt and Madden,
//! "Materialization Strategies in a Column-Oriented DBMS", ICDE 2007). A
//! conjunct of one column, compared with a constant, IS NULL or IS NOT
//! NULL, IN or NOT IN constants, or an OR of these, of a column of exact
//! or approximate numbers, datetimes, truth values or character strings,
//! is judged over that column's vector as the run decodes it: a comparison
//! by a kernel, its constant folded to the column's scale once
//! (`storage::bound`); integers kept as runs once a run; a dictionary's
//! texts once an entry, its rows then by their codes; texts coded by a
//! table of symbols compared with the constant in place, no text decoded
//! (`judged/coded.rs`); anything else a value at a time, as
//! `Conjunct::matches_key` judges it, so what the scan keeps is what the
//! conjunct keeps.
//!
//! A scan is told only conjuncts every row its filter keeps satisfies, of a
//! condition that cannot raise an error (`logical/pushed.rs`), and the
//! filter still judges every row the scan gives (RULES.md D15). So a row
//! left out here is one the filter would have left out, and a conjunct a
//! column's vector cannot be judged over is left to the filter.

mod coded;

use std::cmp::Ordering;

use super::group::{Group, Seal};
use super::scan::{Reading, column_of_run};
use crate::error::Result;
use crate::index::Probe;
use crate::kernels::{Kernels, Portable};
use crate::storage::bound::{Bound, holds, rescaled_bound};
use crate::storage::mask::index;
use crate::storage::sel::rows_of;
use crate::storage::{Column, Condition, Conjunct, Conjunction, Datetimes, Floats, Ints, Mask, Texts};
use crate::value::{CompOp, DataType, Decimal, Value, compare_strings};

/// \[DESIGN\] A conjunct of one column the scan judges, and whether it is
/// true of a null.
#[derive(Debug, Clone)]
struct Pred {
    column: usize,
    conjunct: Conjunct,
    of_null: bool,
}

/// \[DESIGN\] (RULES.md D360) The conjuncts of a scan's condition it judges
/// itself, in their order, each of one column of a type it decodes into a
/// vector of its kind.
#[derive(Debug, Clone, Default)]
pub struct Judgment {
    preds: Vec<Pred>,
}

/// \[DESIGN\] What judging a run gave: the places of its rows kept, from its
/// first, ascending; and each column decoded whole to judge it, by its
/// number, which the run gives as decoded if every row is kept.
pub struct Judged {
    /// The rows kept.
    pub kept: Vec<u32>,
    /// The columns decoded of every row of the run.
    pub decoded: Vec<(usize, Column)>,
}

impl Judgment {
    /// The conjuncts of `condition` a scan of a table of columns of
    /// `types` judges: each of one column (`Conjunct::columns`), a
    /// comparison with a constant, IS NULL, IN, NOT IN or an OR of these,
    /// of a type whose vectors it judges (`judged_type`).
    pub fn of(condition: Option<&Conjunction>, types: &[DataType]) -> Judgment {
        let preds = condition
            .iter()
            .flat_map(|c| c.conjuncts())
            .filter(|c| judged_conjunct(c))
            .filter_map(|c| {
                let columns = c.columns();
                let [column] = columns.as_slice() else { return None };
                let column = *column;
                types.get(column).filter(|t| judged_type(t))?;
                Some(Pred { column, conjunct: c.clone(), of_null: c.matches_key(&[&Value::Null]) })
            })
            .collect();
        Judgment { preds }
    }

    /// Whether it judges nothing.
    pub fn is_empty(&self) -> bool {
        self.preds.is_empty()
    }

    /// \[DESIGN\] The rows of the run `from` to `to` of `group`, of a
    /// table of columns of `types`, of `given`, places from its first, or
    /// of all, that each conjunct keeps, judged in turn, each of the rows
    /// the ones before kept: of a column of texts coded by symbols, in
    /// place; else of its vector, decoded once for the run.
    ///
    /// # Errors
    ///
    /// As decoding a chunk.
    pub fn judge<S: Seal>(
        &self,
        group: &Group,
        (pages, prepared): Reading<'_, '_, S>,
        (types, (from, to)): (&[DataType], (u32, u32)),
        given: Option<Vec<u32>>,
    ) -> Result<Judged> {
        let (mut kept, mut decoded) = (given, Vec::<(usize, Column)>::new());
        for pred in &self.preds {
            if kept.as_ref().is_some_and(Vec::is_empty) {
                break;
            }
            let Some(t) = types.get(pred.column) else { continue };
            if let Some(rows) = coded::kept(group, (pages, &mut *prepared), (pred, t), (from, to), kept.as_deref())? {
                kept = Some(rows);
                continue;
            }
            if !decoded.iter().any(|(c, _)| *c == pred.column) {
                decoded.push((pred.column, column_of_run(group, (pages, &mut *prepared), (pred.column, t), (from, to))?));
            }
            let at = decoded.iter().position(|(c, _)| *c == pred.column).unwrap_or(0);
            if let Some(rows) = decoded.get(at).and_then(|(_, column)| pred.kept(column, kept.as_deref())) {
                kept = Some(rows);
            }
        }
        let all = usize::try_from(to.saturating_sub(from)).unwrap_or(0);
        Ok(Judged { kept: kept.unwrap_or_else(|| rows_of(0..all)), decoded })
    }
}

/// Whether the scan judges `conjunct`: a comparison with a constant, IS
/// NULL, IN, NOT IN, or an OR of these, of one column.
fn judged_conjunct(conjunct: &Conjunct) -> bool {
    match conjunct {
        Conjunct::Any(each) => each.iter().all(judged_leaf),
        Conjunct::Condition(_) | Conjunct::Null { .. } | Conjunct::In { .. } | Conjunct::NotIn { .. } => judged_leaf(conjunct),
    }
}

/// As `judged_conjunct`, of a conjunct not an OR.
fn judged_leaf(conjunct: &Conjunct) -> bool {
    match conjunct {
        Conjunct::Condition(Condition { probe: Probe::Compare { .. }, .. })
        | Conjunct::Null { .. }
        | Conjunct::In { .. }
        | Conjunct::NotIn { .. } => true,
        Conjunct::Condition(_) | Conjunct::Any(_) => false,
    }
}

/// Whether a column of type `t` is decoded into a vector of its kind,
/// which the scan judges: exact or approximate numbers, datetimes of no
/// time zone, truth values, character strings read as text.
fn judged_type(t: &DataType) -> bool {
    match t {
        DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { zoned: false, .. }
        | DataType::Timestamp { zoned: false, .. } => true,
        DataType::Character { .. } | DataType::Varying { .. } | DataType::Distinct(_) => super::padding(t).is_some(),
        DataType::Time { zoned: true, .. }
        | DataType::Timestamp { zoned: true, .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Row(_)
        | DataType::Json
        | DataType::Xml(_)
        | DataType::Extension(_) => false,
    }
}

impl Pred {
    /// The rows of `given`, or of all, of `column` it keeps; none if the
    /// column is not a vector it judges.
    fn kept(&self, column: &Column, given: Option<&[u32]>) -> Option<Vec<u32>> {
        if let Conjunct::Null { not, .. } = self.conjunct {
            let (len, nulls) = shape(column)?;
            return Some(rows(len, given).into_iter().filter(|&r| nulls.is_some_and(|n| n.get(index(r))) != not).collect());
        }
        match column {
            Column::Ints(ints) => Some(self.ints(ints, given)),
            Column::Floats(floats) => Some(self.floats(floats, given)),
            Column::Datetimes(datetimes) => Some(self.datetimes(datetimes, given)),
            Column::Bools(bools) => Some(self.by_values(bools.len(), given, |r| bools.get(r).map(Value::Bool))),
            Column::Texts(texts) => self.texts(texts, given),
            Column::Values(_) | Column::Nulls(_) | Column::Late(_) => None,
        }
    }

    /// Its comparison's operator and constant, if it is one.
    fn compared(&self) -> Option<(CompOp, &Value)> {
        match &self.conjunct {
            Conjunct::Condition(Condition { probe: Probe::Compare { op, value }, .. }) => Some((*op, value)),
            Conjunct::Condition(_) | Conjunct::Null { .. } | Conjunct::In { .. } | Conjunct::NotIn { .. } | Conjunct::Any(_) => None,
        }
    }

    /// The rows of `given`, or of all `len`, it keeps, each judged of its
    /// value, `value` giving none of a null.
    fn by_values(&self, len: usize, given: Option<&[u32]>, value: impl Fn(usize) -> Option<Value>) -> Vec<u32> {
        let keeps = |r: u32| value(index(r)).map_or(self.of_null, |v| self.conjunct.matches_key(&[&v]));
        rows(len, given).into_iter().filter(|&r| keeps(r)).collect()
    }

    /// Of integers: a comparison by the kernel, its constant folded to
    /// their scale once; of integers kept as runs, none null, judged once a
    /// run (a run's first row may be a null, whose unit is not the run's).
    fn ints(&self, ints: &Ints, given: Option<&[u32]>) -> Vec<u32> {
        let bound = self.compared().and_then(|(op, c)| rescaled_bound(ints.scale, op, c));
        let value = |u: i64| Value::Exact(Decimal::new(i128::from(u), ints.scale));
        let keeps = |u: i64| bound.map_or_else(|| self.conjunct.matches_key(&[&value(u)]), |b| b.holds(u));
        if ints.nulls.is_none() && ints.runs().is_some() {
            let all = rows(ints.units.len(), given);
            if let Some(runs) = ints.runs_of(&all) {
                return runs.filter(|&(unit, _)| keeps(unit)).flat_map(|(_, these)| these.iter().copied()).collect();
            }
        }
        let mut out = Vec::new();
        match bound {
            Some(Bound::Of(op, c)) => Portable.select_i64(&ints.units, (op, c), (ints.nulls.as_ref(), given), &mut out),
            Some(Bound::All) => out.extend(rows(ints.units.len(), given).into_iter().filter(|&r| !ints.is_null(index(r)))),
            Some(Bound::None) => {}
            None => {
                return self
                    .by_values(ints.units.len(), given, |r| (!ints.is_null(r)).then(|| value(ints.units.get(r).copied().unwrap_or(0))));
            }
        }
        out
    }

    /// Of approximate numbers: a comparison with an approximate constant
    /// by the kernel, as two numbers compare by algebraic value.
    fn floats(&self, floats: &Floats, given: Option<&[u32]>) -> Vec<u32> {
        if let Some((op, Value::Approx(c))) = self.compared() {
            let mut out = Vec::new();
            Portable.select_f64(&floats.numbers, (op, *c), (floats.nulls.as_ref(), given), &mut out);
            return out;
        }
        self.by_values(floats.numbers.len(), given, |r| floats.get(r).map(Value::Approx))
    }

    /// Of datetimes of no time zone: a comparison with a datetime of their
    /// fields and of none by the kernel, by their instants.
    fn datetimes(&self, datetimes: &Datetimes, given: Option<&[u32]>) -> Vec<u32> {
        if let Some((op, Value::Datetime(c))) = self.compared()
            && c.moment == datetimes.moment
            && !c.zoned
            && !datetimes.zoned
        {
            let mut out = Vec::new();
            Portable.select_i64(&datetimes.utc, (op, c.utc), (datetimes.nulls.as_ref(), given), &mut out);
            return out;
        }
        self.by_values(datetimes.len(), given, |r| datetimes.get(r).map(Value::Datetime))
    }

    /// Of texts, if every constant is a string or a null: of a store's
    /// dictionaries, each entry once, the rows then by the kernel
    /// `select_codes`; else each text in place.
    fn texts(&self, texts: &Texts, given: Option<&[u32]>) -> Option<Vec<u32>> {
        text_keeps(&self.conjunct, &|_| Some(Ordering::Equal))?;
        let keeps = |s: &str| text_keeps(&self.conjunct, &|c| Some(compare_strings(s, c))) == Some(true);
        if let Some(coded) = texts.coded.as_ref().filter(|_| !self.of_null) {
            let mut entries = Mask::none(coded.entries.len());
            for e in (0..coded.entries.len()).filter(|&e| coded.entries.get(e).is_some_and(keeps)) {
                entries.set(e);
            }
            let mut out = Vec::new();
            Portable.select_codes(&coded.codes, entries.words(), (texts.nulls.as_ref(), given), &mut out);
            return Some(out);
        }
        Some(rows(texts.len(), given).into_iter().filter(|&r| texts.get(index(r)).map_or(self.of_null, keeps)).collect())
    }
}

/// \[DESIGN\] Whether `conjunct` is true of a text not null, each
/// constant's order against it as `order` gives it, as `matches_key`
/// judges it of the text's value (`value::compare` of two strings is
/// `compare_strings`); none if a constant is neither a string nor a null.
fn text_keeps(conjunct: &Conjunct, order: &dyn Fn(&str) -> Option<Ordering>) -> Option<bool> {
    match conjunct {
        // Each operand of one column, never itself an OR (`judged_conjunct`).
        Conjunct::Any(each) => each.iter().try_fold(false, |any, c| Some(any | leaf_text_keeps(c, order)?)),
        Conjunct::Condition(_) | Conjunct::Null { .. } | Conjunct::In { .. } | Conjunct::NotIn { .. } => leaf_text_keeps(conjunct, order),
    }
}

/// As `text_keeps`, of a conjunct not an OR.
fn leaf_text_keeps(conjunct: &Conjunct, order: &dyn Fn(&str) -> Option<Ordering>) -> Option<bool> {
    // Of a string, as it compares; of a null, never; of another, not known.
    let of = |v: &Value, truth: &dyn Fn(Option<Ordering>) -> bool| {
        if let Value::Char(c) = v { Some(truth(order(c))) } else { v.is_null().then_some(false) }
    };
    let equal = |v: &Value| of(v, &|o| o == Some(Ordering::Equal));
    let apart = |v: &Value| of(v, &|o| o.is_some_and(Ordering::is_ne));
    match conjunct {
        Conjunct::Null { not, .. } => Some(*not),
        Conjunct::Condition(Condition { probe: Probe::Compare { op, value: Value::Char(c) }, .. }) => Some(holds(*op, order(c))),
        Conjunct::In { values, .. } => values.iter().try_fold(false, |any, v| Some(any | equal(v)?)),
        Conjunct::NotIn { values, .. } => values.iter().try_fold(true, |all, v| Some(all & apart(v)?)),
        Conjunct::Condition(_) | Conjunct::Any(_) => None,
    }
}

/// The rows of a vector of `len` of `given`, or all of them.
fn rows(len: usize, given: Option<&[u32]>) -> Vec<u32> {
    given.map_or_else(|| rows_of(0..len), <[u32]>::to_vec)
}

/// The rows of `column` and its nulls, if it is a vector the scan judges.
fn shape(column: &Column) -> Option<(usize, Option<&Mask>)> {
    match column {
        Column::Ints(ints) => Some((ints.units.len(), ints.nulls.as_ref())),
        Column::Floats(floats) => Some((floats.numbers.len(), floats.nulls.as_ref())),
        Column::Datetimes(datetimes) => Some((datetimes.len(), datetimes.nulls.as_ref())),
        Column::Bools(bools) => Some((bools.len(), bools.nulls.as_ref())),
        Column::Texts(texts) => Some((texts.len(), texts.nulls.as_ref())),
        Column::Values(_) | Column::Nulls(_) | Column::Late(_) => None,
    }
}

// Unit tests: each conjunct of each kind judged as its values are, of
// each encoding (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
