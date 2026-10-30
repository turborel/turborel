//! \[EXTENSION\] [ISO/IEC 9075:1992 §11.37] REVOKE, in a transaction
//! (`db/definitions.rs`). Split from db/definitions.rs (RULES.md R-SIZE-4).
//!
//! The privileges the statement identifies, those its authorization
//! identifier granted [SR6], are destroyed, or with GRANT OPTION FOR made
//! not grantable. A privilege granted stands while its grantor holds it
//! with grant option: as the owner of a base table does, or of a view as
//! its tables allow, or by a privilege granted it that stands. What stands
//! is found from the owners' privileges up, a fixed point, so privileges
//! that grant each other in a cycle fall together: the others are
//! abandoned [SR9, SR11]. So is a view whose owner would no longer hold
//! SELECT on a table it reads [SR13], and a table constraint, assertion or
//! domain constraint whose owner would no longer hold the privileges it
//! needs [SR14–SR16]. RESTRICT refuses to abandon anything [SR18]; CASCADE
//! destroys what is abandoned: the privileges, the views with DROP VIEW
//! CASCADE [GR3], the table constraints with ALTER TABLE DROP CONSTRAINT
//! CASCADE [GR4], the assertions with DROP ASSERTION [GR5], the domain
//! constraints with ALTER DOMAIN DROP CONSTRAINT [GR6].
//!
//! The privileges of a view's owner on it are not kept but found from its
//! tables' (`catalog::applicable`), so they change with them [SR7b, SR10]:
//! one lost is abandoned, which RESTRICT refuses; one only no longer
//! grantable is modified, which it does not.

use std::sync::Arc;

use super::ddl::Dropped;
use super::definitions::Defined;
use super::schema::granted_actions;
use super::{Completion, State, rule};
use crate::ast::{AlterDomain, AlterDomainAction, DropBehavior, RevokeStatement, TableName};
use crate::binder::qualify;
use crate::catalog::{
    Actions, Catalog, ConstraintDesc, ConstraintKind, Privilege, PrivilegeDesc, QualifiedName, applicable, merged, owner_of,
};
use crate::error::{At, Code, Result, SqlError, SqlState};
use crate::txn::LockMode;

/// One privilege granted, as SQL-92's privilege descriptors are
/// [ISO/IEC 9075:1992 §11.36 GR2, GR3].
#[derive(Debug, Clone, PartialEq, Eq)]
struct Granted {
    grantor: String,
    grantee: Option<String>,
    table: QualifiedName,
    privilege: Privilege,
    grantable: bool,
}

/// Every privilege granted on every table and view of `catalog`.
fn granted(catalog: &dyn Catalog) -> Vec<Granted> {
    let names = catalog.tables().into_iter().map(|t| t.name.clone()).chain(catalog.views().into_iter().map(|v| v.name.clone()));
    let mut all = Vec::new();
    for name in names {
        for p in catalog.privileges(&name) {
            all.extend(p.actions.each().into_iter().map(|privilege| Granted {
                grantor: p.grantor.clone(),
                grantee: p.grantee.clone(),
                table: p.table.clone(),
                privilege,
                grantable: p.grantable,
            }));
        }
    }
    all
}

/// `catalog`, the privileges granted on its tables and views `grants`.
fn with_grants(catalog: &dyn Catalog, grants: &[&Granted]) -> Box<dyn Catalog> {
    let mut next = catalog.duplicate();
    let names = catalog.tables().into_iter().map(|t| t.name.clone()).chain(catalog.views().into_iter().map(|v| v.name.clone()));
    for name in names {
        let descs = grants.iter().filter(|g| g.table == name).map(|g| PrivilegeDesc {
            grantor: g.grantor.clone(),
            grantee: g.grantee.clone(),
            table: g.table.clone(),
            actions: Actions::of([g.privilege]),
            grantable: g.grantable,
        });
        next.set_privileges(&name, merged(descs.collect()));
    }
    next
}

/// Which of `grants` stand in `catalog`: each whose grantor holds its
/// privilege with grant option by those that stand, found from none up.
fn standing(catalog: &dyn Catalog, grants: &[Granted]) -> Vec<bool> {
    let mut stands = vec![false; grants.len()];
    loop {
        let now: Vec<&Granted> = grants.iter().zip(&stands).filter(|(_, s)| **s).map(|(g, _)| g).collect();
        let held = with_grants(catalog, &now);
        let next: Vec<bool> = grants.iter().map(|g| applicable(&*held, &g.grantor, &g.table).1.each().contains(&g.privilege)).collect();
        if next == stands {
            return stands;
        }
        stands = next;
    }
}

