//! [ISO/IEC 9075-2:2023 §10.9] A `<hypothetical set function>` bound: the
//! aggregate of the row of its hypothetical values and its sort keys, each
//! pair brought to the type they have in common and, of character
//! strings, made sort keys of the collation they have [SR10c], so the
//! accumulator (`functions/hypothetical.rs`) compares them as GR9's window
//! ordering of their UNION ALL does. DENSE_RANK is DISTINCT, so it is fed
//! each distinct row once.

use std::sync::Arc;

use super::case::{brought, set_type};
use super::composite::{anonymous, bounded, ordering_operand};
use super::set_functions::registered;
use super::window::double;
use super::{Binder, NOT_AGGREGATED, Scope, rule};
use crate::ast::{SetFunctionSpecification, ValueExpression, ValueOrder};
use crate::error::{Code, Result, SqlError};
use crate::functions::{AggregateRef, Hypothesis, Hypothetical, RowOf, ScalarRef};
use crate::logical::{AggCall, Expr, render_expr};
use crate::params::ParameterType;
use crate::value::types::Field;
use crate::value::{DataType, SortOrder};

/// What the parts of a hypothetical set function's row are bound to: the
/// expressions, and their types, the values' then the keys'.
type Parts = (Vec<Expr>, Vec<DataType>);

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §10.9 SR10, SR14, GR9] The hypothetical set
    /// function `sf` of `values`, in `scope`: a set function of the groups,
    /// whose values are not aggregated but of the groups, as a select
    /// list's \[SR14\], and whose sort keys are of its rows.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a Syntax Rule is broken; -900 if a sort key holds an
    /// outer reference, whose enclosing query it would aggregate.
    pub(super) fn hypothetical(
        &self,
        sf: &SetFunctionSpecification,
        values: &[ValueExpression],
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let Some(hypothesis) = Hypothesis::named(sf.name) else {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] {} is no hypothetical set function", sf.name)));
        };
        syntax_rules(sf, values)?;
        let g = match scope {
            Scope::Groups(g, _) => g,
            Scope::Rows(why) => return Err(rule(why)),
            Scope::Select => return Err(rule(NOT_AGGREGATED)),
        };
        for k in &sf.order_by {
            if self.is_outer_argument(&k.key)? {
                return Err(SqlError::new(
                    Code::Unsupported,
                    "[ISO/IEC 9075-2:2023 §6.9 SR6b] a hypothetical set function whose sort key holds an outer reference, of the enclosing query's set functions, is not supported",
                ));
            }
        }
        let rows = Scope::Rows("[ISO/IEC 9075-2:2023 §10.9 SR10a] a hypothetical set function cannot contain a set function");
        let mut parts: (Parts, Parts) = ((Vec::new(), Vec::new()), (Vec::new(), Vec::new()));
        for (v, k) in values.iter().zip(&sf.order_by) {
            self.hypothetical_pair(sf.name, (v, k), (scope, rows), &mut parts)?;
        }
        let ((mut exprs, mut types), (keys, key_types)) = parts;
        let label = self.hypothetical_label(sf, (&exprs, &keys));
        exprs.extend(keys);
        types.extend(key_types);
        let fields = types.into_iter().enumerate().map(|(i, t)| Field { name: anonymous(i), data_type: t, unnamed: true });
        let row = bounded(DataType::row(fields.collect()))?;
        let arg = Expr::Call(ScalarRef(Arc::new(RowOf(row))), exprs);
        // [SR10d i] RANK and DENSE_RANK of COUNT's type (IV174); [SR10d ii]
        // PERCENT_RANK and CUME_DIST DOUBLE PRECISION (IA043).
        let result = match hypothesis {
            Hypothesis::Rank | Hypothesis::DenseRank => self.count_type()?,
            Hypothesis::PercentRank | Hypothesis::CumeDist => double(),
        };
        let orders = sf.order_by.iter().map(|k| SortOrder::of(k.descending, k.nulls_first)).collect();
        let func = AggregateRef(Arc::new(Hypothetical { hypothesis, result: result.clone(), orders }));
        let filter = self.filter_of(sf.filter.as_deref(), rows)?;
        let label = self.filtered_label(label, filter.as_ref());
        let distinct = hypothesis == Hypothesis::DenseRank;
        Ok(registered(g, AggCall { func, arg: Some(arg), distinct, filter, result, label }, None))
    }

    /// The value `v` and sort key `k` of the hypothetical set function
    /// `name`, added to `parts`: the key bound over the rows; the value
    /// over the groups of `scope`, which it is of \[SR14\], and over the
    /// rows, where it is computed; each brought to the type they have in
    /// common and, of character strings, made a sort key of their
    /// collation \[SR10c\].
    fn hypothetical_pair(
        &self,
        name: &str,
        (v, k): (&ValueExpression, &ValueOrder),
        (scope, rows): (Scope<'_>, Scope<'_>),
        ((values, value_types), (keys, key_types)): &mut (Parts, Parts),
    ) -> Result<()> {
        let (key, key_type) = self.value_expression(&k.key, rows)?;
        ordering_operand(&key_type, name)?;
        // [EXTENSION] a `?` takes its sort key's type, where [ISO/IEC
        // 9075-2:2023 §20.7 GR5a] gives it none.
        let taken = || ParameterType::Of(key_type.clone());
        self.typed(v, taken(), scope)?;
        let (value, value_type) = self.typed(v, taken(), rows)?;
        let common = set_type(&[value_type.clone(), key_type.clone()], "§10.9 SR10c").map_err(|_| {
            rule(format!(
                "[ISO/IEC 9075-2:2023 §10.9 SR10c] a value of {name} WITHIN GROUP and its sort key are of compatible types, not {value_type} and {key_type}"
            ))
        })?;
        let collation = if common.is_character() { self.collation_of(&[self.derived(v)?, self.derived(&k.key)?])? } else { None };
        let placed = |e: Expr, from: &DataType| {
            let e = brought(e, from, common.clone());
            collation.as_ref().map_or_else(|| e.clone(), |c| Expr::sort_key(e.clone(), c))
        };
        values.push(placed(value, &value_type));
        keys.push(placed(key, &key_type));
        value_types.push(common.clone());
        key_types.push(common);
        Ok(())
    }

    /// The label of the hypothetical set function `sf` of the values
    /// `values` and sort keys `keys`, as they are compared: so two calls
    /// alike in all are computed once, and no others.
    fn hypothetical_label(&self, sf: &SetFunctionSpecification, (values, keys): (&[Expr], &[Expr])) -> String {
        let shown = |e: &Expr| render_expr(e, &self.names);
        let values: Vec<String> = values.iter().map(shown).collect();
        let keys: Vec<String> = keys
            .iter()
            .zip(&sf.order_by)
            .map(|(e, k)| {
                let order = SortOrder::of(k.descending, k.nulls_first);
                format!("{}{}{}", shown(e), if k.descending { " DESC" } else { "" }, order.null_suffix())
            })
            .collect();
        format!("{}({}) WITHIN GROUP (ORDER BY {})", sf.name, values.join(", "), keys.join(", "))
    }
}

