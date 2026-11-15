//! [ISO/IEC 9075-2:2023 §6.6, §7.6, §11.49] The names a triggered action
//! reads that its trigger gives: a transition variable column reference,
//! `OLD.C` or `NEW.C`, and a transition table name in a `<table
//! reference>`.
//!
//! The trigger's scope encloses each statement of its action [§11.49
//! SR12]: a name of a table or correlation name of the statement is found
//! first, and a transition variable or table name only where none is.
//! Both come from the action's parameters (`params/transition.rs`), so a
//! view or constraint the action reads, bound where it was defined, sees
//! none of them.

use std::collections::BTreeSet;

use super::from::{Bounds, FromItem};
use super::views::Source;
use super::{Binder, Context, Scope, rule};
use crate::ast::{ColumnSpecification, NamedTable, SearchCondition, ValueExpression};
use crate::catalog::{QualifiedName, TableDesc};
use crate::error::Result;
use crate::logical::{Expr, LogicalPlan, Slot};
use crate::params::ParameterType;
use crate::value::DataType;

/// [ISO/IEC 9075-2:2023 §11.49, §15.21 GR5b] The `<triggered when
/// clause>`'s `<search condition>` `c` of a triggered action whose
/// parameters `cx` holds, bound as a boolean value expression of no table.
///
/// # Errors
///
/// It does not bind, or holds a set function outside a subquery.
pub fn bind_when(cx: Context<'_>, c: &SearchCondition) -> Result<Expr> {
    let binder = Binder::new(cx, None, (Vec::new(), Vec::new(), Vec::new()), BTreeSet::new());
    let condition = ValueExpression::Boolean(Box::new(c.clone()));
    let scope = Scope::Rows(
        "[X3.135-1986 §5.8 SR1] a set function needs an argument source (a <table expression>), which a <triggered when clause> does not have",
    );
    binder.value_expression(&condition, scope).map(|(e, _)| e)
}

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §6.6 SR9] `cs`, if it is a transition variable
    /// column reference: of the old or new row of the transition the action
    /// is invoked for, a slot of the action to be kept, else its value.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if its qualifier is a transition variable name and the
    /// subject table has no such column, or it is a generated column of the
    /// new row of a BEFORE trigger's.
    pub(super) fn transition_variable(&self, cs: &ColumnSpecification) -> Result<Option<(Expr, DataType)>> {
        let (Some(transitions), Some(q), None) = (self.params.transitions(), &cs.qualifier, &cs.catalog) else { return Ok(None) };
        if q.auth.is_some() || !transitions.names_variable(&q.ident) {
            return Ok(None);
        }
        if transitions.withheld(&q.ident, &cs.column) {
            let text = format!(
                "[ISO/IEC 9075-2:2023 §11.49 SR14c] a BEFORE trigger's action references generated column {} of {}",
                cs.column, q.ident
            );
            return Err(rule(text));
        }
        let Some((index, t)) = transitions.variable(&q.ident, &cs.column) else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.6 SR9] transition variable {} has no column {}", q.ident, cs.column)));
        };
        // [ISO/IEC 9075-2:2023 §11.49 GR5i] a column of the triggered
        // action column set.
        if let Some(uses) = self.uses {
            uses.borrow_mut().push((transitions.subject.clone(), cs.column.clone()));
        }
        let (value, t) = self.params.dynamic_typed(index, Some(ParameterType::Of(t)))?;
        let bound = if self.params.slots() { Expr::Dynamic(Slot { index, data_type: t.clone(), at: cs.at }) } else { Expr::Const(value) };
        Ok(Some((bound, t)))
    }
}

/// [ISO/IEC 9075-2:2023 §7.6 SR6] The table `t` names, if it is a
/// transition table name: its rows, those of the state change the action
/// is invoked for, the subject table's columns, its first at `offset`.
pub(super) fn transition_item(b: &Bounds<'_>, t: &NamedTable, offset: usize) -> Option<FromItem> {
    let transitions = b.cx.params.transitions()?;
    if t.table.auth.is_some() {
        return None;
    }
    let rows = transitions.table(&t.table.ident)?.clone();
    let names = transitions.columns.iter().map(|c| c.name.clone()).collect();
    let name = QualifiedName { auth: String::new(), ident: t.table.ident.clone() };
    let mut columns = transitions.columns.clone();
    // As a table of no constraints: what a column is drawn from is no
    // column of the subject table.
    for c in &mut columns {
        (c.default, c.generated, c.domain) = (None, None, None);
    }
    let desc = TableDesc {
        name: name.clone(),
        columns,
        constraints: Vec::new(),
        engine: String::new(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    };
    Some(FromItem {
        table: name,
        at: t.table.at,
        correlation: Some(t.correlation.clone().unwrap_or_else(|| t.table.ident.clone())),
        desc,
        offset,
        source: Source::Derived(LogicalPlan::Transition { rows, names }),
        nullable: false,
        common: Vec::new(),
        drawn: None,
    })
}