/// What a revoke would abandon [ISO/IEC 9075:1992 §11.37 SR11-SR16].
#[derive(Default)]
pub(super) struct Abandoned {
    grants: Vec<Granted>,
    pub(super) views: Vec<QualifiedName>,
    /// Privileges of views' owners on their views, lost.
    owners: Vec<QualifiedName>,
    /// Table constraints, each with its table [SR14].
    pub(super) constraints: Vec<(QualifiedName, ConstraintDesc)>,
    /// Domain constraints, each with its domain [SR16].
    pub(super) domain_constraints: Vec<(QualifiedName, QualifiedName)>,
    pub(super) assertions: Vec<QualifiedName>,
}

impl Abandoned {
    pub(super) fn is_empty(&self) -> bool {
        self.grants.is_empty()
            && self.views.is_empty()
            && self.owners.is_empty()
            && self.constraints.is_empty()
            && self.domain_constraints.is_empty()
            && self.assertions.is_empty()
    }

    /// The first of them, named for a refusal.
    pub(super) fn first(&self) -> String {
        if let Some(g) = self.grants.first() {
            let grantee = g.grantee.as_deref().unwrap_or("PUBLIC");
            return format!("a privilege {} granted {grantee} on {}", g.grantor, g.table);
        }
        if let Some(v) = self.views.first() {
            return format!("view {v}");
        }
        if let Some(v) = self.owners.first() {
            return format!("a privilege of {v}'s owner on it");
        }
        if let Some((t, c)) = self.constraints.first() {
            return format!("constraint {} of {t}", c.name);
        }
        if let Some((d, c)) = self.domain_constraints.first() {
            return format!("constraint {c} of domain {d}");
        }
        self.assertions.first().map(|a| format!("assertion {a}")).unwrap_or_default()
    }
}

impl State {
    /// [ISO/IEC 9075:1992 §11.37] REVOKE `r` by `auth`, its names in
    /// schema `schema`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the object is not defined, or, RESTRICT, something
    /// would be abandoned [SR18], or a privilege granted by one whose grant
    /// option goes depends on it (SQLSTATE 2B000) [GR2b]; -104 if `auth`
    /// holds no privilege on the object [AR1].
    pub(super) fn revoke(&mut self, (auth, schema): (&str, &str), r: &RevokeStatement) -> Result<Defined> {
        if r.domain {
            return self.revoke_usage((auth, schema), r).or_at(r.table.at);
        }
        let name = qualify(&r.table, schema);
        let table = self.object(&name).or_at(r.table.at)?;
        let (held, grantable) = applicable(&*self.catalog, auth, &name);
        if held.is_empty() {
            return Err(SqlError::new(Code::Privilege, format!("[ISO/IEC 9075:1992 §11.37 AR1] {auth} holds no privilege on {name}")));
        }
        // [§10.3 SR5] ALL PRIVILEGES: those it holds with grant option.
        let named = match &r.actions {
            None => grantable.each(),
            Some(list) => granted_actions(list, &table).or_at(r.table.at)?.each(),
        };
        let all = granted(&*self.catalog);
        // [SR6] Those it granted, of those named, to those named.
        let identified =
            |g: &Granted| g.grantor == auth && g.table == name && r.grantees.contains(&g.grantee) && named.contains(&g.privilege);
        let warned = not_revoked(r, &named, &all, &identified);
        let remaining: Vec<Granted> = all
            .iter()
            .filter_map(|g| match (identified(g), r.grant_option_for) {
                (false, _) => Some(g.clone()),
                // [GR2] the grant option goes, the privilege stays.
                (true, true) => Some(Granted { grantable: false, ..g.clone() }),
                (true, false) => None,
            })
            .collect();
        if r.grant_option_for && r.behavior == DropBehavior::Restrict {
            dependents(&all, &identified)?;
        }
        let stands = standing(&*self.catalog, &remaining);
        let kept: Vec<&Granted> = remaining.iter().zip(&stands).filter(|(_, s)| **s).map(|(g, _)| g).collect();
        let after = with_grants(&*self.catalog, &kept);
        let mut abandoned = self.abandoned(&*after);
        abandoned.grants = remaining.iter().zip(&stands).filter(|(_, s)| !**s).map(|(g, _)| g.clone()).collect();
        if r.behavior == DropBehavior::Restrict && !abandoned.is_empty() {
            return Err(rule(format!("[ISO/IEC 9075:1992 §11.37 SR18] REVOKE ... RESTRICT would abandon {}", abandoned.first())));
        }
        let (dropped, locks) = self.destroy(Arc::from(after), &abandoned)?;
        let completion = if warned { Completion::Warning(SqlState::PRIVILEGE_NOT_REVOKED) } else { Completion::Success };
        Ok(Defined { completion, dropped, locks, ..Defined::default() })
    }

