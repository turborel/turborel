//! [ISO/IEC 9075-2:2023 §7.17] Abstract syntax of a query expression's
//! `<order by clause>`, `<result offset clause>` and `<fetch first
//! clause>`, and of what a query expression is without them.

use super::ValueExpression;
use super::query::{QueryExpression, QuerySpecification, SortSpecification};
use crate::logical::{Hints, Search};

/// [ISO/IEC 9075-2:2023 §7.17] `<query expression body>` \[`<order by
/// clause>`\] \[`<result offset clause>`\] \[`<fetch first clause>`\], one
/// of them at least written.
#[derive(Debug, Clone)]
pub struct OrderedQuery {
    /// The `<query expression body>`.
    pub body: QueryExpression,
    /// The `<sort specification>`s of its `<order by clause>`, none if it
    /// has none.
    pub order_by: Vec<SortSpecification>,
    /// The `<offset row count>` of its `<result offset clause>`, if written.
    pub offset: Option<RowCount>,
    /// Its `<fetch first clause>`, if written.
    pub fetch: Option<FetchFirst>,
}

/// [ISO/IEC 9075-2:2023 §7.17] An `<offset row count>` or a `<fetch first
/// row count>`: a `<simple value specification>`, or \[EXTENSION\] a
/// `<dynamic parameter specification>`.
#[derive(Debug, Clone)]
pub struct RowCount {
    /// Its value.
    pub value: ValueExpression,
    /// Where it begins in the statement.
    pub at: usize,
}

/// [ISO/IEC 9075-2:2023 §7.17] `<fetch first clause>` ::= FETCH {FIRST |
/// NEXT} \[`<fetch first quantity>`\] {ROW | ROWS} {ONLY | WITH TIES};
/// \[EXTENSION\] with APPROX or EXACT after FETCH, a target accuracy and
/// hints after it (`parser/query/ordered.rs`).
#[derive(Debug, Clone)]
pub struct FetchFirst {
    /// Its `<fetch first quantity>`, if written: 1 if not \[GR7b iii\].
    pub quantity: Option<FetchQuantity>,
    /// WITH TIES was written.
    pub ties: bool,
    /// Where FETCH is written.
    pub at: usize,
    /// \[EXTENSION\] APPROX, EXACT or neither, and its target accuracy.
    pub search: Search,
    /// \[EXTENSION\] Whether a target accuracy is written, and where, as
    /// of none but APPROX it is refused.
    pub accuracy_at: Option<usize>,
    /// \[EXTENSION\] The vector search hints written (R13.4).
    pub hints: Hints,
}

/// [ISO/IEC 9075-2:2023 §7.17] `<fetch first quantity>` ::= `<fetch first
/// row count>` | `<fetch first percentage>`
#[derive(Debug, Clone)]
pub enum FetchQuantity {
    /// A `<fetch first row count>`.
    Rows(RowCount),
    /// A `<fetch first percentage>`: its `<simple value specification>`,
    /// then PERCENT.
    Percent(RowCount),
}

impl QueryExpression {
    /// The `<sort specification>`s of its own `<order by clause>`, after
    /// its `<with clause>`s if it has any [ISO/IEC 9075-2:2023 §7.17].
    pub fn order_by(&self) -> &[SortSpecification] {
        self.ordered().map_or(&[], |o| &o.order_by)
    }

    /// The `<query specification>` it is, alone or with an `<order by
    /// clause>` but no `<result offset clause>` or `<fetch first clause>`,
    /// which leave it not updatable [ISO/IEC 9075-2:2023 §7.17 SR7, SR8,
    /// SR11]: whose updatability decides its own.
    pub fn updatable_spec(&self) -> Option<&QuerySpecification> {
        self.updatable_body().and_then(QueryExpression::spec)
    }

    /// The `<query expression body>` of `updatable_spec`, itself or that
    /// before its ORDER BY.
    pub fn updatable_body(&self) -> Option<&QueryExpression> {
        match self {
            QueryExpression::Spec(_) => Some(self),
            QueryExpression::Ordered(o) => (o.offset.is_none() && o.fetch.is_none() && o.body.spec().is_some()).then_some(&o.body),
            QueryExpression::Values(_) | QueryExpression::SetOp(_) | QueryExpression::With(_) => None,
        }
    }

    /// Itself, or of its own ORDER BY, OFFSET or FETCH FIRST, its `<query
    /// expression body>` before them.
    pub fn unordered(&self) -> &QueryExpression {
        if let QueryExpression::Ordered(o) = self { &o.body } else { self }
    }

    /// Its ORDER BY, OFFSET and FETCH FIRST, after its `<with clause>`s,
    /// if it has any.
    pub fn ordered(&self) -> Option<&OrderedQuery> {
        match self {
            QueryExpression::With(w) => w.body.ordered(),
            QueryExpression::Ordered(o) => Some(o),
            QueryExpression::Spec(_) | QueryExpression::Values(_) | QueryExpression::SetOp(_) => None,
        }
    }
}
