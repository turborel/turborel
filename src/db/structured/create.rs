//! [ISO/IEC 9075-2:2023 §11.51] CREATE TYPE of a structured type: its
//! options, its supertype, its attributes, inherited first, its reference
//! representation, its method specifications and its constructor function
//! [GR1, GR3, GR4, GR5].

use std::iter;
use std::sync::Arc;

use super::super::definitions::Defined;
use super::super::user_types::{options_once, type_name_of};
use super::super::{State, rule};
use super::methods::Family;
use crate::ast::{RefRepresentation as Written, TypeDefinition, TypeOption};
use crate::binder::{alike, qualify};
use crate::catalog::{
    CONSTRUCTOR_SUFFIX, MAX_HIERARCHY_DEPTH, MethodHost, QualifiedName, RefRepresentation, RoutineDesc, Stamps, StructuredDesc, owner_of,
    sole, types_in,
};
use crate::error::{At, Code, Result, SqlError};
use crate::txn::LockMode;
use crate::value::types::{REF_OCTETS, UdtName};
use crate::value::{CharUnits, DataType};

/// The octets a character takes at most, as UTF-8 keeps it: what a
/// character string of a user-defined representation is measured in.
const OCTETS_PER_CHARACTER: u32 = 4;

/// The `<user-defined type option>`s of a structured type's definition.
#[derive(Default)]
struct Options {
    /// NOT INSTANTIABLE was written.
    not_instantiable: bool,
    /// The `<finality>`: FINAL if true.
    finality: Option<bool>,
    /// The `<reference type specification>`, and where it is written.
    reference: Option<(Written, usize)>,
    /// The `<cast to ref>`'s and `<cast to type>`'s identifiers.
    casts: (Option<String>, Option<String>),
}

/// [ISO/IEC 9075-2:2023 §11.51 SR4, SR6, SR8a, SR8d, SR8e] The options of
/// `d`, a structured type's definition: none a distinct type's, a
/// `<finality>` written, FINAL not NOT INSTANTIABLE.
fn options_of(d: &TypeDefinition) -> Result<Options> {
    options_once(&d.options)?;
    let mut options = Options::default();
    for (o, at) in &d.options {
        match o {
            TypeOption::Instantiable(i) => options.not_instantiable = !i,
            TypeOption::Final(f) => options.finality = Some(*f),
            TypeOption::Reference(r) => options.reference = Some((r.clone(), *at)),
            TypeOption::CastToRef(n) => options.casts.0 = Some(n.clone()),
            TypeOption::CastToType(n) => options.casts.1 = Some(n.clone()),
            TypeOption::CastToDistinct(_) | TypeOption::CastToSource(_) => {
                let msg = "[ISO/IEC 9075-2:2023 §11.51 SR8a] a structured type has no <cast to distinct> nor <cast to source>";
                return Err(rule(msg).or_at(*at));
            }
        }
    }
    let Some(is_final) = options.finality else {
        return Err(rule("[ISO/IEC 9075-2:2023 §11.51 SR8e] a structured type's definition writes FINAL or NOT FINAL").or_at(d.name.at));
    };
    if is_final && options.not_instantiable {
        return Err(rule("[ISO/IEC 9075-2:2023 §11.51 SR6] a FINAL type is INSTANTIABLE").or_at(d.name.at));
    }
    Ok(options)
}

impl State {
    /// [ISO/IEC 9075-2:2023 §11.51] CREATE TYPE `d` of the structured type
    /// `qn`, in a schema that holds no type or domain of its name: its
    /// descriptor recorded, and its routines' [GR1, GR3]; USAGE and UNDER
    /// on it its schema's owner's, of `_SYSTEM` [GR4, GR5].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for each Syntax Rule it breaks; -104 if the owner of
    /// its schema holds no UNDER on its supertype \[AR2\], or no USAGE on a
    /// type it names.
    pub(in crate::db) fn create_structured_type(&mut self, qn: &QualifiedName, d: &TypeDefinition) -> Result<Defined> {
        let options = options_of(d)?;
        let owner = owner_of(&*self.catalog, &qn.auth).to_owned();
        let supertype = self.supertype_of(qn, d, (&owner, &options))?;
        let supertypes: Vec<UdtName> = supertype
            .as_ref()
            .map(|s| iter::once(UdtName::new(&s.name.auth, &s.name.ident)).chain(s.supertypes.iter().cloned()).collect())
            .unwrap_or_default();
        let inherited = supertype.as_ref().map(|s| s.attributes.clone()).unwrap_or_default();
        let own = self.attributes_described((qn, &supertypes), d.members.as_deref().unwrap_or_default(), &inherited)?;
        let mut desc = StructuredDesc {
            name: qn.clone(),
            supertype: supertype.as_ref().map(|s| s.name.clone()),
            supertypes,
            instantiable: !options.not_instantiable,
            is_final: options.finality == Some(true),
            attributes: inherited.into_iter().chain(own).collect(),
            methods: Vec::new(),
            reference: RefRepresentation::System,
            ref_casts: None,
            constructor: None,
            usage: Vec::new(),
            under: Vec::new(),
        };
        (desc.reference, desc.ref_casts) = self.reference_of(&desc, supertype.as_ref(), &options)?;
        // [ISO/IEC 9075-2:2023 §11.51 SR9b] a method specification names
        // the type it is of, as its result or a parameter's type: the type
        // is there, of no methods, while they are described, and is not if
        // one is refused.
        let before = Arc::clone(&self.catalog);
        self.put_structured(desc.clone());
        let methods = self.methods_described(&desc, d);
        self.catalog = before;
        desc.methods = methods?;
        if desc.instantiable {
            desc.constructor = Some(self.constructor_of(qn).or_at(d.name.at)?);
        }
        let mut read: Vec<QualifiedName> = desc.supertype.iter().cloned().collect();
        desc.attributes.iter().for_each(|a| types_in(&a.data_type, &mut read));
        read.retain(|r| r != qn);
        self.put_structured(desc);
        let locks = read.into_iter().chain([QualifiedName::of_schema(&qn.auth)]).map(|name| (name, LockMode::Shared)).collect();
        Ok(Defined { locks, ..Defined::default() })
    }

