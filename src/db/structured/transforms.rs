//! Transforms [ISO/IEC 9075-2:2023 §4.9.6, §11.67 to §11.71, §19.13]:
//! CREATE, ALTER and DROP TRANSFORM, and the session's transform groups
//! SET TRANSFORM GROUP sets.
//!
//! No external routine is defined, so none depends on a transform
//! function: the RESTRICT of ALTER and DROP TRANSFORM refuses nothing
//! [§11.70 SR6, §11.71 SR3].

use std::sync::Arc;

use super::super::ddl::Dropped;
use super::super::definitions::Defined;
use super::super::{Session, State, rule};
use super::{compatible, user_defined};
use crate::ast::{
    AlterGroupAction, AlterTransform, DataAccess, RoutineDesignator, TransformDefinition, TransformGroupSetting, TransformKind,
};
use crate::binder::qualify;
use crate::catalog::{QualifiedName, SqlRoutineDesc, TransformGroupDesc, owner_of};
use crate::error::{At, Code, Result, SqlError, SqlState};
use crate::lexer::{Tok, lex};
use crate::value::DataType;

/// A transform function as the rules check it: its routine, of which kind.
type Function = (SqlRoutineDesc, TransformKind);

impl State {
    /// [ISO/IEC 9075-2:2023 §11.67] CREATE TRANSFORM `t`, its names in
    /// schema `schema` if it names none: each group added to the type's
    /// [GR2].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for each Syntax Rule it breaks.
    pub(in crate::db) fn create_transform(&mut self, schema: &str, t: &TransformDefinition) -> Result<Defined> {
        let (qn, data_type) = self.transformed(schema, &t.udt)?;
        let mut groups = self.catalog.transforms(&qn).to_vec();
        for g in &t.groups {
            let (name, at) = (&g.name.0, g.name.1);
            if groups.iter().any(|o| o.name == *name) {
                return Err(
                    rule(format!("[ISO/IEC 9075-2:2023 §11.67 SR2, SR4d] type {qn} has a transform group {name} already")).or_at(at)
                );
            }
            if let Some(other) = self.family_group(&qn, name) {
                return Err(
                    rule(format!("[ISO/IEC 9075-2:2023 §11.67 SR4e] type {other}, of the family of {qn}, has a group {name}")).or_at(at)
                );
            }
            let mut made = TransformGroupDesc { name: name.clone(), to_sql: None, from_sql: None };
            self.add_elements(&mut made, &g.elements, (schema, &qn, &data_type)).or_at(at)?;
            groups.push(made);
        }
        self.put_groups(&qn, groups);
        Ok(Defined::default())
    }

