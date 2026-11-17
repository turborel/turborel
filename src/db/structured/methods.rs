//! [ISO/IEC 9075-2:2023 §11.51 SR9, GR1g, GR1h, §11.56, §11.57] A method
//! specification's descriptor, of CREATE TYPE and of ALTER TYPE ADD: its
//! kind, specific name and parameters checked, an original one against
//! the specifications it would conflict with, an overriding one against
//! the original one it overrides.

use super::super::{State, rule};
use super::{Signature, all_compatible, is_subtype, signatures, user_defined};
use crate::ast::{MethodKind, MethodSpecification, ParameterMode, ParameterType, TypeName};
use crate::binder::qualify;
use crate::catalog::{MethodHost, MethodSpecDesc, QualifiedName, SqlParameter, StructuredDesc, method_host, owner_of, sole, udt_qualified};
use crate::error::{At, Result};
use crate::value::DataType;

/// The name of the augmented parameter a method's subject is [§11.51 SR9b
/// viii, §11.60 SR14].
pub(super) const SELF_NAME: &str = "SELF";

/// The subtype family a method specification is checked in: the type, as
/// defined so far, with its own method specifications; its proper
/// supertypes, the nearest first; and, of ALTER TYPE, its proper subtypes.
pub(super) struct Family<'a> {
    /// The type: a structured type, or a distinct type of no family.
    pub(super) desc: MethodHost<'a>,
    /// Its proper supertypes.
    pub(super) supers: &'a [StructuredDesc],
    /// Its proper subtypes: none as it is created.
    pub(super) subs: &'a [StructuredDesc],
}

impl Family<'_> {
    /// The signatures of the method specifications of the types `of`.
    fn of(of: &[StructuredDesc]) -> Vec<Signature> {
        of.iter().flat_map(|t| signatures(MethodHost::Structured(t))).collect()
    }
}

