//! [ISO/IEC 9075-2:2023 §10.9] An `<inverse distribution function>`
//! bound: the aggregate of its sort key, its values the key's not null,
//! sorted by its grouping (`exec/group/distribution.rs`) or window
//! (`exec/window/distribution.rs`), the fraction NVE kept with it.

use std::sync::Arc;

use super::set_functions::registered;
use super::window::double;
use super::{Binder, NOT_AGGREGATED, Scope, rule};
use crate::ast::{SetFunctionSpecification, ValueExpression};
use crate::error::{Code, Result, SqlError};
use crate::functions::{AggregateRef, FRACTION, InverseDistribution, PERCENTILE_CONT};
use crate::logical::{AggCall, Distribution, Expr, render_expr};
use crate::params::ParameterType;
use crate::value::{DataType, MAX_DECFLOAT_PRECISION, SortOrder};

/// The rule a set function in an inverse distribution function breaks.
const CONTAINED: &str = "[ISO/IEC 9075-2:2023 §10.9 SR12b] an inverse distribution function cannot contain a set function";

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §10.9 SR12, SR14, GR10] The inverse
    /// distribution function `sf` of the fraction `fraction`, in `scope`:
    /// of the groups, its sort key of their rows and its fraction not
    /// aggregated but of the groups, as a select list's \[SR14\]; or, of
    /// a window function, both of the window's rows. Its sort key's columns
    /// all outer references, it is the enclosing query's [§6.9 SR6b], and
    /// is bound there.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a Syntax Rule is broken.
    pub(super) fn inverse_distribution(
        &self,
        sf: &SetFunctionSpecification,
        fraction: &ValueExpression,
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        syntax_rules(sf, fraction)?;
        if self.windowed_next() {
            return self.window_taken(|| self.distribution_call(sf, fraction, (scope, scope)));
        }
        if let Some(outer) = self.outer_ordered(sf)? {
            return Ok(outer);
        }
        let g = match scope {
            Scope::Groups(g, _) => g,
            Scope::Rows(why) => return Err(rule(why)),
            Scope::Select => return Err(rule(NOT_AGGREGATED)),
        };
        // [SR14] the fraction, a non-aggregated argument, of the groups.
        self.fraction(sf.name, fraction, scope)?;
        let call = self.unnested(|| self.distribution_call(sf, fraction, (Scope::Rows(CONTAINED), Scope::Rows(CONTAINED))))?;
        Ok(registered(g, call, None))
    }

    /// The call of the inverse distribution function `sf`, its sort key
    /// bound in `keyed` and its fraction `fraction` in `fractioned`.
    fn distribution_call(
        &self,
        sf: &SetFunctionSpecification,
        fraction: &ValueExpression,
        (keyed, fractioned): (Scope<'_>, Scope<'_>),
    ) -> Result<AggCall> {
        let Some(k) = sf.order_by.first() else {
            return Err(SqlError::new(Code::Api, "[DESIGN] an inverse distribution function is read with a sort key"));
        };
        let (key, key_type) = self.value_expression(&k.key, keyed)?;
        let result = result_type(sf.name, &key_type)?;
        let fraction = self.fraction(sf.name, fraction, fractioned)?;
        let order = SortOrder::of(k.descending, k.nulls_first);
        let shown = |e: &Expr| render_expr(e, &self.names);
        let label = format!(
            "{}({}) WITHIN GROUP (ORDER BY {}{}{})",
            sf.name,
            shown(&fraction),
            shown(&key),
            if k.descending { " DESC" } else { "" },
            order.null_suffix()
        );
        let filter = self.filter_of(sf.filter.as_deref(), keyed)?;
        let label = self.filtered_label(label, filter.as_ref());
        let continuous = sf.name == PERCENTILE_CONT;
        let func = AggregateRef(Arc::new(InverseDistribution { continuous, result: result.clone() }));
        let distribution = Some(Box::new(Distribution { continuous, order, fraction }));
        Ok(AggCall { func, arg: Some(key), distinct: false, filter, distribution, result, label })
    }

    /// The fraction `fraction` of the inverse distribution function `name`
    /// bound in `scope`: a `<numeric value expression>`.
    fn fraction(&self, name: &str, fraction: &ValueExpression, scope: Scope<'_>) -> Result<Expr> {
        // [EXTENSION] a `?` is NUMERIC(18,17), of every fraction from 0 to
        // 1 that type holds, where [ISO/IEC 9075-2:2023 §20.7 GR5a] gives
        // it none.
        let (e, t) = self.typed(fraction, ParameterType::Of(FRACTION), scope)?;
        if !t.is_numeric() {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §10.9] the argument of {name}, an <inverse distribution function argument>, is a <numeric value expression>, not {t}"
            )));
        }
        Ok(e)
    }
}

/// [ISO/IEC 9075-2:2023 §10.9 SR12d, SR12e] The type of the result of the
/// inverse distribution function `name` of a sort key of type `key`:
/// DOUBLE PRECISION of an exact or approximate key and DECFLOAT(34) of a
/// DECFLOAT key, each IV177, and of an interval its type.
fn result_type(name: &str, key: &DataType) -> Result<DataType> {
    if let DataType::DecFloat { .. } = key {
        return Ok(DataType::DecFloat { precision: MAX_DECFLOAT_PRECISION });
    }
    if key.is_numeric() {
        return Ok(double());
    }
    if key.is_interval() {
        return Ok(key.clone());
    }
    Err(rule(format!("[ISO/IEC 9075-2:2023 §10.9 SR12d] the sort key of {name} is numeric or an interval, not {key}")))
}

/// [ISO/IEC 9075-2:2023 §10.9 SR12a, SR12b, §6.11 SR1] That the inverse
/// distribution function `sf` of `fraction` has a single sort key and
/// contains no window function, set function or query expression, nor a
/// nested window function, which is only in an aggregated argument of a
/// window function's set function.
fn syntax_rules(sf: &SetFunctionSpecification, fraction: &ValueExpression) -> Result<()> {
    if sf.order_by.len() != 1 {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §10.9 SR12a] the WITHIN GROUP of {} has a single sort key, not {}",
            sf.name,
            sf.order_by.len()
        )));
    }
    let parts = || std::iter::once(fraction).chain(sf.order_by.iter().map(|k| &k.key));
    let nested = |e: &ValueExpression| matches!(e, ValueExpression::Window(_) | ValueExpression::SetFunction(_));
    let mut queried = false;
    parts().for_each(|p| p.for_each_subquery(&mut |_| queried = true));
    if queried || parts().any(|p| p.any(&nested)) {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §10.9 SR12b] {} cannot contain a window function, a set function or a query expression",
            sf.name
        )));
    }
    if parts().any(|p| p.any(&|e| matches!(e, ValueExpression::NestedWindow(_)))) {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §6.11 SR1] a nested window function is only in an aggregated argument of a window function's set function, not in {}",
            sf.name
        )));
    }
    Ok(())
}