    /// [ISO/IEC 9075-2:2023 §11.51 SR9b] The descriptors of the method
    /// specifications of `d`, of the type `desc`.
    fn methods_described(&self, desc: &StructuredDesc, d: &TypeDefinition) -> Result<Vec<crate::catalog::MethodSpecDesc>> {
        let supers = self.proper_supertypes(desc);
        let created = self.next_created();
        // Each specification checked against those before it [SR9b xii].
        let mut so_far = desc.clone();
        for (ms, n) in d.methods.iter().zip(0_u64..) {
            let family = Family { desc: MethodHost::Structured(&so_far), supers: &supers, subs: &[] };
            let described = self.method_described(&family, ms, created.saturating_add(n))?;
            so_far.methods.push(described);
        }
        Ok(so_far.methods)
    }

    /// [ISO/IEC 9075-2:2023 §11.51 SR9b, GR1g] The descriptors of the
    /// method specifications of `d`, of the distinct type `desc`, each
    /// checked against those before it [SR9b xii]. The type is there, of no
    /// methods, while they are described, so one may name it, and is not
    /// if one is refused.
    pub(in crate::db) fn distinct_methods_described(
        &mut self,
        desc: &crate::catalog::UserTypeDesc,
        d: &TypeDefinition,
    ) -> Result<Vec<crate::catalog::MethodSpecDesc>> {
        if d.methods.is_empty() {
            return Ok(Vec::new());
        }
        let before = Arc::clone(&self.catalog);
        let mut catalog = self.catalog.duplicate();
        catalog.put_user_type(desc.clone());
        self.catalog = Arc::from(catalog);
        let created = self.next_created();
        let mut so_far = desc.clone();
        let mut described = Ok(());
        for (ms, n) in d.methods.iter().zip(0_u64..) {
            let family = Family { desc: MethodHost::Distinct(&so_far), supers: &[], subs: &[] };
            match self.method_described(&family, ms, created.saturating_add(n)) {
                Ok(m) => so_far.methods.push(m),
                Err(e) => {
                    described = Err(e);
                    break;
                }
            }
        }
        self.catalog = before;
        described.map(|()| so_far.methods)
    }

