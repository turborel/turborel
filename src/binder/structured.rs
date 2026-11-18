//! [ISO/IEC 9075-2:2023 §6.18 to §6.24, §8.19] Expressions and predicates
//! of user-defined types and references, bound (RULES.md D81).
//!
//! An observer, a mutator and a type's constructor function are built-in
//! functions told the attribute's position (`functions/structured.rs`). An
//! SQL-invoked method is an invocation whose overriding methods are listed
//! at bind time, the one of a value's most specific type run
//! (`logical/invoked.rs`).

mod casts;
mod combined;
mod elements;
mod family_methods;
mod methods;
mod mutated;
mod orderings;
mod support_methods;
pub(super) use combined::user_defined_set_type;
pub(super) use orderings::{Need, keyed_columns, keyed_operand, mapped, udt_operand};
mod references;
mod transforms;
mod types;

pub(crate) use casts::assignment_cast;

use std::sync::Arc;

use super::{Binder, Scope, rule};
use crate::ast::{ColumnSpecification, RowValue, StructuredExpression, TypeList, TypeName, ValueExpression};
use crate::catalog::StructuredDesc;
use crate::error::{At, Result};
use crate::functions::{Constructed, ScalarFunction, ScalarRef};
use crate::logical::{Cond, Expr};
use crate::params::ParameterType;
use crate::parser::Parser;
use crate::value::DataType;
use crate::value::types::UdtName;

/// The rule a default bound for a constructor function names, were it to
/// contain a set function.
const IN_DEFAULT: &str = "[ISO/IEC 9075-2:2023 §11.5] an attribute's <default option> contains no set function";

/// The call of `f`, of `args`.
pub(super) fn call(f: impl ScalarFunction + 'static, args: Vec<Expr>) -> Expr {
    Expr::Call(ScalarRef(Arc::new(f)), args)
}

/// The `UdtName` of the type `t`.
pub(super) fn udt_name(t: &StructuredDesc) -> UdtName {
    t.structured_type().name
}

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §11.51 GR3, §4.9.3.4] The invocation of the
    /// constructor function of `t`: an instance of it each of whose
    /// attributes is its default, null if it has none.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `t` is not instantiable [§6.20 SR2]; a default's,
    /// as it binds.
    pub(super) fn constructor_invocation(&self, t: &StructuredDesc) -> Result<(Expr, DataType)> {
        if !t.instantiable {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.20 SR2] {} is not instantiable", t.name)));
        }
        let args = t.attributes.iter().map(|a| self.attribute_default(a)).collect::<Result<Vec<Expr>>>()?;
        let result = t.data_type();
        let constructed = Constructed { of: Arc::new(udt_name(t)), result: result.clone() };
        Ok((call(constructed, args), result))
    }

    /// [ISO/IEC 9075-2:2023 §6.5 SR3, §4.9.3.1] The default of attribute
    /// `a`, cast to its type: of no `<default option>`, the null value.
    pub(super) fn attribute_default(&self, a: &crate::catalog::AttributeDesc) -> Result<Expr> {
        let Some(text) = &a.default else { return Ok(Expr::Const(crate::value::Value::Null)) };
        let written = Parser::for_stored(text)?.generation_expression()?;
        let (e, _) = self.typed(&written, ParameterType::Of(a.data_type.clone()), Scope::Rows(IN_DEFAULT))?;
        Ok(Expr::Cast(Box::new(e), a.data_type.clone()))
    }

    /// [ISO/IEC 9075-2:2023 §6.17 to §6.24, §6.33, §6.4] An expression of
    /// user-defined types or references, `s`, written at `at`, bound.
    ///
    /// # Errors
    ///
    /// Each form's, placed at `at`.
    pub(super) fn structured_expression(&self, (s, at): (&StructuredExpression, usize), scope: Scope<'_>) -> Result<(Expr, DataType)> {
        self.structured_at((s, at), scope).or_at(at)
    }

    /// `structured_expression`, its errors not yet placed.
    fn structured_at(&self, (s, at): (&StructuredExpression, usize), scope: Scope<'_>) -> Result<(Expr, DataType)> {
        match s {
            StructuredExpression::Method { subject, as_type, name, args } => {
                self.method_invocation((subject, as_type.as_ref()), (name, at), args, scope)
            }
            StructuredExpression::Static { udt, name, args } => self.static_method(udt, name, args, scope),
            StructuredExpression::New { udt, args } => self.new_specification(udt, args, scope),
            StructuredExpression::Treat { subject, target } => self.treat(subject, target, scope),
            StructuredExpression::SpecificType(value) => self.specific_type(value, scope),
            StructuredExpression::Dereference { subject, attribute } => self.dereference(subject, attribute, scope),
            StructuredExpression::MethodReference { subject, name, args } => self.method_reference(subject, name, args, scope),
            StructuredExpression::Deref(subject) => self.reference_resolution(subject, scope),
            StructuredExpression::CurrentDefaultTransformGroup => Ok(self.current_transform_group(None)),
            StructuredExpression::CurrentTransformGroupForType(udt) => {
                let desc = self.named_structured(&TypeName::Domain(udt.clone()))?;
                Ok(self.current_transform_group(Some(desc)))
            }
        }
    }

    /// [ISO/IEC 9075-2:2023 §6.18, §9.19] A method invocation of `name`
    /// with `args` of `subject`, as a value of `as_type` if a generalized
    /// invocation's.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the subject is of no user-defined type [§9.19 SR3], or
    /// of no subtype of `as_type` [§6.18]; `method_of`'s.
    fn method_invocation(
        &self,
        (subject, as_type): (&ValueExpression, Option<&TypeName>),
        (name, at): (&str, usize),
        args: &[ValueExpression],
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let (e, t) = self.value_expression(subject, scope)?;
        // [§6.36] of the JSON type, an item method.
        if let (DataType::Json, None) = (&t, as_type) {
            return self.json_step((e, t), super::json::Step::Method(name, args), scope);
        }
        if let DataType::Extension(_) = &t {
            return self.extension_method((subject, as_type), (name, at), args, scope);
        }
        if let DataType::Distinct(d) = &t {
            let of = UdtName::new(&d.schema, &d.name);
            // [§6.18 SR, §9.19 SR6a] a distinct type has no proper supertype.
            if let Some(written) = as_type
                && self.db.resolve_type(written, (self.schema, self.who()))?.0 != t
            {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.18, §9.18 SR10a] {} has no proper supertype", d.name)));
            }
            return self.method_of(((e, t.clone()), &of), (name, args), (as_type.is_some(), scope));
        }
        let DataType::Structured(declared) = &t else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §9.19 SR3] method {name} is invoked of a value of {t}, no user-defined type")));
        };
        let Some(written) = as_type else {
            let of = declared.name.clone();
            return self.method_of(((e, t), &of), (name, args), (false, scope));
        };
        // [§9.19 SR6a] (VEP AS DT): SELF of DT, a supertype of VEP's type.
        let dt = self.named_structured(written)?;
        let of = udt_name(dt);
        if of != declared.name && !declared.supertypes.contains(&of) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.18, §9.18 SR10a] {} is no supertype of {}", dt.name, declared.name)));
        }
        self.method_of(((e, dt.data_type()), &of), (name, args), (true, scope))
    }

    /// [ISO/IEC 9075-2:2023 §6.6 SR11, §6.18] `subject` . `name` (`args`),
    /// written as a routine invocation of schema `subject`: the method
    /// invocation of the column, of a structured type or a row type.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the column is of a row type, whose fields are no
    /// methods; `method_of`'s.
    pub(super) fn method_call(
        &self,
        subject: &ColumnSpecification,
        (name, args): (&str, &[ValueExpression]),
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let (e, t) = self.column_expr(subject, scope)?;
        // [§6.36] of the JSON type, an item method.
        if let DataType::Json = &t {
            return self.json_step((e, t), super::json::Step::Method(name, args), scope);
        }
        let of = if let DataType::Structured(declared) = &t {
            declared.name.clone()
        } else if let DataType::Distinct(d) = &t {
            UdtName::new(&d.schema, &d.name)
        } else {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §9.19 SR3] method {name} is invoked of column {}, of {t}, no user-defined type",
                subject.column
            )));
        };
        self.method_of(((e, t), &of), (name, args), (false, scope))
    }

    /// [ISO/IEC 9075-2:2023 §8.19] `<type predicate>`.
    ///
    /// # Errors
    ///
    /// As `type_predicate_of`'s.
    pub(super) fn type_predicate(&self, value: &RowValue, types: &TypeList, not: bool, scope: Scope<'_>) -> Result<Cond> {
        self.type_predicate_of(value, types, not, scope)
    }
}

