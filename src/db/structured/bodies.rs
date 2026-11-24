//! [ISO/IEC 9075-2:2023 §11.60, §10.6] SQL-invoked methods: CREATE METHOD,
//! which gives a method specification its body, and the method a
//! `<specific routine designator>` of METHOD ... FOR a type designates.
//!
//! A method's descriptor is an SQL-invoked function's, of its type and
//! kind, its parameters the augmented list: SELF, of the type, first,
//! unless it is static [§11.60 SR8b xii]. Its text is the CREATE METHOD
//! as written, which the binder reads again to bind its body.

use super::super::definitions::Defined;
use super::super::{State, rule};
use super::methods::SELF_NAME;
use super::{Named, all_compatible};
use crate::ast::{
    MethodDefinition, MethodKind, MethodSignature, ParameterDeclaration, ParameterMode, ParameterType, PrivilegeObject, RoutineDefinition,
    RoutineKind, RoutineType, TableName, TypeName,
};
use crate::binder::{alike, qualify};
use crate::catalog::{
    Horizon, MethodHost, MethodSpecDesc, QualifiedName, SYSTEM_GRANTOR, SqlParameter, SqlRoutineDesc, Stamps, UsageDesc, method_host,
    owner_of, sole,
};
use crate::error::{At, Result};
use crate::txn::LockMode;
use crate::value::DataType;

