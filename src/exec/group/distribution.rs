//! [ISO/IEC 9075-2:2023 §10.9 GR10] The inverse distribution functions of
//! a grouping. \[DESIGN\] Their values may outgrow memory (RULES.md D38),
//! so no accumulator holds them. Each group keeps, of each such call, its
//! fraction, computed of its first row, and how many values not null it
//! was fed. Each value is sorted with the number of its group's first row
//! and its call's place, in a work area of its own that spills, so the
//! values of a group's call come together and in order. Once a pass has
//! fed every row, its groups are finished in the order of their first
//! rows as the sorted values are read alongside, each call reading its
//! values once and keeping only the one or two its fraction picks.

use crate::error::Result;
use crate::exec::sort::Sorter;
use crate::exec::work::Work;
use crate::exec::{RowIter, Runner, broken, column, eval};
use crate::functions::{picked, picked_value};
use crate::logical::{AggCall, Distribution, SortKey, expr_columns};
use crate::storage::Row;
use crate::value::{Decimal, SortOrder, Value, compare};

/// The column of a sorted value that holds the number of its group's
/// first row.
const GROUP: usize = 0;
/// The column that holds the place of its call among the set functions.
const CALL: usize = 1;
/// The column that holds the value of an ascending key, else null.
const ASCENDING: usize = 2;
/// The column that holds the value of a descending key, else null.
const DESCENDING: usize = 3;

/// [GR10a] Of an inverse distribution function of a group, the fraction
/// NVE and how many values not null it was fed so far.
pub(super) struct Pick {
    fraction: Value,
    count: u64,
}

/// [ISO/IEC 9075-2:2023 §10.9 SR14] Of each of `aggs` of a new group, if
/// an inverse distribution function, its fraction, computed of `row`: of
/// the group's grouping columns, constants and outer references alone, so
/// any of its rows gives it. None of any call if none is one.
pub(super) fn picks(aggs: &[AggCall], row: &Row, runner: &Runner) -> Result<Vec<Option<Pick>>> {
    if aggs.iter().all(|a| a.distribution.is_none()) {
        return Ok(Vec::new());
    }
    let pick = |d: &Distribution| eval(&d.fraction, row, runner).map(|fraction| Pick { fraction, count: 0 });
    aggs.iter().map(|a| a.distribution.as_deref().map(pick).transpose()).collect()
}

/// [ISO/IEC 9075-2:2023 §7.13 GR2a] A row of a group of no rows, whose
/// fractions are computed of it: its grouping values `values` at the
/// grouping columns `groups`, every other column null.
pub(super) fn seeded(aggs: &[AggCall], groups: &[usize], values: &Row) -> Row {
    let mut read = groups.to_vec();
    aggs.iter().filter_map(|a| a.distribution.as_deref()).for_each(|d| expr_columns(&d.fraction, &mut read));
    let width = read.iter().max().map_or(0, |c| c.saturating_add(1));
    let mut row = vec![Value::Null; width];
    for (&at, v) in groups.iter().zip(values) {
        if let Some(cell) = row.get_mut(at) {
            *cell = v.clone();
        }
    }
    row
}

/// The number `n` as a value sorted.
fn number(n: u64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// The place `n` of a call as a value sorted.
fn place(n: usize) -> Value {
    Value::Exact(Decimal::new(i128::try_from(n).unwrap_or(i128::MAX), 0))
}

/// The values of a pass's inverse distribution functions being sorted, in
/// a work area of their own.
pub(super) struct Sorting {
    sorter: Sorter,
    work: Work,
}

impl Sorting {
    /// Values to be sorted in `work`: by group, then call, then value,
    /// ascending in one column and descending in the other, so the values
    /// of each call are in its key's order.
    pub(super) fn new(work: Work) -> Sorting {
        let key = |column, descending| SortKey { column, descending, nulls_first: false };
        let keys = vec![key(GROUP, false), key(CALL, false), key(ASCENDING, false), key(DESCENDING, true)];
        Sorting { sorter: Sorter::new(keys), work }
    }

    /// [GR10a, GR10f] Adds `v`, the value of a row of the call of place
    /// `call` of the group whose first row is numbered `first`, if it is
    /// not null, sorted in `order`, and counts it in `pick`.
    pub(super) fn push(&mut self, (first, call): (u64, usize), (order, pick): (SortOrder, &mut Pick), v: Value) -> Result<()> {
        if v.is_null() {
            return Ok(());
        }
        pick.count = pick.count.saturating_add(1);
        let (ascending, descending) = if order.descending { (Value::Null, v) } else { (v, Value::Null) };
        self.sorter.push(vec![number(first), place(call), ascending, descending], &mut self.work)
    }

    /// The values added, sorted.
    pub(super) fn sorted(self) -> Result<Sorted> {
        let Sorting { sorter, mut work } = self;
        let rows = sorter.sorted(&mut work)?;
        Ok(Sorted { rows, _work: work })
    }
}

/// The values of a pass's inverse distribution functions, sorted, read in
/// the order of their groups and calls, with the work area that holds
/// their runs.
pub(super) struct Sorted {
    rows: RowIter,
    _work: Work,
}

/// [ISO/IEC 9075-2:2023 §10.9 GR10] The result of the inverse
/// distribution function `call`, of place `at` in the group whose first
/// row is numbered `first`, of `pick`, its values read from `sorted`: the
/// one or two its fraction picks of them.
///
/// # Errors
///
/// SQLSTATE 22003 if its fraction is below 0 or above 1, even of no
/// values; as computing its result raises.
pub(super) fn picked_of(sorted: Option<&mut Sorted>, (first, at): (u64, usize), call: &AggCall, pick: &Pick) -> Result<Value> {
    let Some(d) = call.distribution.as_deref() else { return Err(broken("an inverse distribution function is no such call")) };
    let chosen = picked(d.continuous, &pick.fraction, pick.count)?;
    if pick.count == 0 {
        return Ok(Value::Null);
    }
    let Some(sorted) = sorted else { return Err(broken("the values of an inverse distribution function were not sorted")) };
    let (mut low, mut high) = (Value::Null, Value::Null);
    let (key, spot) = ((number(first), place(at)), chosen.map(|c| (c.low, c.high)));
    for n in 0..pick.count {
        let row = sorted.rows.next().ok_or_else(|| broken("an inverse distribution function's values ran out"))??;
        let alike = |c, v| column(&row, c).map(|w| compare(w, v).is_some_and(std::cmp::Ordering::is_eq));
        if !(alike(GROUP, &key.0)? && alike(CALL, &key.1)?) {
            return Err(broken("an inverse distribution function's values are not its group's"));
        }
        let v = if d.order.descending { column(&row, DESCENDING)? } else { column(&row, ASCENDING)? };
        if spot.is_some_and(|(l, _)| l == n) {
            low = v.clone();
        }
        if spot.is_some_and(|(_, h)| h == n) {
            high = v.clone();
        }
    }
    let Some(c) = chosen else { return Ok(Value::Null) };
    picked_value(d.continuous, (low, high), c.factor, &call.result)
}
