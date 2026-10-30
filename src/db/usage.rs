//! \[EXTENSION\] USAGE on a domain [ISO/IEC 9075:1992 §10.3, §11.36,
//! §11.37]: granted, and revoked, with what its revoke abandons. Split from
//! db/domains.rs (RULES.md R-SIZE-2).

use std::sync::Arc;

use super::definitions::Defined;
use super::domains::{casts_check, casts_to};
use super::revoke::Abandoned;
use super::{Completion, State, rule};
use crate::ast::{Action, DropBehavior, PrivilegeDefinition, RevokeStatement};
use crate::binder::qualify;
use crate::catalog::{Catalog, ConstraintDesc, ConstraintKind, DomainDesc, QualifiedName, UsageDesc, applicable, owner_of};
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
/// whether grantably: the owner always, grantably if `owners` says so
/// (`State::owner_grantable`).
fn holds(usage: &[UsageDesc], who: &str, (owner, owners): (&str, bool)) -> (bool, bool) {
    if who == owner {
        return (true, owners);
    }
    usage.iter().filter(|u| u.grantee.as_deref().is_none_or(|g| g == who)).fold((false, false), |(_, g), u| (true, g || u.grantable))
}

/// The position of column `column` of table or view `table`.
fn position(catalog: &dyn Catalog, table: &QualifiedName, column: &str) -> Option<usize> {
    let columns = catalog.table(table).map(|t| &t.columns).or_else(|| catalog.view(table).map(|v| &v.columns))?;
    columns.iter().position(|c| c.name == column)
}

impl State {
    /// [ISO/IEC 9075:1992 §11.21 GR4] Whether the owner of `domain` holds
    /// USAGE on it grantably: while it holds grantable REFERENCES on each
    /// column the domain's constraints reference, and grantable USAGE on
    /// each other domain they CAST to. Found when asked, as a view's
    /// owner's privileges are (`catalog::applicable`), so a privilege
    /// revoked since it was defined counts.
    ///
    /// # Errors
    ///
    /// A constraint's condition no longer binds.
    pub(crate) fn owner_grantable(&self, domain: &DomainDesc) -> Result<bool> {
        let owner = owner_of(&*self.catalog, &domain.name.auth);
        for c in &domain.constraints {
            let ConstraintKind::Check { text, names, .. } = &c.kind else { continue };
            for (table, column) in self.domain_constraint_uses(domain, (text, names))? {
                let grantable = applicable(&*self.catalog, owner, &table).1;
                if !position(&*self.catalog, &table, &column).is_some_and(|at| grantable.references.contains(&at)) {
                    return Ok(false);
                }
            }
            for other in self.catalog.domains().into_iter().filter(|d| d.name != domain.name && casts_to(text, names, &d.name)) {
                let others = owner_of(&*self.catalog, &other.name.auth);
                // No domain is in usage by its own constraints [SR4], so this ends.
                let owners = others == owner && self.owner_grantable(other)?;
                if !holds(&other.usage, owner, (others, owners)).1 {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

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
        let (held, grantable) = holds(&domain.usage, auth, (&owner, auth == owner && self.owner_grantable(&domain)?));
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
    /// and the views, CHECKs, domain constraints and assertions of those
    /// left with no USAGE that CAST to it. A warning if `auth` granted none
    /// of them it [GR8, GR9].
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
        let owners = self.owner_grantable(&domain)?;
        loop {
            let held = domain.usage.clone();
            domain.usage.retain(|u| holds(&held, &u.grantor, (&owner, owners)).1);
            if domain.usage.len() == held.len() {
                break;
            }
        }
        let abandoned = self.casting_without_usage(&domain, &owner);
        let lost = before.iter().filter(|u| !named(u) && !domain.usage.contains(u)).count();
        if r.behavior == DropBehavior::Restrict && (lost > 0 || !abandoned.is_empty()) {
            let what = if abandoned.is_empty() { "a grant of it".to_owned() } else { abandoned.first() };
            return Err(rule(format!(
                "[ISO/IEC 9075:1992 §11.37 SR18] revoking USAGE on {} abandons {what}: revoke it with CASCADE",
                domain.name
            )));
        }
        let mut catalog = self.catalog.duplicate();
        catalog.put_domain(domain);
        self.catalog = Arc::from(catalog);
        let (dropped, locks) = self.destroy(Arc::clone(&self.catalog), &abandoned)?;
        let mut defined = Defined { dropped, locks, ..Defined::default() };
        if warned {
            defined.completion = Completion::Warning(SqlState::PRIVILEGE_NOT_REVOKED);
        }
        Ok(defined)
    }

    /// What CASTs to `domain` in a schema whose owner holds no USAGE on
    /// it: abandoned [ISO/IEC 9075:1992 §11.37 SR13-SR16]. Its views, and
    /// its tables' CHECKs, other domains' constraints and assertions; a
    /// copy of a domain constraint on a column goes with the domain's.
    fn casting_without_usage(&self, domain: &DomainDesc, owner: &str) -> Abandoned {
        let unheld = |schema: &str| !holds(&domain.usage, owner_of(&*self.catalog, schema), (owner, true)).0;
        let casts = |c: &ConstraintDesc| casts_check(c, &domain.name);
        let mut abandoned = Abandoned::default();
        abandoned.views = self
            .catalog
            .views()
            .into_iter()
            .filter(|v| unheld(&v.name.auth) && casts_to(&v.query, &v.names, &domain.name))
            .map(|v| v.name.clone())
            .collect();
        for t in self.catalog.tables().into_iter().filter(|t| unheld(&t.name.auth)) {
            let own = t.constraints.iter().filter(|c| c.domain.is_none() && casts(c));
            abandoned.constraints.extend(own.map(|c| (t.name.clone(), c.clone())));
        }
        for d in self.catalog.domains().into_iter().filter(|d| d.name != domain.name && unheld(&d.name.auth)) {
            abandoned.domain_constraints.extend(d.constraints.iter().filter(|c| casts(c)).map(|c| (d.name.clone(), c.name.clone())));
        }
        abandoned.assertions =
            self.catalog.assertions().into_iter().filter(|a| unheld(&a.name.auth) && casts(a)).map(|a| a.name.clone()).collect();
        abandoned
    }
}
