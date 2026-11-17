//! EXECUTE on the observer or mutator of an attribute of a structured type
//! [ISO/IEC 9075-2:2023 §4.9.3.2, §4.9.3.3, §9.18 SR6, §10.6, §12.2,
//! §12.7]. Each is designated by its specific name, or as METHOD of the
//! attribute's name FOR the type that defines it, and granted and revoked
//! as a method's EXECUTE is, a revoke abandoning what needed it.

use super::super::definitions::Defined;
use super::super::generated::Column;
use super::super::revoke::Abandoned;
use super::super::{Completion, State, rule};
use crate::ast::{DropBehavior, MethodKind, PrivilegeDefinition, PrivilegeObject, RevokeStatement, RoutineType, TableName, TypeName};
use crate::binder::qualify;
use crate::catalog::{RoutineDesc, StructuredDesc, UsageDesc, owner_of, sole, usage_held};
use crate::error::{At, Code, Result, SqlError, SqlState};

/// The observer or the mutator of an attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    /// The observer, of no parameter but SELF.
    Observer,
    /// The mutator, of SELF and the attribute's new value.
    Mutator,
}

/// The routine of an attribute a GRANT or REVOKE designates: the type
/// that defines the attribute, the attribute's position and which.
type Designated = (StructuredDesc, usize, Which);

/// The routine `which` of the attribute at `at` of `desc`, to change.
fn routine_mut(desc: &mut StructuredDesc, (at, which): (usize, Which)) -> Option<&mut RoutineDesc> {
    let a = desc.attributes.get_mut(at)?;
    Some(if which == Which::Observer { &mut a.observer } else { &mut a.mutator })
}

/// The grants of EXECUTE on the routine `which` of the attribute at `at`
/// of `desc`.
fn grants_of(desc: &StructuredDesc, (at, which): (usize, Which)) -> &[UsageDesc] {
    desc.attributes.get(at).map_or(&[], |a| if which == Which::Observer { &a.observer.execute } else { &a.mutator.execute })
}