    /// The views and constraints abandoned in `after`, the catalog with
    /// the privileges that stand [ISO/IEC 9075:1992 §11.37 SR13, SR14]; and
    /// the privileges of views' owners on them lost [SR11].
    fn abandoned(&self, after: &dyn Catalog) -> Abandoned {
        let mut abandoned = Abandoned::default();
        for v in self.catalog.views() {
            let owner = owner_of(&*self.catalog, &v.name.auth);
            if v.reads.iter().any(|t| !applicable(after, owner, t).0.select) {
                abandoned.views.push(v.name.clone());
                continue;
            }
            let (before, now) = (applicable(&*self.catalog, owner, &v.name).0, applicable(after, owner, &v.name).0);
            if before.common(&now) != before {
                abandoned.owners.push(v.name.clone());
            }
        }
        self.abandoned_constraints(after, &mut abandoned);
        abandoned
    }

    /// Adds to `abandoned` the constraints abandoned in `after`: of tables
    /// [SR14], of domains [SR16], each with its copies on columns, and
    /// assertions [SR15].
    fn abandoned_constraints(&self, after: &dyn Catalog, abandoned: &mut Abandoned) {
        let mut bound = self.clone();
        bound.catalog = Arc::from(after.duplicate());
        let lost = |r: Result<()>| r.is_err_and(|e| e.code == Code::Privilege);
        for d in self.catalog.domains() {
            for c in &d.constraints {
                let ConstraintKind::Check { text, names, .. } = &c.kind else { continue };
                if lost(bound.bound_on_value(&d.name, d.data_type, (text, names)).map(|_| ())) {
                    abandoned.domain_constraints.push((d.name.clone(), c.name.clone()));
                }
            }
        }
        for t in self.catalog.tables() {
            for c in &t.constraints {
                if self.referenceable(after, &t.name, c) {
                    continue;
                }
                // A copy of a domain's: the domain's constraint [SR16].
                let Some(dc) = self.domain_constraint_of(c) else {
                    abandoned.constraints.push((t.name.clone(), c.clone()));
                    continue;
                };
                if !abandoned.domain_constraints.contains(&dc) {
                    abandoned.domain_constraints.push(dc);
                }
            }
        }
        for a in self.catalog.assertions() {
            if lost(bound.assertion_plan(a).map(|_| ())) {
                abandoned.assertions.push(a.name.clone());
            }
        }
    }

    /// The domain, and the name, of the domain constraint `c` is a copy of,
    /// if it is one.
    fn domain_constraint_of(&self, c: &ConstraintDesc) -> Option<(QualifiedName, QualifiedName)> {
        let dc = c.domain.as_ref()?;
        let domain = self.catalog.domains().into_iter().find(|d| d.constraints.iter().any(|k| k.name == *dc))?;
        Some((domain.name.clone(), dc.clone()))
    }

    /// Whether the owner of `table` holds in `after` the REFERENCES its
    /// constraint `c` needs [ISO/IEC 9075:1992 §11.8 AR1, §11.9 AR1].
    fn referenceable(&self, after: &dyn Catalog, table: &QualifiedName, c: &ConstraintDesc) -> bool {
        match &c.kind {
            ConstraintKind::ForeignKey(fk) => {
                let held = applicable(after, &table.auth, &fk.table).0;
                fk.referenced.iter().all(|column| held.references.contains(column))
            }
            ConstraintKind::Check { text, names, horizon, .. } => {
                // Bound again as its owner's, in `after`: as it was defined.
                let mut bound = self.clone();
                bound.catalog = Arc::from(after.duplicate());
                bound.check_condition(table, (text, names, horizon)).map_or_else(|e| e.code != Code::Privilege, |_| true)
            }
            ConstraintKind::NotNull(_) | ConstraintKind::Unique { .. } => true,
        }
    }