impl State {
    /// [ISO/IEC 9075-2:2023 §11.51 SR9, GR1g, GR1h] The descriptor of the
    /// method specification `ms` of the type of `family`, created as
    /// `created`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for each Syntax Rule it breaks; -104 if the owner of
    /// the type's schema holds no USAGE on a type it names.
    pub(super) fn method_described(&self, family: &Family<'_>, ms: &MethodSpecification, created: u64) -> Result<MethodSpecDesc> {
        let qn = family.desc.name();
        kind_rules(ms, family.desc).or_at(ms.at)?;
        let specific = self.method_specific(family.desc, ms, created).or_at(ms.at)?;
        let parameters = self.method_parameters(qn, ms).or_at(ms.at)?;
        let owner = owner_of(&*self.catalog, &qn.auth);
        let returns = match self.resolve_type(&ms.returns, (&qn.auth, sole(&*self.catalog, owner))).or_at(ms.at)? {
            (_, Some(d)) => {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.60 SR16] a result's type is no domain, as {} is", d.name)).or_at(ms.at));
            }
            (t, None) => t,
        };
        if ms.self_as_result && returns != family.desc.data_type() {
            let msg =
                format!("[ISO/IEC 9075-2:2023 §11.51 SR9b xi] method {} is SELF AS RESULT, so it returns {qn}, not {returns}", ms.name);
            return Err(rule(msg).or_at(ms.at));
        }
        let types: Vec<DataType> = parameters.iter().map(|p| p.data_type.clone()).collect();
        let sig = Signature { name: ms.name.clone(), kind: ms.kind, types, implicit: false };
        duplicate(family.desc, &sig).or_at(ms.at)?;
        self.function_conflict(family, &sig).or_at(ms.at)?;
        let mut desc = MethodSpecDesc {
            name: ms.name.clone(),
            specific,
            kind: ms.kind,
            overrides: None,
            parameters,
            returns,
            self_as_result: ms.self_as_result,
            deterministic: ms.characteristics.deterministic,
            access: ms.characteristics.access,
            null_call: ms.characteristics.null_call,
            created,
        };
        if ms.overriding {
            overriding(family, ms, &mut desc).or_at(ms.at)?;
        } else {
            original_conflict(family, &sig).or_at(ms.at)?;
        }
        Ok(desc)
    }

    /// [ISO/IEC 9075-2:2023 §11.51 SR9b v to vii] The specific name of
    /// `ms`, a method specification of `desc`, created as `created`: the
    /// one written, in the type's schema; else its type's name, its own
    /// and its timestamp (UV102), then a count if that is taken.
    fn method_specific(&self, desc: MethodHost<'_>, ms: &MethodSpecification, created: u64) -> Result<QualifiedName> {
        let qn = desc.name();
        let taken = |s: &QualifiedName| self.specific_in_use(s) || super::host_specifics(desc).contains(&s.ident.as_str());
        let Some(written) = &ms.specific else {
            let mut specific = QualifiedName { auth: qn.auth.clone(), ident: format!("{}_{}_{created}", qn.ident, ms.name) };
            let mut n: u64 = 0;
            while taken(&specific) {
                n = n.saturating_add(1);
                specific.ident = format!("{}_{}_{created}_{n}", qn.ident, ms.name);
            }
            return Ok(specific);
        };
        let specific = qualify(written, &qn.auth);
        if specific.auth != qn.auth {
            let msg = format!("[ISO/IEC 9075-2:2023 §11.51 SR9b vi] specific method name {specific} is not of schema {}", qn.auth);
            return Err(rule(msg).or_at(written.at));
        }
        if taken(&specific) {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §11.51 SR9b vii] schema {} has a routine or method specification of specific name {} already",
                specific.auth, specific.ident
            );
            return Err(rule(msg).or_at(written.at));
        }
        Ok(specific)
    }

    /// [ISO/IEC 9075-2:2023 §11.51 SR9b viii, SR9b ix] The unaugmented
    /// parameters of `ms`, of type `qn`: named apart, none SELF, of no mode,
    /// each of a `<data type>` no row type.
    fn method_parameters(&self, qn: &QualifiedName, ms: &MethodSpecification) -> Result<Vec<SqlParameter>> {
        let owner = owner_of(&*self.catalog, &qn.auth);
        let mut made: Vec<SqlParameter> = Vec::new();
        for p in &ms.parameters {
            if p.name == SELF_NAME || made.iter().any(|m| m.name == p.name) {
                let msg =
                    format!("[ISO/IEC 9075-2:2023 §11.51 SR9b viii] parameter {} of method {} is SELF or named twice", p.name, ms.name);
                return Err(rule(msg).or_at(p.at));
            }
            if p.mode.is_some() {
                let msg =
                    format!("[ISO/IEC 9075-2:2023 §11.51 SR9b ix] parameter {} of a method specification has no <parameter mode>", p.name);
                return Err(rule(msg).or_at(p.at));
            }
            let ParameterType::Data(written) = &p.data_type else {
                let msg = format!("[ISO/IEC 9075-2:2023 §11.60 SR10a] parameter {} of a method is of a <data type>", p.name);
                return Err(rule(msg).or_at(p.at));
            };
            if matches!(written, TypeName::Row(_) | TypeName::Data(DataType::Row(_))) {
                let msg = format!("[ISO/IEC 9075-2:2023 §11.51 SR9b ix] parameter {} of a method is of no row type", p.name);
                return Err(rule(msg).or_at(p.at));
            }
            let data_type = match self.resolve_type(written, (&qn.auth, sole(&*self.catalog, owner))).or_at(p.at)? {
                (_, Some(d)) => {
                    return Err(
                        rule(format!("[ISO/IEC 9075-2:2023 §11.60 SR16] a parameter's type is no domain, as {} is", d.name)).or_at(p.at)
                    );
                }
                (t, None) => t,
            };
            made.push(SqlParameter { name: p.name.clone(), data_type, mode: ParameterMode::In, generic: None });
        }
        Ok(made)
    }

    /// [ISO/IEC 9075-2:2023 §11.60 SR22] That the regular function `name`
    /// of parameter types `types` is told apart from each instance method
    /// of the user-defined type of its first parameter, the type's own or a
    /// supertype's: none of its name, in the type's schema, as many
    /// parameters as its augmented list, the others' types compatible.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if one is not told apart from it.
    pub(in crate::db) fn told_apart_from_methods(&self, name: &QualifiedName, types: &[DataType]) -> Result<()> {
        let Some((first, rest)) = types.split_first() else { return Ok(()) };
        let udt = if let DataType::Structured(s) = first {
            udt_qualified(&s.name)
        } else if let DataType::Distinct(d) = first {
            QualifiedName { auth: d.schema.clone(), ident: d.name.clone() }
        } else {
            return Ok(());
        };
        let Some(host) = method_host(&*self.catalog, &udt) else { return Ok(()) };
        let supers = host.supertypes().iter().filter_map(|s| method_host(&*self.catalog, &udt_qualified(s)));
        for h in std::iter::once(host).chain(supers) {
            let alike = |m: &&MethodSpecDesc| {
                let own: Vec<DataType> = m.parameters.iter().map(|p| p.data_type.clone()).collect();
                m.kind == MethodKind::Instance && m.name == name.ident && h.name().auth == name.auth && all_compatible(rest, &own)
            };
            if let Some(m) = h.methods().iter().find(alike) {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §11.60 SR22] function {name} would not be told apart from method {} of type {}",
                    m.name,
                    h.name()
                )));
            }
        }
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §11.51 SR9b xiv 11, SR9b xiv 12, SR9b xv 9,
    /// §11.56 SR18, SR19, §11.57 SR17] That no regular function of the name
    /// of `sig` would be told apart from it by its subject alone: one whose
    /// first parameter is of a type of the family of the method's type, of
    /// one more parameter than a static method has, as many as an instance
    /// method's augmented list, the others' types compatible.
    fn function_conflict(&self, family: &Family<'_>, sig: &Signature) -> Result<()> {
        if sig.kind == MethodKind::Constructor {
            return Ok(());
        }
        let own = family.desc.data_type();
        let related = |t: &DataType| is_subtype(&own, t) || (!family.subs.is_empty() && is_subtype(t, &own));
        let conflicts =
            self.catalog.routines().into_iter().filter(|r| r.method.is_none() && r.function() && r.name.ident == sig.name).find(|r| {
                let types = r.parameter_types();
                types.split_first().is_some_and(|(first, rest)| related(first) && all_compatible(rest, &sig.types))
            });
        match conflicts {
            Some(f) => Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.51 SR9b xiv 11] method {} of type {} would not be told apart from function {}",
                sig.name,
                family.desc.name(),
                f.specific
            ))),
            None => Ok(()),
        }
    }
}