impl State {
    /// [ISO/IEC 9075-2:2023 §10.6 SR1, SR3b, SR3c] The observer or mutator
    /// of an attribute that a GRANT or REVOKE of EXECUTE designates, its
    /// names in schema `schema`, its types named as `auth` may: by its
    /// specific name, or as an instance METHOD of the attribute's name FOR
    /// `udt`, the type that defines it, of no parameter or of the
    /// attribute's type. None if it designates none.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if both are designated, METHOD written with no
    /// `<data type list>`; as naming a type in the list does.
    fn attribute_routine_designated(
        &self,
        (auth, schema): (&str, &str),
        (name, object, parameters): (&TableName, PrivilegeObject, Option<&[TypeName]>),
        udt: Option<&TableName>,
    ) -> Result<Option<Designated>> {
        let qn = qualify(name, schema);
        if let PrivilegeObject::SpecificRoutine(RoutineType::Routine | RoutineType::Function | RoutineType::Method(MethodKind::Instance)) =
            object
            && udt.is_none()
        {
            let of = |t: &&StructuredDesc| {
                let own = t.attributes.iter().enumerate().filter(|(_, a)| a.defined_by == t.name);
                own.into_iter().find_map(|(at, a)| match qn.ident.as_str() {
                    s if s == a.observer.specific => Some((at, Which::Observer)),
                    s if s == a.mutator.specific => Some((at, Which::Mutator)),
                    _ => None,
                })
            };
            let types = self.catalog.structured_types();
            let found = types.iter().filter(|t| t.name.auth == qn.auth).find_map(|t| of(t).map(|(at, w)| ((*t).clone(), at, w)));
            return Ok(found);
        }
        let (PrivilegeObject::Routine(RoutineType::Method(MethodKind::Instance)), Some(udt)) = (object, udt) else { return Ok(None) };
        let Some(t) = self.catalog.structured_type(&qualify(udt, schema)) else { return Ok(None) };
        let Some((at, a)) = t.attribute(&name.ident).filter(|(_, a)| a.defined_by == t.name) else { return Ok(None) };
        let Some(list) = parameters else {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §10.6 SR3c] METHOD {} FOR {} designates both the observer and the mutator of its attribute",
                name.ident, t.name
            );
            return Err(rule(msg).or_at(name.at));
        };
        let types = list.iter().map(|x| self.data_type_of(x, (&t.name.auth, sole(&*self.catalog, auth)))).collect::<Result<Vec<_>>>();
        Ok(match types.or_at(name.at)?.as_slice() {
            [] => Some((t.clone(), at, Which::Observer)),
            [one] if *one == a.data_type => Some((t.clone(), at, Which::Mutator)),
            _ => None,
        })
    }

    /// [ISO/IEC 9075-2:2023 §12.2] GRANT EXECUTE `p` by `auth` on the
    /// observer or mutator of an attribute, its names in schema `schema`,
    /// if it designates one: granted if `auth` holds it grantably, else a
    /// warning \[GR9\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it grants another privilege [§12.3 SR4d]; -104 if
    /// `auth` holds none \[AR1\].
    pub(in crate::db) fn grant_on_attribute_routine(
        &mut self,
        (auth, schema): (&str, &str),
        p: &PrivilegeDefinition,
    ) -> Option<Result<Defined>> {
        let named = (&p.table, p.object, p.parameters.as_deref());
        let (mut desc, at, which) = match self.attribute_routine_designated((auth, schema), named, p.udt.as_ref()) {
            Ok(Some(d)) => d,
            Ok(None) => return None,
            Err(e) => return Some(Err(e)),
        };
        Some(self.attribute_routine_granted(auth, p, (&mut desc, at, which)))
    }

    /// GRANT `p` by `auth` of EXECUTE on the routine `which` of the
    /// attribute at `at` of `desc`.
    fn attribute_routine_granted(
        &mut self,
        auth: &str,
        p: &PrivilegeDefinition,
        (desc, at, which): (&mut StructuredDesc, usize, Which),
    ) -> Result<Defined> {
        execute_alone(p.actions.as_deref()).or_at(p.table.at)?;
        let owner = owner_of(&*self.catalog, &desc.name.auth).to_owned();
        let (held, grantable) = usage_held(grants_of(desc, (at, which)), sole(&*self.catalog, auth), &owner);
        if !held {
            let msg = format!("[ISO/IEC 9075-2:2023 §12.2 AR1] {auth} holds no EXECUTE on {}", described(desc, (at, which)));
            return Err(SqlError::new(Code::Privilege, msg).or_at(p.table.at));
        }
        if !grantable {
            return Ok(Defined { completion: Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED), ..Defined::default() });
        }
        if let Some(r) = routine_mut(desc, (at, which)) {
            for grantee in &p.grantees {
                match r.execute.iter_mut().find(|u| u.grantor == auth && u.grantee == *grantee) {
                    Some(u) => u.grantable |= p.grant_option,
                    None => r.execute.push(UsageDesc { grantor: auth.to_owned(), grantee: grantee.clone(), grantable: p.grant_option }),
                }
            }
        }
        self.put_structured(desc.clone());
        Ok(Defined::default())
    }

    /// [ISO/IEC 9075-2:2023 §12.7] REVOKE EXECUTE `r` by `auth` on the
    /// observer or mutator of an attribute, its names in schema `schema`,
    /// if it designates one: the grants it names, or their grant option,
    /// gone, and each grant whose grantor holds it grantably no more [GR9];
    /// with CASCADE, the views, constraints, triggers and routines that
    /// needed it gone too.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it revokes another privilege [§12.3 SR4d]; with
    /// RESTRICT, -103 and SQLSTATE 2B000 if it abandons anything \[GR12q\].
    pub(in crate::db) fn revoke_on_attribute_routine(
        &mut self,
        (auth, schema): (&str, &str),
        r: &RevokeStatement,
    ) -> Option<Result<Defined>> {
        let named = (&r.table, r.object, r.parameters.as_deref());
        let designated = match self.attribute_routine_designated((auth, schema), named, r.udt.as_ref()) {
            Ok(Some(d)) => d,
            Ok(None) => return None,
            Err(e) => return Some(Err(e)),
        };
        Some(self.attribute_routine_revoked(auth, r, designated))
    }

    /// REVOKE `r` by `auth` of EXECUTE on the routine `which` of the
    /// attribute at `at` of `desc`, the columns it loses gone already
    /// (`Session::dropping_generations`).
    fn attribute_routine_revoked(&mut self, auth: &str, r: &RevokeStatement, designated: Designated) -> Result<Defined> {
        let Revoked { desc, at, which, warned, lost, mut abandoned } = self.attribute_revoked(auth, r, designated)?;
        if r.behavior == DropBehavior::Restrict && (lost || !abandoned.is_empty()) {
            let what = if abandoned.is_empty() { "a grant of it".to_owned() } else { abandoned.first() };
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §12.7 GR12q] revoking EXECUTE on {} abandons {what}: revoke it with CASCADE",
                described(&desc, (at, which))
            );
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::DEPENDENT_PRIVILEGES));
        }
        abandoned.columns.clear();
        self.put_structured(desc);
        let (dropped, locks) = self.destroy(std::sync::Arc::clone(&self.catalog), &abandoned)?;
        let completion = if warned { Completion::Warning(SqlState::PRIVILEGE_NOT_REVOKED) } else { Completion::Success };
        Ok(Defined { completion, dropped, locks, ..Defined::default() })
    }

    /// [ISO/IEC 9075-2:2023 §12.7 GR12h] The generated columns REVOKE `r`
    /// by `auth` of EXECUTE on an attribute's observer or mutator loses,
    /// its names in schema `schema`, if it designates one.
    ///
    /// # Errors
    ///
    /// As `revoke_on_attribute_routine`, but for RESTRICT's.
    pub(in crate::db) fn attribute_routine_columns(
        &self,
        (auth, schema): (&str, &str),
        r: &RevokeStatement,
    ) -> Option<Result<Vec<Column>>> {
        let named = (&r.table, r.object, r.parameters.as_deref());
        let designated = match self.attribute_routine_designated((auth, schema), named, r.udt.as_ref()) {
            Ok(Some(d)) => d,
            Ok(None) => return None,
            Err(e) => return Some(Err(e)),
        };
        Some(self.attribute_revoked(auth, r, designated).map(|revoked| revoked.abandoned.columns))
    }

    /// The type's descriptor as REVOKE `r` by `auth` of EXECUTE on the
    /// routine designated leaves it, and what that abandons.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it revokes another privilege [§12.3 SR4d].
    fn attribute_revoked(&self, auth: &str, r: &RevokeStatement, (mut desc, at, which): Designated) -> Result<Revoked> {
        execute_alone(r.actions.as_deref()).or_at(r.table.at)?;
        let owner = owner_of(&*self.catalog, &desc.name.auth).to_owned();
        let named = |u: &UsageDesc| u.grantor == auth && r.grantees.contains(&u.grantee);
        let before = grants_of(&desc, (at, which)).to_vec();
        let warned = !r.grantees.iter().all(|g| before.iter().any(|u| u.grantor == auth && u.grantee == *g));
        if let Some(routine) = routine_mut(&mut desc, (at, which)) {
            if r.grant_option_for {
                routine.execute.iter_mut().filter(|u| named(u)).for_each(|u| u.grantable = false);
            } else {
                routine.execute.retain(|u| !named(u));
            }
        }
        // [GR9] each pass takes one grant at least, so there are no more
        // passes than grants.
        for _ in 0..before.len() {
            let kept = grants_of(&desc, (at, which)).to_vec();
            let stands = |u: &UsageDesc| usage_held(&kept, sole(&*self.catalog, &u.grantor), &owner).1;
            let Some(routine) = routine_mut(&mut desc, (at, which)) else { break };
            routine.execute.retain(stands);
            if routine.execute.len() == kept.len() {
                break;
            }
        }
        let lost = before.iter().any(|u| !named(u) && !grants_of(&desc, (at, which)).contains(u));
        let mut after = self.clone();
        after.put_structured(desc.clone());
        let abandoned = self.abandoned_in(&after, &|e| e.code == Code::Privilege);
        Ok(Revoked { desc, at, which, warned, lost, abandoned })
    }
}

