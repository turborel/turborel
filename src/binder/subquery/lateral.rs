//! [ISO/IEC 9075-2:2023 §7.6] A `<lateral derived table>`, bound (RULES.md
//! D778).
//!
//! Its table is its query's [SR10], and its query may reference every
//! table reference before it in its FROM clause [SR5, SR20a]. One that
//! references none is bound as a derived table is. One that does is bound
//! as a collection derived table of one collection is, which is lateral
//! already (`binder/unnest.rs`): the collection is the multiset of its
//! query's rows, each the ROW of its columns, a subquery whose outer
//! references are the columns it reads, so it is computed again for
//! each row before it, as the rows of a lateral query are.

use std::cell::RefCell;
use std::sync::Arc;

use super::super::composite::{anonymous, bounded};
use super::super::from::{Bounds, From, FromItem, column_of, derived, renamed};
use super::super::order_by::ordered_query;
use super::super::query::Nesting;
use super::super::unnest::{Unnested, lateral_binder, lateral_item};
use super::super::{Binder, Context, Scope};
use super::{Outer, aggregated};
use crate::ast::{DerivedTable, Subquery};
use crate::catalog::ColumnDesc;
use crate::error::{At, Result};
use crate::functions::{AggregateRef, Gathering, MultisetAggregate, RowOf, ScalarRef};
use crate::logical::{Expr, render_expr};
use crate::value::DataType;
use crate::value::types::Field;

/// [ISO/IEC 9075-2:2023 §6.9 SR7] Why a set function of a lateral
/// derived table's query over an outer reference to the FROM row is
/// refused: there are no groups there to aggregate.
const NO_SET_FUNCTION: &str = "[ISO/IEC 9075-2:2023 §6.9 SR7] a set function in a <lateral derived table> is over an outer reference";

/// [ISO/IEC 9075-2:2023 §7.6 SR5, SR10, SR20a] The lateral derived table
/// `d`, its first column at `offset`: its query bound over every item of
/// `from` before it, its columns named as its query names them, else by
/// its derived column list [ISO/IEC 9075:1992 §6.3 SR7].
pub(in crate::binder) fn lateral_derived(b: Bounds<'_>, d: &DerivedTable, offset: usize, from: &mut From) -> Result<FromItem> {
    let binder = lateral_binder(b, from);
    let (rows, mut columns) = binder.lateral_rows(&d.query)?;
    let reads = binder.used.into_inner();
    if reads.is_empty() {
        return derived(b, d, offset).or_at(d.query.at);
    }
    if let Some(names) = &d.columns {
        renamed(&mut columns, names)?;
    }
    from.used.extend(reads.iter().copied());
    let width = columns.len();
    Ok(lateral_item(&d.correlation, d.query.at, columns, offset, Unnested::rows_of(rows, width, reads)))
}

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §6.52 SR1e, GR3] The multiset of the rows of the
    /// query of `s`, each the ROW of its columns, as a `<table value
    /// constructor by query>` is, but of an ORDER BY too, which orders no
    /// multiset and so only says which rows OFFSET and FETCH keep; and the
    /// columns of its rows.
    fn lateral_rows(&self, s: &Subquery) -> Result<(Expr, Vec<ColumnDesc>)> {
        let cx = self.context();
        let outer = Outer { binder: self, scope: Scope::Rows(NO_SET_FUNCTION), captured: RefCell::new(Vec::new()) };
        let nesting = Nesting::Subquery { outer: &outer, result: None };
        let ordered = ordered_query(Context { draws: false, ..cx }, &s.query, (&[], &nesting), false).or_at(s.at)?.limited();
        let own = ordered.all.get(..ordered.visible).unwrap_or_default();
        let columns: Vec<ColumnDesc> = own.iter().enumerate().map(|(k, c)| column_of(k, c)).collect();
        let fields = own.iter().enumerate().map(|(i, c)| Field { name: anonymous(i), data_type: c.data_type.clone(), unnamed: true });
        let row = DataType::row(fields.collect());
        let value = Expr::Call(ScalarRef(Arc::new(RowOf(row.clone()))), (0..own.len()).map(Expr::Column).collect());
        let label = format!("LATERAL({})", render_expr(&value, &[]));
        let element = Field { name: anonymous(0), data_type: row.clone(), unnamed: true };
        let arg = Expr::Call(ScalarRef(Arc::new(RowOf(DataType::row(vec![element])))), vec![value]);
        let result = bounded(DataType::multiset(row))?;
        let func = AggregateRef(Arc::new(MultisetAggregate { kind: Gathering::Collect, by_query: true }));
        Ok((aggregated((func, arg, label), result, ordered.plan, outer).0, columns))
    }
}
