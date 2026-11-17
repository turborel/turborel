//! Structured types and what is defined for user-defined types [ISO/IEC
//! 9075-2:2023 §11.51 to §11.71, §19.13]: CREATE TYPE of a structured
//! type, ALTER TYPE, SQL-invoked methods, user-defined casts, orderings
//! and transforms, their drops, and SET TRANSFORM GROUP, in a transaction
//! as the other schema statements are (RULES.md D33, D81).
//!
//! What this module holds is what its submodules share: the names a
//! type's definition takes, the type relations its rules compare by, and
//! the routine a `<specific routine designator>` names.

mod alter;
mod attribute_privileges;
mod attributes;
mod bodies;
mod casts;
mod create;
mod drop;
mod functions;
mod methods;
mod objects;
mod orderings;
mod privileges;
mod table_methods;
mod transforms;

#[cfg(test)]
mod tests;

pub(in crate::db) use objects::type_named;

use std::sync::Arc;

use super::{State, rule};
use crate::ast::{MethodKind, PrivilegeObject, RoutineDesignator};
use crate::catalog::{MethodHost, QualifiedName, SqlRoutineDesc, StructuredDesc, Who, based_on, owner_of, sole, types_in, usage_held};
use crate::error::Result;
use crate::value::DataType;

/// What a designator names: its name, its kind, its `<data type list>` if
/// written, and the actions granted or revoked on it, as GRANT and REVOKE
/// name a routine (`routines/privileges.rs`).
pub(in crate::db) type Named<'a> =
    (&'a crate::ast::TableName, PrivilegeObject, Option<&'a [crate::ast::TypeName]>, Option<&'a [crate::ast::Action]>);

/// [ISO/IEC 9075-2:2023 §11.51 SR9b] A method specification as the rules
/// on duplicates and conflicts compare it: its name, kind, unaugmented
/// parameter types, and whether it is an attribute's observer or mutator
/// \[SR8g\], of which none overrides [SR9b xv 5].
#[derive(Debug, Clone)]
pub(super) struct Signature {
    /// The `<method name>`.
    pub(super) name: String,
    /// INSTANCE, STATIC or CONSTRUCTOR.
    pub(super) kind: MethodKind,
    /// The unaugmented parameter types.
    pub(super) types: Vec<DataType>,
    /// An observer's or a mutator's.
    pub(super) implicit: bool,
}

/// [ISO/IEC 9075-2:2023 §11.51 SR8g, SR9] The method specifications of
/// `host`'s own: of a structured type its originally-defined attributes'
/// observers and mutators, then its written ones.
pub(super) fn signatures(host: MethodHost<'_>) -> Vec<Signature> {
    let mut all = Vec::new();
    let own = host.structured().into_iter().flat_map(|s| s.attributes.iter().filter(|a| a.defined_by == s.name));
    for a in own {
        all.push(Signature { name: a.name.clone(), kind: MethodKind::Instance, types: Vec::new(), implicit: true });
        all.push(Signature { name: a.name.clone(), kind: MethodKind::Instance, types: vec![a.data_type.clone()], implicit: true });
    }
    for m in host.methods() {
        let types = m.parameters.iter().map(|p| p.data_type.clone()).collect();
        all.push(Signature { name: m.name.clone(), kind: m.kind, types, implicit: false });
    }
    all
}

/// [ISO/IEC 9075-2:2023 §4.1.4] Whether the types `a` and `b` are
/// compatible: of a user-defined or a reference type, the one type; else
/// comparable.
pub(super) fn compatible(a: &DataType, b: &DataType) -> bool {
    match (a, b) {
        (DataType::Structured(x), DataType::Structured(y)) => x.name == y.name,
        (DataType::Reference(x), DataType::Reference(y)) => x.referenced.name == y.referenced.name,
        (DataType::Structured(_) | DataType::Reference(_), _) | (_, DataType::Structured(_) | DataType::Reference(_)) => false,
        (_, _) => a.comparable(b),
    }
}

/// Whether each of `a` is compatible with the one of `b` in its place.
pub(super) fn all_compatible(a: &[DataType], b: &[DataType]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| compatible(x, y))
}

/// [ISO/IEC 9075-2:2023 §4.9.3.1] Whether `a` is a subtype of `b`: a
/// structured type of it or of a proper subtype; any other the type.
pub(super) fn is_subtype(a: &DataType, b: &DataType) -> bool {
    match (a, b) {
        (DataType::Structured(x), DataType::Structured(y)) => x.is_subtype_of(&y.name),
        (_, _) => a == b,
    }
}

/// [ISO/IEC 9075-2:2023 §11.59 SR4f iv, GR3 to GR6] Whether `t` names
/// the user-defined type `qn`: is based on it, or is a reference type of
/// it, or holds one.
pub(in crate::db) fn names_type(t: &DataType, qn: &QualifiedName) -> bool {
    let mut named = Vec::new();
    types_in(t, &mut named);
    based_on(t, qn) || named.contains(qn)
}

/// Whether `t` is a user-defined type or a reference type.
pub(super) fn user_defined(t: &DataType) -> bool {
    matches!(t, DataType::Distinct(_) | DataType::Structured(_) | DataType::Reference(_))
}

