//! [ISO/IEC 9075-2:2023 §6.18, §6.19, §9.18, §9.19] Method invocations,
//! bound: an attribute's observer or mutator, or the SQL-invoked method
//! the arguments' types choose among those of the subject's type and its
//! supertypes [§9.6], with the methods that override it in each subtype,
//! the one of a value's most specific type run [§9.18 GR3b] (RULES.md D81).

use std::sync::Arc;

use super::super::invocation::{Prebound, held};
use super::super::{Binder, Scope, rule};
use super::{call, udt_name};
use crate::ast::{MethodKind, ValueExpression};
use crate::catalog::{
    AttributeDesc, MethodHost, MethodSpecDesc, SqlRoutineDesc, StructuredDesc, method_host, owner_of, udt_qualified, usage_held,
};
use crate::error::{Code, Result, SqlError};
use crate::functions::{Mutator, Observer};
use crate::logical::{Expr, Invocation};
use crate::params::ParameterType;
use crate::value::DataType;
use crate::value::precedence::{subject_routines, takes};
use crate::value::types::{StructuredType, UdtName};

/// A method specification and the type whose descriptor includes it.
type Spec<'a> = (MethodHost<'a>, &'a MethodSpecDesc);

impl<'a> Binder<'a> {
    /// The descriptor of the structured type `t`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if no type of its name is defined, as one dropped.
    pub(in crate::binder) fn structured_desc(&self, t: &UdtName) -> Result<&'a StructuredDesc> {
        let qn = udt_qualified(t);
        self.db.catalog.structured_type(&qn).ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §11.51] no structured type {qn}")))
    }

    /// The descriptor of the user-defined type `t`, structured or distinct.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if no type of its name is defined, as one dropped.
    pub(in crate::binder) fn method_host(&self, t: &UdtName) -> Result<MethodHost<'a>> {
        let qn = udt_qualified(t);
        method_host(&*self.db.catalog, &qn).ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §11.51] no user-defined type {qn}")))
    }

    /// [ISO/IEC 9075-2:2023 §6.6 SR11, §6.18] `bound` . `name` of no
    /// argument list, `bound` of the structured type `t`: its attribute's
    /// observer, or the method of the name of no argument.
    ///
    /// # Errors
    ///
    /// As `method_of`'s.
    pub(in crate::binder) fn attribute_or_method(
        &self,
        bound: (Expr, DataType),
        (t, name): (&StructuredType, &str),
    ) -> Result<(Expr, DataType)> {
        self.method_of((bound, &t.name), (name, &[]), (false, Scope::Rows(NO_SET_FUNCTION)))
    }

    /// [ISO/IEC 9075-2:2023 §6.18, §9.19] The method invocation of `name`
    /// with `args` of the subject `bound`, as a value of the type `of`, a
    /// generalized invocation's if `generalized`: an observer of no
    /// argument; a mutator of one assignable to its attribute; else an
    /// SQL-invoked instance method, the subject routine of SELF and the
    /// arguments [§9.18 SR10], and, of one not generalized, those that
    /// override it in each subtype [GR3b v].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if no method takes the arguments, more than one is the
    /// subject routine, or one is not invoked here; -104 if EXECUTE on it
    /// is not held [§9.18 SR6].
    pub(in crate::binder) fn method_of(
        &self,
        (bound, of): ((Expr, DataType), &UdtName),
        (name, args): (&str, &[ValueExpression]),
        (generalized, scope): (bool, Scope<'_>),
    ) -> Result<(Expr, DataType)> {
        let host = self.method_host(of)?;
        if let Some(desc) = host.structured()
            && let Some((index, a)) = desc.attribute(name)
        {
            match args {
                [] => return self.observed(bound, (index, a), desc),
                [value] => return self.mutated((bound, value), (index, a), (desc, scope)),
                _ => {}
            }
        }
        let prebound = self.prebound(args, scope)?;
        let spec = self.chosen_method((host, name, MethodKind::Instance), &prebound)?;
        let routine = self.method_routine(spec)?;
        let result = if spec.1.self_as_result { bound.1.clone() } else { spec.1.returns.clone() };
        let mut given = vec![bound.0];
        given.extend(self.method_arguments((routine, 1), (args, prebound), scope)?);
        let mut invocation = self.bound_invocation(routine, given)?;
        if !generalized && host.structured().is_some() {
            invocation.overrides = Some(self.overriding(spec, of)?);
        }
        Ok((Expr::Invoked(Arc::new(invocation)), result))
    }

    /// [ISO/IEC 9075-2:2023 §6.19, §9.18] The static method named `name`
    /// of `host`'s type or a supertype that is the subject routine of
    /// `args`.
    ///
    /// # Errors
    ///
    /// As `method_of`'s.
    pub(in crate::binder) fn static_of(
        &self,
        host: MethodHost<'a>,
        name: &str,
        (args, scope): (&[ValueExpression], Scope<'_>),
    ) -> Result<(Expr, DataType)> {
        let prebound = self.prebound(args, scope)?;
        let spec = self.chosen_method((host, name, MethodKind::Static), &prebound)?;
        let routine = self.method_routine(spec)?;
        let given = self.method_arguments((routine, 0), (args, prebound), scope)?;
        let invocation = self.bound_invocation(routine, given)?;
        Ok((Expr::Invoked(Arc::new(invocation)), spec.1.returns.clone()))
    }

    /// [ISO/IEC 9075-2:2023 §6.20 SR3, §9.19 SR5a] `constructed`, the
    /// invocation of `desc`'s constructor function, then the constructor
    /// method of the type's name of the type's schema that is the subject
    /// routine of it and `args`: of the type [§11.52 SR].
    ///
    /// # Errors
    ///
    /// As `method_of`'s.
    pub(in crate::binder) fn constructor_method(
        &self,
        (constructed, desc): ((Expr, DataType), &'a StructuredDesc),
        args: &[ValueExpression],
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let prebound = self.prebound(args, scope)?;
        let spec = self.chosen_method((MethodHost::Structured(desc), &desc.name.ident, MethodKind::Constructor), &prebound)?;
        let routine = self.method_routine(spec)?;
        let mut given = vec![constructed.0];
        given.extend(self.method_arguments((routine, 1), (args, prebound), scope)?);
        let invocation = self.bound_invocation(routine, given)?;
        Ok((Expr::Invoked(Arc::new(invocation)), constructed.1))
    }

    /// [ISO/IEC 9075-2:2023 §4.9.3.2] The observer of attribute `a`, at
    /// `index`, of a value `bound` of a type of `desc`'s family.
    fn observed(&self, bound: (Expr, DataType), (index, a): (usize, &AttributeDesc), desc: &StructuredDesc) -> Result<(Expr, DataType)> {
        self.executes(true, desc, &a.name)?;
        let observer = Observer { name: a.name.clone(), index, result: a.data_type.clone() };
        Ok((call(observer, vec![bound.0]), a.data_type.clone()))
    }

    /// [ISO/IEC 9075-2:2023 §4.9.3.3, §9.18 SR10l iii] The mutator of
    /// attribute `a` of `bound`, given `value`, assignable to it: of the
    /// declared type of `bound`.
    fn mutated(
        &self,
        (bound, value): ((Expr, DataType), &ValueExpression),
        (index, a): (usize, &AttributeDesc),
        (desc, scope): (&StructuredDesc, Scope<'_>),
    ) -> Result<(Expr, DataType)> {
        self.executes(false, desc, &a.name)?;
        let (e, t) = self.typed(value, ParameterType::Of(a.data_type.clone()), scope)?;
        let Some(e) = self.assigned((e, &t), &a.data_type)? else {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §9.18 SR10l iii] attribute {} is of {}, which a value of {t} cannot be stored in",
                a.name, a.data_type
            );
            return Err(rule(msg));
        };
        let mutator = Mutator { name: a.name.clone(), index, attribute: a.data_type.clone(), result: bound.1.clone() };
        Ok((call(mutator, vec![bound.0, e]), bound.1))
    }

    /// [ISO/IEC 9075-2:2023 §9.18 SR6, SR10l iii] Of the structured type
    /// `of`, the position and type of the attribute `name`, whose mutator
    /// an assignment to it invokes.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the type has no such attribute; -104 if EXECUTE on
    /// its mutator is not held.
    pub(in crate::binder) fn mutator_of(&self, of: &UdtName, name: &str) -> Result<(usize, DataType)> {
        let desc = self.structured_desc(of)?;
        let Some((index, a)) = desc.attribute(name) else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §9.18 SR10] type {} has no attribute {name}", desc.name)));
        };
        self.executes(false, desc, &a.name)?;
        Ok((index, a.data_type.clone()))
    }

    /// That EXECUTE on the observer of attribute `name` of the structured
    /// type `of` is held [ISO/IEC 9075-2:2023 §9.18 SR6].
    pub(in crate::binder) fn observer_held(&self, of: &UdtName, name: &str) -> Result<()> {
        let desc = self.structured_desc(of)?;
        let (_, a) = desc
            .attribute(name)
            .ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §9.18 SR10] type {} has no attribute {name}", desc.name)))?;
        self.executes(true, desc, &a.name)
    }

    /// That EXECUTE on the observer, if `observer`, else the mutator, of
    /// attribute `attribute` of `desc` is held [ISO/IEC 9075-2:2023 §9.18
    /// SR6]: granted on the routine of the type that defines it, of which a
    /// subtype's inherited attribute is a copy as it was created.
    fn executes(&self, observer: bool, desc: &StructuredDesc, attribute: &str) -> Result<()> {
        let own = desc.attribute(attribute).map(|(_, a)| a);
        let defining = own.filter(|a| a.defined_by != desc.name).and_then(|a| self.db.catalog.structured_type(&a.defined_by));
        let held = defining.and_then(|d| d.attribute(attribute)).map(|(_, a)| a).or(own).is_some_and(|a| {
            let execute = if observer { &a.observer.execute } else { &a.mutator.execute };
            usage_held(execute, self.who(), owner_of(&*self.db.catalog, &desc.name.auth)).0
        });
        if held {
            return Ok(());
        }
        let msg = format!("[ISO/IEC 9075-2:2023 §9.18 SR6] no EXECUTE is held on the method of attribute {attribute} of {}", desc.name);
        Err(SqlError::new(Code::Privilege, msg))
    }

    /// [ISO/IEC 9075-2:2023 §9.19 SR5b, §9.18 SR10, §9.6] The method of
    /// kind `kind` named `name` of `host`'s type or a supertype, defined by
    /// an SQL-invoked method, that is the subject routine of SELF, a value
    /// of the type, and the arguments `prebound`.
    fn chosen_method(&self, (host, name, kind): (MethodHost<'a>, &str, MethodKind), prebound: &Prebound) -> Result<Spec<'a>> {
        let mut specs: Vec<Spec<'a>> = Vec::new();
        // [§9.19 SR5a] a constructor method of the type's own schema alone.
        let inherited = if kind == MethodKind::Constructor { &[][..] } else { host.supertypes() };
        for t in std::iter::once(Ok(host)).chain(inherited.iter().map(|s| self.structured_desc(s).map(MethodHost::Structured))) {
            let t = t?;
            let named = t.methods().iter().filter(|m| m.name == name && m.kind == kind);
            specs.extend(named.filter(|m| self.db.catalog.routine(&m.specific).is_some()).map(|m| (t, m)));
        }
        let with_self = kind != MethodKind::Static;
        let mut args: Vec<Option<DataType>> = if with_self { vec![Some(host.data_type())] } else { Vec::new() };
        args.extend(prebound.iter().map(|b| b.as_ref().map(|(_, t)| t.clone())));
        let lists: Vec<Vec<DataType>> = specs.iter().map(|(t, m)| augmented(*t, m, with_self)).collect();
        let taking: Vec<usize> = (0..lists.len()).filter(|&i| lists.get(i).is_some_and(|l| takes(l, &args))).collect();
        let set: Vec<&[DataType]> = taking.iter().filter_map(|&i| lists.get(i).map(Vec::as_slice)).collect();
        match subject_routines(&set, &args).as_slice() {
            [one] => taking.get(*one).and_then(|&i| specs.get(i).copied()).ok_or_else(|| rule("[DESIGN] a subject routine out of range")),
            [] => Err(rule(format!("[ISO/IEC 9075-2:2023 §9.18 SR10] no method {name} of {} takes these arguments", host.name()))),
            [..] => Err(rule(format!("[ISO/IEC 9075-2:2023 §9.18 SR10, §9.6] more than one method {name} of {} is alike", host.name()))),
        }
    }

    /// The SQL-invoked method of `spec`, whose EXECUTE is held and which
    /// may be invoked here.
    ///
    /// # Errors
    ///
    /// SQLCODE -104 if EXECUTE on it is not held; -103 as `permitted`.
    fn method_routine(&self, (t, spec): Spec<'a>) -> Result<&'a SqlRoutineDesc> {
        let r = self
            .db
            .catalog
            .routine(&spec.specific)
            .ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §9.18 SR10] method {} of {} has no body", spec.name, t.name())))?;
        let who = self.who();
        if !(who.includes(owner_of(&*self.db.catalog, &r.name.auth)) || held(self.db, r, who)) {
            let msg = format!("[ISO/IEC 9075-2:2023 §9.18 SR6] no EXECUTE is held on method {} of {}", spec.name, t.name());
            return Err(SqlError::new(Code::Privilege, msg));
        }
        self.permitted(r)?;
        Ok(r)
    }

    /// [ISO/IEC 9075-2:2023 §9.18 SR10l iii] The arguments but SELF of an
    /// invocation of `r`, its first `skip` parameters SELF's, `args` bound
    /// as `prebound`, each but a `?` stored in its parameter.
    fn method_arguments(
        &self,
        (r, skip): (&SqlRoutineDesc, usize),
        (args, prebound): (&[ValueExpression], Prebound),
        scope: Scope<'_>,
    ) -> Result<Vec<Expr>> {
        let mut out = Vec::with_capacity(args.len());
        for ((arg, slot), p) in args.iter().zip(prebound).zip(r.parameters.iter().skip(skip)) {
            let (e, t) = match slot {
                Some(given) => given,
                None => self.typed(arg, ParameterType::Of(p.data_type.clone()), scope)?,
            };
            let Some(e) = self.assigned((e, &t), &p.data_type)? else {
                let msg = format!(
                    "[ISO/IEC 9075-2:2023 §9.18 SR10l iii] parameter {} of method {} is of {}, which an argument of {t} cannot be stored in",
                    p.name, r.name, p.data_type
                );
                return Err(rule(msg));
            };
            out.push(e);
        }
        Ok(out)
    }

    /// [ISO/IEC 9075-2:2023 §9.18 GR3b v] For each proper subtype of `of`
    /// whose nearest method of the original of `spec` is another, that
    /// method's invocation, of no arguments: those of the invocation of
    /// `spec` are its.
    fn overriding(&self, (_, spec): Spec<'a>, of: &UdtName) -> Result<Vec<(UdtName, Arc<Invocation>)>> {
        let original = spec.overrides.as_ref().unwrap_or(&spec.specific);
        let mut out = Vec::new();
        for sub in self.db.catalog.structured_types().into_iter().filter(|s| s.supertypes.contains(of)) {
            let Some(nearest) = self.nearest_override(sub, original)? else { continue };
            if nearest.specific == spec.specific {
                continue;
            }
            let Some(r) = self.db.catalog.routine(&nearest.specific) else { continue };
            out.push((udt_name(sub), Arc::new(self.bound_invocation(r, Vec::new())?)));
        }
        Ok(out)
    }

    /// The method specification of `sub` or its nearest supertype that is
    /// `original` or overrides it.
    fn nearest_override(&self, sub: &'a StructuredDesc, original: &crate::catalog::QualifiedName) -> Result<Option<&'a MethodSpecDesc>> {
        for t in std::iter::once(Ok(sub)).chain(sub.supertypes.iter().map(|s| self.structured_desc(s))) {
            let found = t?.methods.iter().find(|m| m.specific == *original || m.overrides.as_ref() == Some(original));
            if found.is_some() {
                return Ok(found);
            }
        }
        Ok(None)
    }
}

/// The rule a set function in an argument of a method of no argument list
/// would break: it has none.
const NO_SET_FUNCTION: &str = "[ISO/IEC 9075-2:2023 §6.18] a method of no argument list has no set function";

/// The parameter list of the method `m` of the type `t`: SELF, of `t`,
/// then its own, augmented if `with_self`, as a static method's is not.
fn augmented(t: MethodHost<'_>, m: &MethodSpecDesc, with_self: bool) -> Vec<DataType> {
    let mut list = if with_self { vec![t.data_type()] } else { Vec::new() };
    list.extend(m.parameters.iter().map(|p| p.data_type.clone()));
    list
}
