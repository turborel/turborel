//! GREATEST and LEAST bound [ISO/IEC 9075-2:2023 §6.15]: their arguments
//! each cast to the type of all, ordered under the collation of all.

use std::sync::Arc;

use super::case::{brought, set_type};
use super::{Binder, Scope, rule};
use crate::ast::ValueExpression;
use crate::error::Result;
use crate::functions::{GREATEST, GreatestOrLeast, LEAST, ScalarRef};
use crate::logical::Expr;
use crate::value::DataType;

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §6.15] GREATEST, if `greatest`, else LEAST, of
    /// `args`, two at least: of the type of all [SR6], each argument cast
    /// to it [SR7], ordered under the collation of all [§9.14 SR1h, §9.15].
    /// \[EXTENSION\] A `?` takes the first argument's type, as one of
    /// COALESCE does [ISO/IEC 9075:1992 §17.6 GR5j]; the first none.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if two arguments are not comparable [SR4], or their
    /// collations conflict [§9.15 SR4].
    pub(super) fn greatest_or_least(&self, greatest: bool, args: &[ValueExpression], scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let name = if greatest { GREATEST } else { LEAST };
        let first = args.first().map(|v| self.value_expression(v, scope)).transpose()?.map(|(_, t)| t);
        let bound = args.iter().map(|v| self.typed_by(v, first.clone(), scope)).collect::<Result<Vec<_>>>()?;
        let types: Vec<DataType> = bound.iter().map(|(_, t)| t.clone()).collect();
        let result = set_type(&types, "§9.3").map_err(|_| {
            let written: Vec<String> = types.iter().map(ToString::to_string).collect();
            rule(format!(
                "[ISO/IEC 9075-2:2023 §6.15 SR4] the arguments of {name} are each comparable with each, not {}",
                written.join(", ")
            ))
        })?;
        // [SR5] each an operand of an ordering operation [§9.14].
        types.iter().try_for_each(|t| super::composite::ordering_operand(t, name))?;
        let collation = if result.is_character() {
            let derived = args.iter().map(|v| self.derived(v)).collect::<Result<Vec<_>>>()?;
            self.collation_of(&derived)?
        } else {
            None
        };
        let exprs = bound.into_iter().map(|(e, t)| brought(e, &t, result.clone())).collect();
        let function = GreatestOrLeast { greatest, result: result.clone(), collation };
        Ok((Expr::Call(ScalarRef(Arc::new(function)), exprs), result))
    }
}
