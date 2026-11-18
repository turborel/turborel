//! \[EXTENSION\] USAGE on a registered XML Schema, granted and revoked
//! [ISO/IEC 9075-14:2006 draft §4.7.1]: Part 14 leaves how to the
//! implementation, and turborel grants and revokes it as ISO/IEC
//! 9075-2:2023 does USAGE on a sequence generator [§12.2, §12.3, §12.7],
//! by GRANT and REVOKE USAGE ON XML SCHEMA.
//!
//! Its schema's owner holds it with grant option, and every user holds
//! it on a permanently registered XML Schema, as PUBLIC does, without
//! grant option (`catalog/xml_schemas/permanent.rs`): a GRANT of one
//! grants nothing, a REVOKE revokes nothing. A revoke takes away the
//! grants it names, or their grant option, and with them each grant left
//! with no grantor holding it grantably, abandoned [§12.7 GR9], and each
//! registered XML Schema that uses it, of a schema whose owner it leaves
//! without USAGE, abandoned too, as a view is, and dropped as DROP XML
//! SCHEMA with CASCADE drops it: with RESTRICT, abandoning either is
//! refused.

use std::sync::Arc;

use super::super::definitions::Defined;
use super::super::definitions::xml_schemas::XmlSchemaDependents;
use super::super::{Completion, State, rule};
use super::usage_alone;
use crate::ast::{DropBehavior, PrivilegeDefinition, RevokeStatement};
use crate::binder::qualify;
use crate::catalog::xml_schemas::permanent;
use crate::catalog::{Catalog, QualifiedName, UsageDesc, XmlSchemaDesc, owner_of, sole, usage_held};
use crate::error::{Code, Result, SqlError, SqlState};

/// What a GRANT or REVOKE names.
enum Named {
    /// A registered XML Schema the catalog holds, and its schema's owner.
    Held(XmlSchemaDesc, String),
    /// A permanently registered XML Schema [§4.2.6 Table 1].
    Permanent,
}

/// A warning `state`, as a GRANT or REVOKE that did less than it named
/// completes.
pub(super) fn warned(state: SqlState) -> Defined {
    Defined { completion: Completion::Warning(state), ..Defined::default() }
}

impl State {
    /// The registered XML Schema `qn`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is none [ISO/IEC 9075-2:2023 §12.2 SR2].
    fn xml_schema_named(&self, qn: &QualifiedName) -> Result<Named> {
        if permanent(qn).is_some() {
            return Ok(Named::Permanent);
        }
        let Some(desc) = self.catalog.xml_schema(qn).cloned() else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §12.2 SR2] there is no registered XML Schema {qn}")));
        };
        Ok(Named::Held(desc, owner_of(&*self.catalog, &qn.auth).to_owned()))
    }

    /// Records `desc`, its grants changed.
    fn put_xml_schema_usage(&mut self, desc: XmlSchemaDesc) {
        let mut catalog = self.catalog.duplicate();
        catalog.put_xml_schema(desc);
        self.catalog = Arc::from(catalog);
    }

    /// \[EXTENSION\] [ISO/IEC 9075-2:2023 §12.2] GRANT USAGE ON XML SCHEMA
    /// `p` by `auth`, its names in schema `schema`: granted if `auth`
    /// holds it grantably, else a warning \[GR9\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it grants other than USAGE [§12.3 SR4a], or there is
    /// no such registered XML Schema \[SR2\]; -104 if `auth` holds no USAGE
    /// on it \[AR1\].
    pub(in crate::db) fn grant_xml_schema_usage(&mut self, auth: &str, schema: &str, p: &PrivilegeDefinition) -> Result<Defined> {
        usage_alone(p.actions.as_deref())?;
        let qn = qualify(&p.table, schema);
        let Named::Held(mut desc, owner) = self.xml_schema_named(&qn)? else {
            return Ok(warned(SqlState::PRIVILEGE_NOT_GRANTED));
        };
        let (held, grantable) = desc.usage_of(sole(&*self.catalog, auth), &owner);
        if !held {
            let msg = format!("[ISO/IEC 9075-2:2023 §12.2 AR1] {auth} holds no USAGE on registered XML Schema {qn}");
            return Err(SqlError::new(Code::Privilege, msg));
        }
        if !grantable {
            return Ok(warned(SqlState::PRIVILEGE_NOT_GRANTED));
        }
        granted(&mut desc.usage, auth, p);
        self.put_xml_schema_usage(desc);
        Ok(Defined::default())
    }

    /// \[EXTENSION\] [ISO/IEC 9075-2:2023 §12.7] REVOKE USAGE ON XML
    /// SCHEMA `r` by `auth`, its names in schema `schema`: the grants
    /// `auth` made those it names, or their grant option alone, gone, and
    /// with CASCADE the grants they abandon \[GR9\]; and, of each schema
    /// whose owner it leaves without USAGE, what names it, abandoned as a
    /// view needing a privilege is \[GR12a\] (`State::xml_schema_dependents`):
    /// the registered XML Schemas that use it, dropped as DROP XML SCHEMA
    /// with CASCADE drops them, and the views, constraints, assertions,
    /// triggers and routines that indicate it, destroyed. A column, domain
    /// or user-defined type of an XML type of it is refused, as DROP XML
    /// SCHEMA refuses it. A warning if `auth` granted it to one it names
    /// not \[GR12ai\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it revokes other than USAGE [§12.3 SR4a], or there
    /// is no such registered XML Schema; with RESTRICT, -103 and SQLSTATE
    /// 2B000 if anything is abandoned, and with CASCADE if a column,
    /// domain or user-defined type is \[GR12q\]; as DROP XML SCHEMA of each
    /// registered XML Schema abandoned, and the drops of what else is.
    pub(in crate::db) fn revoke_xml_schema_usage(&mut self, (auth, schema): (&str, &str), r: &RevokeStatement) -> Result<Defined> {
        usage_alone(r.actions.as_deref())?;
        let qn = qualify(&r.table, schema);
        let Named::Held(mut desc, owner) = self.xml_schema_named(&qn)? else {
            return Ok(warned(SqlState::PRIVILEGE_NOT_REVOKED));
        };
        let (lost, not_revoked) = usage_revoked(&*self.catalog, (&mut desc.usage, &owner), auth, r);
        let held = &*self.catalog;
        let loses = |s: &str| !desc.usage_of(sole(held, owner_of(held, s)), &owner).0;
        let dependents = self.xml_schema_dependents(&qn, &loses);
        refuse_abandoning(&qn, &dependents, (lost, r.behavior))?;
        let mut catalog = self.catalog.duplicate();
        catalog.put_xml_schema(desc);
        let (dropped, locks) = self.destroy(Arc::from(catalog), &dependents.abandoned)?;
        let completion = if not_revoked { Completion::Warning(SqlState::PRIVILEGE_NOT_REVOKED) } else { Completion::Success };
        let mut defined = Defined { completion, dropped, locks, ..Defined::default() };
        for dependent in &dependents.schemas {
            if self.catalog.xml_schema(dependent).is_some() {
                defined.dropped.absorb(self.drop_xml_schema(dependent, true)?);
            }
        }
        Ok(defined)
    }
}

