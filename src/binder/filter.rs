//! [ISO/IEC 9075-2:2023 §10.9] The `<filter clause>` of an `<aggregate
//! function>` bound: its `<search condition>`, over the rows the set
//! function aggregates, so a row it is not true of is not fed to it
//! \[GR4\].

use super::set_functions::{column_references, is_aggregate_call};
use super::{Binder, Scope, rule};
use crate::ast::{SearchCondition, ValueExpression};
use crate::error::Result;
use crate::logical::{Cond, render_cond};

impl Binder<'_> {
    /// `filter`, bound in `scope`, the rows the set function aggregates.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it contains a set function [§6.9 SR5], a query
    /// expression, a window function or an outer reference \[SR8\], or a
    /// nested window function, which is only in an aggregated argument
    /// [§6.11 SR1].
    pub(super) fn filter_of(&self, filter: Option<&SearchCondition>, scope: Scope<'_>) -> Result<Option<Cond>> {
        let Some(c) = filter else { return Ok(None) };
        let values = values_of(c);
        let aggregated = |e: &ValueExpression| is_aggregate_call(e, (self.db, self.schema));
        if values.iter().any(|v| v.any(&aggregated)) {
            return Err(rule("[ISO/IEC 9075-2:2023 §6.9 SR5] a <filter clause> cannot contain a set function"));
        }
        let mut queried = false;
        c.for_each_subquery(&mut |_| queried = true);
        if queried {
            return Err(rule("[ISO/IEC 9075-2:2023 §10.9 SR8] a <filter clause> cannot contain a query expression"));
        }
        if values.iter().any(|v| v.any(&|e| matches!(e, ValueExpression::Window(_)))) {
            return Err(rule("[ISO/IEC 9075-2:2023 §10.9 SR8] a <filter clause> cannot contain a window function"));
        }
        if values.iter().any(|v| v.any(&|e| matches!(e, ValueExpression::NestedWindow(_)))) {
            return Err(rule(
                "[ISO/IEC 9075-2:2023 §6.11 SR1] a nested window function is only in an aggregated argument, not in a <filter clause>",
            ));
        }
        let bound = self.search_condition(c, scope)?;
        if self.filter_columns(c)?.1 {
            return Err(rule("[ISO/IEC 9075-2:2023 §10.9 SR8] a <filter clause> cannot contain an outer reference"));
        }
        Ok(Some(bound))
    }

    /// [ISO/IEC 9075-2:2023 §6.9 SR6, §10.9 SR8] `filter`, of a set
    /// function of an enclosing query, is over that query's rows, so a
    /// column of it would be of no row it filters or an outer reference.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it has a column reference.
    pub(super) fn outer_filter(&self, filter: Option<&SearchCondition>) -> Result<()> {
        let Some(c) = filter else { return Ok(()) };
        if self.filter_columns(c)?.0 {
            return Err(rule(
                "[ISO/IEC 9075-2:2023 §6.9 SR6, §10.9 SR8] a <filter clause> of a set function of an enclosing query cannot contain a column reference, an outer reference here",
            ));
        }
        Ok(())
    }

    /// `label`, of a set function, followed by its filter `filter` if
    /// written.
    pub(super) fn filtered_label(&self, label: String, filter: Option<&Cond>) -> String {
        match filter {
            Some(c) => format!("{label} FILTER (WHERE {})", render_cond(c, &self.names)),
            None => label,
        }
    }

    /// Of `filter`, whether it has a column reference, and whether one is
    /// an outer reference.
    pub(super) fn filter_columns(&self, filter: &SearchCondition) -> Result<(bool, bool)> {
        let values = values_of(filter);
        let mut columns = Vec::new();
        for v in values {
            column_references(v, &mut columns);
        }
        let mut outer = false;
        for c in &columns {
            outer = outer || !self.is_local(c)?;
        }
        Ok((!columns.is_empty(), outer))
    }
}

/// The value expressions `c` compares, not those of its subqueries.
fn values_of(c: &SearchCondition) -> Vec<&ValueExpression> {
    let mut values = Vec::new();
    c.for_each_value(&mut |v| values.push(v));
    values
}
