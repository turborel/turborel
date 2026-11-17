//! [ISO/IEC 9075-2:2023 §10.9] ARRAY_AGG, and the binding it shares with
//! XMLAGG [ISO/IEC 9075-14:2006 draft §11.2]: an aggregate of the row of
//! a value and its sort keys.

use std::sync::Arc;

use super::{Binder, Scope};
use crate::ast::{Composite, SetArg, SetFunctionSpecification, ValueExpression};
use crate::error::{Code, Result, SqlError};
use crate::functions::{AggregateRef, ArrayAggregate};
use crate::logical::Expr;
use crate::value::collation::CollationRef;
use crate::value::{DataType, SortOrder};

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §10.9] ARRAY_AGG ( V ORDER BY K1, ... ): the
    /// aggregate of the row ( V, K1, ... ), so a null V is kept \[NOTE
    /// 528\], ordered by its keys, each as its collation orders it [§9.15]
    /// and an operand of an ordering operation [§9.14 SR2f].
    pub(super) fn array_aggregate(&self, sf: &SetFunctionSpecification, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        self.ordered_aggregate(sf, scope, |orders, collations| {
            AggregateRef(Arc::new(ArrayAggregate { orders, collations, by_query: false }))
        })
    }

    /// An aggregate of the row ( V, K1, ... ) of `sf`'s value and sort
    /// keys, as ARRAY_AGG and XMLAGG are: so a null V is kept, ordered by
    /// its keys, each as its collation orders it [ISO/IEC 9075-2:2023
    /// §9.15] and an operand of an ordering operation [§9.14 SR2f]; the
    /// function `make` gives of the keys' orders and collations.
    pub(super) fn ordered_aggregate(
        &self,
        sf: &SetFunctionSpecification,
        scope: Scope<'_>,
        make: impl FnOnce(Vec<SortOrder>, Vec<Option<CollationRef>>) -> AggregateRef,
    ) -> Result<(Expr, DataType)> {
        let (SetArg::All(value) | SetArg::Distinct(value)) = &sf.arg else {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] {} is read with an argument", sf.name)));
        };
        let mut parts = vec![(**value).clone()];
        let mut collations = Vec::new();
        for key in &sf.order_by {
            // A key not a character string is its own sort key.
            collations.push(self.collation_of(&[self.derived(&key.key)?])?);
            parts.push(key.key.clone());
        }
        let row = ValueExpression::Composite(Box::new(Composite::Row(parts)), sf.at);
        let orders = sf.order_by.iter().map(|k| SortOrder::of(k.descending, k.nulls_first)).collect();
        let func = make(orders, collations);
        self.sorted(|| self.aggregate(func, (Some(&row), false), sf.filter.as_deref(), scope))
    }
}
