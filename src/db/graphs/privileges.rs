//! \[EXTENSION\] SELECT on a property graph (§20): granted and revoked as
//! a privilege on a view is, its owner's found from what the graph reads
//! (`GraphDesc::select_of`). A grant stands while its grantor holds SELECT
//! on the graph with grant option [ISO/IEC 9075:1992 §11.37 SR9]: one whose
//! grantor no longer does is abandoned with it, found as a fixed point
//! from the owner's up. A revoke abandons the views whose owners it leaves
//! without SELECT on a graph they read, which RESTRICT refuses.

use std::sync::Arc;

use super::super::definitions::Defined;
use super::super::revoke::Abandoned;
use super::super::{Completion, State};
use super::graph_rule;
use crate::ast::{Action, DropBehavior, PrivilegeDefinition, RevokeStatement};
use crate::binder::qualify;
use crate::catalog::{GraphDesc, QualifiedName, UsageDesc, sole};
use crate::error::{Code, Result, SqlError, SqlState};

/// SELECT on a graph as a revoke leaves it, and what that abandons.
struct Revoked {
    desc: GraphDesc,
    /// Whether it named a grantee it was not granted to.
    warned: bool,
    /// Whether a grant of another grantee goes with those named.
    lost: bool,
    abandoned: Abandoned,
}

impl State {
    /// GRANT SELECT ON PROPERTY GRAPH `p` by `auth`, its names in schema
    /// `schema`: granted if `auth` holds it grantably, else a warning
    /// [ISO/IEC 9075:1992 §11.36 GR14].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it grants other than SELECT, or there is no such
    /// graph (GRF-0001); -104 if `auth` holds no SELECT on it.
    pub(in crate::db) fn grant_on_graph(&mut self, auth: &str, schema: &str, p: &PrivilegeDefinition) -> Result<Defined> {
        select_alone(p.actions.as_deref())?;
        let mut desc = self.graph_named(&qualify(&p.table, schema))?;
        let (held, grantable) = desc.select_of(&*self.catalog, sole(&*self.catalog, auth));
        if !held {
            let msg = format!("[EXTENSION] {auth} holds no SELECT on property graph {} (§20)", desc.name);
            return Err(SqlError::new(Code::Privilege, msg));
        }
        if !grantable {
            return Ok(Defined { completion: Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED), ..Defined::default() });
        }
        for grantee in &p.grantees {
            match desc.select.iter_mut().find(|u| u.grantor == auth && u.grantee == *grantee) {
                Some(u) => u.grantable |= p.grant_option,
                None => desc.select.push(UsageDesc { grantor: auth.to_owned(), grantee: grantee.clone(), grantable: p.grant_option }),
            }
        }
        self.put_graph(desc);
        Ok(Defined::default())
    }

    /// REVOKE SELECT ON PROPERTY GRAPH `r` by `auth`, its names in schema
    /// `schema`: the grants `auth` made those it names, or their grant
    /// option alone, gone; with CASCADE, what that abandons gone too. A
    /// warning if `auth` granted it to one it names not [ISO/IEC 9075:1992
    /// §11.37 GR].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it revokes other than SELECT, or there is no such
    /// graph; with RESTRICT, -103 and SQLSTATE 2B000 if it abandons
    /// anything [§11.37 SR18]; as each drop's.
    pub(in crate::db) fn revoke_on_graph(&mut self, (auth, schema): (&str, &str), r: &RevokeStatement) -> Result<Defined> {
        let revoked = self.graph_revoked((auth, schema), r)?;
        if r.behavior == DropBehavior::Restrict && (revoked.lost || !revoked.abandoned.is_empty()) {
            let what = if revoked.abandoned.is_empty() { "a grant of it".to_owned() } else { revoked.abandoned.first() };
            let msg = format!(
                "[ISO/IEC 9075:1992 §11.37 SR18] revoking SELECT on property graph {} abandons {what}: revoke it with CASCADE",
                revoked.desc.name
            );
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::DEPENDENT_PRIVILEGES));
        }
        self.put_graph(revoked.desc);
        let (dropped, locks) = self.destroy(Arc::clone(&self.catalog), &revoked.abandoned)?;
        let completion = if revoked.warned { Completion::Warning(SqlState::PRIVILEGE_NOT_REVOKED) } else { Completion::Success };
        Ok(Defined { completion, dropped, locks, ..Defined::default() })
    }

    /// SELECT on the graph REVOKE `r` by `auth` names, as it leaves it,
    /// and what that abandons.
    fn graph_revoked(&self, (auth, schema): (&str, &str), r: &RevokeStatement) -> Result<Revoked> {
        select_alone(r.actions.as_deref())?;
        let mut desc = self.graph_named(&qualify(&r.table, schema))?;
        if !desc.select_of(&*self.catalog, sole(&*self.catalog, auth)).0 {
            let msg = format!("[ISO/IEC 9075:1992 §11.37 AR1] {auth} holds no privilege on property graph {}", desc.name);
            return Err(SqlError::new(Code::Privilege, msg));
        }
        let named = |u: &UsageDesc| u.grantor == auth && r.grantees.contains(&u.grantee);
        let before = desc.select.clone();
        let warned = !r.grantees.iter().all(|g| before.iter().any(|u| u.grantor == auth && u.grantee == *g));
        if r.grant_option_for {
            desc.select.iter_mut().filter(|u| named(u)).for_each(|u| u.grantable = false);
        } else {
            desc.select.retain(|u| !named(u));
        }
        self.standing_of(&mut desc);
        let lost = before.iter().any(|u| !named(u) && !desc.select.contains(u));
        let mut after = self.clone();
        after.put_graph(desc.clone());
        let abandoned = self.abandoned_in(&after, &|e| e.code == Code::Privilege);
        Ok(Revoked { desc, warned, lost, abandoned })
    }

    /// Of graph `qn`, the grants whose grantors hold SELECT on it with
    /// grant option no more, as a change of what it reads leaves it,
    /// abandoned.
    pub(in crate::db) fn graph_grants_standing(&mut self, qn: &QualifiedName) {
        let Some(mut desc) = self.catalog.graph(qn).cloned() else { return };
        let before = desc.select.len();
        self.standing_of(&mut desc);
        if desc.select.len() != before {
            self.put_graph(desc);
        }
    }

    /// [ISO/IEC 9075:1992 §11.37 SR9] Of `desc`, the grants that stand:
    /// each pass takes one at least, so there are no more than grants.
    fn standing_of(&self, desc: &mut GraphDesc) {
        for _ in 0..desc.select.len() {
            let kept = desc.clone();
            desc.select.retain(|u| kept.select_of(&*self.catalog, sole(&*self.catalog, &u.grantor)).1);
            if desc.select.len() == kept.select.len() {
                break;
            }
        }
    }
}

/// That `actions` are SELECT alone, as of a graph only SELECT is granted
/// or revoked (§20); ALL PRIVILEGES is SELECT.
///
/// # Errors
///
/// SQLCODE -103 if one is another.
fn select_alone(actions: Option<&[Action]>) -> Result<()> {
    if actions.is_some_and(|list| list.iter().any(|a| *a != Action::Select)) {
        return Err(graph_rule(None, "of a property graph, SELECT alone is granted or revoked (§20)".to_owned()));
    }
    Ok(())
}
