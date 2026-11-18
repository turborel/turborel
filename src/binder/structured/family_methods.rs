//! [ISO/IEC 9075-2:2023 §6.18, §9.18, §9.19; ISO/IEC 13249-3:2016 (DIS,
//! SC32 N2593) §5 to §10] A method invocation of a value of a geometry
//! type, bound by the types' method table (`geometry/methods.rs`): the
//! subject routine its declared type, or the type a `<generalized
//! expression>` names, finds first [§9.18 SR10b], run as the method of
//! the overriding set the value's most specific type, or that named type,
//! finds first [§9.18 GR3b] (RULES.md D-SPATIAL-FAMILY).

use std::sync::Arc;

use super::super::{Binder, Scope, rule};
use crate::ast::{TypeName, ValueExpression};
use crate::error::Result;
use crate::functions::ScalarRef;
use crate::geometry::methods::{Dispatched, overriding_set, subject_method};
use crate::geometry::model::Kind;
use crate::geometry::{data_type_of, of_data_type};
use crate::logical::Expr;
use crate::value::DataType;

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §6.18, §9.19] The invocation of the method
    /// `name` of `args` of `subject`, as `as_type` if written, if
    /// `subject` is of a geometry type; none else.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `as_type` is no supertype of the subject's type
    /// [§6.18 SR, §9.18 SR10a], or no method of the name and arity is the
    /// type's or a supertype's [§9.18 SR10b]; an argument's.
    pub(super) fn family_method(
        &self,
        (subject, as_type): (&ValueExpression, Option<&TypeName>),
        name: &str,
        args: &[ValueExpression],
        scope: Scope<'_>,
    ) -> Result<Option<(Expr, DataType)>> {
        let (e, t) = self.value_expression(subject, scope)?;
        let Some(declared) = of_data_type(&t) else { return Ok(None) };
        let fixed = as_type.map(|written| self.generalized_kind(written, declared)).transpose()?;
        let kind = fixed.unwrap_or(declared);
        let upper = name.to_ascii_uppercase();
        let Some(method) = subject_method(kind, &upper, args.len()) else {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §9.18 SR10b, §9.19 SR3] {} and its supertypes have no method {upper} of {} arguments",
                kind.sql_name(),
                args.len()
            )));
        };
        let routine = (method.routine)();
        // [§9.18 SR10g i] SELF of a generalized expression is of its type.
        let subject_type = if fixed.is_some() { data_type_of(kind) } else { t };
        let mut bound = vec![Some((e, subject_type))];
        bound.extend(self.prebound(args, scope)?);
        let written: Vec<ValueExpression> = std::iter::once(subject.clone()).chain(args.iter().cloned()).collect();
        let (mut exprs, mut types) = self.arguments_of(&*routine, &written, bound, scope)?;
        for (arg, ty) in self.appended_arguments(&*routine, (&written, &exprs), scope)? {
            exprs.push(arg);
            types.push(ty);
        }
        let result = routine.return_type(&types)?;
        let set = overriding_set(method);
        if let ([_], None) = (set.as_slice(), fixed) {
            return Ok(Some((Expr::Call(ScalarRef(routine), exprs), result)));
        }
        let set = set.into_iter().map(|m| (m, ScalarRef((m.routine)()))).collect();
        let dispatched = Dispatched { name: method.name, fixed, set, subject: ScalarRef(routine), result: result.clone() };
        Ok(Some((Expr::Call(ScalarRef(Arc::new(dispatched)), exprs), result)))
    }

    /// [ISO/IEC 9075-2:2023 §6.18, §9.18 SR10a] The geometry type `written`
    /// names, a supertype of `declared`, the subject's declared type.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it names no geometry type, or none of `declared`'s
    /// supertypes.
    fn generalized_kind(&self, written: &TypeName, declared: Kind) -> Result<Kind> {
        let (t, _) = self.db.resolve_type(written, (self.schema, self.who()))?;
        of_data_type(&t)
            .filter(|k| declared.is_subtype_of(*k))
            .ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §6.18, §9.18 SR10a] {t} is no supertype of {}", declared.sql_name())))
    }
}
