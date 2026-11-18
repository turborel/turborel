//! [ISO/IEC 9075-2:2023 §11.52] `<attribute definition>`: an attribute's
//! descriptor, of CREATE TYPE and of ALTER TYPE ADD ATTRIBUTE, with its
//! observer and mutator [GR5, GR6].

use super::super::defaults::default_of;
use super::super::{State, rule};
use crate::ast::{AttributeDefinition, TypeName};
use crate::binder::qualify;
use crate::catalog::{AttributeDesc, MUTATOR_SUFFIX, OBSERVER_SUFFIX, QualifiedName, RoutineDesc, based_on, owner_of, sole};
use crate::error::{At, Result};
use crate::value::DataType;
use crate::value::types::{MAX_TYPE_NESTING, StructuredType, UdtName};

/// The type an attribute is defined in: its name and its proper
/// supertypes, the nearest first.
pub(super) type Owning<'a> = (&'a QualifiedName, &'a [UdtName]);

impl State {
    /// [ISO/IEC 9075-2:2023 §11.52, §11.51 SR8f, SR8j v] The descriptors of
    /// the attributes `defs` of the type `owning`, which has the attributes
    /// `existing` already, inherited ones among them.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a name is an attribute's already, a type is based on
    /// the type \[SR5\], a default does not suit its type, or a specific
    /// name an observer or mutator takes is taken [§11.51 SR9b vii]; -104
    /// if the owner of the type's schema holds no USAGE on a type named.
    pub(super) fn attributes_described(
        &self,
        owning: Owning<'_>,
        defs: &[AttributeDefinition],
        existing: &[AttributeDesc],
    ) -> Result<Vec<AttributeDesc>> {
        let mut made: Vec<AttributeDesc> = Vec::new();
        for def in defs {
            if let Some(other) = existing.iter().chain(&made).find(|a| a.name == def.name) {
                let cited = if other.defined_by == *owning.0 { "§11.51 SR8f" } else { "§11.51 SR8j v" };
                let msg = format!("[ISO/IEC 9075-2:2023 {cited}] type {} has an attribute {} already", owning.0, def.name);
                return Err(rule(msg).or_at(def.at));
            }
            made.push(self.attribute_described(owning, def).or_at(def.at)?);
        }
        Ok(made)
    }

