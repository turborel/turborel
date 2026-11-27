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
//! \[DESIGN\] (RULES.md D1241) The bound of a sort of which only the
//! first rows are asked for (`Conjunct::Dynamic`) is judged too, as the
//! test the bound gives of a row as the run is judged
//! (`Threshold::row_test`), a comparison of its column with the value it
//! holds, and none while it holds no value. So a run's rows past the
//! bound are left out before the filter's other columns are decoded for
//! them. The sort would drop each such row, so leaving it out changes
//! nothing it gives.
//!
//! \[DESIGN\] (RULES.md D1187) Of a key that orders character strings
//! by their octets the bound holds a string, and its test is a
//! comparison of octets (`Conjunct::Unpadded`), judged as any such
//! comparison is, of a dictionary's entries, of texts coded by symbols in
//! place and of plain texts.
//!
//! \[DESIGN\] (RULES.md D1251, D1253) A LIKE or NOT LIKE of a constant
//! pattern of no `_` is judged of a VARCHAR column's texts where they
//! lie, a chunk at a time, after the conjuncts that cost less a row:
//! texts coded by symbols by the pattern's machine stepped a code at a
//! time (`judged/coded.rs`), a dictionary's entries once each
//! (`judged/entries.rs`), and other texts by their octets as written.
//! Its column is never decoded to judge it. Of a CHARACTER column, and
//! of a chunk that is malformed, it is not judged, and the filter judges
//! it of the rows given.
//!
//! \[DESIGN\] (RULES.md D1293) A comparison with a constant that the
//! bounds of every part a run lies in prove true of every row of it is
//! not judged of that run, and its column is not decoded to judge it
//! (`judged/proved.rs`).
//!
//! A scan is told only conjuncts every row its filter keeps satisfies, of a
//! condition that cannot raise an error (`logical/pushed.rs`), and the
//! filter still judges every row the scan gives (RULES.md D15). So a row
//! left out here is one the filter would have left out, and a conjunct a
//! column's vector cannot be judged over is left to the filter.

mod coded;
mod entries;
mod lengths;
mod proved;
mod sketched;
mod sorted;

use std::borrow::Cow;
use std::cmp::Ordering;

use super::group::{Group, Seal};
use super::scan::{Reading, column_of_run};
use crate::error::Result;
use crate::index::Probe;
use crate::kernels::Kernels;
use crate::storage::bound::{Bound, holds, rescaled_bound};
use crate::storage::mask::index;
use crate::storage::sel::rows_of;
use crate::storage::{Column, Condition, Conjunct, Conjunction, Datetimes, Floats, Ints, Mask, Texts};
use crate::value::{CompOp, DataType, Decimal, Value, compare_strings};

/// \[DESIGN\] The share of a run's rows, one in this many, past which the
/// run is given whole and the filter selects the rows kept of it
/// (`rows/frozen/batches/run.rs`): so many rows are read as cheaply
/// decoded whole as at their places, and a batch of them selected costs
/// less than one gathered. A LIKE stops being judged of a run once the
/// rows it keeps are seen to reach that share (`coded.rs`).
pub const KEPT_SHARE: usize = 8;

