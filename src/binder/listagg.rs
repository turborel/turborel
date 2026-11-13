//! [ISO/IEC 9075-2:2023 §10.9] A `<listagg set function>` bound: the
//! aggregate of the row of its value and sort keys, null where the value
//! is, so the null values are removed and warned of as any set function's
//! are \[GR11b\], and the rows ordered by the keys in the accumulator
//! (`functions/listagg.rs`).

use std::sync::Arc;

use super::{Binder, Scope, rule};
use crate::ast::{CaseExpression, Composite, Overflow, Predicate, SearchCondition, SetArg, SetFunctionSpecification, ValueExpression};
use crate::error::{Code, Result, SqlError};
use crate::functions::{AggregateRef, Listagg, ListaggSpec, MAX_LISTAGG_LENGTH, Truncation};
use crate::logical::Expr;
use crate::value::DataType;

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §10.9 SR11] LISTAGG ( \[DISTINCT\] V, LAS ...
    /// ) WITHIN GROUP ( ORDER BY K1, ... ): the aggregate of CASE WHEN V IS
    /// NULL THEN NULL ELSE ROW ( V, K1, ... ) END, each key ordered by its
    /// collation [§9.15] and V compared by its own under DISTINCT \[GR11c\].
    /// DISTINCT is the accumulator's, as the rows' keys are no part of it.
    pub(super) fn listagg(&self, sf: &SetFunctionSpecification, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let (SetArg::All(value) | SetArg::Distinct(value)) = &sf.arg else { return Err(not_read()) };
        let Some(written) = &sf.listagg else { return Err(not_read()) };
        let keys = sf.order_by.iter().map(|k| &k.key);
        // [SR11a] no <set function specification> or <query expression>,
        // of the value or a key; turborel has no <window function>.
        let nested = |e: &ValueExpression| matches!(e, ValueExpression::SetFunction(_) | ValueExpression::Subquery(_));
        if std::iter::once(&**value).chain(keys.clone()).any(|part| part.any(&nested)) {
            return Err(rule("[ISO/IEC 9075-2:2023 §10.9 SR11a] LISTAGG cannot contain a set function or a query expression"));
        }
        let truncation = match &written.overflow {
            Overflow::Error => None,
            Overflow::Truncate { filler, count } => {
                Some(Truncation { filler: within_most(filler, "SR11g", "<listagg truncation filler>")?, count: *count })
            }
        };
        let separator = within_most(&written.separator, "SR11e", "<listagg separator>")?;
        let distinct = matches!(sf.arg, SetArg::Distinct(_));
        let collation = if distinct { self.collation_of(&[self.derived(value)?])? } else { None };
        // A key not a character string is its own sort key.
        let collations = keys.clone().map(|k| self.collation_of(&[self.derived(k)?])).collect::<Result<Vec<_>>>()?;
        let descending = sf.order_by.iter().map(|k| k.descending).collect();
        let spec = ListaggSpec { separator, truncation, distinct, collation, descending, collations };
        let row = ValueExpression::Composite(Box::new(Composite::Row(std::iter::once(&**value).chain(keys).cloned().collect())), sf.at);
        let null = Predicate::Null { value: vec![(**value).clone()], not: false };
        let whens = vec![(SearchCondition::Predicate(Box::new(null), sf.at), None)];
        let arg = ValueExpression::Case(Box::new(CaseExpression::Searched { whens, otherwise: Some(row) }));
        self.aggregate(AggregateRef(Arc::new(Listagg(Arc::new(spec)))), Some(&arg), false, scope)
    }
}

/// `literal`, the `what` of a LISTAGG, if no longer than MLRT, as the
/// Syntax Rule `clause` says it shall be; else SQLCODE -103.
fn within_most(literal: &str, clause: &str, what: &str) -> Result<String> {
    let length = literal.chars().count();
    if u32::try_from(length).map_or(true, |n| n > MAX_LISTAGG_LENGTH) {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §10.9 {clause}] a {what} of {length} characters is longer than LISTAGG's result, of {MAX_LISTAGG_LENGTH}"
        )));
    }
    Ok(literal.to_owned())
}

fn not_read() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] LISTAGG was read without its argument, separator or overflow behavior")
}
