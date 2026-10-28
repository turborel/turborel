//! \[EXTENSION\] USAGE on a domain [ISO/IEC 9075:1992 §10.3, §11.36,
//! §11.37]: granted, and revoked, with what its revoke abandons. Split from
//! db/domains.rs (RULES.md R-SIZE-2).

use std::sync::Arc;

use super::definitions::Defined;
use super::{Completion, State, rule};
use crate::ast::{Action, DropBehavior, PrivilegeDefinition, RevokeStatement};
use crate::binder::qualify;
use crate::catalog::{DomainDesc, QualifiedName, UsageDesc, owner_of};
use crate::error::{Code, Result, SqlError, SqlState};

/// [ISO/IEC 9075:1992 §11.36 SR1, §11.37 SR1] That `actions`, of a domain,
/// are USAGE, or ALL PRIVILEGES.
fn usage_alone(actions: Option<&[Action]>) -> Result<()> {
    if actions.is_some_and(|list| list.iter().any(|a| *a != Action::Usage)) {
        return Err(rule("[ISO/IEC 9075:1992 §11.36 SR1, §11.37 SR1] of a domain, USAGE alone is granted or revoked".to_owned()));
    }
    Ok(())
}

/// Whether `who` holds USAGE in `usage`, its domain's owner `owner`, and
/// whether grantably.
fn holds(usage: &[UsageDesc], who: &str, owner: &str) -> (bool, bool) {
    if who == owner {
        return (true, true);
    }
    usage.iter().filter(|u| u.grantee.as_deref().is_none_or(|g| g == who)).fold((false, false), |(_, g), u| (true, g || u.grantable))
}

impl State {
    /// The domain `qn`, and its schema's owner.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such domain.
    fn domain_named(&self, qn: &QualifiedName) -> Result<(DomainDesc, String)> {
        let domain =
            self.catalog.domain(qn).cloned().ok_or_else(|| rule(format!("[ISO/IEC 9075:1992 §11.36 SR2] there is no domain {qn}")))?;
        Ok((domain, owner_of(&*self.catalog, &qn.auth).to_owned()))
    }

    /// [ISO/IEC 9075:1992 §11.36] GRANT USAGE ON DOMAIN `p` by `auth`, its
    /// names in schema `schema`: granted if `auth` holds it with grant
    /// option, else a warning [GR14, GR15].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it grants other than USAGE [SR1], or there is no
    /// such domain; -104 if `auth` holds no USAGE on it [AR1].
    pub(super) fn grant_usage(&mut self, auth: &str, schema: &str, p: &PrivilegeDefinition) -> Result<Defined> {
        usage_alone(p.actions.as_deref())?;
        let (mut domain, owner) = self.domain_named(&qualify(&p.table, schema))?;
        let (held, grantable) = holds(&domain.usage, auth, &owner);
        if !held {
            return Err(SqlError::new(
                Code::Privilege,
                format!("[ISO/IEC 9075:1992 §11.36 AR1] {auth} holds no USAGE on domain {}", domain.name),
            ));
        }
        if !grantable {
            return Ok(Defined { completion: Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED), ..Defined::default() });
        }
        for grantee in &p.grantees {
            match domain.usage.iter_mut().find(|u| u.grantor == auth && u.grantee == *grantee) {
                Some(u) => u.grantable |= p.grant_option,
                None => domain.usage.push(UsageDesc { grantor: auth.to_owned(), grantee: grantee.clone(), grantable: p.grant_option }),
            }
        }
        let mut catalog = self.catalog.duplicate();
        catalog.put_domain(domain);
        self.catalog = Arc::from(catalog);
        Ok(Defined::default())
    }

    /// [ISO/IEC 9075:1992 §11.37] REVOKE USAGE ON DOMAIN `r` by `auth`, its
    /// names in schema `schema`: the grants `auth` made the grantees, or
    /// their grant option alone, gone; with CASCADE, what that abandons
    /// [SR7–SR17] gone too: the grants of those left with no grant option,
    /// and the views and CHECKs of those left with no USAGE that CAST to
    /// it. A warning if `auth` granted none of them it [GR8, GR9].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it revokes other than USAGE [SR1], there is no such
    /// domain, or, with RESTRICT, it abandons any [SR18].
    pub(super) fn revoke_usage(&mut self, (auth, schema): (&str, &str), r: &RevokeStatement) -> Result<Defined> {
        usage_alone(r.actions.as_deref())?;
        let (mut domain, owner) = self.domain_named(&qualify(&r.table, schema))?;
        let named = |u: &UsageDesc| u.grantor == auth && r.grantees.contains(&u.grantee);
        let warned = !domain.usage.iter().any(named);
        let before = domain.usage.clone();
        if r.grant_option_for {
            domain.usage.iter_mut().filter(|u| named(u)).for_each(|u| u.grantable = false);
        } else {
            domain.usage.retain(|u| !named(u));
        }
        // [SR7] a grant whose grantor holds no grant option is abandoned.
        loop {
            let held = domain.usage.clone();
            domain.usage.retain(|u| holds(&held, &u.grantor, &owner).1);
            if domain.usage.len() == held.len() {
                break;
            }
        }
        let abandoned = self.casting_without_usage(&domain, &owner);
        let lost = before.iter().filter(|u| !named(u) && !domain.usage.contains(u)).count();
        if r.behavior == DropBehavior::Restrict && (lost > 0 || !abandoned.is_empty()) {
            let what = abandoned.first().map_or_else(|| "a grant of it".to_owned(), ToString::to_string);
            return Err(rule(format!(
                "[ISO/IEC 9075:1992 §11.37 SR18] revoking USAGE on {} abandons {what}: revoke it with CASCADE",
                domain.name
            )));
        }
        let mut catalog = self.catalog.duplicate();
        catalog.put_domain(domain);
        self.catalog = Arc::from(catalog);
        let mut defined = Defined::default();
        for view in abandoned {
            if self.catalog.view(&view).is_some() {
                let dropped = self.drop_view(&view, true)?;
                defined.dropped.absorb(dropped);
            }
        }
        if warned {
            defined.completion = Completion::Warning(SqlState::PRIVILEGE_NOT_REVOKED);
        }
        Ok(defined)
    }

    /// The views of schemas whose owners hold no USAGE on `domain` that
    /// CAST to it: abandoned [ISO/IEC 9075:1992 §11.37 SR13].
    fn casting_without_usage(&self, domain: &DomainDesc, owner: &str) -> Vec<QualifiedName> {
        let unheld = |schema: &str| !holds(&domain.usage, owner_of(&*self.catalog, schema), owner).0;
        self.catalog
            .views()
            .into_iter()
            .filter(|v| unheld(&v.name.auth) && super::domains::casts_to(&v.query, &v.name.auth, &domain.name))
            .map(|v| v.name.clone())
            .collect()
    }
}