    /// [ISO/IEC 9075-2:2023 §11.52 GR4] The descriptor of the attribute
    /// `def` of the type `owning`.
    fn attribute_described(&self, owning: Owning<'_>, def: &AttributeDefinition) -> Result<AttributeDesc> {
        let (qn, supertypes) = owning;
        let data_type = self.attribute_type(owning, &def.data_type)?;
        if self.based_deep(&data_type, owning, 0) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.52 SR5] attribute {} is of a type based on type {qn}", def.name)));
        }
        let collation = def.collate.as_ref().map(|c| qualify(c, &qn.auth));
        if collation.is_some() && !data_type.is_character() {
            let msg = format!("[ISO/IEC 9075-2:2023 §11.52 SR4] attribute {} of {data_type} takes no <collate clause>", def.name);
            return Err(rule(msg));
        }
        if let Some(option) = &def.default {
            // [§11.52 GR4d] as a column's default suits its type [§11.5].
            default_of(&data_type, option, 0)?;
        }
        let routine = |suffix: &str| {
            let specific = format!("{}_{}{suffix}", qn.ident, def.name);
            if self.specific_in_use(&QualifiedName { auth: qn.auth.clone(), ident: specific.clone() }) {
                let msg =
                    format!("[ISO/IEC 9075-2:2023 §11.51 SR9b vii] schema {} has a routine of specific name {specific} already", qn.auth);
                return Err(rule(msg));
            }
            Ok(RoutineDesc { name: def.name.clone(), specific, execute: Vec::new() })
        };
        let (observer, mutator) = (routine(OBSERVER_SUFFIX)?, routine(MUTATOR_SUFFIX)?);
        let _ = supertypes;
        Ok(AttributeDesc {
            name: def.name.clone(),
            data_type,
            default: def.default_text.clone(),
            collation,
            defined_by: qn.clone(),
            observer,
            mutator,
        })
    }

    /// [ISO/IEC 9075-2:2023 §11.52 SR3, §6.1 SR44] The declared type `t`
    /// writes, as the owner of the schema of `owning` names it: REF of the
    /// type itself the type, with no scope (Feature S026), as no typed
    /// table of it is there yet.
    fn attribute_type(&self, (qn, supertypes): Owning<'_>, t: &TypeName) -> Result<DataType> {
        if let TypeName::Reference(referenced, scope) = t
            && qualify(referenced, &qn.auth) == *qn
            && self.catalog.structured_type(qn).is_none()
        {
            if let Some(s) = scope {
                let msg =
                    format!("[ISO/IEC 9075-2:2023 §6.1 SR45] no typed table of type {qn} is there to be the scope, as {} is not", s.ident);
                return Err(rule(msg).or_at(s.at));
            }
            let itself = StructuredType { name: UdtName::new(&qn.auth, &qn.ident), supertypes: supertypes.to_vec() };
            return Ok(DataType::reference(itself, None));
        }
        if names_type(t, (qn, supertypes), 0) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.52 SR5] an attribute of type {qn} is of a type based on it")));
        }
        let owner = owner_of(&*self.catalog, &qn.auth);
        match self.resolve_type(t, (&qn.auth, sole(&*self.catalog, owner)))? {
            (_, Some(d)) => {
                Err(rule(format!("[ISO/IEC 9075-2:2023 §11.52 SR3] an attribute's type is a <data type>, not domain {}", d.name)))
            }
            (data_type, None) => Ok(data_type),
        }
    }

    /// [ISO/IEC 9075-2:2023 §4.2] Whether `t` is based on the type
    /// `owning`: on it or a type of its subtype family above it, or holds
    /// a structured type an attribute of which is so based, at most
    /// `MAX_TYPE_NESTING` attributes deep.
    pub(super) fn based_deep(&self, t: &DataType, owning: Owning<'_>, depth: usize) -> bool {
        let (qn, supertypes) = owning;
        if based_on(t, qn) {
            return true;
        }
        let mut held = Vec::new();
        structured_in(t, &mut held, 0);
        for s in &held {
            if supertypes.contains(&s.name) {
                return true;
            }
            let Some(desc) = self.catalog.structured_type(&crate::catalog::udt_qualified(&s.name)) else { continue };
            if depth < MAX_TYPE_NESTING && desc.attributes.iter().any(|a| self.based_deep(&a.data_type, owning, depth.saturating_add(1))) {
                return true;
            }
        }
        false
    }
}

/// The structured types `t` holds, not through a reference type, which
/// is based on nothing [§4.2]; types nest at most `MAX_TYPE_NESTING` deep.
fn structured_in(t: &DataType, into: &mut Vec<StructuredType>, depth: usize) {
    if depth > MAX_TYPE_NESTING {
        return;
    }
    let deeper = depth.saturating_add(1);
    match t {
        DataType::Structured(s) => into.push(StructuredType::clone(s)),
        DataType::Array(a) => structured_in(&a.element, into, deeper),
        DataType::Multiset(m) => structured_in(&m.element, into, deeper),
        DataType::Row(r) => r.fields.iter().for_each(|f| structured_in(&f.data_type, into, deeper)),
        DataType::Distinct(d) => structured_in(&d.source, into, deeper),
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Reference(_)
        | DataType::Json
        | DataType::Extension(_)
        | DataType::Xml(_) => {}
    }
}

/// [ISO/IEC 9075-2:2023 §11.52 SR5] Whether the type `t` writes names the
/// type `owning`, or one of its supertypes, as itself, an element or a
/// field: not as a reference type's referenced type.
fn names_type(t: &TypeName, (qn, supertypes): Owning<'_>, depth: usize) -> bool {
    if depth > MAX_TYPE_NESTING {
        return false;
    }
    let deeper = depth.saturating_add(1);
    match t {
        TypeName::Domain(name) => {
            let named = qualify(name, &qn.auth);
            named == *qn || supertypes.iter().any(|s| s.schema == named.auth && s.name == named.ident)
        }
        TypeName::Collection(element, _) => names_type(element, (qn, supertypes), deeper),
        TypeName::Row(fields) => fields.iter().any(|(_, f)| names_type(f, (qn, supertypes), deeper)),
        TypeName::Data(_) | TypeName::Reference(..) | TypeName::Extension { .. } | TypeName::XmlSchema(..) => false,
    }
}
