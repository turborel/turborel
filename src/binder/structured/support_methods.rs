//! [ISO/IEC 9075-2:2023 §9.18, §9.19; ISO/IEC 13249-3:2016 (DIS, SC32
//! N2593) §14, §16, §17] A method invocation of a value of one of SQL/MM
//! Spatial's types beside the geometry types (`geometry/support.rs`):
//! of the methods of its name and arity the type declares, the one its
//! arguments' types choose [§9.6] (RULES.md D-SPATIAL-SUPPORT). None of
//! these types has a proper supertype, so the method runs as bound.

use std::sync::Arc;

use super::super::{Binder, Scope, rule};
use crate::ast::{TypeName, ValueExpression};
use crate::error::Result;
use crate::functions::ScalarRef;
use crate::geometry::support::{self, Routine};
use crate::logical::Expr;
use crate::value::DataType;

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §9.19] The invocation of the method `name` of
    /// `args` of `subject`, if `subject` is of one of the types; none
    /// else.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `as_type` is written [§6.18 SR], or no method of the
    /// name, arity and arguments' types is the type's [§9.18 SR10b]; an
    /// argument's.
    pub(super) fn support_method(
        &self,
        (subject, as_type): (&ValueExpression, Option<&TypeName>),
        name: &str,
        args: &[ValueExpression],
        scope: Scope<'_>,
    ) -> Result<Option<(Expr, DataType)>> {
        let (e, t) = self.value_expression(subject, scope)?;
        let Some(of) = support::of_data_type(&t) else { return Ok(None) };
        if as_type.is_some() {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.18, §9.18 SR10a] {} has no proper supertype", of.sql_name())));
        }
        let upper = name.to_ascii_uppercase();
        let none = || {
            rule(format!(
                "[ISO/IEC 9075-2:2023 §9.18 SR10b, §9.19 SR3] {} has no method {upper} of these {} arguments",
                of.sql_name(),
                args.len()
            ))
        };
        let candidates: Vec<ScalarRef> =
            support::methods(of, &upper, args.len()).into_iter().map(|op| ScalarRef(Arc::new(Routine(op)))).collect();
        let mut bound = vec![Some((e, t))];
        bound.extend(self.prebound(args, scope)?);
        let routine = super::super::arguments::registered_subject(&upper, candidates, &bound)?.ok_or_else(none)?;
        let written: Vec<ValueExpression> = std::iter::once(subject.clone()).chain(args.iter().cloned()).collect();
        let (exprs, types) = self.arguments_of(&*routine.0, &written, bound, scope)?;
        let result = routine.0.return_type(&types)?;
        Ok(Some((Expr::Call(routine, exprs), result)))
    }
}
