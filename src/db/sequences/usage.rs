//! USAGE on a sequence generator [ISO/IEC 9075-2:2023 §12.2, §12.3,
//! §12.7]: granted, and revoked. Nothing a schema holds needs it, as no
//! view, constraint or default holds NEXT VALUE FOR [§6.14 SR1]: a revoke
//! abandons at most the grants of those it leaves with no grant option.

use super::super::definitions::Defined;
use super::super::usage::usage_alone;
use super::super::{Completion, State};
use crate::ast::{DropBehavior, PrivilegeDefinition, RevokeStatement};
use crate::binder::qualify;
use crate::catalog::{UsageDesc, owner_of};
use crate::error::{Code, Result, SqlError, SqlState};

impl State {
    /// [ISO/IEC 9075-2:2023 §12.2] GRANT USAGE ON SEQUENCE `p` by `auth`,
    /// its names in schema `schema`: granted if `auth` holds it grantably,
    /// else a warning \[GR9\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it grants other than USAGE [§12.3 SR4a], or there is
    /// no such sequence generator \[SR2\]; -104 if `auth` holds no USAGE on
    /// it \[AR1\].
    pub(in crate::db) fn grant_sequence_usage(&mut self, auth: &str, schema: &str, p: &PrivilegeDefinition) -> Result<Defined> {
        usage_alone(p.actions.as_deref())?;
        let mut seq = self.sequence_named(&qualify(&p.table, schema), "§12.2 SR2")?;
        let owner = owner_of(&*self.catalog, &seq.name.auth).to_owned();
        let (held, grantable) = seq.usage_of(auth, &owner);
        if !held {
            let msg = format!("[ISO/IEC 9075-2:2023 §12.2 AR1] {auth} holds no USAGE on sequence generator {}", seq.name);
            return Err(SqlError::new(Code::Privilege, msg));
        }
        if !grantable {
            return Ok(Defined { completion: Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED), ..Defined::default() });
        }
        for grantee in &p.grantees {
            match seq.usage.iter_mut().find(|u| u.grantor == auth && u.grantee == *grantee) {
                Some(u) => u.grantable |= p.grant_option,
                None => seq.usage.push(UsageDesc { grantor: auth.to_owned(), grantee: grantee.clone(), grantable: p.grant_option }),
            }
        }
        self.put_sequence(seq);
        Ok(Defined::default())
    }

    /// [ISO/IEC 9075-2:2023 §12.7] REVOKE USAGE ON SEQUENCE `r` by `auth`,
    /// its names in schema `schema`: the grants `auth` made those it names,
    /// or their grant option alone, gone, and with CASCADE the grants of
    /// those it leaves with no grant option, abandoned \[GR9\]. A warning
    /// if `auth` granted it to one it names not.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it revokes other than USAGE [§12.3 SR4a], or there is
    /// no such sequence generator; with RESTRICT, -103 and SQLSTATE 2B000
    /// if a grant is abandoned \[GR12q\].
    pub(in crate::db) fn revoke_sequence_usage(&mut self, (auth, schema): (&str, &str), r: &RevokeStatement) -> Result<Defined> {
        usage_alone(r.actions.as_deref())?;
        let mut seq = self.sequence_named(&qualify(&r.table, schema), "§12.7 SR1a")?;
        let owner = owner_of(&*self.catalog, &seq.name.auth).to_owned();
        let named = |u: &UsageDesc| u.grantor == auth && r.grantees.contains(&u.grantee);
        let granted = |grantee: &Option<String>| seq.usage.iter().any(|u| u.grantor == auth && u.grantee == *grantee);
        let warned = !r.grantees.iter().all(granted);
        let before = seq.usage.clone();
        if r.grant_option_for {
            seq.usage.iter_mut().filter(|u| named(u)).for_each(|u| u.grantable = false);
        } else {
            seq.usage.retain(|u| !named(u));
        }
        loop {
            let held = seq.clone();
            seq.usage.retain(|u| held.usage_of(&u.grantor, &owner).1);
            if seq.usage.len() == held.usage.len() {
                break;
            }
        }
        let lost = before.iter().any(|u| !named(u) && !seq.usage.contains(u));
        if r.behavior == DropBehavior::Restrict && lost {
            let msg =
                format!("[ISO/IEC 9075-2:2023 §12.7 GR12q] revoking USAGE on {} abandons a grant of it: revoke it with CASCADE", seq.name);
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::DEPENDENT_PRIVILEGES));
        }
        self.put_sequence(seq);
        let completion = if warned { Completion::Warning(SqlState::PRIVILEGE_NOT_REVOKED) } else { Completion::Success };
        Ok(Defined { completion, ..Defined::default() })
    }
}