/// \[DESIGN\] A conjunct of one column the scan judges, and whether it is
/// true of a null.
#[derive(Debug, Clone)]
struct Pred {
    /// Its conjunct's place in the condition.
    at: usize,
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
        let mut preds: Vec<Pred> = condition
            .iter()
            .flat_map(|c| c.conjuncts().iter().enumerate())
            .filter(|(_, c)| judged_conjunct(c))
            .filter_map(|(at, c)| {
                let columns = c.columns();
                let [column] = columns.as_slice() else { return None };
                let column = *column;
                types.get(column).filter(|t| judged_type(t))?;
                Some(Pred { at, column, conjunct: c.clone(), of_null: c.matches_key(&[&Value::Null]) })
            })
            .collect();
        // A LIKE goes last, as it reads every code of a text where a
        // comparison reads a code or two, so the others leave it fewer rows.
        preds.sort_by_key(Pred::liked);
        Judgment { preds }
    }

    /// Puts its conjuncts in the order of `order`, their places in the
    /// condition, narrowest first (`scan::read_ordered`, RULES.md D511);
    /// one `order` leaves out after the rest, as they were, and a LIKE
    /// after them all (RULES.md D1251).
    pub fn order_by(&mut self, order: &[usize]) {
        self.preds.sort_by_key(|p| (p.liked(), order.iter().position(|&at| at == p.at).unwrap_or(usize::MAX)));
    }

    /// Whether it judges nothing.
    pub fn is_empty(&self) -> bool {
        self.preds.is_empty()
    }

    /// \[DESIGN\] (RULES.md D1241) Whether it judges a sort's bound: its
    /// reader is a sort of its first rows, which keeps few of the rows it
    /// is given, so a run given whole leaves its late columns late.
    pub fn bounded(&self) -> bool {
        self.preds.iter().any(|p| matches!(p.conjunct, Conjunct::Dynamic { .. }))
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
            // A sort's bound is judged as the test it is now. One not yet
            // set keeps every row, so its column is not decoded for it.
            let Some(pred) = pred.now() else { continue };
            let pred = &*pred;
            let Some(t) = types.get(pred.column) else { continue };
            // A LIKE is judged in place or left to the filter, its
            // column not decoded to judge it (RULES.md D1251).
            if let Conjunct::Like { like, .. } = &pred.conjunct {
                kept = coded::liked(group, (pages, &mut *prepared), (pred, like, t), (from, to), kept.as_deref())?.or(kept);
                continue;
            }
            // A comparison the bounds of the run's parts prove keeps every
            // row, its column not decoded to judge it (RULES.md D1293).
            if proved::proved(group, (pred, t), (from, to))? {
                continue;
            }
            if let Some(rows) = coded::kept(group, (pages, &mut *prepared), (pred, t), (from, to), kept.as_deref())? {
                kept = Some(rows);
                continue;
            }
            if !decoded.iter().any(|(c, _)| *c == pred.column) {
                decoded.push((pred.column, column_of_run(group, (pages, &mut *prepared), (pred.column, t), (from, to))?));
            }
            let at = decoded.iter().position(|(c, _)| *c == pred.column).unwrap_or(0);
            if let Some(rows) = decoded.get(at).and_then(|(_, column)| pred.kept(column, kept.as_deref(), pages.kernels)) {
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
        Conjunct::Condition(_) | Conjunct::Null { .. } | Conjunct::In { .. } | Conjunct::NotIn { .. } | Conjunct::Unpadded { .. } => {
            judged_leaf(conjunct)
        }
        // A sort's bound is the test it is of a row as a run is judged
        // (`Pred::now`, RULES.md D1241), and a LIKE is judged of texts
        // coded by symbols (RULES.md D1251).
        Conjunct::Dynamic { .. } | Conjunct::Like { .. } => true,
        // A join's spans pass over groups and parts alone (RULES.md D600).
        Conjunct::Spans { .. } => false,
    }
}

/// As `judged_conjunct`, of a conjunct not an OR.
fn judged_leaf(conjunct: &Conjunct) -> bool {
    match conjunct {
        Conjunct::Condition(Condition { probe: Probe::Compare { .. }, .. })
        | Conjunct::Null { .. }
        | Conjunct::In { .. }
        | Conjunct::NotIn { .. }
        | Conjunct::Unpadded { .. } => true,
        Conjunct::Condition(_) | Conjunct::Any(_) | Conjunct::Dynamic { .. } | Conjunct::Spans { .. } | Conjunct::Like { .. } => false,
    }
}