impl State {
    /// [ISO/IEC 9075-2:2023 §11.60] CREATE METHOD `m`, its names in schema
    /// `schema` if it names none: an SQL-invoked method of the method
    /// specification it designates [SR8], its body bound as a function's,
    /// EXECUTE on it its schema's owner's, of `_SYSTEM` [GR1, GR4].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it designates no method specification, the
    /// specification has a body already, or it breaks another Syntax Rule
    /// [SR8, SR20b]; its body's, bound.
    pub(in crate::db) fn create_method(&mut self, schema: &str, m: &MethodDefinition) -> Result<Defined> {
        let (udt, subject, spec) = self.method_spec_of(schema, &m.signature).or_at(m.at)?;
        if let Some(body) = self.catalog.routine(&spec.specific) {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §11.60 SR20b] method {} of type {udt} has a body already, routine {}",
                spec.name, body.specific
            );
            return Err(rule(msg).or_at(m.at));
        }
        let parameters = augmented(&subject, &spec);
        let name = QualifiedName { auth: udt.auth.clone(), ident: spec.name.clone() };
        if spec.kind == MethodKind::Instance
            && let Some(other) = alike(self, &name, &parameters.iter().map(|p| p.data_type.clone()).collect::<Vec<_>>())
        {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §11.60 SR20b] method {name} of type {udt} is not told apart from function {}",
                other.specific()
            );
            return Err(rule(msg).or_at(m.at));
        }
        let mut desc = SqlRoutineDesc {
            method: Some((udt.clone(), spec.kind)),
            specific: spec.specific.clone(),
            name: name.clone(),
            kind: RoutineKind::Function,
            parameters,
            returns: Some(spec.returns.clone()),
            deterministic: spec.deterministic,
            access: spec.access,
            null_call: spec.null_call,
            text: m.text.clone(),
            body: m.body_text.clone(),
            schema: schema.to_owned(),
            reads: Vec::new(),
            horizon: Horizon::default(),
            sequences: Vec::new(),
            execute: Vec::new(),
            debug: Vec::new(),
            created: self.next_created(),
            // [§11.60 GR5] its own, not its specification's [GR4].
            stamps: Stamps::at(self.stamp),
            module: None,
            ptf: None,
            distance: None,
        };
        let bound = self.bind_routine_body(&desc, &definition(&name, &spec, m))?;
        desc.horizon = Horizon::of(&*self.catalog, &bound.reads).invoking(bound.invokes).indicating(bound.indicates).typing(bound.typed);
        (desc.reads, desc.sequences) = (bound.reads, bound.sequences);
        let owner = owner_of(&*self.catalog, &name.auth).to_owned();
        desc.execute = vec![UsageDesc { grantor: SYSTEM_GRANTOR.to_owned(), grantee: Some(owner), grantable: bound.grantable }];
        let locks = desc.reads.iter().cloned().chain([udt]).map(|t| (t, LockMode::Shared)).collect();
        self.put_routine(desc);
        Ok(Defined { locks, ..Defined::default() })
    }

    /// [ISO/IEC 9075-2:2023 §11.60 SR8] The type, SELF's type, and the
    /// method specification `signature` designates, its names in schema
    /// `schema`: of a structured or a distinct type.
    fn method_spec_of(&self, schema: &str, signature: &MethodSignature) -> Result<(QualifiedName, DataType, MethodSpecDesc)> {
        let catalog = &*self.catalog;
        let MethodSignature::Signature { kind, name, parameters, returns, udt, .. } = signature else {
            let MethodSignature::Specific(written) = signature else { return Err(rule("[ISO/IEC 9075-2:2023 §11.60 SR8] no method")) };
            return self.method_of_specific(schema, written);
        };
        let uq = qualify(udt, schema);
        let host = method_host(catalog, &uq)
            .ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §11.60 SR8b ii] there is no user-defined type {uq}")).or_at(udt.at))?;
        let types = self.declared_types(&uq, parameters)?;
        let found: Vec<&MethodSpecDesc> = host
            .methods()
            .iter()
            .filter(|s| {
                s.name == name.0
                    && s.kind == *kind
                    && all_compatible(&s.parameters.iter().map(|p| p.data_type.clone()).collect::<Vec<_>>(), &types)
            })
            .collect();
        let [spec] = found.as_slice() else {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §11.60 SR8b iii] type {uq} has no {} method specification {} of those parameters",
                kind.name(),
                name.0
            );
            return Err(rule(msg).or_at(name.1));
        };
        let names = parameters.iter().map(|p| &p.name).eq(spec.parameters.iter().map(|p| &p.name));
        let identical = spec.parameters.iter().map(|p| &p.data_type).eq(types.iter());
        if !names || !identical {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §11.60 SR8b vi to viii] method {} names its parameters, of the types, as its specification does",
                name.0
            );
            return Err(rule(msg).or_at(name.1));
        }
        if let Some(r) = returns {
            let owner = owner_of(&*self.catalog, &uq.auth);
            if self.data_type_of(r, (&uq.auth, sole(&*self.catalog, owner)))? != spec.returns {
                let msg =
                    format!("[ISO/IEC 9075-2:2023 §11.60 SR8b ix] method {} returns {}, as its specification does", name.0, spec.returns);
                return Err(rule(msg).or_at(name.1));
            }
        }
        Ok((uq.clone(), host.data_type(), (*spec).clone()))
    }

    /// [ISO/IEC 9075-2:2023 §11.60 SR8a] Of the specific name `written`, in
    /// schema `schema`, the type of its schema with a method of that name,
    /// SELF's type and the method specification.
    fn method_of_specific(&self, schema: &str, written: &TableName) -> Result<(QualifiedName, DataType, MethodSpecDesc)> {
        let catalog = &*self.catalog;
        // [SR8a] of a type of the schema of the name.
        let sq = qualify(written, schema);
        let hosts = catalog
            .structured_types()
            .into_iter()
            .map(MethodHost::Structured)
            .chain(catalog.user_types().into_iter().map(MethodHost::Distinct));
        let found = hosts
            .filter(|h| h.name().auth == sq.auth)
            .flat_map(|h| h.methods().iter().map(move |m| (h, m)))
            .find(|(_, m)| m.specific == sq);
        found.map(|(h, m)| (h.name().clone(), h.data_type(), m.clone())).ok_or_else(|| {
            rule(format!("[ISO/IEC 9075-2:2023 §11.60 SR8a iii] no type of schema {} has a method of specific name {sq}", sq.auth))
        })
    }

    /// The declared types of the parameters `parameters` of a method of
    /// type `qn`, named as the owner of its schema names them.
    fn declared_types(&self, qn: &QualifiedName, parameters: &[ParameterDeclaration]) -> Result<Vec<DataType>> {
        let owner = owner_of(&*self.catalog, &qn.auth);
        let typed = |p: &ParameterDeclaration| match &p.data_type {
            ParameterType::Data(t) => self.data_type_of(t, (&qn.auth, sole(&*self.catalog, owner))).or_at(p.at),
            ParameterType::Table(_) | ParameterType::Descriptor => {
                Err(rule(format!("[ISO/IEC 9075-2:2023 §11.60 SR10a] parameter {} of a method is of a <data type>", p.name)).or_at(p.at))
            }
        };
        parameters.iter().map(typed).collect()
    }

    /// [ISO/IEC 9075-2:2023 §10.6 SR3b, SR3c] The SQL-invoked method that
    /// the designator `named`, of a method of the type `udt`, designates,
    /// its names in schema `schema`, its types named as `auth` may: of its
    /// kind and name, the one, or the one of the unaugmented parameter
    /// types of its `<data type list>` [§9.30].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `udt` is no user-defined type, or not exactly one
    /// method is designated.
    pub(in crate::db) fn method_designated(
        &self,
        (auth, schema): (&str, &str),
        named: Named<'_>,
        udt: &TableName,
    ) -> Result<Option<SqlRoutineDesc>> {
        let (name, object, parameters, _) = named;
        let (PrivilegeObject::Routine(RoutineType::Method(kind)) | PrivilegeObject::SpecificRoutine(RoutineType::Method(kind))) = object
        else {
            return Err(rule("[ISO/IEC 9075-2:2023 §10.6 SR3a] FOR a type is written of a METHOD alone".to_owned()).or_at(name.at));
        };
        let uq = qualify(udt, schema);
        if method_host(&*self.catalog, &uq).is_none() {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §10.6 SR3b] there is no user-defined type {uq}")).or_at(udt.at));
        }
        let types = match parameters {
            Some(list) => Some(
                list.iter()
                    .map(|t| self.data_type_of(t, (&uq.auth, sole(&*self.catalog, auth))))
                    .collect::<Result<Vec<_>>>()
                    .or_at(name.at)?,
            ),
            None => None,
        };
        let of = |r: &&SqlRoutineDesc| r.method.as_ref().is_some_and(|(t, k)| *t == uq && *k == kind) && r.name.ident == name.ident;
        let unaugmented = |r: &SqlRoutineDesc| {
            let all = r.parameter_types();
            if kind == MethodKind::Static { all } else { all.into_iter().skip(1).collect() }
        };
        let found: Vec<&SqlRoutineDesc> =
            self.catalog.routines().into_iter().filter(of).filter(|r| types.as_ref().is_none_or(|t| unaugmented(r) == *t)).collect();
        match found.as_slice() {
            [one] => Ok(Some((*one).clone())),
            _ => Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §10.6 SR3c] {} {} FOR {uq} designates not exactly one method",
                kind.name(),
                name.ident
            ))
            .or_at(name.at)),
        }
    }
}

