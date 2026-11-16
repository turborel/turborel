//! [ISO/IEC 9075-2:2023 §6.6 SR8, SR9a] An `<SQL parameter reference>` in
//! the body of an SQL-invoked procedure: `P`, or `R.P` of the procedure
//! `R`, a parameter of it.
//!
//! The procedure's scope encloses its body's statement, so a column of a
//! table it reads is found first, and a parameter only where no column is
//! of the name. Parameters come from the body's parameters
//! (`params/routine.rs`), so a view or constraint the body reads, bound
//! where it was defined, sees none of them. A function's parameters are
//! instead the outer row of its body (`binder/invocation.rs`).

use std::collections::BTreeSet;

use super::{Binder, Context, Scope};
use crate::ast::{ColumnSpecification, ValueExpression};
use crate::error::Result;
use crate::logical::{Expr, Slot};
use crate::params::ParameterType;
use crate::value::DataType;

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §6.6 SR8] `cs`, if it is an SQL parameter
    /// reference of the procedure whose body is bound: a slot of the body
    /// to be kept, else its value.
    ///
    /// # Errors
    ///
    /// As a dynamic value of the type of its parameter is bound.
    pub(super) fn sql_parameter(&self, cs: &ColumnSpecification) -> Result<Option<(Expr, DataType)>> {
        let Some(scope) = self.params.routine_scope() else { return Ok(None) };
        if cs.catalog.is_some() || cs.qualifier.as_ref().is_some_and(|q| q.auth.is_some()) {
            return Ok(None);
        }
        let Some((index, t)) = scope.parameter(cs.qualifier.as_ref().map(|q| q.ident.as_str()), &cs.column) else { return Ok(None) };
        let (value, t) = self.params.dynamic_typed(index, Some(ParameterType::Of(t)))?;
        let bound = if self.params.slots() { Expr::Dynamic(Slot { index, data_type: t.clone(), at: cs.at }) } else { Expr::Const(value) };
        Ok(Some((bound, t)))
    }
}

/// [ISO/IEC 9075-2:2023 §10.4, §16.1] An `<SQL argument>` of a CALL that
/// is a `<value expression>`, of no table: bound, and its type.
///
/// # Errors
///
/// It does not bind.
pub fn bind_argument(cx: Context<'_>, v: &ValueExpression) -> Result<(Expr, DataType)> {
    let binder = Binder::new(cx, None, (Vec::new(), Vec::new(), Vec::new()), BTreeSet::new());
    binder.value_expression(v, Scope::Rows("[ISO/IEC 9075-2:2023 §10.9 SR2] an SQL argument of CALL has no set function"))
}
