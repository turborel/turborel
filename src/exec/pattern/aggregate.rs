//! [ISO/IEC 9075-2:2023 §10.9 GR1a to c, GR4, GR10] A set function over
//! the rows of a row pattern match mapped to a variable (R030): each row
//! evaluated with its classifier, its filter judged, nulls left out and
//! told of, a DISTINCT one's values kept in a bag of its work area that
//! spills, an inverse distribution function's values held as rows are
//! and sorted when it finishes. Rows may be fed and the function finished
//! again and again, so a running set function over a match's rows in
//! order is fed each row once.

use std::slice;

use super::scene::{Reading, Scene};
use crate::error::{Result, SqlState};
use crate::exec::sort::Sorter;
use crate::exec::work::Bag;
use crate::exec::{HeldRows, broken, eval, eval_cond, warn};
use crate::functions::{Accumulator, picked, picked_value};
use crate::logical::{AggCall, Distribution, SortKey, Truth};
use crate::spill::norm_row;
use crate::storage::Row;
use crate::value::Value;

/// The columns a value's expressions read beyond a row of the input: its
/// classifier and the match's number.
const EXTRA: usize = 2;

/// A set function fed some rows of a match.
pub(in crate::exec) struct Fed {
    acc: Box<dyn Accumulator>,
    /// Of DISTINCT, the values it took [§10.9 GR5].
    seen: Option<Bag>,
    /// Of an inverse distribution function, what it holds.
    inverse: Option<Inverse>,
}

/// [ISO/IEC 9075-2:2023 §10.9 GR10] What an inverse distribution function
/// fed some rows of a match holds. Its distribution is its call's, kept
/// with its values so that whether it is one is decided once.
struct Inverse {
    /// What it picks of its values.
    distribution: Distribution,
    /// Its values not null [GR10f].
    values: HeldRows<Row>,
    /// Its fraction, computed of the first row fed [§10.9 SR14].
    fraction: Option<Value>,
}

impl Fed {
    /// `call`, fed no row yet, of `reading`.
    pub(in crate::exec) fn new(call: &AggCall, reading: &Reading<'_>) -> Fed {
        Fed {
            acc: call.func.0.accumulator(call.result.clone()),
            seen: call.distinct.then(|| Bag::new(reading.work.another())),
            inverse: call.distribution.as_ref().map(|d| Inverse {
                distribution: Distribution::clone(d),
                values: HeldRows::new(reading.space.clone()),
                fraction: None,
            }),
        }
    }

    /// Feeds `call` the `place`th row of the partition of `scene`.
    ///
    /// # Errors
    ///
    /// Its filter's, argument's and fraction's evaluations'; the set
    /// function's, taking the value; the work area's.
    pub(in crate::exec) fn feed(&mut self, call: &AggCall, reading: &Reading<'_>, scene: &mut Scene<'_>, place: usize) -> Result<()> {
        let row = reading.extended(scene, place)?;
        if let Some(inverse) = &mut self.inverse
            && inverse.fraction.is_none()
        {
            inverse.fraction = Some(eval(&inverse.distribution.fraction, &row, reading.runner)?);
        }
        // [ISO/IEC 9075-2:2023 §10.9 GR4] a row its filter is not true of
        // is not aggregated, its argument not computed.
        if let Some(c) = &call.filter {
            let truth = eval_cond(c, &row, reading.runner)?;
            if truth != Truth::True {
                return Ok(());
            }
        }
        // [X3.135-1986 §5.8 GR4b] COUNT(*) counts rows.
        let Some(e) = &call.arg else { return self.acc.update(&Value::Null) };
        let v = eval(e, &row, reading.runner)?;
        if v.is_null() {
            // [ISO/IEC 9075:1992 §6.5 GR1b] a null eliminated warns.
            warn(reading.runner, SqlState::NULL_VALUE_ELIMINATED);
            return Ok(());
        }
        if let Some(inverse) = &mut self.inverse {
            return inverse.values.push(vec![v]);
        }
        if let Some(seen) = &mut self.seen {
            let new = seen.add_new(&norm_row(slice::from_ref(&v)))?;
            if !new {
                return Ok(());
            }
        }
        self.acc.update(&v)
    }

    /// The result of `call` over the rows fed so far, of `scene`; it may
    /// be fed more after.
    ///
    /// # Errors
    ///
    /// The set function's; of an inverse distribution function, SQLSTATE
    /// 22003 if its fraction is below 0 or above 1, even of no rows.
    pub(in crate::exec) fn finish(&mut self, call: &AggCall, reading: &Reading<'_>, scene: &mut Scene<'_>) -> Result<Value> {
        let Some(Inverse { distribution: d, values, fraction }) = self.inverse.as_mut() else { return self.acc.finish() };
        let fraction = if let Some(f) = fraction {
            f.clone()
        } else {
            // [§10.9 SR14] Its fraction reads no row's columns, so of no row
            // fed, a row of nulls gives it.
            let width = scene.rows.get(0)?.map_or(0, |r| r.len()).saturating_add(EXTRA);
            eval(&d.fraction, &vec![Value::Null; width], reading.runner)?
        };
        let n = u64::try_from(values.len()).unwrap_or(u64::MAX);
        let Some(chosen) = picked(d.continuous, &fraction, n)? else { return Ok(Value::Null) };
        if n == 0 {
            return Ok(Value::Null);
        }
        let mut work = reading.work.part(reading.space.limit);
        let mut sorter = Sorter::new(vec![SortKey { column: 0, descending: d.order.descending, nulls_first: false, octets: false }]);
        values.visit(&mut |row: &Row| sorter.push(row.clone(), &mut work))?;
        let (mut low, mut high) = (Value::Null, Value::Null);
        for (at, row) in (0_u64..).zip(sorter.sorted(&mut work)?) {
            let v = row?.into_iter().next().ok_or_else(|| broken("a sorted value of no column"))?;
            if at == chosen.low {
                low = v.clone();
            }
            if at == chosen.high {
                high = v;
                break;
            }
        }
        picked_value(d.continuous, (low, high), chosen.factor, &call.result)
    }
}