/// [ISO/IEC 9075-2:2023 §10.9 SR10a, SR10b, §6.11 SR1] That the
/// hypothetical set function `sf` of `values` contains no window function,
/// set function or query expression, nor a nested window function, which
/// is only in an aggregated argument of a window function's set function;
/// and has as many values as sort keys.
fn syntax_rules(sf: &SetFunctionSpecification, values: &[ValueExpression]) -> Result<()> {
    let parts = || values.iter().chain(sf.order_by.iter().map(|k| &k.key));
    let nested = |e: &ValueExpression| matches!(e, ValueExpression::Window(_) | ValueExpression::SetFunction(_));
    let mut queried = false;
    parts().for_each(|p| p.for_each_subquery(&mut |_| queried = true));
    if queried || parts().any(|p| p.any(&nested)) {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §10.9 SR10a] {} WITHIN GROUP cannot contain a window function, a set function or a query expression",
            sf.name
        )));
    }
    if parts().any(|p| p.any(&|e| matches!(e, ValueExpression::NestedWindow(_)))) {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §6.11 SR1] a nested window function is only in an aggregated argument of a window function's set function, not in {} WITHIN GROUP",
            sf.name
        )));
    }
    if values.len() != sf.order_by.len() {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §10.9 SR10b] {} WITHIN GROUP has {} values and {} sort keys, which are as many",
            sf.name,
            values.len(),
            sf.order_by.len()
        )));
    }
    Ok(())
}
