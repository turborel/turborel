//! [ISO/IEC 9075-2:2023 §7.17] A query expression's `<order by clause>`,
//! `<result offset clause>` and `<fetch first clause>`, bound: its body
//! sorted by its keys (`binder/order_by.rs`), then the rows OFFSET passes
//! over and FETCH FIRST keeps of them. Each count is a `<simple value
//! specification>`, or \[EXTENSION\] a `<dynamic parameter
//! specification>` of type INTEGER, as NTILE's count is [§20.7 GR5a
//! xxxii]; so it is of no column, and is evaluated once.

use std::collections::BTreeSet;

use super::composite::integer;
use super::order_by::{Ordered, of_result, ordered_query};
use super::query::Nesting;
use super::{Binder, Bound, Context, Scope, rule};
use crate::ast::{FetchQuantity, OrderedQuery, RowCount};
use crate::error::{At, Code, Result, SqlError};
use crate::logical::{Expr, Limit};
use crate::params::ParameterType;
use crate::value::{DataType, Decimal, Value};

/// [§7.17 GR7b iii] The count of a `<fetch first clause>` of none written.
const ONE_ROW: i128 = 1;

/// The query `o`, bound as `nesting` says, its select list drawing values
/// if `draws`: its body and the keys that sort it, and the rows its OFFSET
/// and FETCH FIRST keep.
///
/// # Errors
///
/// Any Syntax Rule its body, a key or a count breaks, or a missing
/// privilege; SQLCODE -900 for PERCENT or WITH TIES.
pub(super) fn ordered_and_limited(cx: Context<'_>, o: &OrderedQuery, nesting: &Nesting<'_>, draws: bool) -> Result<Ordered> {
    let mut ordered = ordered_query(cx, &o.body, (&o.order_by, nesting), draws)?;
    ordered.limit = limit_of(cx, o)?;
    Ok(ordered)
}

/// [ISO/IEC 9075-2:2023 §7.17 SR26 d ii] `bound`, the rows of the body of
/// `o` bound already, sorted by its keys, each of the columns of its rows
/// alone, and limited by its OFFSET and FETCH FIRST.
///
/// # Errors
///
/// Any Syntax Rule a key or a count breaks; SQLCODE -900 for PERCENT or
/// WITH TIES.
pub(super) fn ordered_result(cx: Context<'_>, bound: Bound, o: &OrderedQuery) -> Result<Bound> {
    let mut ordered = of_result(cx, bound, &o.order_by)?;
    ordered.limit = limit_of(cx, o)?;
    Ok(ordered.sorted())
}

/// The OFFSET and FETCH FIRST of `o`, bound as `cx`, if it has either.
fn limit_of(cx: Context<'_>, o: &OrderedQuery) -> Result<Option<Limit>> {
    if o.offset.is_none() && o.fetch.is_none() {
        return Ok(None);
    }
    let binder = Binder::new(Context { draws: false, ..cx }, None, (Vec::new(), Vec::new(), Vec::new()), BTreeSet::new());
    let offset = o.offset.as_ref().map(|c| row_count(&binder, c, "SR27] the offset row count")).transpose()?;
    let Some(f) = &o.fetch else { return Ok(Some(Limit { offset, fetch: None })) };
    if f.ties {
        return Err(unsupported("WITH TIES is Feature F867", "§7.17 GR7d i").or_at(f.at));
    }
    let fetch = match &f.quantity {
        None => Expr::Const(Value::Exact(Decimal::new(ONE_ROW, 0))),
        Some(FetchQuantity::Rows(c)) => row_count(&binder, c, "SR28] the fetch first row count")?,
        Some(FetchQuantity::Percent(c)) => return Err(unsupported("PERCENT is Feature F866", "§7.17 GR7b ii").or_at(c.at)),
    };
    Ok(Some(Limit { offset, fetch: Some(fetch) }))
}

/// The count `c`, bound by `binder`, of no column: of a type exact numeric
/// of scale 0, else the rule `broken` of §7.17 is.
fn row_count(binder: &Binder<'_>, c: &RowCount, broken: &str) -> Result<Expr> {
    let none = Scope::Rows("[ISO/IEC 9075-2:2023 §7.17] a row count is a <simple value specification>, of no set function");
    let (e, t) = binder.typed(&c.value, ParameterType::Of(integer()), none).or_at(c.at)?;
    if !matches!(t, DataType::Exact { scale: 0, .. }) {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §7.17 {broken} is an exact number of scale 0, not {t}")).or_at(c.at));
    }
    Ok(e)
}

/// SQLCODE -900: `what`, which turborel does not support, of the rule
/// `cited` of ISO/IEC 9075-2:2023.
fn unsupported(what: &str, cited: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[ISO/IEC 9075-2:2023 {cited}] {what}, which turborel does not support"))
}
