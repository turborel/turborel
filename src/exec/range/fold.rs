//! \[DESIGN\] The set functions of a direct read folded over its rows
//! (RULES.md D930): a grouping by no column is its one group, even of no
//! rows [X3.135-1986 §5.23 GR1, §5.25 GR1], so each function is fed each
//! row as it is read and finished once they are, with no group keyed and
//! no row held, as `group/streamed.rs` makes one whole. The values a
//! DISTINCT function took are kept as the grouping keeps them, the keys of
//! a bag of the statement's work area (`group.rs`), so they spill as its
//! do. A grouping by a column, one that makes groups before any row, and
//! an inverse distribution function are left to the executor.

use super::At;
use crate::error::{Result, SqlState};
use crate::exec::work::{Bag, Work};
use crate::exec::{eval, eval_cond, warn};
use crate::functions::Accumulator;
use crate::logical::{AggCall, Truth};
use crate::physical::Plan;
use crate::spill::put_norm;
use crate::storage::Row;
use crate::value::Value;

/// The set functions of a grouping by no column.
#[derive(Debug, Clone)]
pub(crate) struct Fold {
    aggs: Vec<AggCall>,
}

/// One run's fold: each function's state, and the values the DISTINCT
/// ones took.
pub(crate) struct Folding {
    states: Vec<Box<dyn Accumulator>>,
    seen: Option<Bag>,
    key: Vec<u8>,
}

impl Fold {
    /// The fold the aggregation `plan` is, if it is one: by no column,
    /// no group made before its rows, and set functions `of_aggs` folds.
    pub(super) fn of(plan: &Plan) -> Option<Fold> {
        let Plan::Aggregate { groups, collations, aggs, seeds, .. } = plan else { return None };
        let whole = groups.is_empty() && collations.is_empty() && seeds.is_empty();
        whole.then(|| Fold::of_aggs(aggs)).flatten()
    }

    /// The fold of the set functions `aggs` of a grouping by no column,
    /// if none is an inverse distribution function and no expression of
    /// them reads beyond its row.
    pub(crate) fn of_aggs(aggs: &[AggCall]) -> Option<Fold> {
        let own = aggs.iter().all(|a| a.distribution.is_none() && a.exprs().into_iter().all(crate::logical::Expr::reads_nothing));
        let whole = aggs.iter().all(|a| a.filter.as_ref().is_none_or(|c| !c.has_subquery()));
        (own && whole).then(|| Fold { aggs: aggs.to_vec() })
    }

    /// A fold begun, of no row yet.
    pub(crate) fn begun(&self, at: &At<'_>) -> Folding {
        let states = self.aggs.iter().map(|a| a.func.0.accumulator(a.result.clone())).collect();
        let seen = self.aggs.iter().any(|a| a.distinct).then(|| Bag::new(Work::of(at.st, at.tx)));
        Folding { states, seen, key: Vec::new() }
    }

    /// Feeds `row` to each function of `folding`.
    ///
    /// # Errors
    ///
    /// An argument's or a filter's, the function's, or the bag's spill's.
    pub(crate) fn feed(&self, folding: &mut Folding, row: &Row, at: &At<'_>) -> Result<()> {
        let Folding { states, seen, key } = folding;
        for (n, (a, state)) in self.aggs.iter().zip(states.iter_mut()).enumerate() {
            // [ISO/IEC 9075-2:2023 §10.9 GR4] a row its filter is not true
            // of is not aggregated, its argument not computed.
            if let Some(filter) = &a.filter
                && eval_cond(filter, row, at.subqueries)? != Truth::True
            {
                continue;
            }
            // [X3.135-1986 §5.8 GR4b] COUNT(*) counts rows.
            let Some(arg) = &a.arg else {
                state.update(&Value::Null)?;
                continue;
            };
            let value = eval(arg, row, at.subqueries)?;
            // [ISO/IEC 9075:1992 §6.5 GR1b] a null eliminated warns.
            if value.is_null() {
                warn(at.subqueries, SqlState::NULL_VALUE_ELIMINATED);
                continue;
            }
            if let (true, Some(seen)) = (a.distinct, seen.as_mut()) {
                // The function's place and the value: one key for each
                // value each function took.
                key.clear();
                key.extend_from_slice(&u32::try_from(n).unwrap_or(u32::MAX).to_le_bytes());
                put_norm(&value, key);
                if !seen.add_new(key)? {
                    continue;
                }
            }
            state.update(&value)?;
        }
        Ok(())
    }
}

impl Folding {
    /// The row of each function's value.
    ///
    /// # Errors
    ///
    /// A function's, as its result is out of range.
    pub(crate) fn finished(self) -> Result<Row> {
        self.states.into_iter().map(|state| state.finish()).collect()
    }
}
