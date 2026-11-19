//! [OGC 06-104r4 §7.2] The routines of Simple feature access, SQL option,
//! called by the names it writes them with, without SQL/MM's prefix ST_
//! [§7.2.2.2 Note, Annex C.3.3.3] (`geometry/sfa.rs`, RULES.md D136).
//!
//! A call of no function of the schema, unqualified, of such a name is
//! bound in turn as:
//!
//! * a constructor (`sfa::CONSTRUCTORS`), unless a function of its name
//!   is registered: the call of SQL/MM's function of the name with ST_;
//! * a routine of a geometry value (`sfa::SUBJECT_ROUTINES`), its first
//!   argument of a geometry type: the method of the name with ST_ that
//!   the argument's declared type finds [ISO/IEC 9075-2:2023 §9.18
//!   SR10b], before any function registered of the name, CONTAINS of
//!   full-text say; else SQL/MM's function of the name with ST_, if one
//!   is registered; else the method of the one type below the declared
//!   type that declares it, of TREAT of the argument as that type
//!   [§6.17], as a column of ST_Geometry holds a collection;
//! * else as any call of a function registered is (`Binder::call`).

use super::invocation::Passed;
use super::{Binder, Scope};
use crate::ast::{StructuredExpression, TableName, TypeName, ValueExpression};
use crate::error::Result;
use crate::geometry::methods::{METHODS, subject_method};
use crate::geometry::model::Kind;
use crate::geometry::{of_data_type, sfa};
use crate::logical::Expr;
use crate::value::DataType;

impl Binder<'_> {
    /// [OGC 06-104r4 §7.2] The call of `name` with `args`, at `at`, of no
    /// function of the schema: a routine of Simple feature access, if it
    /// is one; else the function registered (`Binder::call`).
    ///
    /// # Errors
    ///
    /// As `Binder::call`; the method's of a geometry value of no type that
    /// declares it.
    pub(super) fn simple_feature(
        &self,
        (schema, name, extension): (Option<&str>, &str, bool),
        (args, at): (&[ValueExpression], usize),
        passed: Passed,
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let plain = schema.is_none() && extension && passed.refusal.is_none();
        if !plain {
            return self.call((schema, name, extension), args, passed, scope);
        }
        let registered = !self.db.functions.scalars(name).is_empty();
        if let Some(target) = sfa::constructor(name).filter(|_| !registered) {
            return self.value_expression(&call(target, args.to_vec(), at), scope);
        }
        let (Some(method), Some((subject, rest))) = (sfa::subject_routine(name), args.split_first()) else {
            return self.call((schema, name, extension), args, passed, scope);
        };
        let bound = match passed.bound {
            Some(bound) => bound,
            None => self.prebound(args, scope)?,
        };
        let declared = bound.first().and_then(Option::as_ref).and_then(|(_, t)| of_data_type(t));
        let Some(declared) = declared else {
            return self.call((schema, name, extension), args, Passed { bound: Some(bound), refusal: None }, scope);
        };
        self.subject_routine((subject, declared, at), &method, rest, scope)
    }

    /// The routine `method` of SQL/MM of `subject`, of declared type
    /// `declared`, with `rest`.
    fn subject_routine(
        &self,
        (subject, declared, at): (&ValueExpression, Kind, usize),
        method: &str,
        rest: &[ValueExpression],
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let named = sfa::patch_method(declared, method).unwrap_or(method);
        if subject_method(declared, named, rest.len()).is_some() {
            return self.value_expression(&invoke(subject.clone(), method, rest, at), scope);
        }
        if !self.db.functions.scalars(method).is_empty() {
            let args = std::iter::once(subject.clone()).chain(rest.iter().cloned()).collect();
            return self.value_expression(&call(method, args, at), scope);
        }
        let below: Vec<Kind> = METHODS
            .iter()
            .filter(|m| m.name == named && m.arity == rest.len() && !m.overriding && m.of.is_subtype_of(declared))
            .map(|m| m.of)
            .collect();
        let subject = match below.as_slice() {
            [one] => treat(subject.clone(), *one, at),
            _ => subject.clone(),
        };
        self.value_expression(&invoke(subject, method, rest, at), scope)
    }
}

/// The call of the function `name` with `args`, unqualified.
fn call(name: &str, args: Vec<ValueExpression>, at: usize) -> ValueExpression {
    ValueExpression::Call { schema: None, name: name.to_owned(), args, at, extension: true }
}

/// The invocation of the method `name` of `subject` with `args`.
fn invoke(subject: ValueExpression, name: &str, args: &[ValueExpression], at: usize) -> ValueExpression {
    let method = StructuredExpression::Method { subject, as_type: None, name: name.to_owned(), args: args.to_vec() };
    ValueExpression::Structured(Box::new(method), at)
}

/// TREAT ( `subject` AS `kind` ) [ISO/IEC 9075-2:2023 §6.17].
fn treat(subject: ValueExpression, kind: Kind, at: usize) -> ValueExpression {
    let target = TypeName::Domain(TableName { auth: None, ident: kind.type_name().to_owned(), at });
    ValueExpression::Structured(Box::new(StructuredExpression::Treat { subject, target }), at)
}