    /// [ISO/IEC 9075-2:2023 §11.51 SR8b, SR8j i to iii, AR2] The direct
    /// supertype of the type `qn` that `d` defines, if it names one: a
    /// structured type not FINAL, on which the owner `owner` of `qn`'s
    /// schema holds UNDER, the type having no `<reference type
    /// specification>` of its own.
    fn supertype_of(&self, qn: &QualifiedName, d: &TypeDefinition, (owner, options): (&str, &Options)) -> Result<Option<StructuredDesc>> {
        let Some(under) = &d.under else { return Ok(None) };
        let sq = qualify(under, &qn.auth);
        if sq == *qn {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.51 SR8j i] type {qn} is not a subtype of itself")).or_at(under.at));
        }
        let Some(desc) = self.catalog.structured_type(&sq).cloned() else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.51 SR8j ii] {sq} is no structured type")).or_at(under.at));
        };
        if desc.supertypes.len() >= MAX_HIERARCHY_DEPTH.saturating_sub(1) {
            let msg = format!("[DESIGN] a type has at most {MAX_HIERARCHY_DEPTH} types above it and itself (RULES.md D81)");
            return Err(rule(msg).or_at(under.at));
        }
        if desc.is_final {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.51 SR8j iii] type {sq} is FINAL, so it has no subtype")).or_at(under.at));
        }
        if !self.under_held(&desc, sole(&*self.catalog, owner)).0 {
            let msg = format!("[ISO/IEC 9075-2:2023 §11.51 AR2] {owner} holds no UNDER on type {sq}");
            return Err(SqlError::new(Code::Privilege, msg).or_at(under.at));
        }
        if let Some((_, at)) = &options.reference {
            return Err(rule("[ISO/IEC 9075-2:2023 §11.51 SR8b] a subtype's reference representation is its supertype's").or_at(*at));
        }
        Ok(Some(desc))
    }

    /// [ISO/IEC 9075-2:2023 §11.51 SR8c, SR8h, SR8i, SR8j vi, SR8k, GR1f v,
    /// GR1f vi] The reference representation of `desc`, and of a
    /// user-defined one the names of its cast functions to the reference
    /// type and from it: a subtype's its supertype's.
    fn reference_of(
        &self,
        desc: &StructuredDesc,
        supertype: Option<&StructuredDesc>,
        options: &Options,
    ) -> Result<(RefRepresentation, Option<(String, String)>)> {
        let reference = match (supertype, &options.reference) {
            (Some(s), _) => s.reference.clone(),
            (None, None | Some((Written::System, _))) => RefRepresentation::System,
            (None, Some((Written::User(t), at))) => RefRepresentation::User(self.representation_type(desc, t).or_at(*at)?),
            (None, Some((Written::Derived(names), at))) => RefRepresentation::Derived(derived(desc, names).or_at(*at)?),
        };
        let RefRepresentation::User(bt) = &reference else {
            if options.casts.0.is_some() || options.casts.1.is_some() {
                let msg = "[ISO/IEC 9075-2:2023 §11.51 SR8k] <cast to ref> and <cast to type> are of a user-defined representation alone";
                return Err(rule(msg).or_at(desc_at(options)));
            }
            return Ok((reference, None));
        };
        // [SR8h] FNREF, else the type's name; FNTYP, else §9.9's of BT.
        let to_ref = options.casts.0.clone().unwrap_or_else(|| desc.name.ident.clone());
        let to_type = match &options.casts.1 {
            Some(n) => n.clone(),
            None => type_name_of(bt).ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §9.9] {bt} has no type name")))?.to_owned(),
        };
        Ok((reference, Some((to_ref, to_type))))
    }

    /// [ISO/IEC 9075-2:2023 §11.51 SR8h, §4.11.1] The representation type
    /// `t` writes: an exact numeric type, or a character string type no
    /// large object one, whose values take no more than `REF_OCTETS`
    /// octets (IV071).
    fn representation_type(&self, desc: &StructuredDesc, t: &crate::ast::TypeName) -> Result<DataType> {
        let owner = owner_of(&*self.catalog, &desc.name.auth);
        let bt = self.data_type_of(t, (&desc.name.auth, sole(&*self.catalog, owner)))?;
        let limit = u32::try_from(REF_OCTETS).unwrap_or(u32::MAX);
        let fits = if let DataType::Character { length, units } | DataType::Varying { length, units } = &bt {
            let per = if *units == CharUnits::Octets { 1 } else { OCTETS_PER_CHARACTER };
            length.saturating_mul(per) <= limit
        } else {
            matches!(bt, DataType::Exact { .. })
        };
        if fits {
            return Ok(bt);
        }
        Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §11.51 SR8h, §4.11.1] REF USING takes an exact numeric type or a character string of at most {REF_OCTETS} octets (IV071), not {bt}"
        )))
    }

    /// [ISO/IEC 9075-2:2023 §11.51 GR3c, §11.60 SR20b] The constructor
    /// function of the instantiable type `qn`: named as it, of no
    /// parameter, of a specific name no routine of its schema has, told
    /// apart from each function of its name.
    fn constructor_of(&self, qn: &QualifiedName) -> Result<RoutineDesc> {
        let specific = format!("{}{CONSTRUCTOR_SUFFIX}", qn.ident);
        if self.specific_in_use(&QualifiedName { auth: qn.auth.clone(), ident: specific.clone() }) {
            let msg = format!("[ISO/IEC 9075-2:2023 §11.51 SR9b vii] schema {} has a routine of specific name {specific} already", qn.auth);
            return Err(rule(msg));
        }
        if let Some(other) = alike(self, qn, &[]) {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §11.60 SR20b] the constructor function {qn} () is not told apart from function {}",
                other.specific()
            );
            return Err(rule(msg));
        }
        // [§11.60 GR5] made as the type is defined.
        Ok(RoutineDesc { name: qn.ident.clone(), specific, execute: Vec::new(), stamps: Stamps::at(self.stamp) })
    }
}

/// Where the type's definition writes its `<reference type
/// specification>`, if it does.
fn desc_at(options: &Options) -> usize {
    options.reference.as_ref().map_or(0, |(_, at)| *at)
}

/// [ISO/IEC 9075-2:2023 §11.51 SR8i] The derivational attributes `names`
/// of `desc`: each named once, each one of its attributes.
fn derived(desc: &StructuredDesc, names: &[String]) -> Result<Vec<String>> {
    for (i, n) in names.iter().enumerate() {
        if names.iter().take(i).any(|m| m == n) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.51 SR8i] REF FROM names attribute {n} twice")));
        }
        if desc.attribute(n).is_none() {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.51 SR8i] type {} has no attribute {n} to derive its references from",
                desc.name
            )));
        }
    }
    Ok(names.to_vec())
}