/// An attribute's routine's privileges as a REVOKE leaves them.
struct Revoked {
    /// The type's descriptor, the grants revoked and those they took gone.
    desc: StructuredDesc,
    /// The attribute's position.
    at: usize,
    /// Its observer or its mutator.
    which: Which,
    /// A grantee named held no grant of it from the revoker [§12.7 GR4].
    warned: bool,
    /// A grant not named went with them [GR9].
    lost: bool,
    /// What its revoking abandons.
    abandoned: Abandoned,
}

/// [ISO/IEC 9075-2:2023 §12.3 SR4d] That the actions `actions` name
/// EXECUTE alone, ALL PRIVILEGES naming it.
///
/// # Errors
///
/// SQLCODE -103 if they name another.
fn execute_alone(actions: Option<&[crate::ast::Action]>) -> Result<()> {
    if actions.is_some_and(|list| list.iter().any(|a| *a != crate::ast::Action::Execute)) {
        return Err(rule("[ISO/IEC 9075-2:2023 §12.3 SR4d] of a routine, EXECUTE alone is granted or revoked".to_owned()));
    }
    Ok(())
}

/// The routine `which` of the attribute at `at` of `desc`, described.
fn described(desc: &StructuredDesc, (at, which): (usize, Which)) -> String {
    let attribute = desc.attributes.get(at).map_or("", |a| a.name.as_str());
    let kind = if which == Which::Observer { "observer" } else { "mutator" };
    format!("the {kind} of attribute {attribute} of type {}", desc.name)
}