/// [ISO/IEC 9075-2:2023 §11.51 SR9b ii to iv, SR9b xv 1] That a STATIC
/// method specification is neither SELF AS RESULT nor OVERRIDING; a
/// CONSTRUCTOR one SELF AS RESULT, not OVERRIDING, named as its type,
/// which it returns (its result checked with SR9b xi), and of a structured
/// type; and an OVERRIDING one of a structured type, a distinct type
/// having no supertype whose method it could override.
fn kind_rules(ms: &MethodSpecification, host: MethodHost<'_>) -> Result<()> {
    let qn = host.name();
    let distinct = host.structured().is_none();
    let broken = match ms.kind {
        MethodKind::Constructor if distinct => Some("SR9b iv] a CONSTRUCTOR method is of a structured type"),
        MethodKind::Instance if distinct && ms.overriding => Some("SR9b xv 1] an OVERRIDING method is of a structured type"),
        MethodKind::Static if ms.self_as_result || ms.overriding => {
            Some("SR9b ii] a STATIC method is neither SELF AS RESULT nor OVERRIDING")
        }
        MethodKind::Constructor if !ms.self_as_result || ms.overriding => {
            Some("SR9b iii] a CONSTRUCTOR method is SELF AS RESULT and not OVERRIDING")
        }
        MethodKind::Constructor if ms.name != qn.ident => Some("SR9b iii] a CONSTRUCTOR method is named as its type"),
        MethodKind::Static | MethodKind::Constructor | MethodKind::Instance => None,
    };
    match broken {
        Some(cited) => Err(rule(format!("[ISO/IEC 9075-2:2023 §11.51 {cited}, as method {} is not", ms.name))),
        None => Ok(()),
    }
}

/// [ISO/IEC 9075-2:2023 §11.51 SR9b xii, §11.57 SR18] That `desc` has no
/// method specification of its own of the name of `sig`, both constructors
/// or neither, of compatible parameters.
fn duplicate(desc: MethodHost<'_>, sig: &Signature) -> Result<()> {
    let constructor = |k: MethodKind| k == MethodKind::Constructor;
    let same = |o: &Signature| o.name == sig.name && constructor(o.kind) == constructor(sig.kind) && all_compatible(&o.types, &sig.types);
    match signatures(desc).iter().find(|o| same(o)) {
        Some(o) if o.implicit => Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §11.51 SR9b xii] method {} of type {} is not told apart from the observer or mutator of its attribute {}",
            sig.name,
            desc.name(),
            o.name
        ))),
        Some(_) => Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §11.51 SR9b xii] type {} has a method {} of its parameters already",
            desc.name(),
            sig.name
        ))),
        None => Ok(()),
    }
}

