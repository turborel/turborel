//! \[EXTENSION\] USAGE on a collation a definition made [ISO/IEC 9075:1992
//! §11.36, §11.37; ISO/IEC 9075-2:2023 §12.7]: granted, and revoked, with
//! the columns and domains its revoke impacts and what it abandons.
//!
//! SQL-92 has no rule for a column of a collation whose schema's owner
//! loses USAGE on it, so the newer text governs (RULES.md D32): such a
//! column or domain is impacted [GR12n], refused with RESTRICT [GR12q] and
//! reverted to the default with CASCADE [GR12ad] (`collations/revert.rs`).

use std::sync::Arc;

use super::super::definitions::Defined;
use super::super::generated::Column;
use super::super::revoke::Abandoned;
use super::super::usage::usage_alone;
use super::super::{Completion, State, api};
use super::collation_name;
use crate::ast::{DropBehavior, PrivilegeDefinition, RevokeStatement};
use crate::catalog::{CollationDesc, ColumnDesc, DomainDesc, QualifiedName, UsageDesc, owner_of};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::txn::LockMode;

/// A collation's USAGE as a REVOKE leaves it.
struct Revoked {
    /// The collation.
    qn: QualifiedName,
    /// Its descriptor, the grants revoked and those they took gone.
    desc: CollationDesc,
    /// Its schema's owner.
    owner: String,
    /// A grantee named held no grant of it from the revoker [GR8, GR9].
    warned: bool,
    /// Grants not named that went with those that were [SR7].
    lost: usize,
}

impl Revoked {
    /// Whether the owner of schema `schema` holds no USAGE on it.
    fn loses(&self, st: &State, schema: &str) -> bool {
        !self.desc.usage_of(owner_of(&*st.catalog, schema), &self.owner).0
    }
}