    /// [ISO/IEC 9075-2:2023 §11.68] ALTER TRANSFORM `t`, its names in schema
    /// `schema` if it names none: each group named changed [GR1].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for each Syntax Rule it breaks.
    pub(in crate::db) fn alter_transform(&mut self, schema: &str, t: &AlterTransform) -> Result<Defined> {
        let (qn, data_type) = self.transformed(schema, &t.udt)?;
        let mut groups = self.catalog.transforms(&qn).to_vec();
        for g in &t.groups {
            let (name, at) = (&g.name.0, g.name.1);
            let Some(group) = groups.iter_mut().find(|o| o.name == *name) else {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.68 SR3b] type {qn} has no transform group {name}")).or_at(at));
            };
            match &g.action {
                AlterGroupAction::Add(elements) => self.add_elements(group, elements, (schema, &qn, &data_type)).or_at(at)?,
                AlterGroupAction::Drop(kinds, _) => drop_elements(group, kinds).or_at(at)?,
            }
        }
        self.put_groups(&qn, groups);
        Ok(Defined::default())
    }

    /// [ISO/IEC 9075-2:2023 §11.71] Drops `udt`'s transform group `group`,
    /// or all of them [GR1].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it names a group the type has not \[SR2\].
    pub(in crate::db) fn drop_transform(&mut self, udt: &QualifiedName, group: Option<&str>, _cascade: bool) -> Result<Dropped> {
        let mut groups = self.catalog.transforms(udt).to_vec();
        match group {
            Some(g) if !groups.iter().any(|o| o.name == g) => {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.71 SR2] type {udt} has no transform group {g}")));
            }
            Some(g) => groups.retain(|o| o.name != g),
            None => groups.clear(),
        }
        self.put_groups(udt, groups);
        Ok(Dropped { names: vec![udt.clone()], ..Dropped::default() })
    }

    /// [ISO/IEC 9075-2:2023 §11.67 SR1] The user-defined type `udt` names,
    /// in schema `schema` if it names none, and its declared type.
    fn transformed(&self, schema: &str, udt: &crate::ast::TableName) -> Result<(QualifiedName, DataType)> {
        let qn = qualify(udt, schema);
        let found = match (self.catalog.structured_type(&qn), self.catalog.user_type(&qn)) {
            (Some(s), _) => s.data_type(),
            (None, Some(d)) => d.data_type(),
            (None, None) => return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.67 SR1] there is no user-defined type {qn}")).or_at(udt.at)),
        };
        Ok((qn, found))
    }

    /// [ISO/IEC 9075-2:2023 §11.67 SR4e] A proper supertype or subtype of
    /// `qn` with a group named `name`, if one has.
    fn family_group(&self, qn: &QualifiedName, name: &str) -> Option<QualifiedName> {
        let desc = self.catalog.structured_type(qn)?;
        let supers = self.proper_supertypes(desc).into_iter().map(|s| s.name);
        let family: Vec<QualifiedName> = supers.chain(self.proper_subtypes(qn).into_iter().map(|s| s.name)).collect();
        family.into_iter().find(|t| self.catalog.transforms(t).iter().any(|g| g.name == name))
    }

    /// [ISO/IEC 9075-2:2023 §11.67 SR4b, SR4c, SR4f to SR4h, §11.69 SR4 to
    /// SR9] The functions `elements` added to `group` of the type `qn`,
    /// declared `data_type`, their names in schema `schema`: each kind once,
    /// in no group that has one, each of the signature its kind takes, and
    /// the one's predefined type compatible with the other's.
    fn add_elements(
        &self,
        group: &mut TransformGroupDesc,
        elements: &[(TransformKind, RoutineDesignator)],
        (schema, qn, data_type): (&str, &QualifiedName, &DataType),
    ) -> Result<()> {
        let owner = owner_of(&*self.catalog, &qn.auth).to_owned();
        for (i, (kind, d)) in elements.iter().enumerate() {
            if elements.iter().take(i).any(|(k, _)| k == kind) || group.function(*kind).is_some() {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §11.67 SR4b, §11.69 SR5, SR6] group {} has one {} function at most",
                    group.name,
                    kind_name(*kind)
                ))
                .or_at(d.name.at));
            }
            let f = self.function_designated(d, (schema, &owner), "§11.67 SR3")?;
            let predefined = transform_signature(&(f.clone(), *kind), data_type, self.self_as_result(&f)).or_at(d.name.at)?;
            let other = match kind {
                TransformKind::ToSql => group.from_sql.clone(),
                TransformKind::FromSql => group.to_sql.clone(),
            };
            if let Some(o) = other.and_then(|o| self.catalog.routine(&o).cloned()) {
                let opposite = if *kind == TransformKind::ToSql { TransformKind::FromSql } else { TransformKind::ToSql };
                let theirs = transform_signature(&(o, opposite), data_type, true)?;
                if !compatible(&predefined, &theirs) {
                    return Err(rule(format!(
                        "[ISO/IEC 9075-2:2023 §11.67 SR4h] the to-sql and from-sql functions of group {} take compatible types",
                        group.name
                    ))
                    .or_at(d.name.at));
                }
            }
            match kind {
                TransformKind::ToSql => group.to_sql = Some(f.specific),
                TransformKind::FromSql => group.from_sql = Some(f.specific),
            }
        }
        Ok(())
    }

    /// Whether the method `f`, if it is one, is of a SELF AS RESULT method
    /// specification: a type-preserving function.
    fn self_as_result(&self, f: &SqlRoutineDesc) -> bool {
        let Some((udt, _)) = &f.method else { return true };
        self.catalog.structured_type(udt).is_some_and(|t| t.methods.iter().any(|m| m.specific == f.specific && m.self_as_result))
    }

    /// `qn`'s transform groups `groups`, recorded.
    fn put_groups(&mut self, qn: &QualifiedName, groups: Vec<TransformGroupDesc>) {
        let mut catalog = self.catalog.duplicate();
        catalog.put_transforms(qn, groups);
        self.catalog = Arc::from(catalog);
    }
}

/// The key words of the transform function of kind `kind`.
const fn kind_name(kind: TransformKind) -> &'static str {
    match kind {
        TransformKind::ToSql => "TO SQL",
        TransformKind::FromSql => "FROM SQL",
    }
}