/// [ISO/IEC 9075-2:2023 §11.51 SR9b xiv 7, SR9b xiv 8, §11.56 SR15,
/// SR16] That no method specification of a proper supertype, nor of
/// ALTER TYPE of a proper subtype, conflicts with the original one `sig`:
/// of its name and compatible parameters, not both static; a constructor
/// conflicts with none (but its own type's, a duplicate).
fn original_conflict(family: &Family<'_>, sig: &Signature) -> Result<()> {
    if sig.kind == MethodKind::Constructor {
        return Ok(());
    }
    let statics = |k: MethodKind| k == MethodKind::Static;
    let conflicting = |o: &Signature| {
        o.name == sig.name
            && o.kind != MethodKind::Constructor
            && !(statics(o.kind) && statics(sig.kind))
            && all_compatible(&o.types, &sig.types)
    };
    let others = Family::of(family.supers).into_iter().chain(Family::of(family.subs));
    match others.into_iter().find(|o| conflicting(o)) {
        Some(_) => Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §11.51 SR9b xiv 8, §11.56 SR16] a supertype or subtype of {} has a method {} of its parameters: write OVERRIDING",
            family.desc.name(),
            sig.name
        ))),
        None => Ok(()),
    }
}

/// [ISO/IEC 9075-2:2023 §11.51 SR9b xv, GR1h, §11.57 SR4, SR9 to SR13]
/// The overriding specification `desc`, as written `ms`, made of the one
/// original specification of a proper supertype it overrides: its
/// parameters named alike, its result as the rules allow, the rest copied.
fn overriding(family: &Family<'_>, ms: &MethodSpecification, desc: &mut MethodSpecDesc) -> Result<()> {
    if family.supers.iter().chain(family.subs).any(|t| t.name.ident == ms.name) {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.57 SR4] overriding method {} is named as a type of its family", ms.name)));
    }
    let types: Vec<DataType> = desc.parameters.iter().map(|p| p.data_type.clone()).collect();
    let same = |m: &MethodSpecDesc| m.name == desc.name && m.parameters.iter().map(|p| &p.data_type).eq(types.iter());
    let originals: Vec<&MethodSpecDesc> = family
        .supers
        .iter()
        .flat_map(|t| &t.methods)
        .filter(|m| same(m) && m.overrides.is_none() && m.kind == MethodKind::Instance)
        .collect();
    let [coms] = originals.as_slice() else {
        let implicit = Family::of(family.supers).into_iter().any(|o| o.implicit && o.name == desc.name && o.types == types);
        let cited = if implicit {
            "SR9b xv 5] an observer or mutator is overridden by no method"
        } else {
            "SR9b xv 3] an overriding method overrides one original instance method of a proper supertype of its name and parameter types"
        };
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.51 {cited}, as {} does not", desc.name)));
    };
    if !coms.parameters.iter().map(|p| &p.name).eq(desc.parameters.iter().map(|p| &p.name)) {
        let msg = format!(
            "[ISO/IEC 9075-2:2023 §11.51 SR9b xv 6] overriding method {} names its parameters as {} does",
            desc.name, coms.specific
        );
        return Err(rule(msg));
    }
    overriding_result(family, coms, desc)?;
    desc.overrides = Some(coms.specific.clone());
    (desc.self_as_result, desc.deterministic, desc.access, desc.null_call) =
        (coms.self_as_result, coms.deterministic, coms.access, coms.null_call);
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §11.51 SR9b xv 7, §11.57 SR13] That the result of
/// the overriding `desc` is, of an original `coms` of a user-defined
/// result, a subtype of each overriding one's of a proper supertype and a
/// supertype of each of a proper subtype's; else `coms`'s.
fn overriding_result(family: &Family<'_>, coms: &MethodSpecDesc, desc: &MethodSpecDesc) -> Result<()> {
    if !user_defined(&coms.returns) {
        if desc.returns == coms.returns {
            return Ok(());
        }
        let msg = format!(
            "[ISO/IEC 9075-2:2023 §11.51 SR9b xv 7] overriding method {} returns {}, as {} does",
            desc.name, coms.returns, coms.specific
        );
        return Err(rule(msg));
    }
    let of = |t: &StructuredDesc| {
        t.methods.iter().filter(|m| m.overrides.as_ref() == Some(&coms.specific)).map(|m| m.returns.clone()).collect::<Vec<_>>()
    };
    let above = std::iter::once(coms.returns.clone()).chain(family.supers.iter().flat_map(of));
    let below: Vec<DataType> = family.subs.iter().flat_map(of).collect();
    let fits = above.into_iter().all(|r| is_subtype(&desc.returns, &r)) && below.iter().all(|r| is_subtype(r, &desc.returns));
    if fits {
        return Ok(());
    }
    let msg = format!(
        "[ISO/IEC 9075-2:2023 §11.51 SR9b xv 7] overriding method {} returns a subtype of what the method it overrides returns",
        desc.name
    );
    Err(rule(msg))
}