impl Binder<'_> {
    /// \[EXTENSION\] (RULES.md D87) A method of a value of a registered
    /// type, as `doc.Contains(p)` of SQL/MM Full-Text [ISO/IEC 13249-2:2002
    /// §5.1.2]: the call of the registered function of its name whose
    /// first argument is the value, `CONTAINS(doc, p)`. A registered type
    /// is no user-defined type, so it declares no methods of its own, and
    /// has no supertype for AS to name; a geometry type has its method
    /// table (`family_method`), and each of SQL/MM Spatial's types beside
    /// them its methods (`support_method`).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if AS names a type, or as the call's binding.
    pub(super) fn extension_method(
        &self,
        (subject, as_type): (&ValueExpression, Option<&TypeName>),
        (name, at): (&str, usize),
        args: &[ValueExpression],
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        if let Some(bound) = self.family_method((subject, as_type), name, args, scope)? {
            return Ok(bound);
        }
        if let Some(bound) = self.support_method((subject, as_type), name, args, scope)? {
            return Ok(bound);
        }
        if as_type.is_none() && args.is_empty() && name.eq_ignore_ascii_case("ST_ISEMPTY") {
            let (e, t) = self.value_expression(subject, scope)?;
            if let Some(f) = crate::geometry::tin_element_is_empty(&t) {
                let result = f.return_type(&[t])?;
                return Ok((Expr::Call(ScalarRef(f), vec![e]), result));
            }
        }
        if as_type.is_some() {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.18, §9.18 SR10a] a value of a registered type has no supertype for {name}")));
        }
        let args = std::iter::once(subject.clone()).chain(args.iter().cloned()).collect();
        // A call by name, as SQL.FUNCTION_CALLS takes it or not, would
        // refuse a method written as the standard writes it.
        let call = ValueExpression::Call { schema: None, name: name.to_owned(), args, at, extension: true };
        self.value_expression(&call, scope)
    }

    /// The column `name` of this query, of no table named, if it is of a
    /// registered type, so `name.m(...)` is a method of it rather than a
    /// function of a schema `name`.
    pub(super) fn extension_basis(&self, name: &str, at: usize) -> Option<ColumnSpecification> {
        let basis = ColumnSpecification { qualifier: None, column: name.to_owned(), catalog: None, at };
        matches!(self.local(&basis), Ok(Some((_, DataType::Extension(_), _)))).then_some(basis)
    }
}