/// [ISO/IEC 9075-2:2023 §11.67 SR4c, SR4f, SR4g, §11.69 SR7 to SR9] That
/// the function `f` of its kind is one of the type `t`, `preserving` if it
/// is a type-preserving method: deterministic, modifying no SQL-data; a
/// to-sql method of the type and a predefined type, a to-sql function of a
/// predefined type, returning the type, a type-preserving one of a
/// structured type; a from-sql one of the type returning a predefined
/// type. Its predefined type, the one its kind trades the type with.
fn transform_signature((f, kind): &Function, t: &DataType, preserving: bool) -> Result<DataType> {
    let refused = |why: &str| Err(rule(format!("[ISO/IEC 9075-2:2023 §11.67 {why}, as {} is not", f.name)));
    if !f.deterministic || f.access == DataAccess::Modifies {
        return refused("SR4c] a transform function is deterministic and modifies no SQL-data");
    }
    let predefined = |x: &DataType| !user_defined(x) && !matches!(x, DataType::Extension(_));
    let types = f.parameter_types();
    let returns = f.returns.clone().unwrap_or(DataType::Boolean);
    match kind {
        TransformKind::ToSql => {
            let argument = match (f.method.is_some(), types.as_slice()) {
                (true, [own, p]) if own == t && predefined(p) => p.clone(),
                (false, [p]) if predefined(p) => p.clone(),
                (_, _) => return refused(&format!("SR4f] a to-sql function takes a predefined type and returns {t}")),
            };
            if returns != *t || (matches!(t, DataType::Structured(_)) && !preserving) {
                return refused(&format!("SR4f, §11.69 SR8b] a to-sql function returns {t}, a structured type's type-preserving"));
            }
            Ok(argument)
        }
        TransformKind::FromSql => {
            if types.as_slice() != [t.clone()] || !predefined(&returns) {
                return refused(&format!("SR4g] a from-sql function takes {t} and returns a predefined type"));
            }
            Ok(returns)
        }
    }
}

/// [ISO/IEC 9075-2:2023 §11.70 SR3 to SR5, GR1, GR2] The functions of the
/// kinds `kinds` dropped from `group`: each kind once, each a function the
/// group has.
fn drop_elements(group: &mut TransformGroupDesc, kinds: &[TransformKind]) -> Result<()> {
    for (i, kind) in kinds.iter().enumerate() {
        if kinds.iter().take(i).any(|k| k == kind) || group.function(*kind).is_none() {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.70 SR3 to SR5] group {} has a {} function, dropped once",
                group.name,
                kind_name(*kind)
            )));
        }
        match kind {
            TransformKind::ToSql => group.to_sql = None,
            TransformKind::FromSql => group.from_sql = None,
        }
    }
    Ok(())
}

impl Session {
    /// [ISO/IEC 9075-2:2023 §19.13] SET TRANSFORM GROUP: the session's
    /// default transform group, or the group of a user-defined type and of
    /// its subtypes \[GR1\], the name trimmed of spaces.
    ///
    /// # Errors
    ///
    /// SQLSTATE 0S000 if the name, trimmed, is no `<group name>` \[GR1\];
    /// SQLCODE -103 if it names no user-defined type.
    pub(in crate::db) fn set_transform_group(&self, st: &State, g: &TransformGroupSetting) -> Result<()> {
        let written = match g {
            TransformGroupSetting::Default(n) | TransformGroupSetting::ForType(_, n) => n,
        };
        let name = group_name(written)?;
        match g {
            TransformGroupSetting::Default(_) => self.own.borrow_mut().transforms_mut().default = name,
            TransformGroupSetting::ForType(udt, _) => {
                let qn = qualify(udt, &self.default_schema());
                if st.catalog.structured_type(&qn).is_none() && st.catalog.user_type(&qn).is_none() {
                    return Err(rule(format!("[ISO/IEC 9075-2:2023 §19.13 SR1] there is no user-defined type {qn}")).or_at(udt.at));
                }
                let mut own = self.own.borrow_mut();
                let groups = own.transforms_mut();
                groups.for_type.retain(|(t, _)| *t != qn);
                groups.for_type.push((qn, name));
            }
        }
        Ok(())
    }
}

/// [ISO/IEC 9075-2:2023 §19.13 GR1] The `<group name>` `written` writes,
/// without leading or trailing spaces: an `<identifier>`.
fn group_name(written: &str) -> Result<String> {
    let trimmed = written.trim_matches(' ');
    if let Ok(tokens) = lex(trimmed)
        && let [one, end] = tokens.as_slice()
        && end.tok == Tok::End
        && let Tok::Identifier(name) = &one.tok
    {
        return Ok(name.clone());
    }
    let msg = format!("[ISO/IEC 9075-2:2023 §19.13 GR1] '{trimmed}' is no <group name>");
    Err(SqlError::new(Code::Data, msg).with_state(SqlState::INVALID_TRANSFORM_GROUP_NAME))
}
