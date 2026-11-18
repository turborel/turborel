//! \[EXTENSION\] [ISO/IEC 9075:1992 §11.37] REVOKE, in a transaction
//! (`db/definitions.rs`).
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
use super::generated::Column;
use super::schema::granted_actions;
use super::{Completion, State, api, rule};
use crate::ast::{AlterDomain, AlterDomainAction, DropBehavior, PrivilegeObject, RevokeStatement, TableName};
use crate::binder::qualify;
use crate::catalog::{
    Actions, Catalog, ConstraintDesc, ConstraintKind, Privilege, PrivilegeDesc, QualifiedName, SYSTEM_GRANTOR, TableDesc, applicable,
    merged, owner_of, sole,
};
use crate::error::{At, Code, Result, SqlError, SqlState};
use crate::txn::LockMode;

/// One privilege granted, as SQL-92's privilege descriptors are
/// [ISO/IEC 9075:1992 §11.36 GR2, GR3].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::db) struct Granted {
    pub(in crate::db) grantor: String,
    pub(in crate::db) grantee: Option<String>,
    pub(in crate::db) table: QualifiedName,
    pub(in crate::db) privilege: Privilege,
    pub(in crate::db) grantable: bool,
}

/// Every privilege granted on every table and view of `catalog`.
pub(in crate::db) fn granted(catalog: &dyn Catalog) -> Vec<Granted> {
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
pub(in crate::db) fn with_grants(catalog: &dyn Catalog, grants: &[&Granted]) -> Box<dyn Catalog> {
    let mut next = catalog.duplicate();
    let names = catalog.tables().into_iter().map(|t| t.name.clone()).chain(catalog.views().into_iter().map(|v| v.name.clone()));
    for name in names {
        let descs = grants.iter().filter(|g| g.table == name).map(|g| PrivilegeDesc {
            grantor: g.grantor.clone(),
            grantee: g.grantee.clone(),
            table: g.table.clone(),
            actions: Actions::of([g.privilege.clone()]),
            grantable: g.grantable,
        });
        next.set_privileges(&name, merged(descs.collect()));
    }
    next
}

/// Which of `grants` stand in `catalog`: each whose grantor holds its
/// privilege with grant option by those that stand, found from none up.
pub(in crate::db) fn standing(catalog: &dyn Catalog, grants: &[Granted]) -> Vec<bool> {
    let mut stands = vec![false; grants.len()];
    loop {
        let now: Vec<&Granted> = grants.iter().zip(&stands).filter(|(_, s)| **s).map(|(g, _)| g).collect();
        let held = with_grants(catalog, &now);
        // [ISO/IEC 9075-2:2023 §11.49 GR6] one of the special grantor
        // stands as its trigger does.
        let holds = |g: &Granted| {
            g.grantor == SYSTEM_GRANTOR || applicable(&*held, sole(&*held, &g.grantor), &g.table).1.each().contains(&g.privilege)
        };
        let next: Vec<bool> = grants.iter().map(holds).collect();
        if next == stands {
            return stands;
        }
        stands = next;
    }
}

/// What a revoke would abandon [ISO/IEC 9075:1992 §11.37 SR11-SR16].
#[derive(Default)]
pub(in crate::db) struct Abandoned {
    pub(in crate::db) grants: Vec<Granted>,
    pub(in crate::db) views: Vec<QualifiedName>,
    /// Privileges of views' owners on their views, lost.
    pub(in crate::db) owners: Vec<QualifiedName>,
    /// Table constraints, each with its table [SR14].
    pub(in crate::db) constraints: Vec<(QualifiedName, ConstraintDesc)>,
    /// Domain constraints, each with its domain [SR16].
    pub(in crate::db) domain_constraints: Vec<(QualifiedName, QualifiedName)>,
    pub(in crate::db) assertions: Vec<QualifiedName>,
    /// \[DESIGN\] Generated columns whose generation expressions name the
    /// object, each with its table, lost (`db/generated/named.rs`).
    pub(in crate::db) columns: Vec<Column>,
    /// [ISO/IEC 9075-2:2023 §12.7 SR3b] Triggers whose owner loses a
    /// privilege their definition needs.
    pub(in crate::db) triggers: Vec<QualifiedName>,
    /// [ISO/IEC 9075-2:2023 §12.7 GR12o, §11.62 GR1] SQL-invoked routines,
    /// by specific name, whose owner loses a privilege their bodies need,
    /// or whose bodies invoke a routine dropped.
    pub(in crate::db) routines: Vec<QualifiedName>,
}

impl Abandoned {
    pub(in crate::db) fn is_empty(&self) -> bool {
        self.grants.is_empty()
            && self.views.is_empty()
            && self.owners.is_empty()
            && self.constraints.is_empty()
            && self.domain_constraints.is_empty()
            && self.assertions.is_empty()
            && self.columns.is_empty()
            && self.triggers.is_empty()
            && self.routines.is_empty()
    }

    /// The first of them, named for a refusal.
    pub(in crate::db) fn first(&self) -> String {
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
        if let Some(a) = self.assertions.first() {
            return format!("assertion {a}");
        }
        if let Some(t) = self.triggers.first() {
            return format!("trigger {t}");
        }
        if let Some(r) = self.routines.first() {
            return format!("routine {r}");
        }
        self.columns.first().map(|(t, c)| format!("generated column {t}.{c}")).unwrap_or_default()
    }
}

impl State {
    /// REVOKE `r` by `auth`, its names in schema `schema`, of an object
    /// other than a table or view: a collation, a domain, a sequence
    /// generator, a module, a type, a routine or a registered XML Schema,
    /// each as its own revoke takes it; none of a table or view.
    fn revoke_of_other(&mut self, (auth, schema): (&str, &str), r: &RevokeStatement) -> Option<Result<Defined>> {
        let revoked = match r.object {
            PrivilegeObject::Collation => self.revoke_collation_usage((auth, schema), r),
            PrivilegeObject::Domain => self.revoke_usage((auth, schema), r).or_at(r.table.at),
            PrivilegeObject::Sequence => self.revoke_sequence_usage((auth, schema), r).or_at(r.table.at),
            PrivilegeObject::Module => self.revoke_on_module((auth, schema), r).or_at(r.table.at),
            PrivilegeObject::Model => self.revoke_model_execute((auth, schema), r).or_at(r.table.at),
            PrivilegeObject::XmlSchema => self.revoke_xml_schema_usage((auth, schema), r).or_at(r.table.at),
            PrivilegeObject::Type | PrivilegeObject::SpecificRoutine(_) | PrivilegeObject::Routine(_) => {
                self.revoke_on_type((auth, schema), r).or_at(r.table.at)
            }
            PrivilegeObject::Table => return None,
        };
        Some(revoked)
    }
    /// [ISO/IEC 9075:1992 §11.37; ISO/IEC 9075-2:2023 §12.7] The
    /// privileges `r` names on `table`, revoked by `auth` of schema
    /// `schema`: of ALL PRIVILEGES, those it holds with grant option
    /// (`grantable`) [§10.3 SR5]; of HIERARCHY OPTION FOR, the option of
    /// SELECT alone, SELECT staying [§12.7 GR].
    fn revoked_privileges(
        &self,
        (auth, schema): (&str, &str),
        r: &RevokeStatement,
        (table, grantable): (&TableDesc, &Actions),
    ) -> Result<Vec<Privilege>> {
        let mut named = match &r.actions {
            None => grantable.each(),
            Some(list) => {
                let methods = self.privilege_methods((auth, schema), table, (&r.methods, "§12.7 AR1a iii"))?;
                granted_actions(list, table, &methods).or_at(r.table.at)?.each()
            }
        };
        if r.hierarchy_option_for {
            named.retain(|p| *p == Privilege::Select);
            named = if named.is_empty() { Vec::new() } else { vec![Privilege::Hierarchy] };
        }
        Ok(named)
    }

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
        if let Some(revoked) = self.revoke_of_other((auth, schema), r) {
            return revoked;
        }
        let name = qualify(&r.table, schema);
        let table = self.object(&name).or_at(r.table.at)?;
        let (held, grantable) = applicable(&*self.catalog, sole(&*self.catalog, auth), &name);
        if held.is_empty() {
            return Err(SqlError::new(Code::Privilege, format!("[ISO/IEC 9075:1992 §11.37 AR1] {auth} holds no privilege on {name}")));
        }
        let named = self.revoked_privileges((auth, schema), r, (&table, &grantable))?;
        let mut going = named.clone();
        if going.contains(&Privilege::Select) && !going.contains(&Privilege::Hierarchy) {
            going.push(Privilege::Hierarchy);
        }
        // [ISO/IEC 9075-2:2023 §12.7 SR1c] SELECT takes SELECT on the
        // table/method pair of each method with it.
        let methods_too = going.contains(&Privilege::Select) && !r.hierarchy_option_for;
        let all = granted(&*self.catalog);
        // [SR6] Those it granted, of those named, to those named.
        let identified = |g: &Granted| {
            let going_too = going.contains(&g.privilege) || (methods_too && matches!(g.privilege, Privilege::Method(_)));
            g.grantor == auth && g.table == name && r.grantees.contains(&g.grantee) && going_too
        };
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
            return Err(abandoning(&abandoned));
        }
        let (dropped, locks) = self.destroy(Arc::from(after), &abandoned)?;
        let completion = if warned { Completion::Warning(SqlState::PRIVILEGE_NOT_REVOKED) } else { Completion::Success };
        Ok(Defined { completion, dropped, locks, ..Defined::default() })
    }

    /// The views and constraints abandoned in `after`, the catalog with
    /// the privileges that stand [ISO/IEC 9075:1992 §11.37 SR13, SR14]; and
    /// the privileges of views' owners on them lost [SR11].
    pub(in crate::db) fn abandoned(&self, after: &dyn Catalog) -> Abandoned {
        let mut abandoned = Abandoned::default();
        for v in self.catalog.views() {
            let owner = owner_of(&*self.catalog, &v.name.auth);
            if v.reads.iter().any(|t| !applicable(after, sole(after, owner), t).0.select) {
                abandoned.views.push(v.name.clone());
                continue;
            }
            let (before, now) =
                (applicable(&*self.catalog, sole(&*self.catalog, owner), &v.name).0, applicable(after, sole(after, owner), &v.name).0);
            if before.common(&now) != before {
                abandoned.owners.push(v.name.clone());
            }
        }
        self.abandoned_constraints(after, &mut abandoned);
        abandoned
    }

    /// Adds to `abandoned` the constraints abandoned in `after`: of tables
    /// [SR14], of domains [SR16], each with its copies on columns, and
    /// assertions [SR15]; and the triggers and routines abandoned.
    fn abandoned_constraints(&self, after: &dyn Catalog, abandoned: &mut Abandoned) {
        let mut bound = self.clone();
        bound.catalog = Arc::from(after.duplicate());
        let lost = |r: Result<()>| r.is_err_and(|e| e.code == Code::Privilege);
        for d in self.catalog.domains() {
            for c in &d.constraints {
                let ConstraintKind::Check { text, names, horizon, .. } = &c.kind else { continue };
                if lost(bound.bound_on_value(&d.name, d.data_type.clone(), (text, names, horizon)).map(|_| ())) {
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
        for t in self.catalog.triggers() {
            if lost(bound.trigger_binds(t)) {
                abandoned.triggers.push(t.name.clone());
            }
        }
        // [ISO/IEC 9075-2:2023 §12.7 GR12o] routines whose bodies need
        // what the owner no longer holds.
        for r in self.catalog.routines() {
            if lost(bound.routine_binds(r)) {
                abandoned.routines.push(r.specific.clone());
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
                let held = applicable(after, sole(after, owner_of(after, &table.auth)), &fk.table).0;
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
    ///
    /// # Errors
    ///
    /// As each drop's; SQLCODE -910 if it abandons a generated column,
    /// which goes first.
    pub(in crate::db) fn destroy(
        &mut self,
        after: Arc<dyn Catalog>,
        abandoned: &Abandoned,
    ) -> Result<(Dropped, Vec<(QualifiedName, LockMode)>)> {
        // Dropped first, as their rows are copied (`db/generated/named.rs`).
        if let Some((t, c)) = abandoned.columns.first() {
            return Err(api(format!("[DESIGN] generated column {t}.{c} is lost, and was not dropped first")));
        }
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
            let taken = self.drop_constraint((table, &table.auth), &name, DropBehavior::Cascade)?;
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
            // With the table of its kept totals, if any.
            let taken = self.drop_assertion(a)?;
            dropped.storage.extend(taken.storage);
            locks.extend(taken.names.into_iter().map(|name| (name, LockMode::Exclusive)));
        }
        // [ISO/IEC 9075-2:2023 §12.7 GR6] DROP TRIGGER.
        let triggers: Vec<QualifiedName> = abandoned.triggers.iter().filter(|t| self.catalog.trigger(t).is_some()).cloned().collect();
        for t in &triggers {
            // A pipeline's goes with the pipeline, and so its others.
            let pipeline = self.catalog.embeddings().pipelines.iter().find(|p| p.triggers().contains(t)).map(|p| p.name.clone());
            let taken = match (pipeline, self.catalog.trigger(t).is_some()) {
                (_, false) => continue,
                (Some(p), true) => self.drop_pipeline(&p)?,
                (None, true) => self.drop_trigger(t)?,
            };
            locks.extend(taken.names.iter().map(|name| (name.clone(), LockMode::Exclusive)));
            dropped.absorb(taken);
        }
        // [ISO/IEC 9075-2:2023 §12.7 GR12o] DROP ROUTINE ... CASCADE.
        let taken = self.drop_routines(&abandoned.routines)?;
        locks.extend(taken.names.iter().map(|name| (name.clone(), LockMode::Exclusive)));
        dropped.absorb(taken);
        Ok((dropped, locks))
    }

    /// [ISO/IEC 9075-2:2023 §11.50 GR1] The privileges `revoked`, on view
    /// `view` by `SYSTEM_GRANTOR` to `grantee`, revoked with CASCADE: what
    /// that took away, the names of what it abandoned among them.
    ///
    /// # Errors
    ///
    /// As each drop's.
    pub(super) fn revoke_system(&mut self, view: &QualifiedName, (revoked, grantee): (&[Privilege], &str)) -> Result<Dropped> {
        let all = granted(&*self.catalog);
        let identified = |g: &Granted| {
            g.grantor == SYSTEM_GRANTOR && g.table == *view && g.grantee.as_deref() == Some(grantee) && revoked.contains(&g.privilege)
        };
        let remaining: Vec<Granted> = all.iter().filter(|g| !identified(g)).cloned().collect();
        let stands = standing(&*self.catalog, &remaining);
        let kept: Vec<&Granted> = remaining.iter().zip(&stands).filter(|(_, s)| **s).map(|(g, _)| g).collect();
        let after = with_grants(&*self.catalog, &kept);
        let abandoned = self.abandoned(&*after);
        let (mut dropped, locks) = self.destroy(Arc::from(after), &abandoned)?;
        dropped.names.extend(locks.into_iter().map(|(name, _)| name));
        Ok(dropped)
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

/// [ISO/IEC 9075-2:2023 §12.7 GR12q] The refusal of a REVOKE with RESTRICT
/// that would abandon what `abandoned` holds: SQLSTATE 2B000.
pub(in crate::db) fn abandoning(abandoned: &Abandoned) -> SqlError {
    let msg =
        format!("[ISO/IEC 9075-2:2023 §12.7 GR12q; ISO/IEC 9075:1992 §11.37 SR18] REVOKE ... RESTRICT would abandon {}", abandoned.first());
    rule(msg).with_state(SqlState::DEPENDENT_PRIVILEGES)
}
