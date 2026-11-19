//! USAGE and UNDER on a structured type [ISO/IEC 9075-2:2023 §12.2, §12.3,
//! §12.7]: granted, and revoked, with what a revoke abandons [§12.7
//! GR12]: the views, constraints, assertions, triggers, routines and
//! generated columns whose owners need what they no longer hold, each
//! column of a type based on it of a schema whose owner holds no USAGE,
//! and each subtype of a schema whose owner holds no UNDER.

use std::sync::Arc;

use super::super::definitions::Defined;
use super::super::generated::Column;
use super::super::revoke::Abandoned;
use super::super::{Completion, State, rule};
use crate::ast::{Action, DropBehavior, PrivilegeDefinition, RevokeStatement};
use crate::binder::qualify;
use crate::catalog::{QualifiedName, StructuredDesc, UsageDesc, owner_of, sole};
use crate::error::{At, Code, Result, SqlError, SqlState};
use crate::txn::LockMode;

/// [ISO/IEC 9075-2:2023 §12.3 SR4a, SR4c] The privileges on a structured
/// type actions `actions` name, ALL PRIVILEGES both.
fn actions_of(actions: Option<&[Action]>) -> Result<Vec<Action>> {
    let Some(list) = actions else { return Ok(vec![Action::Usage, Action::Under]) };
    if list.iter().any(|a| !matches!(a, Action::Usage | Action::Under)) {
        return Err(rule("[ISO/IEC 9075-2:2023 §12.3 SR4a, SR4c] of a structured type, USAGE and UNDER alone are granted or revoked"));
    }
    Ok(list.to_vec())
}

/// The grants of `action` on `desc`, to change.
fn grants_mut<'a>(desc: &'a mut StructuredDesc, action: &Action) -> &'a mut Vec<UsageDesc> {
    if *action == Action::Under { &mut desc.under } else { &mut desc.usage }
}

/// The grants of `action` on `desc`.
fn grants_of<'a>(desc: &'a StructuredDesc, action: &Action) -> &'a [UsageDesc] {
    if *action == Action::Under { &desc.under } else { &desc.usage }
}

/// The two privileges on a structured type.
const ACTIONS: [Action; 2] = [Action::Usage, Action::Under];

/// A structured type's privileges as a REVOKE leaves them.
struct Revoked {
    /// The type's descriptor, the grants revoked and those they took gone.
    desc: StructuredDesc,
    /// A grantee named held no grant of it from the revoker [§12.7 GR4].
    warned: bool,
    /// A grant not named went with them, its grantor's grant option gone
    /// [GR9].
    lost: bool,
    /// What its revoking abandons.
    abandoned: Abandoned,
    /// The columns of a type based on it of the schemas whose owners hold
    /// no USAGE on it now [GR12h ii].
    columns: Vec<Column>,
    /// The subtypes of the schemas whose owners hold no UNDER on it now.
    types: Vec<QualifiedName>,
}