impl State {
    /// The descriptor of structured type `qn`, as the rule `cited` says
    /// there must be one.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is none.
    pub(in crate::db) fn structured_named(&self, qn: &QualifiedName, cited: &str) -> Result<StructuredDesc> {
        let found = self.catalog.structured_type(qn).cloned();
        found.ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 {cited}] there is no structured type {qn}")))
    }

    /// The structured types of which `qn` is a proper supertype, the
    /// nearest first, as each holds its supertypes.
    pub(in crate::db) fn proper_subtypes(&self, qn: &QualifiedName) -> Vec<StructuredDesc> {
        let mut found: Vec<StructuredDesc> =
            self.catalog.structured_types().into_iter().filter(|t| t.is_proper_subtype_of(qn)).cloned().collect();
        found.sort_by_key(|t| t.supertypes.len());
        found
    }

    /// The descriptors of the proper supertypes of `desc`, the nearest
    /// first.
    pub(in crate::db) fn proper_supertypes(&self, desc: &StructuredDesc) -> Vec<StructuredDesc> {
        let named = desc.supertypes.iter().map(crate::catalog::udt_qualified);
        named.filter_map(|qn| self.catalog.structured_type(&qn).cloned()).collect()
    }

    /// `desc`, recorded in place of the descriptor of its name.
    pub(in crate::db) fn put_structured(&mut self, desc: StructuredDesc) {
        let mut catalog = self.catalog.duplicate();
        catalog.put_structured_type(desc);
        self.catalog = Arc::from(catalog);
    }

    /// The timestamp of creation of a routine or a method specification
    /// made now, as an order: one more than any there is.
    pub(in crate::db) fn next_created(&self) -> u64 {
        let routines = self.catalog.routines().into_iter().map(|r| r.created).max().unwrap_or(0);
        let methods = self.catalog.structured_types().into_iter().flat_map(|t| t.methods.iter().map(|m| m.created)).max().unwrap_or(0);
        routines.max(methods).saturating_add(1)
    }

    /// [ISO/IEC 9075-2:2023 §11.51 SR9b vii, §11.60 SR9u] Whether a
    /// routine or a method specification of its schema has the specific
    /// name `specific`: an SQL-invoked routine; a distinct type's function
    /// or method specification; a structured type's observer, mutator or
    /// constructor function; or the method specification of either.
    pub(in crate::db) fn specific_in_use(&self, specific: &QualifiedName) -> bool {
        let named = |h: MethodHost<'_>| h.name().auth == specific.auth && host_specifics(h).contains(&specific.ident.as_str());
        self.catalog.routine(specific).is_some()
            || self.catalog.user_types().into_iter().any(|t| named(MethodHost::Distinct(t)))
            || self.catalog.structured_types().into_iter().any(|t| named(MethodHost::Structured(t)))
    }

    /// [ISO/IEC 9075-2:2023 §12.3, §11.51 GR5] Whether `who` holds UNDER
    /// on `desc`, and whether grantably. A cell enabling the owner of its
    /// schema holds it, grantably if it has no supertype or the owner holds
    /// UNDER on its direct supertype grantably \[GR5\]; another as granted.
    pub(in crate::db) fn under_held(&self, desc: &StructuredDesc, who: Who<'_>) -> (bool, bool) {
        let catalog = &*self.catalog;
        let (mut current, mut who, mut first) = (desc, who, true);
        // Each pass goes up one supertype: no more than it has.
        for _ in 0..=desc.supertypes.len() {
            let owner = owner_of(catalog, &current.name.auth);
            if !who.includes(owner) {
                let granted = usage_held(&current.under, who, owner);
                return if first { granted } else { (true, granted.1) };
            }
            let Some(next) = current.supertype.as_ref().and_then(|s| catalog.structured_type(s)) else { return (true, true) };
            (current, who, first) = (next, sole(catalog, owner), false);
        }
        (true, true)
    }

    /// [ISO/IEC 9075-2:2023 §10.6] The SQL-invoked function `d` designates,
    /// its names in schema `schema`, its types named as `owner` may, as
    /// the rule `cited` needs one.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it designates no SQL-invoked function, or not
    /// exactly one.
    pub(in crate::db) fn function_designated(
        &self,
        d: &RoutineDesignator,
        (schema, owner): (&str, &str),
        cited: &str,
    ) -> Result<SqlRoutineDesc> {
        let object = if d.specific { PrivilegeObject::SpecificRoutine(d.routine_type) } else { PrivilegeObject::Routine(d.routine_type) };
        let named = (&d.name, object, d.parameters.as_deref(), None);
        let found = match d.udt.as_ref() {
            Some(udt) => self.method_designated((owner, schema), named, udt)?,
            None => self.sql_routine_named((owner, schema), named)?,
        };
        let Some(found) = found.filter(SqlRoutineDesc::function) else {
            let msg = format!("[ISO/IEC 9075-2:2023 {cited}] {} designates no SQL-invoked function", qualify_written(d, schema));
            return Err(rule(msg).or_at(d.name.at));
        };
        Ok(found)
    }
}

/// The name `d` writes, qualified by schema `schema`.
fn qualify_written(d: &RoutineDesignator, schema: &str) -> QualifiedName {
    crate::binder::qualify(&d.name, schema)
}

/// The specific names of the routines and method specifications of
/// `host`: of a structured type its own attributes' observers and
/// mutators and its constructor function, of a distinct type its two
/// functions; and its method specifications.
fn host_specifics(host: MethodHost<'_>) -> Vec<&str> {
    let mut all: Vec<&str> = match host {
        MethodHost::Structured(desc) => {
            let own = desc.attributes.iter().filter(|a| a.defined_by == desc.name);
            own.flat_map(|a| [a.observer.specific.as_str(), a.mutator.specific.as_str()])
                .chain(desc.constructor.iter().map(|c| c.specific.as_str()))
                .collect()
        }
        MethodHost::Distinct(d) => vec![d.to_distinct.specific.as_str(), d.to_source.specific.as_str()],
    };
    all.extend(host.methods().iter().map(|m| m.specific.ident.as_str()));
    all
}
