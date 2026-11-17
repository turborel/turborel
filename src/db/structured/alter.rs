//! [ISO/IEC 9075-2:2023 §11.53 to §11.58] ALTER TYPE of a structured
//! type: ADD and DROP ATTRIBUTE, each refused while a base table's column
//! is of a type of its subtype family (RULES.md D81), and ADD and DROP of
//! a method specification.

use std::sync::Arc;

use super::super::definitions::Defined;
use super::super::{State, rule};
use super::methods::Family;
use super::{all_compatible, is_subtype, names_type};
use crate::ast::{AlterType, AlterTypeAction, AttributeDefinition, MethodDesignator, MethodKind};
use crate::binder::qualify;
use crate::catalog::{MethodHost, QualifiedName, StructuredDesc, owner_of, sole};
use crate::error::{At, Code, Result, SqlError};
use crate::txn::LockMode;
use crate::value::types::UdtName;

impl State {
    /// [ISO/IEC 9075-2:2023 §11.53] ALTER TYPE `a`, its names in schema
    /// `schema` if it names none: the type's descriptor, and those of its
    /// proper subtypes, changed \[GR1\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it names no user-defined type \[SR2\], an action of a
    /// structured type alone of a distinct one \[SR4\], or for each
    /// Syntax Rule its action breaks; -104 if the owner of its schema holds
    /// no UNDER on a proper supertype \[AR2\].
    pub(in crate::db) fn alter_user_type(&mut self, schema: &str, a: &AlterType) -> Result<Defined> {
        let qn = qualify(&a.name, schema);
        if let Some(d) = self.catalog.user_type(&qn).cloned() {
            self.alter_distinct_type(d, &a.action)?;
            return Ok(Defined {
                locks: vec![(qn.clone(), LockMode::Exclusive), (QualifiedName::of_schema(&qn.auth), LockMode::Shared)],
                ..Defined::default()
            });
        }
        let desc = self.structured_named(&qn, "§11.53 SR2").or_at(a.name.at)?;
        let owner = owner_of(&*self.catalog, &qn.auth).to_owned();
        for s in self.proper_supertypes(&desc) {
            if !self.under_held(&s, sole(&*self.catalog, &owner)).0 {
                let msg = format!("[ISO/IEC 9075-2:2023 §11.53 AR2] {owner} holds no UNDER on type {}, a supertype of {qn}", s.name);
                return Err(SqlError::new(Code::Privilege, msg).or_at(a.name.at));
            }
        }
        let subs = self.proper_subtypes(&qn);
        match &a.action {
            AlterTypeAction::AddAttribute(def) => self.add_attribute(&desc, &subs, def).or_at(def.at)?,
            AlterTypeAction::DropAttribute(name, at) => self.drop_attribute(&desc, &subs, name).or_at(*at)?,
            AlterTypeAction::AddMethod(ms) => {
                let supers = self.proper_supertypes(&desc);
                let made = self.method_described(
                    &Family { desc: MethodHost::Structured(&desc), supers: &supers, subs: &subs },
                    ms,
                    self.next_created(),
                )?;
                if let Some(original) = &made.overrides {
                    self.methods_overridden(&qn, (original, &made.specific));
                }
                let mut changed = desc.clone();
                changed.methods.push(made);
                self.put_structured(changed);
            }
            AlterTypeAction::DropMethod(m) => self.drop_method_spec(&desc, &subs, m).or_at(m.name.1)?,
        }
        let locks = subs.into_iter().map(|s| (s.name, LockMode::Exclusive)).chain([(QualifiedName::of_schema(&qn.auth), LockMode::Shared)]);
        Ok(Defined { locks: locks.collect(), ..Defined::default() })
    }

