//! \[EXTENSION\] EXECUTE on an embedding model (the vector design's §16):
//! granted and revoked as USAGE on a sequence generator is [ISO/IEC
//! 9075-2:2023 §12.2, §12.7], its schema's owner holding it grantably. A
//! call naming the model needs it (`binder/embedding.rs`).

use super::super::definitions::Defined;
use super::super::{Completion, State, rule};
use crate::ast::{Action, DropBehavior, PrivilegeDefinition, RevokeStatement};
use crate::binder::qualify;
use crate::catalog::{ModelDesc, UsageDesc, owner_of, sole};
use crate::error::{Code, Result, SqlError, SqlState};

/// That `actions` are EXECUTE, or ALL PRIVILEGES.
fn execute_alone(actions: Option<&[Action]>) -> Result<()> {
    if actions.is_some_and(|list| list.iter().any(|a| *a != Action::Execute)) {
        return Err(rule("[EXTENSION] of an embedding model, EXECUTE alone is granted or revoked".to_owned()));
    }
    Ok(())
}

impl State {
    /// Whether the grantor of each grant of `model` still holds EXECUTE on
    /// it grantably, its schema's owner `owner`.
    fn model_grants_held(&self, model: &ModelDesc, owner: &str) -> Vec<UsageDesc> {
        model.execute.iter().filter(|u| model.execute_of(sole(&*self.catalog, &u.grantor), owner).1).cloned().collect()
    }

    /// GRANT EXECUTE ON EMBEDDING MODEL `p` by `auth`, its names in schema
    /// `schema`: granted if `auth` holds it grantably, else a warning.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it grants other than EXECUTE, or there is no such
    /// model; -104 if `auth` holds no EXECUTE on it.
    pub(in crate::db) fn grant_model_execute(&mut self, auth: &str, schema: &str, p: &PrivilegeDefinition) -> Result<Defined> {
        execute_alone(p.actions.as_deref())?;
        let mut model = self.model_desc(&qualify(&p.table, schema))?;
        let owner = owner_of(&*self.catalog, &model.name.auth).to_owned();
        let (held, grantable) = model.execute_of(sole(&*self.catalog, auth), &owner);
        if !held {
            return Err(SqlError::new(Code::Privilege, format!("[EXTENSION] {auth} holds no EXECUTE on embedding model {}", model.name)));
        }
        if !grantable {
            return Ok(Defined { completion: Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED), ..Defined::default() });
        }
        for grantee in &p.grantees {
            match model.execute.iter_mut().find(|u| u.grantor == auth && u.grantee == *grantee) {
                Some(u) => u.grantable |= p.grant_option,
                None => model.execute.push(UsageDesc { grantor: auth.to_owned(), grantee: grantee.clone(), grantable: p.grant_option }),
            }
        }
        self.put_model(model);
        Ok(Defined::default())
    }

    /// REVOKE EXECUTE ON EMBEDDING MODEL `r` by `auth`, its names in
    /// schema `schema`: the grants `auth` made those it names, or their
    /// grant option alone, gone, and with CASCADE the grants of those it
    /// leaves with no grant option. A warning if `auth` granted it to one
    /// it names not.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it revokes other than EXECUTE, or there is no such
    /// model; with RESTRICT, -103 and SQLSTATE 2B000 if a grant is
    /// abandoned.
    pub(in crate::db) fn revoke_model_execute(&mut self, (auth, schema): (&str, &str), r: &RevokeStatement) -> Result<Defined> {
        execute_alone(r.actions.as_deref())?;
        let mut model = self.model_desc(&qualify(&r.table, schema))?;
        let owner = owner_of(&*self.catalog, &model.name.auth).to_owned();
        let named = |u: &UsageDesc| u.grantor == auth && r.grantees.contains(&u.grantee);
        let warned = !r.grantees.iter().all(|g| model.execute.iter().any(|u| u.grantor == auth && u.grantee == *g));
        let before = model.execute.clone();
        if r.grant_option_for {
            model.execute.iter_mut().filter(|u| named(u)).for_each(|u| u.grantable = false);
        } else {
            model.execute.retain(|u| !named(u));
        }
        loop {
            let kept = self.model_grants_held(&model, &owner);
            if kept.len() == model.execute.len() {
                break;
            }
            model.execute = kept;
        }
        if r.behavior == DropBehavior::Restrict && before.iter().any(|u| !named(u) && !model.execute.contains(u)) {
            let msg = format!("[EXTENSION] revoking EXECUTE on {} abandons a grant of it: revoke it with CASCADE", model.name);
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::DEPENDENT_PRIVILEGES));
        }
        self.put_model(model);
        let completion = if warned { Completion::Warning(SqlState::PRIVILEGE_NOT_REVOKED) } else { Completion::Success };
        Ok(Defined { completion, ..Defined::default() })
    }
}