impl State {
    /// [ISO/IEC 9075-2:2023 §12.2] GRANT USAGE or UNDER ON TYPE `p` by
    /// `auth` of a structured type, its names in schema `schema`: granted
    /// if `auth` holds it grantably, else a warning \[GR9\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it grants another privilege [§12.3 SR4]; -104 if
    /// `auth` holds none it names \[AR1\].
    pub(in crate::db) fn grant_on_structured(&mut self, auth: &str, schema: &str, p: &PrivilegeDefinition) -> Result<Defined> {
        let actions = actions_of(p.actions.as_deref()).or_at(p.table.at)?;
        let mut desc = self.structured_named(&qualify(&p.table, schema), "§12.3 SR7").or_at(p.table.at)?;
        let who = sole(&*self.catalog, auth);
        let owner = owner_of(&*self.catalog, &desc.name.auth).to_owned();
        let (mut any, mut warned) = (false, false);
        for action in actions {
            let (held, grantable) = if action == Action::Under { self.under_held(&desc, who) } else { desc.usage_of(who, &owner) };
            any |= held;
            warned |= !grantable;
            if !grantable {
                continue;
            }
            let list = grants_mut(&mut desc, &action);
            for grantee in &p.grantees {
                match list.iter_mut().find(|u| u.grantor == auth && u.grantee == *grantee) {
                    Some(u) => u.grantable |= p.grant_option,
                    None => list.push(UsageDesc { grantor: auth.to_owned(), grantee: grantee.clone(), grantable: p.grant_option }),
                }
            }
        }
        if !any {
            let msg = format!("[ISO/IEC 9075-2:2023 §12.2 AR1] {auth} holds no privilege named on type {}", desc.name);
            return Err(SqlError::new(Code::Privilege, msg).or_at(p.table.at));
        }
        self.put_structured(desc);
        let completion = if warned { Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED) } else { Completion::Success };
        Ok(Defined { completion, ..Defined::default() })
    }

    /// [ISO/IEC 9075-2:2023 §12.7] REVOKE USAGE or UNDER ON TYPE `r` by
    /// `auth` of a structured type, its names in schema `schema`: with
    /// CASCADE, what that abandons gone too, the columns it loses gone
    /// already (`Session::dropping_generations`).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it revokes another privilege [§12.3 SR4]; with
    /// RESTRICT, -103 and SQLSTATE 2B000 if it abandons anything \[GR12q\].
    pub(in crate::db) fn revoke_on_structured(&mut self, (auth, schema): (&str, &str), r: &RevokeStatement) -> Result<Defined> {
        let revoked = self.structured_revoked((auth, schema), r)?;
        let lost = revoked.lost || !revoked.abandoned.is_empty() || !revoked.columns.is_empty() || !revoked.types.is_empty();
        if r.behavior == DropBehavior::Restrict && lost {
            let other = || if revoked.abandoned.is_empty() { "a grant of it".to_owned() } else { revoked.abandoned.first() };
            let what = revoked
                .types
                .first()
                .map_or_else(|| revoked.columns.first().map_or_else(other, |(t, c)| format!("column {t}.{c}")), |t| format!("type {t}"));
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §12.7 GR12q] revoking a privilege on type {} abandons {what}: revoke it with CASCADE",
                revoked.desc.name
            );
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::DEPENDENT_PRIVILEGES));
        }
        let mut abandoned = revoked.abandoned;
        abandoned.columns.clear();
        self.put_structured(revoked.desc);
        let (mut dropped, mut locks) = self.destroy(Arc::clone(&self.catalog), &abandoned)?;
        // [GR12ag] DROP TYPE CASCADE of each subtype abandoned.
        for t in revoked.types.iter().filter(|t| self.catalog.structured_type(t).is_some()).cloned().collect::<Vec<_>>() {
            let taken = self.drop_user_type(&t, true)?;
            locks.extend(taken.names.iter().cloned().map(|name| (name, LockMode::AccessExclusive)));
            dropped.absorb(taken);
        }
        let completion = if revoked.warned { Completion::Warning(SqlState::PRIVILEGE_NOT_REVOKED) } else { Completion::Success };
        Ok(Defined { completion, dropped, locks, ..Defined::default() })
    }

    /// The columns REVOKE `r` by `auth` of a privilege on a structured type
    /// loses [§12.7 GR12h], its names in schema `schema`.
    ///
    /// # Errors
    ///
    /// As `revoke_on_structured`, but for RESTRICT's.
    pub(in crate::db) fn structured_columns_revoked(&self, (auth, schema): (&str, &str), r: &RevokeStatement) -> Result<Vec<Column>> {
        let revoked = self.structured_revoked((auth, schema), r)?;
        let mut columns = revoked.abandoned.columns;
        columns.extend(revoked.columns.into_iter().filter(|c| !columns.contains(c)).collect::<Vec<_>>());
        columns.extend(self.columns_lost_with(&revoked.types, &|_| true).into_iter().filter(|c| !columns.contains(c)).collect::<Vec<_>>());
        Ok(columns)
    }

    /// The privileges REVOKE `r` by `auth` names, its names in schema
    /// `schema`, as it leaves them, and what that abandons.
    fn structured_revoked(&self, (auth, schema): (&str, &str), r: &RevokeStatement) -> Result<Revoked> {
        let actions = actions_of(r.actions.as_deref()).or_at(r.table.at)?;
        let mut desc = self.structured_named(&qualify(&r.table, schema), "§12.3 SR7").or_at(r.table.at)?;
        let named = |u: &UsageDesc| u.grantor == auth && r.grantees.contains(&u.grantee);
        let mut warned = false;
        for action in actions {
            let list = grants_mut(&mut desc, &action);
            warned |= !r.grantees.iter().all(|g| list.iter().any(|u| u.grantor == auth && u.grantee == *g));
            if r.grant_option_for {
                list.iter_mut().filter(|u| named(u)).for_each(|u| u.grantable = false);
            } else {
                list.retain(|u| !named(u));
            }
        }
        let owner = owner_of(&*self.catalog, &desc.name.auth).to_owned();
        let lost = self.grants_abandoned(&mut desc, &owner, &named);
        let mut after = self.clone();
        after.put_structured(desc.clone());
        let abandoned = self.abandoned_in(&after, &|e| e.code == Code::Privilege);
        let unheld = |s: &str| !desc.usage_of(sole(&*self.catalog, owner_of(&*self.catalog, s)), &owner).0;
        let mut columns = Vec::new();
        for t in self.catalog.tables().into_iter().filter(|t| unheld(&t.name.auth)) {
            columns
                .extend(t.columns.iter().filter(|c| super::names_type(&c.data_type, &desc.name)).map(|c| (t.name.clone(), c.name.clone())));
        }
        let subs = self.proper_subtypes(&desc.name).into_iter().filter(|s| s.supertype.as_ref() == Some(&desc.name));
        let types = subs
            .filter(|s| !after.under_held(&desc, sole(&*self.catalog, owner_of(&*self.catalog, &s.name.auth))).0)
            .map(|s| s.name)
            .collect();
        Ok(Revoked { desc, warned, lost, abandoned, columns, types })
    }

    /// Whether `grantor` holds `action` on `desc` grantably, its schema's
    /// owner `owner`.
    fn held_grantably(&self, desc: &StructuredDesc, action: &Action, (grantor, owner): (&str, &str)) -> bool {
        let who = sole(&*self.catalog, grantor);
        if *action == Action::Under { self.under_held(desc, who).1 } else { desc.usage_of(who, owner).1 }
    }

    /// [ISO/IEC 9075-2:2023 §12.7 GR9] `desc` without each grant whose
    /// grantor holds its privilege grantably no more, at any remove, its
    /// schema's owner `owner`: whether one went that `named` does not name.
    /// Each pass takes one at least, so there are no more passes than
    /// grants.
    fn grants_abandoned(&self, desc: &mut StructuredDesc, owner: &str, named: &dyn Fn(&UsageDesc) -> bool) -> bool {
        let before = desc.clone();
        for action in ACTIONS {
            for _ in 0..grants_of(&before, &action).len() {
                let kept = desc.clone();
                grants_mut(desc, &action).retain(|u| self.held_grantably(&kept, &action, (&u.grantor, owner)));
                if grants_of(desc, &action).len() == grants_of(&kept, &action).len() {
                    break;
                }
            }
        }
        ACTIONS.iter().any(|a| grants_of(&before, a).iter().any(|u| !named(u) && !grants_of(desc, a).contains(u)))
    }
}