    /// [ISO/IEC 9075-2:2023 §11.54 SR3 to SR5, §11.55 SR4 to SR7] (RULES.md
    /// D81) That no column of a base table, nor typed table, is of a type of
    /// the subtype family of `desc`, or of a reference type of one, or holds
    /// one, so no stored value changes its attributes.
    fn family_unstored(&self, desc: &StructuredDesc, cited: &str) -> Result<()> {
        let maximal = desc.maximal();
        let members: Vec<QualifiedName> =
            std::iter::once(maximal.clone()).chain(self.proper_subtypes(&maximal).into_iter().map(|s| s.name)).collect();
        if let Some(t) = self.typed_of(&members).first() {
            let msg = format!("[ISO/IEC 9075-2:2023 {cited}] {t} is a typed table of a type of the family of {}", desc.name);
            return Err(rule(msg));
        }
        for t in self.catalog.tables() {
            if let Some(c) = t.columns.iter().find(|c| members.iter().any(|m| names_type(&c.data_type, m))) {
                let msg = format!(
                    "[ISO/IEC 9075-2:2023 {cited}] (RULES.md D81) column {}.{} is of a type of the family of {}, whose stored values keep their attributes",
                    t.name, c.name, desc.name
                );
                return Err(rule(msg));
            }
        }
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §11.54] ADD ATTRIBUTE `def` to `desc`, of proper
    /// subtypes `subs`: after its own, and inherited by each subtype after
    /// what it inherits of `desc` [GR2 to GR4].
    fn add_attribute(&mut self, desc: &StructuredDesc, subs: &[StructuredDesc], def: &AttributeDefinition) -> Result<()> {
        self.family_unstored(desc, "§11.54 SR4")?;
        if let Some(s) = subs.iter().find(|s| s.attribute(&def.name).is_some()) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.51 SR8j v] subtype {} has an attribute {} already", s.name, def.name)));
        }
        let supertypes: Vec<UdtName> = desc.supertypes.clone();
        let mut made = self.attributes_described((&desc.name, &supertypes), std::slice::from_ref(def), &desc.attributes)?;
        let Some(added) = made.pop() else { return Ok(()) };
        self.attribute_routines_free(desc, subs, &added)?;
        let at = desc.attributes.len();
        let mut catalog = self.catalog.duplicate();
        let mut changed = desc.clone();
        changed.attributes.push(added.clone());
        catalog.put_structured_type(changed);
        for s in subs {
            let mut sub = s.clone();
            sub.attributes.insert(at.min(sub.attributes.len()), added.clone());
            catalog.put_structured_type(sub);
        }
        self.catalog = Arc::from(catalog);
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §11.54 SR6, SR7] That no routine and no method
    /// specification of the family of `desc` would not be told apart from
    /// the observer or mutator of the attribute `added`: one of its name,
    /// of one parameter of a subtype or supertype of the type, or of two,
    /// the second compatible with the attribute's type.
    fn attribute_routines_free(&self, desc: &StructuredDesc, subs: &[StructuredDesc], added: &crate::catalog::AttributeDesc) -> Result<()> {
        let own = desc.data_type();
        let related = |t: &crate::value::DataType| is_subtype(&own, t) || is_subtype(t, &own);
        let attribute = std::slice::from_ref(&added.data_type);
        let routine = self.catalog.routines().into_iter().filter(|r| r.name.ident == added.name).find(|r| {
            let types = r.parameter_types();
            types.split_first().is_some_and(|(first, rest)| related(first) && (rest.is_empty() || all_compatible(rest, attribute)))
        });
        let family: Vec<StructuredDesc> =
            self.proper_supertypes(desc).into_iter().chain(std::iter::once(desc.clone())).chain(subs.iter().cloned()).collect();
        let spec = family.iter().flat_map(|t| super::signatures(MethodHost::Structured(t))).find(|s| {
            s.name == added.name && s.kind == MethodKind::Instance && (s.types.is_empty() || all_compatible(&s.types, attribute))
        });
        if routine.is_some() || spec.is_some() {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §11.54 SR6, SR7] a routine or method {} of the family of {} takes the place of the attribute's observer or mutator",
                added.name, desc.name
            );
            return Err(rule(msg));
        }
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §11.55] DROP ATTRIBUTE `name` RESTRICT of
    /// `desc`, of proper subtypes `subs`: from it and each subtype, with its
    /// observer and mutator [GR1 to GR4].
    fn drop_attribute(&mut self, desc: &StructuredDesc, subs: &[StructuredDesc], name: &str) -> Result<()> {
        let Some((_, a)) = desc.attribute(name).filter(|(_, a)| a.defined_by == desc.name) else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.55 SR3] {name} is no attribute type {} defines itself", desc.name)));
        };
        if desc.attributes.len() == 1 {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.55 SR3] {name} is the only attribute of type {}", desc.name)));
        }
        if let crate::catalog::RefRepresentation::Derived(names) = &desc.reference
            && names.iter().any(|n| n == name)
        {
            return Err(rule(format!(
                "[DESIGN] attribute {name} is one the references of type {} are derived from (RULES.md D81)",
                desc.name
            )));
        }
        self.family_unstored(desc, "§11.55 SR6")?;
        let routines =
            [&a.observer.specific, &a.mutator.specific].map(|s| QualifiedName { auth: desc.name.auth.clone(), ident: s.clone() });
        if self.catalog.casts().iter().any(|c| routines.contains(&c.function)) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.55 SR8b] a cast's function is the observer or mutator of {name}")));
        }
        if self.catalog.orderings().iter().any(|o| o.function().is_some_and(|f| routines.contains(f))) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.55 SR8c] an ordering's function is the observer or mutator of {name}")));
        }
        let mut after = self.clone();
        let mut catalog = self.catalog.duplicate();
        for t in std::iter::once(desc).chain(subs) {
            let mut changed = t.clone();
            changed.attributes.retain(|x| !(x.name == name && x.defined_by == desc.name));
            catalog.put_structured_type(changed);
        }
        after.catalog = Arc::from(catalog);
        let abandoned = self.abandoned_in(&after, &|_| true);
        if !abandoned.is_empty() {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.55 SR8a] {} invokes the observer or mutator of attribute {name}",
                abandoned.first()
            )));
        }
        self.catalog = after.catalog;
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §11.53 SR4, §11.56, §11.58] ALTER TYPE of the
    /// distinct type `d`: an original method specification added or one
    /// dropped; an attribute added or dropped, or an overriding method
    /// specification added, is of a structured type alone \[SR4\].
    fn alter_distinct_type(&mut self, mut d: crate::catalog::UserTypeDesc, action: &AlterTypeAction) -> Result<()> {
        match action {
            AlterTypeAction::AddMethod(ms) if !ms.overriding => {
                let made =
                    self.method_described(&Family { desc: MethodHost::Distinct(&d), supers: &[], subs: &[] }, ms, self.next_created())?;
                d.methods.push(made);
            }
            AlterTypeAction::DropMethod(m) => {
                let gone = self.method_spec_designated(MethodHost::Distinct(&d), m).or_at(m.name.1)?.specific.clone();
                d.methods.retain(|s| s.specific != gone);
            }
            AlterTypeAction::AddMethod(_) | AlterTypeAction::AddAttribute(_) | AlterTypeAction::DropAttribute(..) => {
                let msg = format!(
                    "[ISO/IEC 9075-2:2023 §11.53 SR4] {} is a distinct type, so it is given or loses no attribute nor overriding method",
                    d.name
                );
                return Err(rule(msg));
            }
        }
        let mut catalog = self.catalog.duplicate();
        catalog.put_user_type(d);
        self.catalog = Arc::from(catalog);
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §11.58 SR3, SR4] The method specification of
    /// `host` the designator `m` designates, of no body.
    fn method_spec_designated<'h>(&self, host: MethodHost<'h>, m: &MethodDesignator) -> Result<&'h crate::catalog::MethodSpecDesc> {
        let qn = host.name();
        let owner = owner_of(&*self.catalog, &qn.auth).to_owned();
        let types =
            m.parameters.iter().map(|t| self.data_type_of(t, (&qn.auth, sole(&*self.catalog, &owner)))).collect::<Result<Vec<_>>>()?;
        let kind_of = |k: MethodKind| k == m.kind;
        let found: Vec<&crate::catalog::MethodSpecDesc> = host
            .methods()
            .iter()
            .filter(|s| s.name == m.name.0 && kind_of(s.kind) && s.parameters.iter().map(|p| &p.data_type).eq(types.iter()))
            .collect();
        let [spec] = found.as_slice() else {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §11.58 SR3c] type {qn} has not exactly one {} method {} of those parameters",
                m.kind.name(),
                m.name.0
            );
            return Err(rule(msg));
        };
        if self.catalog.routine(&spec.specific).is_some() {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §11.58 SR4] method {} has a body, of specific name {}: drop it first",
                spec.name, spec.specific
            );
            return Err(rule(msg));
        }
        Ok(spec)
    }

    /// [ISO/IEC 9075-2:2023 §11.58] DROP `m` RESTRICT, a method
    /// specification of `desc`, of proper subtypes `subs` [GR1 to GR3].
    fn drop_method_spec(&mut self, desc: &StructuredDesc, subs: &[StructuredDesc], m: &MethodDesignator) -> Result<()> {
        let spec = self.method_spec_designated(MethodHost::Structured(desc), m)?;
        if let Some(s) = subs.iter().find(|s| s.methods.iter().any(|o| o.overrides.as_ref() == Some(&spec.specific))) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.58 SR4a iii 1] type {} overrides method {}", s.name, spec.name)));
        }
        let gone = spec.specific.clone();
        let mut changed = desc.clone();
        changed.methods.retain(|s| s.specific != gone);
        self.put_structured(changed);
        Ok(())
    }
}