    /// Makes `after` the catalog, the views, constraints and assertions
    /// `abandoned` dropped with CASCADE [ISO/IEC 9075:1992 §11.37 GR3-GR6]: what that
    /// took away, and the locks it needs.
    pub(super) fn destroy(&mut self, after: Arc<dyn Catalog>, abandoned: &Abandoned) -> Result<(Dropped, Vec<(QualifiedName, LockMode)>)> {
        self.catalog = after;
        let mut locks = Vec::new();
        let mut catalog = self.catalog.duplicate();
        for v in &abandoned.views {
            for gone in std::iter::once(v.clone()).chain(self.views_reading(v)) {
                catalog.remove_view(&gone);
                catalog.revoke_all(&gone);
                locks.push((gone, LockMode::Exclusive));
            }
        }
        self.catalog = Arc::from(catalog);
        let mut dropped = Dropped::default();
        for (table, c) in &abandoned.constraints {
            let name = TableName { auth: Some(c.name.auth.clone()), ident: c.name.ident.clone(), at: 0 };
            let taken = self.drop_constraint(table, &name, DropBehavior::Cascade)?;
            dropped.storage.extend(taken.storage);
            dropped.indexes.extend(taken.indexes);
            locks.push((table.clone(), LockMode::Exclusive));
        }
        // [GR6] ALTER DOMAIN DROP CONSTRAINT, with its copies.
        for (domain, c) in &abandoned.domain_constraints {
            let written = |q: &QualifiedName| TableName { auth: Some(q.auth.clone()), ident: q.ident.clone(), at: 0 };
            let statement = AlterDomain { name: written(domain), action: AlterDomainAction::DropConstraint(written(c)) };
            // Dropping a constraint makes no default: no displacement is needed.
            locks.extend(self.alter_domain((&domain.auth, 0), &statement)?.locks);
        }
        // [GR5] DROP ASSERTION.
        for a in &abandoned.assertions {
            self.drop_assertion(a)?;
            locks.push((a.clone(), LockMode::Exclusive));
        }
        Ok((dropped, locks))
    }
}

/// [ISO/IEC 9075:1992 §11.37 GR8, GR9] Whether a privilege `r` names is
/// not revoked from a grantee it names: none identified of one it names,
/// or, for ALL PRIVILEGES, none at all.
fn not_revoked(r: &RevokeStatement, named: &[Privilege], all: &[Granted], identified: &dyn Fn(&Granted) -> bool) -> bool {
    r.grantees.iter().any(|grantee| {
        let of = |p: &Privilege| all.iter().any(|g| identified(g) && g.grantee == *grantee && g.privilege == *p);
        match &r.actions {
            Some(_) => !named.iter().all(of),
            None => !named.iter().any(of),
        }
    })
}

/// [ISO/IEC 9075:1992 §11.37 GR2b] With GRANT OPTION FOR and RESTRICT: no
/// privilege granted by the grantee of one identified, by its grant option,
/// is left: of the same privilege on the same table.
fn dependents(all: &[Granted], identified: &dyn Fn(&Granted) -> bool) -> Result<()> {
    let by_option = |g: &Granted| {
        all.iter().any(|p| {
            identified(p)
                && p.grantable
                // [SR7a ii] the grantee of P is D's grantor, or PUBLIC.
                && p.grantee.as_deref().is_none_or(|grantee| grantee == g.grantor)
                && p.table == g.table
                && p.privilege == g.privilege
        })
    };
    match all.iter().find(|g| by_option(g)) {
        Some(g) => Err(SqlError::new(
            Code::SyntaxRule,
            format!(
                "[ISO/IEC 9075:1992 §11.37 GR2b] {} granted a privilege on {} by the grant option revoked: use CASCADE",
                g.grantor, g.table
            ),
        )
        .with_state(SqlState::DEPENDENT_PRIVILEGES)),
        None => Ok(()),
    }
}
