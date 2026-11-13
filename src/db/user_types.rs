//! Distinct types [ISO/IEC 9075-2:2023 §4.9, §11.51, §11.59]: CREATE and
//! DROP TYPE, in a transaction as the other schema statements are
//! (RULES.md D33), each with the two functions, casts and transform its
//! definition implies [§11.51 GR2] (RULES.md D69).
//!
//! What references a type is found by binding again, without it, each
//! view's query, each search condition and each generation expression
//! (`user_types/uses.rs`): one that no longer binds referenced it, by a
//! CAST to it, an array or row type naming it, or a call of its routines.

use std::sync::Arc;

mod usage;
mod uses;

use super::ddl::Dropped;
use super::definitions::Defined;
use super::{State, rule};
use crate::ast::{TypeDefinition, TypeName, TypeOption};
use crate::binder::qualify;
use crate::catalog::{QualifiedName, RoutineDesc, TO_DISTINCT_SUFFIX, TO_SOURCE_SUFFIX, UserTypeDesc, owner_of, types_in};
use crate::error::{At, Code, Result, SqlError, SqlState, fail};
use crate::txn::LockMode;
use crate::value::precedence::in_precedence_list;
use crate::value::{DataType, Spelled};

impl State {
    /// [ISO/IEC 9075-2:2023 §11.51] CREATE TYPE `d`, its name in schema
    /// `schema` if it names none: a distinct type, its routines owned, as
    /// it is, by its schema's owner [GR2].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if its schema is not there (SQLSTATE 3F000), the
    /// schema has a type or domain of its name \[SR3\], or it breaks
    /// another Syntax Rule; -104 if the schema's owner holds no USAGE on a
    /// type its source names [§6.1 AR1]; -900 if it defines a structured
    /// type.
    pub(super) fn create_user_type(&mut self, schema: &str, d: &TypeDefinition) -> Result<Defined> {
        let qn = qualify(&d.name, schema);
        if !self.catalog.schema_exists(&qn.auth) {
            let msg = format!("[ISO/IEC 9075-2:2023 §11.51 SR1] there is no schema {0}: CREATE SCHEMA AUTHORIZATION {0} first", qn.auth);
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::INVALID_SCHEMA_NAME).or_at(d.name.at));
        }
        if self.catalog.user_type(&qn).is_some() || self.catalog.domain(&qn).is_some() {
            let msg = format!("[ISO/IEC 9075-2:2023 §11.51 SR3] schema {} has a type or domain {} already", qn.auth, qn.ident);
            return Err(rule(msg).or_at(d.name.at));
        }
        let desc = self.user_type_described(qn.clone(), d)?;
        let mut read = Vec::new();
        types_in(&desc.source, &mut read);
        let mut catalog = self.catalog.duplicate();
        catalog.put_user_type(desc);
        self.catalog = Arc::from(catalog);
        let locks = read.into_iter().chain([QualifiedName::of_schema(&qn.auth)]).map(|name| (name, LockMode::Shared)).collect();
        Ok(Defined { locks, ..Defined::default() })
    }

    /// [ISO/IEC 9075-2:2023 §11.51] CREATE TYPE `d` of the `<schema
    /// definition>` of schema `schema`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if its name is of another schema \[SR2\]; as
    /// `create_user_type`.
    pub(super) fn schema_user_type(&mut self, schema: &str, d: &TypeDefinition) -> Result<()> {
        if let Some(other) = d.name.auth.as_deref().filter(|a| *a != schema) {
            let msg = format!("[ISO/IEC 9075-2:2023 §11.51 SR2] type {} of schema {schema} is named in schema {other}", d.name.ident);
            return Err(rule(msg).or_at(d.name.at));
        }
        self.create_user_type(schema, d).map(|_| ())
    }

    /// [ISO/IEC 9075-2:2023 §11.51 SR4 to SR7, GR1, GR2] The descriptor of
    /// the distinct type `qn` that `d` defines.
    fn user_type_described(&self, qn: QualifiedName, d: &TypeDefinition) -> Result<UserTypeDesc> {
        options_once(&d.options)?;
        let Some(representation) = &d.representation else {
            let msg = "[DESIGN] a structured type [ISO/IEC 9075-2:2023 §11.51 SR5b] is valid SQL this implementation does not take";
            return fail(Code::Unsupported, msg).or_at(d.name.at);
        };
        distinct_options(d)?;
        let owner = owner_of(&*self.catalog, &qn.auth).to_owned();
        let source = respelled(self.source_type(representation, (&qn.auth, &owner))?);
        let cast = |of: fn(&TypeOption) -> Option<&String>| d.options.iter().find_map(|(o, _)| of(o)).cloned();
        // [SR7g, SR7h] FNUDT, else the type's name; FNSDT, else §9.9's.
        let to_distinct = cast(|o| if let TypeOption::CastToDistinct(n) = o { Some(n) } else { None }).unwrap_or_else(|| qn.ident.clone());
        let to_source = match cast(|o| if let TypeOption::CastToSource(n) = o { Some(n) } else { None }) {
            Some(n) => n,
            None => type_name_of(&source).ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §9.9] {source} has no type name")))?.to_owned(),
        };
        self.routine_unique(&qn, &to_distinct, &source).or_at(d.name.at)?;
        let routine = |name: String, suffix: &str| RoutineDesc { name, specific: format!("{}{suffix}", qn.ident), execute: Vec::new() };
        Ok(UserTypeDesc {
            to_distinct: routine(to_distinct, TO_DISTINCT_SUFFIX),
            to_source: routine(to_source, TO_SOURCE_SUFFIX),
            name: qn,
            source,
            usage: Vec::new(),
        })
    }

    /// [ISO/IEC 9075-2:2023 §11.51 SR5a, SR7a] The type PSDT `of` names, in
    /// schema `schema`, its owner `owner`: a `<predefined type>` or a
    /// `<collection type>`, of any element type the owner holds USAGE on.
    fn source_type(&self, of: &TypeName, (schema, owner): (&str, &str)) -> Result<DataType> {
        let refused = |what: &str, at: usize| {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §11.51 SR5a] a distinct type's representation is a <predefined type> or a <collection type>, not {what}"
            );
            Err(rule(msg).or_at(at))
        };
        match of {
            TypeName::Data(DataType::Row(_)) | TypeName::Row(_) => refused("a row type", 0),
            TypeName::Data(DataType::Distinct(_)) => refused("a user-defined type", 0),
            TypeName::Data(DataType::Extension(t)) => refused(t.kind.name(), 0),
            TypeName::Data(t) => Ok(t.clone()),
            TypeName::Domain(name) | TypeName::Extension { name, .. } => refused(&name.ident, name.at),
            TypeName::Array(..) => self.data_type_of(of, (schema, owner)),
        }
    }

    /// [ISO/IEC 9075-2:2023 §11.60 SR20b] That no other function of schema
    /// `qn`'s, of the name `name`, takes a parameter of the type designator
    /// of `source`, as FNUDT of `qn` would: of the one parameter each takes,
    /// only another distinct type's FNUDT may, its source's designator the
    /// same, so neither would be the one subject routine of a call [§9.6].
    /// FNSDT's parameter is of the type itself, as no other's is.
    fn routine_unique(&self, qn: &QualifiedName, name: &str, source: &DataType) -> Result<()> {
        let same = |t: &UserTypeDesc| {
            t.name.auth == qn.auth
                && t.to_distinct.name == name
                && in_precedence_list(&t.source, source)
                && in_precedence_list(source, &t.source)
        };
        match self.catalog.user_types().into_iter().find(|t| same(t)) {
            Some(t) => Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.60 SR20b] function {}.{name} of type {} takes a parameter of the type designator of {source} already",
                qn.auth, t.name
            ))),
            None => Ok(()),
        }
    }

    /// The descriptor of user-defined type `qn`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is none, as the Syntax Rule `cited` says.
    pub(crate) fn user_type_named(&self, qn: &QualifiedName, cited: &str) -> Result<UserTypeDesc> {
        let found = self.catalog.user_type(qn).cloned();
        found.ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 {cited}] there is no type {qn}")))
    }

    /// The user-defined types of schema `auth`.
    pub(super) fn schema_user_types(&self, auth: &str) -> Vec<QualifiedName> {
        self.catalog.user_types().into_iter().filter(|t| t.name.auth == auth).map(|t| t.name.clone()).collect()
    }

    /// [ISO/IEC 9075-2:2023 §11.2 GR] The user-defined types of schema
    /// `auth`, which is being dropped, each dropped with CASCADE, those
    /// a drop of another took gone already.
    pub(super) fn drop_schema_user_types(&mut self, auth: &str) -> Result<Dropped> {
        let mut dropped = Dropped::default();
        for qn in self.schema_user_types(auth) {
            if self.catalog.user_type(&qn).is_some() {
                dropped.absorb(self.drop_user_type(&qn, true)?);
            }
        }
        Ok(dropped)
    }
}