/// [ISO/IEC 9075-2:2023 §12.2 GR4] `usage`, the grants of USAGE on a
/// registered XML Schema or XQuery module, with GRANT `p` by `auth`: each
/// grantee's grant by `auth` added, or made grantable if `p` grants the
/// grant option.
pub(super) fn granted(usage: &mut Vec<UsageDesc>, auth: &str, p: &PrivilegeDefinition) {
    for grantee in &p.grantees {
        match usage.iter_mut().find(|u| u.grantor == auth && u.grantee == *grantee) {
            Some(u) => u.grantable |= p.grant_option,
            None => usage.push(UsageDesc { grantor: auth.to_owned(), grantee: grantee.clone(), grantable: p.grant_option }),
        }
    }
}

/// `usage`, the grants of USAGE on a registered XML Schema or XQuery
/// module of `catalog`, its schema's owner `owner`, as REVOKE `r` by
/// `auth` leaves them: those `auth` made those it names, or their grant
/// option alone, gone, then each left with no grantor holding it
/// grantably, abandoned [ISO/IEC 9075-2:2023 §12.7 GR9]. Whether a grant
/// not named went, and whether one named was not made \[GR12ai\].
pub(super) fn usage_revoked(
    catalog: &dyn Catalog,
    (usage, owner): (&mut Vec<UsageDesc>, &str),
    auth: &str,
    r: &RevokeStatement,
) -> (bool, bool) {
    let named = |u: &UsageDesc| u.grantor == auth && r.grantees.contains(&u.grantee);
    let made = |grantee: &Option<String>| usage.iter().any(|u| u.grantor == auth && u.grantee == *grantee);
    let not_revoked = !r.grantees.iter().all(made);
    let before = usage.clone();
    if r.grant_option_for {
        usage.iter_mut().filter(|u| named(u)).for_each(|u| u.grantable = false);
    } else {
        usage.retain(|u| !named(u));
    }
    loop {
        let held = usage.clone();
        usage.retain(|u| usage_held(&held, sole(catalog, &u.grantor), owner).1);
        if usage.len() == held.len() {
            break;
        }
    }
    (before.iter().any(|u| !named(u) && !usage.contains(u)), not_revoked)
}

/// [ISO/IEC 9075-2:2023 §12.7 GR12q] That REVOKE USAGE on registered XML
/// Schema `qn`, of `behavior`, abandoning a grant if `lost`, may abandon
/// `dependents`: with RESTRICT, nothing; with CASCADE, no column, domain or
/// user-defined type of an XML type of it, which nothing drops, as DROP
/// XML SCHEMA refuses one even with CASCADE.
///
/// # Errors
///
/// SQLCODE -103, SQLSTATE 2B000, if it may not.
fn refuse_abandoning(qn: &QualifiedName, dependents: &XmlSchemaDependents, (lost, behavior): (bool, DropBehavior)) -> Result<()> {
    let what = if let Some(t) = dependents.typed.first() {
        format!("{t}, which is of an XML type of it and is dropped by nothing, even with CASCADE")
    } else if behavior == DropBehavior::Cascade {
        return Ok(());
    } else if let Some(s) = dependents.schemas.first() {
        format!("registered XML Schema {s}: revoke it with CASCADE")
    } else if !dependents.abandoned.is_empty() {
        format!("{}: revoke it with CASCADE", dependents.abandoned.first())
    } else if lost {
        "a grant of it: revoke it with CASCADE".to_owned()
    } else {
        return Ok(());
    };
    let msg = format!("[ISO/IEC 9075-2:2023 §12.7 GR12q] revoking USAGE on {qn} abandons {what}");
    Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::DEPENDENT_PRIVILEGES))
}