impl State {
    /// [ISO/IEC 9075:1992 §11.36] GRANT USAGE `p` on a collation, by `auth`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is no collation definitions made, or the
    /// privilege is other than USAGE; -104 if `auth` holds none grantably.
    pub(in crate::db) fn grant_collation_usage(&mut self, auth: &str, p: &PrivilegeDefinition) -> Result<Defined> {
        usage_alone(p.actions.as_deref())?;
        let qn = collation_name(&p.table);
        let mut desc = self.made_collation(&qn)?;
        let owner = owner_of(&*self.catalog, &qn.auth).to_owned();
        let (held, grantable) = desc.usage_of(auth, &owner);
        if !held {
            return Err(SqlError::new(Code::Privilege, format!("[ISO/IEC 9075:1992 §11.36 AR1] {auth} holds no USAGE on collation {qn}")));
        }
        let mut defined = Defined::default();
        if !grantable {
            defined.completion = Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED);
            return Ok(defined);
        }
        for grantee in &p.grantees {
            desc.usage.retain(|u| !(u.grantor == auth && u.grantee == *grantee));
            desc.usage.push(UsageDesc { grantor: auth.to_owned(), grantee: grantee.clone(), grantable: p.grant_option });
        }
        self.put_collation(desc);
        Ok(defined)
    }

    /// The USAGE REVOKE `r` on a collation by `auth` leaves: the grants
    /// named, or their grant option, gone, then each grant whose grantor
    /// holds it no longer grantably [ISO/IEC 9075:1992 §11.37 SR7].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it revokes other than USAGE [SR1], or there is no
    /// such collation a definition made.
    fn usage_after(&self, auth: &str, r: &RevokeStatement) -> Result<Revoked> {
        usage_alone(r.actions.as_deref())?;
        let qn = collation_name(&r.table);
        let mut desc = self.made_collation(&qn)?;
        let owner = owner_of(&*self.catalog, &qn.auth).to_owned();
        let named = |u: &UsageDesc| u.grantor == auth && r.grantees.contains(&u.grantee);
        let granted = |grantee: &Option<String>| desc.usage.iter().any(|u| u.grantor == auth && u.grantee == *grantee);
        let warned = !r.grantees.iter().all(granted);
        let before = desc.usage.clone();
        if r.grant_option_for {
            desc.usage.iter_mut().filter(|u| named(u)).for_each(|u| u.grantable = false);
        } else {
            desc.usage.retain(|u| !named(u));
        }
        loop {
            let held = desc.clone();
            desc.usage.retain(|u| held.usage_of(&u.grantor, &owner).1);
            if desc.usage.len() == held.usage.len() {
                break;
            }
        }
        let lost = before.iter().filter(|u| !named(u) && !desc.usage.contains(u)).count();
        Ok(Revoked { qn, desc, owner, warned, lost })
    }

    /// [ISO/IEC 9075-2:2023 §12.7 GR12n] The tables REVOKE USAGE `r` on a
    /// collation by `auth` impacts: those with a column of it in a schema
    /// whose owner it leaves without USAGE. And the collation.
    ///
    /// # Errors
    ///
    /// As `usage_after`.
    pub(in crate::db) fn impacted_tables(&self, auth: &str, r: &RevokeStatement) -> Result<(QualifiedName, Vec<QualifiedName>)> {
        let after = self.usage_after(auth, r)?;
        let mut tables = self.tables_of_collation(&after.qn, None);
        tables.retain(|t| after.loses(self, &t.auth));
        Ok((after.qn, tables))
    }

    /// The generated columns REVOKE USAGE `r` on a collation by `auth`
    /// loses: those that name it of the tables of each schema whose owner it
    /// leaves with no USAGE (`db/generated/named.rs`).
    ///
    /// # Errors
    ///
    /// As `usage_after`.
    pub(in crate::db) fn collation_generations_revoked(&self, auth: &str, r: &RevokeStatement) -> Result<Vec<Column>> {
        let after = self.usage_after(auth, r)?;
        Ok(self.naming_collation(&after.qn, &|schema| after.loses(self, schema)).columns)
    }

    /// [ISO/IEC 9075:1992 §11.37; ISO/IEC 9075-2:2023 §12.7] REVOKE USAGE `r`
    /// on a collation, by `auth`: with CASCADE, the impacted domains
    /// reverted [GR12ad], their columns reverted already
    /// (`Session::revert_impacted`), and the views, CHECKs, domain
    /// constraints and assertions of a schema left without it that name it
    /// gone [GR12a, GR12c, GR12d, GR12f].
    ///
    /// # Errors
    ///
    /// As `usage_after`; with RESTRICT, SQLCODE -103 if a column or domain
    /// is impacted (SQLSTATE 2B000) [GR12q], or a grant or object is
    /// abandoned [ISO/IEC 9075:1992 §11.37 SR18]; with CASCADE, SQLCODE
    /// -910 if a column impacted is not reverted.
    pub(in crate::db) fn revoke_collation_usage(&mut self, (auth, _): (&str, &str), r: &RevokeStatement) -> Result<Defined> {
        let after = self.usage_after(auth, r)?;
        let loses = |schema: &str| after.loses(self, schema);
        let of = |c: &ColumnDesc| c.collation.as_ref() == Some(&after.qn);
        let column = self.catalog.tables().into_iter().find(|t| loses(&t.name.auth) && t.columns.iter().any(of)).map(|t| t.name.clone());
        let domains: Vec<DomainDesc> = self
            .catalog
            .domains()
            .into_iter()
            .filter(|d| loses(&d.name.auth) && d.collation.as_ref() == Some(&after.qn))
            .cloned()
            .collect();
        let abandoned = self.naming_collation(&after.qn, &loses);
        if r.behavior == DropBehavior::Restrict {
            refuse_abandoning(&after, column.as_ref().or(domains.first().map(|d| &d.name)), &abandoned)?;
        } else if let Some(table) = column {
            return Err(api(format!("[DESIGN] a column of {} in table {table} is not reverted", after.qn)));
        }
        let mut views = self.views_collated(&after.qn, &loses);
        views.retain(|v| !abandoned.views.contains(&v.name));
        let mut catalog = self.catalog.duplicate();
        catalog.put_collation(after.desc);
        let mut locks: Vec<(QualifiedName, LockMode)> = Vec::new();
        for domain in domains {
            locks.push((domain.name.clone(), LockMode::Exclusive));
            catalog.put_domain(DomainDesc { collation: None, ..domain });
        }
        for view in views {
            locks.push((view.name.clone(), LockMode::Exclusive));
            catalog.remove_view(&view.name);
            catalog.add_views(vec![view]);
        }
        let (dropped, destroyed) = self.destroy(Arc::from(catalog), &abandoned)?;
        locks.extend(destroyed);
        let mut defined = Defined { dropped, locks, ..Defined::default() };
        if after.warned {
            defined.completion = Completion::Warning(SqlState::PRIVILEGE_NOT_REVOKED);
        }
        Ok(defined)
    }
}

/// [ISO/IEC 9075-2:2023 §12.7 GR12q; ISO/IEC 9075:1992 §11.37 SR18] That a
/// REVOKE with RESTRICT, leaving `after`, impacts no column or domain, the
/// first `impacted` if any, nor abandons a grant or anything of `abandoned`.
fn refuse_abandoning(after: &Revoked, impacted: Option<&QualifiedName>, abandoned: &Abandoned) -> Result<()> {
    let qn = &after.qn;
    if let Some(first) = impacted {
        let msg =
            format!("[ISO/IEC 9075-2:2023 §12.7 GR12q] revoking USAGE on {qn} impacts {first}, which is of it: revoke it with CASCADE");
        return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::DEPENDENT_PRIVILEGES));
    }
    if after.lost > 0 || !abandoned.is_empty() {
        let what = if abandoned.is_empty() { "a grant of it".to_owned() } else { abandoned.first() };
        let msg = format!("[ISO/IEC 9075:1992 §11.37 SR18] revoking USAGE on {qn} abandons {what}: revoke it with CASCADE");
        return Err(SqlError::new(Code::SyntaxRule, msg));
    }
    Ok(())
}