/// [ISO/IEC 9075-2:2023 §11.51 SR9b xiv 9, §11.60 SR8b xii] The augmented
/// parameter list of `spec`, a method specification of a type: SELF, of
/// the type `subject`, then its own, but of a static method its own alone.
fn augmented(subject: &DataType, spec: &MethodSpecDesc) -> Vec<SqlParameter> {
    let own = spec.parameters.iter().cloned();
    if spec.kind == MethodKind::Static {
        return own.collect();
    }
    let subject = SqlParameter { name: SELF_NAME.to_owned(), data_type: subject.clone(), mode: ParameterMode::In, generic: None };
    std::iter::once(subject).chain(own).collect()
}

/// The routine definition the body of the method `m`, named `name`, of
/// specification `spec`, binds as: a function's, returning what it does.
fn definition(name: &QualifiedName, spec: &MethodSpecDesc, m: &MethodDefinition) -> RoutineDefinition {
    RoutineDefinition {
        kind: RoutineKind::Function,
        name: TableName { auth: Some(name.auth.clone()), ident: name.ident.clone(), at: m.at },
        parameters: Vec::new(),
        returns: Some(TypeName::Data(spec.returns.clone())),
        specific: None,
        deterministic: spec.deterministic,
        access: spec.access,
        null_call: spec.null_call,
        new_savepoint_level: false,
        body: m.body.clone(),
        text: m.text.clone(),
        body_text: m.body_text.clone(),
        distance: None,
    }
}