/// [ISO/IEC 9075-2:2023 §11.51 SR4] That no kind of option is written
/// twice.
fn options_once(options: &[(TypeOption, usize)]) -> Result<()> {
    for (i, (o, at)) in options.iter().enumerate() {
        if options.iter().take(i).any(|(earlier, _)| earlier.kind() == o.kind()) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.51 SR4] {} is written once", o.kind())).or_at(*at));
        }
    }
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §11.51 SR7b to SR7f] That `d`, a distinct type's
/// definition, writes none of the options of a structured type: FINAL
/// alone of them is implicit.
fn distinct_options(d: &TypeDefinition) -> Result<()> {
    if let Some(at) = d.under {
        return Err(rule("[ISO/IEC 9075-2:2023 §11.51 SR7d] a distinct type has no <subtype clause>".to_owned()).or_at(at));
    }
    for (o, at) in &d.options {
        let cited = match o {
            TypeOption::Instantiable(_) => "SR7b] a distinct type has no <instantiable clause>",
            TypeOption::Final(false) => "SR7c] a distinct type is FINAL",
            TypeOption::Reference => "SR7e] a distinct type has no <reference type specification>",
            TypeOption::CastToRef | TypeOption::CastToType => "SR7f] a distinct type has no <cast to ref> nor <cast to type>",
            TypeOption::Final(true) | TypeOption::CastToDistinct(_) | TypeOption::CastToSource(_) => continue,
        };
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.51 {cited}")).or_at(*at));
    }
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §11.51 SR7a] SDT, of PSDT `t`: of an exact
/// numeric type, NUMERIC of its precision and scale; of an approximate
/// one, FLOAT of its precision (IV223); else `t`.
fn respelled(t: DataType) -> DataType {
    match t {
        DataType::Exact { precision, scale, .. } => DataType::Exact { precision, scale, spelled: Spelled::Numeric },
        DataType::Approximate { precision, .. } => DataType::Approximate { precision, spelled: Spelled::Float },
        other @ (DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Extension(_)) => other,
    }
}

/// [ISO/IEC 9075-2:2023 §9.9 SR2] The IDENTIFIER of SDT `t`, as IV223
/// spells a number's type; none of a type no source is.
fn type_name_of(t: &DataType) -> Option<&'static str> {
    Some(match t {
        DataType::Character { .. } => "CHAR",
        DataType::Varying { .. } => "VARCHAR",
        DataType::Binary { .. } => "BINARY",
        DataType::VarBinary { .. } => "VARBINARY",
        DataType::Exact { .. } => "NUMERIC",
        DataType::Approximate { .. } => "FLOAT",
        DataType::DecFloat { .. } => "DECFLOAT",
        DataType::Date => "DATE",
        DataType::Time { .. } => "TIME",
        DataType::Timestamp { .. } => "TIMESTAMP",
        DataType::Interval(_) => "INTERVAL",
        DataType::Boolean => "BOOLEAN",
        DataType::Array(_) => "ARRAY",
        DataType::Row(_) | DataType::Distinct(_) | DataType::Extension(_) => return None,
    })
}