/// Whether a column of type `t` is decoded into a vector of its kind,
/// which the scan judges: exact numbers, read as integers where 64 bits
/// hold a run's values (`scan::ints`, RULES.md D1287) and left to the
/// filter where they do not, approximate numbers, datetimes of no time
/// zone, truth values, character strings read as text.
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
    fn kept(&self, column: &Column, given: Option<&[u32]>, kernels: &dyn Kernels) -> Option<Vec<u32>> {
        if let Conjunct::Null { not, .. } = self.conjunct {
            let (len, nulls) = shape(column)?;
            return Some(rows(len, given).into_iter().filter(|&r| nulls.is_some_and(|n| n.get(index(r))) != not).collect());
        }
        match column {
            Column::Ints(ints) => Some(self.ints(ints, given, kernels)),
            Column::Floats(floats) => Some(self.floats(floats, given, kernels)),
            Column::Datetimes(datetimes) => Some(self.datetimes(datetimes, given, kernels)),
            Column::Bools(bools) => Some(self.by_values(bools.len(), given, |r| bools.get(r).map(Value::Bool))),
            Column::Texts(texts) => self.texts(texts, given, kernels),
            Column::Values(_) | Column::Nulls(_) | Column::Late(_) => None,
        }
    }

    /// Its comparison's operator and constant, if it is one.
    fn compared(&self) -> Option<(CompOp, &Value)> {
        match &self.conjunct {
            Conjunct::Condition(Condition { probe: Probe::Compare { op, value }, .. }) => Some((*op, value)),
            Conjunct::Condition(_)
            | Conjunct::Null { .. }
            | Conjunct::In { .. }
            | Conjunct::NotIn { .. }
            | Conjunct::Any(_)
            | Conjunct::Dynamic { .. }
            | Conjunct::Spans { .. }
            | Conjunct::Unpadded { .. }
            | Conjunct::Like { .. } => None,
        }
    }

    /// Whether it is a LIKE (RULES.md D1251).
    fn liked(&self) -> bool {
        matches!(self.conjunct, Conjunct::Like { .. })
    }

    /// \[DESIGN\] (RULES.md D1241) It as a run is judged: itself, but of a
    /// sort's bound the test the bound is of a row now
    /// (`Threshold::row_test`), a conjunct of a kind judged as any other,
    /// never true of a null; none while the bound holds no value, when it
    /// keeps every row. The bound chooses the test by what it holds, so a
    /// bound of another kind of key is judged here with no change.
    fn now(&self) -> Option<Cow<'_, Pred>> {
        let Conjunct::Dynamic { column, threshold } = &self.conjunct else { return Some(Cow::Borrowed(self)) };
        let conjunct = threshold.row_test(*column)?;
        Some(Cow::Owned(Pred { at: self.at, column: self.column, conjunct, of_null: false }))
    }

    /// The rows of `given`, or of all `len`, it keeps, each judged of its
    /// value, `value` giving none of a null.
    fn by_values(&self, len: usize, given: Option<&[u32]>, value: impl Fn(usize) -> Option<Value>) -> Vec<u32> {
        let keeps = |r: u32| value(index(r)).map_or(self.of_null, |v| self.conjunct.matches_key(&[&v]));
        rows(len, given).into_iter().filter(|&r| keeps(r)).collect()
    }

    /// Of integers: a comparison by the kernel, its constant folded to
    /// their scale once; of integers kept as runs, none null, judged once a
    /// run, the runs and the rows walked together (`Ints::kept_by_runs`,
    /// RULES.md D1084; a run's first row may be a null, whose unit is not
    /// the run's).
    fn ints(&self, ints: &Ints, given: Option<&[u32]>, kernels: &dyn Kernels) -> Vec<u32> {
        let bound = self.compared().and_then(|(op, c)| rescaled_bound(ints.scale, op, c));
        let value = |u: i64| Value::Exact(Decimal::new(i128::from(u), ints.scale));
        let keeps = |u: i64| bound.map_or_else(|| self.conjunct.matches_key(&[&value(u)]), |b| b.holds(u));
        if ints.runs().is_some()
            && let Some(kept) = ints.kept_by_runs(given, &keeps)
        {
            return kept;
        }
        let mut out = Vec::new();
        match bound {
            Some(Bound::Of(op, c)) => kernels.select_i64(&ints.units, (op, c), (ints.nulls.as_ref(), given), &mut out),
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
    fn floats(&self, floats: &Floats, given: Option<&[u32]>, kernels: &dyn Kernels) -> Vec<u32> {
        if let Some((op, Value::Approx(c))) = self.compared() {
            let mut out = Vec::new();
            kernels.select_f64(&floats.numbers, (op, *c), (floats.nulls.as_ref(), given), &mut out);
            return out;
        }
        self.by_values(floats.numbers.len(), given, |r| floats.get(r).map(Value::Approx))
    }

    /// Of datetimes of no time zone: a comparison with a datetime of their
    /// fields and of none by the kernel, by their instants.
    fn datetimes(&self, datetimes: &Datetimes, given: Option<&[u32]>, kernels: &dyn Kernels) -> Vec<u32> {
        if let Some((op, Value::Datetime(c))) = self.compared()
            && c.moment == datetimes.moment
            && !c.zoned
            && !datetimes.zoned
        {
            let mut out = Vec::new();
            kernels.select_i64(&datetimes.utc, (op, c.utc), (datetimes.nulls.as_ref(), given), &mut out);
            return out;
        }
        self.by_values(datetimes.len(), given, |r| datetimes.get(r).map(Value::Datetime))
    }

    /// Of texts, if every constant is a string or a null: of a store's
    /// dictionaries, each entry once, the rows then by the kernel
    /// `select_codes`; else each text in place.
    fn texts(&self, texts: &Texts, given: Option<&[u32]>, kernels: &dyn Kernels) -> Option<Vec<u32>> {
        text_keeps(&self.conjunct, &|_| Some(Ordering::Equal))?;
        let keeps = |s: &str| text_keeps(&self.conjunct, &|c| Some(text_order(s, c))) == Some(true);
        if let Some(coded) = texts.coded.as_ref().filter(|_| !self.of_null) {
            let mut entries = Mask::none(coded.entries.len());
            for e in (0..coded.entries.len()).filter(|&e| coded.entries.get(e).is_some_and(keeps)) {
                entries.set(e);
            }
            let mut out = Vec::new();
            kernels.select_codes(&coded.codes, entries.words(), (texts.nulls.as_ref(), given), &mut out);
            return Some(out);
        }
        Some(rows(texts.len(), given).into_iter().filter(|&r| texts.get(index(r)).map_or(self.of_null, keeps)).collect())
    }
}

/// A constant a text is compared with, and how.
#[derive(Debug, Clone, Copy)]
enum Against<'c> {
    /// Each padded with spaces to the other's length [X3.135-1986 §5.11
    /// GR5].
    Padded(&'c str),
    /// [ISO/IEC 9075-2:2023 §4.3.2] Their octets, a prefix the less, as a
    /// collation of NO PAD that orders strings as their octets do compares
    /// them (RULES.md D1183).
    Unpadded(&'c [u8]),
}

/// The order of `text` against `constant`, as it says they compare.
fn text_order(text: &str, constant: Against<'_>) -> Ordering {
    match constant {
        Against::Padded(c) => compare_strings(text, c),
        Against::Unpadded(octets) => text.as_bytes().cmp(octets),
    }
}

/// \[DESIGN\] Whether `conjunct` is true of a text not null, each
/// constant's order against it as `order` gives it, as `matches_key`
/// judges it of the text's value (`value::compare` of two strings is
/// `compare_strings`); none if a constant is neither a string nor a null.
fn text_keeps(conjunct: &Conjunct, order: &dyn Fn(Against<'_>) -> Option<Ordering>) -> Option<bool> {
    match conjunct {
        // Each operand of one column, never itself an OR (`judged_conjunct`).
        Conjunct::Any(each) => each.iter().try_fold(false, |any, c| Some(any | leaf_text_keeps(c, order)?)),
        Conjunct::Condition(_) | Conjunct::Null { .. } | Conjunct::In { .. } | Conjunct::NotIn { .. } | Conjunct::Unpadded { .. } => {
            leaf_text_keeps(conjunct, order)
        }
        Conjunct::Dynamic { .. } | Conjunct::Spans { .. } | Conjunct::Like { .. } => None,
    }
}

/// As `text_keeps`, of a conjunct not an OR.
fn leaf_text_keeps(conjunct: &Conjunct, order: &dyn Fn(Against<'_>) -> Option<Ordering>) -> Option<bool> {
    // Of a string, as it compares; of a null, never; of another, not known.
    let of = |v: &Value, truth: &dyn Fn(Option<Ordering>) -> bool| {
        if let Value::Char(c) = v { Some(truth(order(Against::Padded(c)))) } else { v.is_null().then_some(false) }
    };
    let equal = |v: &Value| of(v, &|o| o == Some(Ordering::Equal));
    let apart = |v: &Value| of(v, &|o| o.is_some_and(Ordering::is_ne));
    match conjunct {
        Conjunct::Null { not, .. } => Some(*not),
        Conjunct::Condition(Condition { probe: Probe::Compare { op, value: Value::Char(c) }, .. }) => {
            Some(holds(*op, order(Against::Padded(c))))
        }
        Conjunct::Unpadded { op, octets, .. } => Some(holds(*op, order(Against::Unpadded(octets)))),
        Conjunct::In { values, .. } => values.iter().try_fold(false, |any, v| Some(any | equal(v)?)),
        Conjunct::NotIn { values, .. } => values.iter().try_fold(true, |all, v| Some(all & apart(v)?)),
        Conjunct::Condition(_) | Conjunct::Any(_) | Conjunct::Dynamic { .. } | Conjunct::Spans { .. } | Conjunct::Like { .. } => None,
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
