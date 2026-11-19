//! \[EXTENSION\] USAGE on a registered XQuery module (RULES.md D106),
//! granted and revoked by GRANT and REVOKE USAGE ON XQUERY MODULE as
//! USAGE on a registered XML Schema is (`usage/xml_schemas.rs`), as
//! ISO/IEC 9075-2:2023 grants and revokes it on a sequence generator
//! [§12.2, §12.3, §12.7].
//!
//! Its schema's owner holds it with grant option. A revoke takes away the
//! grants it names, or their grant option, and with them each grant left
//! with no grantor holding it grantably, abandoned [§12.7 GR9]; and of
//! each schema whose owner it leaves without USAGE, what imported the
//! module, abandoned as a view is \[GR12a\] and dropped as DROP XQUERY
//! MODULE with CASCADE drops it: with RESTRICT, abandoning any is refused.

use std::sync::Arc;

use super::super::definitions::Defined;
use super::super::{Completion, State, rule};
use super::usage_alone;
use super::xml_schemas::{granted, usage_revoked, warned};
use crate::ast::{DropBehavior, PrivilegeDefinition, RevokeStatement};
use crate::binder::qualify;
use crate::catalog::{QualifiedName, XQueryModuleDesc, owner_of, sole};
use crate::error::{Code, Result, SqlError, SqlState};

impl State {
    /// The registered XQuery module `qn`, and its schema's owner.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is none [ISO/IEC 9075-2:2023 §12.2 SR2].
    fn xquery_module_named(&self, qn: &QualifiedName) -> Result<(XQueryModuleDesc, String)> {
        let Some(desc) = self.catalog.xquery_module(qn).cloned() else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §12.2 SR2] there is no registered XQuery module {qn}")));
        };
        Ok((desc, owner_of(&*self.catalog, &qn.auth).to_owned()))
    }

    /// \[EXTENSION\] [ISO/IEC 9075-2:2023 §12.2] GRANT USAGE ON XQUERY
    /// MODULE `p` by `auth`, its names in schema `schema`: granted if
    /// `auth` holds it grantably, else a warning \[GR9\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it grants other than USAGE [§12.3 SR4a], or there is
    /// no such registered XQuery module \[SR2\]; -104 if `auth` holds no
    /// USAGE on it \[AR1\].
    pub(in crate::db) fn grant_xquery_module_usage(&mut self, auth: &str, schema: &str, p: &PrivilegeDefinition) -> Result<Defined> {
        usage_alone(p.actions.as_deref())?;
        let qn = qualify(&p.table, schema);
        let (mut desc, owner) = self.xquery_module_named(&qn)?;
        let (held, grantable) = desc.usage_of(sole(&*self.catalog, auth), &owner);
        if !held {
            let msg = format!("[ISO/IEC 9075-2:2023 §12.2 AR1] {auth} holds no USAGE on registered XQuery module {qn}");
            return Err(SqlError::new(Code::Privilege, msg));
        }
        if !grantable {
            return Ok(warned(SqlState::PRIVILEGE_NOT_GRANTED));
        }
        granted(&mut desc.usage, auth, p);
        let mut catalog = self.catalog.duplicate();
        catalog.put_xquery_module(desc);
        self.catalog = Arc::from(catalog);
        Ok(Defined::default())
    }

    /// \[EXTENSION\] [ISO/IEC 9075-2:2023 §12.7] REVOKE USAGE ON XQUERY
    /// MODULE `r` by `auth`, its names in schema `schema`: the grants
    /// `auth` made those it names, or their grant option alone, gone, and
    /// with CASCADE the grants they abandon \[GR9\]; and, of each schema
    /// whose owner it leaves without USAGE, what imported the module,
    /// abandoned \[GR12a\] (`State::xquery_module_dependents`) and dropped. A
    /// warning if `auth` granted it to one it names not \[GR12ai\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it revokes other than USAGE [§12.3 SR4a], or there
    /// is no such registered XQuery module; with RESTRICT, -103 and SQLSTATE
    /// 2B000 if anything is abandoned, and with CASCADE if a generated
    /// column's query imports it \[GR12q\]; the drops' of what is
    /// abandoned.
    pub(in crate::db) fn revoke_xquery_module_usage(&mut self, (auth, schema): (&str, &str), r: &RevokeStatement) -> Result<Defined> {
        usage_alone(r.actions.as_deref())?;
        let qn = qualify(&r.table, schema);
        let (mut desc, owner) = self.xquery_module_named(&qn)?;
        let (lost, not_revoked) = usage_revoked(&*self.catalog, (&mut desc.usage, &owner), auth, r);
        let held = &*self.catalog;
        let loses = |s: &str| !desc.usage_of(sole(held, owner_of(held, s)), &owner).0;
        let (abandoned, typed) = self.xquery_module_dependents(std::slice::from_ref(&qn), &loses);
        let what = if let Some(t) = typed.first() {
            Some(format!("a generated column of {t}, which imports it and is dropped by nothing, even with CASCADE"))
        } else if r.behavior == DropBehavior::Cascade {
            None
        } else if !abandoned.is_empty() {
            Some(format!("{}: revoke it with CASCADE", abandoned.first()))
        } else {
            lost.then(|| "a grant of it: revoke it with CASCADE".to_owned())
        };
        if let Some(what) = what {
            let msg = format!("[ISO/IEC 9075-2:2023 §12.7 GR12q] revoking USAGE on {qn} abandons {what}");
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::DEPENDENT_PRIVILEGES));
        }
        let mut catalog = self.catalog.duplicate();
        catalog.put_xquery_module(desc);
        let (dropped, locks) = self.destroy(Arc::from(catalog), &abandoned)?;
        let completion = if not_revoked { Completion::Warning(SqlState::PRIVILEGE_NOT_REVOKED) } else { Completion::Success };
        Ok(Defined { completion, dropped, locks, ..Defined::default() })
    }
}
