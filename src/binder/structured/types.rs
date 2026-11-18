//! [ISO/IEC 9075-2:2023 §6.17, §6.19, §6.20, §6.33, §8.19] Expressions of
//! the types of structured values, bound: TREAT, the `<type predicate>`,
//! SPECIFICTYPE, a static method invocation and NEW (RULES.md D81).

use std::sync::Arc;

use super::super::{Binder, Scope, rule};
use super::{call, udt_name};
use crate::ast::{MethodKind, RowValue, TableName, TypeList, TypeName, ValueExpression};
use crate::catalog::StructuredDesc;
use crate::error::Result;
use crate::functions::{FamilyTreat, FamilyTypeTest, SpecificType, Treat, TypeTest, below};
use crate::logical::{Cond, Expr};
use crate::value::DataType;
use crate::value::SqlType;
use crate::value::types::UdtName;
use crate::value::types::family::{Family, family_of};

impl<'a> Binder<'a> {
    /// The structured type `named` names, its names in the statement's
    /// default schema if it names none, USAGE on it held.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it names no structured type; -104 if USAGE on it is
    /// not held.
    pub(in crate::binder) fn named_structured(&self, named: &TypeName) -> Result<&'a StructuredDesc> {
        let (t, _) = self.db.resolve_type(named, (self.schema, self.who()))?;
        let DataType::Structured(s) = &t else { return Err(rule(format!("[ISO/IEC 9075-2:2023 §4.9.3] {t} is no structured type"))) };
        self.structured_desc(&s.name)
    }

    /// The names of `desc`'s type and of each of its proper subtypes.
    pub(in crate::binder) fn family_below(&self, desc: &StructuredDesc) -> Vec<UdtName> {
        let top = udt_name(desc);
        let subtypes = self.db.catalog.structured_types().into_iter().filter(|s| s.supertypes.contains(&top)).map(udt_name);
        std::iter::once(top.clone()).chain(subtypes).collect()
    }

    /// [ISO/IEC 9075-2:2023 §6.17] TREAT (`subject` AS `target`): of the
    /// target type, a supertype of the operand's, each value checked to be
    /// of it as it runs.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the operand is of no structured type, or the target
    /// is not its subtype [SR1, SR3]; of a reference, as
    /// `treat_reference`'s.
    pub(in crate::binder) fn treat(&self, subject: &ValueExpression, target: &TypeName, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let (e, t) = self.value_expression(subject, scope)?;
        if matches!(t, DataType::Reference(_)) {
            return self.treat_reference((e, t), target);
        }
        if let Some((family, from)) = family_of(&t) {
            let (target_type, _) = self.db.resolve_type(target, (self.schema, self.who()))?;
            let to = family_of(&target_type).filter(|(f, to)| std::ptr::addr_eq(*f, family) && family.is_subtype(*to, from));
            let Some((_, to)) = to else {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.17 SR3] {target_type} is no subtype of {t}")));
            };
            let f = FamilyTreat { family, types: below(family, to), result: target_type.clone() };
            return Ok((call(f, vec![e]), target_type));
        }
        let vt = if let DataType::Structured(s) = &t {
            Arc::clone(s)
        } else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.17 SR1] TREAT of {t}, which is neither a structured nor a reference type")));
        };
        let dt = self.named_structured(target)?;
        if udt_name(dt) != vt.name && !dt.supertypes.contains(&vt.name) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.17 SR3] {} is no subtype of {}", dt.name, vt.name)));
        }
        let result = dt.data_type();
        Ok((call(Treat { types: self.family_below(dt), result: result.clone() }, vec![e]), result))
    }

    /// [ISO/IEC 9075-2:2023 §8.19] `value` IS \[NOT\] OF (`types`): whether
    /// a value's most specific type is one written with ONLY, or a subtype
    /// of one written without.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the value is no single value of a user-defined type
    /// [SR1, SR2], or a type is no subtype of its declared type [SR5].
    pub(in crate::binder) fn type_predicate_of(&self, value: &RowValue, types: &TypeList, not: bool, scope: Scope<'_>) -> Result<Cond> {
        let [v] = value.as_slice() else {
            return Err(rule("[ISO/IEC 9075-2:2023 §8.19 SR1] the operand of IS OF is a common value expression, not a row"));
        };
        let (e, t) = self.value_expression(v, scope)?;
        if let Some((family, declared)) = family_of(&t) {
            return self.family_type_test((e, &t), (family, declared), (types, not));
        }
        let DataType::Structured(declared) = &t else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §8.19 SR2] the operand of IS OF is of {t}, no structured type")));
        };
        let mut set = Vec::new();
        for (only, named) in &types.0 {
            let st = self.named_structured(named)?;
            if udt_name(st) != declared.name && !st.supertypes.contains(&declared.name) {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §8.19 SR5] {} is no subtype of {}", st.name, declared.name)));
            }
            // [GR2] an exclusively specified type, or each subtype of an
            // inclusively specified one.
            if *only { set.push(udt_name(st)) } else { set.extend(self.family_below(st)) }
        }
        // [SR7] NOT ( CVE IS OF (TL) ).
        Ok(super::super::predicates::negated(Cond::Value(call(TypeTest { types: set }, vec![e])), not))
    }

    /// [ISO/IEC 9075-2:2023 §8.19] IS OF of a value of a type of a family
    /// built in (RULES.md D120).
    fn family_type_test(
        &self,
        (e, t): (Expr, &DataType),
        (family, declared): (&'static dyn Family, &'static dyn SqlType),
        (types, not): (&TypeList, bool),
    ) -> Result<Cond> {
        let mut set = Vec::new();
        for (only, named) in &types.0 {
            let (named_type, _) = self.db.resolve_type(named, (self.schema, self.who()))?;
            let of = family_of(&named_type).filter(|(f, k)| std::ptr::addr_eq(*f, family) && family.is_subtype(*k, declared));
            let Some((_, kind)) = of else {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §8.19 SR5] {named_type} is no subtype of {t}")));
            };
            if *only {
                set.extend(family.kinds().iter().copied().filter(|k| k.name() == kind.name()));
            } else {
                set.extend(below(family, kind));
            }
        }
        Ok(super::super::predicates::negated(Cond::Value(call(FamilyTypeTest { family, types: set }, vec![e])), not))
    }

    /// [ISO/IEC 9075-2:2023 §6.33 SR19, GR15] `value`.SPECIFICTYPE.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the value is of no user-defined type.
    pub(in crate::binder) fn specific_type(&self, value: &ValueExpression, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let (e, t) = self.value_expression(value, scope)?;
        if let Some((family, _)) = family_of(&t) {
            let f = crate::functions::FamilySpecificType { family, catalog: self.db.catalog_name() };
            let result = DataType::Varying { length: crate::functions::SPECIFICTYPE_LENGTH, units: crate::value::CharUnits::Characters };
            return Ok((call(f, vec![e]), result));
        }
        if !matches!(t, DataType::Structured(_)) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.33] SPECIFICTYPE of {t}, which is no user-defined type value expression")));
        }
        let f = SpecificType { catalog: self.db.catalog_name() };
        let result = DataType::Varying { length: crate::functions::SPECIFICTYPE_LENGTH, units: crate::value::CharUnits::Characters };
        Ok((call(f, vec![e]), result))
    }

    /// [ISO/IEC 9075-2:2023 §6.19, §9.18] `udt` :: `name` (`args`): the
    /// static method of the name of the type or a supertype that is the
    /// subject routine of the arguments.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if none is, or more than one; as `method_routine`'s.
    pub(in crate::binder) fn static_method(
        &self,
        udt: &TableName,
        name: &str,
        args: &[ValueExpression],
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let (t, _) = self.db.resolve_type(&TypeName::Domain(udt.clone()), (self.schema, self.who()))?;
        let host = if let DataType::Structured(s) = &t {
            self.method_host(&s.name)?
        } else if let DataType::Distinct(d) = &t {
            self.method_host(&UdtName::new(&d.schema, &d.name))?
        } else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.19 SR1] {t} is no user-defined type")));
        };
        self.static_of(host, name, (args, scope))
    }

    /// [ISO/IEC 9075-2:2023 §6.20 SR3] NEW `udt` (`args`): `RN()` of none
    /// if the type's schema has no constructor method of the type's name
    /// and no parameters; else `RN().MN(args)`, of the constructor method
    /// of the arguments.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the type is not instantiable [SR2], or no
    /// constructor method takes the arguments.
    pub(in crate::binder) fn new_specification(
        &self,
        udt: &TableName,
        args: &[ValueExpression],
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        // [EXTENSION] (RULES.md D87) NEW of a registered type, as SQL/MM
        // Full-Text's NEW FullText(...) [ISO/IEC 13249-2:2002 §5.1.9]: the
        // call of the registered function of its name.
        if let Some(kind) = self.db.types.get(&udt.ident).filter(|_| udt.auth.is_none())
            && let Some(family) = kind.family()
            && !family.instantiable(kind)
        {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.20 SR2] NEW {}: the type is not instantiable", udt.ident)));
        }
        if udt.auth.is_none() && self.db.types.get(&udt.ident).is_some() && !self.db.functions.scalars(&udt.ident).is_empty() {
            let call = ValueExpression::Call { schema: None, name: udt.ident.clone(), args: args.to_vec(), at: udt.at, extension: true };
            return self.value_expression(&call, scope);
        }
        let desc = self.named_structured(&TypeName::Domain(udt.clone()))?;
        let constructed = self.constructor_invocation(desc)?;
        let ident = &desc.name.ident;
        let has_nullary = desc.methods.iter().any(|m| m.kind == MethodKind::Constructor && m.name == *ident && m.parameters.is_empty());
        if args.is_empty() && !has_nullary {
            return Ok(constructed);
        }
        self.constructor_method((constructed, desc), args, scope)
    }
}
