//! [ISO/IEC 9075-2:2023 §12.7 GR8, GR9, GR12] The privileges a change of
//! role authorizations leaves, and what that abandons.
//!
//! A privilege granted by a role stands while that role holds it grantably,
//! through its own applicable roles too [GR2b i 2]; one granted by a user,
//! through none. So with the role authorizations a REVOKE or DROP ROLE
//! leaves, every privilege of every kind is settled again: each grant whose
//! grantor holds it grantably no more is abandoned, kind by kind, as the
//! revoke of each settles its own (`db/revoke.rs`, `db/usage.rs` and the
//! others), until none changes, as a domain's owner's grant option follows
//! the REFERENCES its constraints need. What then binds no more, or whose
//! owner holds a USAGE it needs no more, is abandoned as each kind's revoke
//! finds it.

use std::sync::Arc;

use super::super::State;
use super::super::revoke::{Abandoned, granted, standing, with_grants};
use super::super::usage::holds;
use crate::catalog::{Catalog, QualifiedName, Roles, SYSTEM_GRANTOR, UsageDesc, Who, owner_of, usage_held};
use crate::error::{Code, Result};

/// What a change of role authorizations abandons [ISO/IEC 9075-2:2023
/// §12.7 GR9, GR12].
#[derive(Default)]
pub(super) struct Lost {
    /// A privilege abandoned, the first, as a refusal names it.
    pub(super) grant: Option<String>,
    /// The views, constraints, assertions, generated columns, triggers and
    /// routines abandoned.
    pub(super) abandoned: Abandoned,
    /// The types based on a type whose schema's owner holds USAGE on it no
    /// more [GR12j].
    pub(super) types: Vec<QualifiedName>,
    /// Each collation a definition made, and the schemas whose owners hold
    /// USAGE on it no more: its columns there impacted [GR12n].
    pub(super) collations: Vec<(QualifiedName, Vec<String>)>,
}

/// `grants` with each whose grantor holds it no more grantably, as `holds`
/// says of those left, gone, until none goes; one of the special grantor
/// stays [ISO/IEC 9075-2:2023 §11.60 GR1].
fn retained(grants: &mut Vec<UsageDesc>, holds: &dyn Fn(&[UsageDesc], &str) -> bool) {
    loop {
        let held = grants.clone();
        grants.retain(|u| u.grantor == SYSTEM_GRANTOR || holds(&held, &u.grantor));
        if grants.len() == held.len() {
            return;
        }
    }
}

/// Every grant of `catalog`, of each kind, as one line: so two catalogs'
/// are compared.
fn every_grant(catalog: &dyn Catalog) -> Vec<String> {
    let line = |kind: &str, on: &QualifiedName, u: &UsageDesc| {
        format!("a privilege {} granted {} on {kind} {on}", u.grantor, u.grantee.as_deref().unwrap_or("PUBLIC"))
    };
    let mut all: Vec<String> = granted(catalog)
        .iter()
        .map(|g| format!("a privilege {} granted {} on {}", g.grantor, g.grantee.as_deref().unwrap_or("PUBLIC"), g.table))
        .collect();
    for d in catalog.domains() {
        all.extend(d.usage.iter().map(|u| line("domain", &d.name, u)));
    }
    for c in catalog.collations() {
        all.extend(c.usage.iter().map(|u| line("collation", &c.name, u)));
    }
    for s in catalog.sequences() {
        all.extend(s.usage.iter().map(|u| line("sequence", &s.name, u)));
    }
    for t in catalog.user_types() {
        all.extend(t.usage.iter().chain(&t.to_distinct.execute).chain(&t.to_source.execute).map(|u| line("type", &t.name, u)));
    }
    for r in catalog.routines() {
        all.extend(r.execute.iter().chain(&r.debug).map(|u| line("routine", &r.specific, u)));
    }
    for m in &catalog.embeddings().models {
        all.extend(m.execute.iter().map(|u| line("embedding model", &m.name, u)));
    }
    all
}

impl State {
    /// These definitions with the role authorizations `roles`, and with no
    /// privilege granted to or by the role `gone`, if given, which is dropped:
    /// each privilege settled, and the first abandoned, if any.
    ///
    /// # Errors
    ///
    /// A domain's constraint binds no more as its owner's grant option is
    /// found (`State::owner_grantable`).
    pub(super) fn settled(&self, roles: Roles, gone: Option<&str>) -> Result<(State, Option<String>)> {
        let mut after = self.clone();
        after.put_roles(roles);
        if let Some(role) = gone {
            after.revoke_from(role);
        }
        let before = every_grant(&*after.catalog);
        loop {
            let tables = after.settle_tables();
            if !after.settle_usage()? && !tables {
                break;
            }
        }
        let now = every_grant(&*after.catalog);
        let lost = before.into_iter().find(|g| !now.contains(g));
        Ok((after, lost))
    }

    /// Every privilege granted to `role` or by it gone.
    fn revoke_from(&mut self, role: &str) {
        let to = |u: &UsageDesc| u.grantee.as_deref() != Some(role) && u.grantor != role;
        let kept: Vec<_> =
            granted(&*self.catalog).into_iter().filter(|g| g.grantee.as_deref() != Some(role) && g.grantor != role).collect();
        let mut catalog = with_grants(&*self.catalog, &kept.iter().collect::<Vec<_>>());
        for mut d in self.catalog.domains().into_iter().cloned() {
            d.usage.retain(to);
            catalog.put_domain(d);
        }
        for mut c in self.catalog.collations().into_iter().cloned() {
            c.usage.retain(to);
            catalog.put_collation(c);
        }
        for mut s in self.catalog.sequences().into_iter().cloned() {
            s.usage.retain(to);
            catalog.put_sequence(s);
        }
        for mut t in self.catalog.user_types().into_iter().cloned() {
            t.usage.retain(to);
            t.to_distinct.execute.retain(to);
            t.to_source.execute.retain(to);
            catalog.put_user_type(t);
        }
        for mut r in self.catalog.routines().into_iter().cloned() {
            r.execute.retain(to);
            r.debug.retain(to);
            catalog.put_routine(r);
        }
        let mut embeddings = self.catalog.embeddings().clone();
        for m in &mut embeddings.models {
            m.execute.retain(to);
        }
        catalog.set_embeddings(embeddings);
        self.catalog = Arc::from(catalog);
    }

    /// The privileges on tables and views settled (`revoke::standing`):
    /// whether any went.
    fn settle_tables(&mut self) -> bool {
        let all = granted(&*self.catalog);
        let stands = standing(&*self.catalog, &all);
        if stands.iter().all(|s| *s) {
            return false;
        }
        let kept: Vec<_> = all.iter().zip(&stands).filter(|(_, s)| **s).map(|(g, _)| g).collect();
        self.catalog = Arc::from(with_grants(&*self.catalog, &kept));
        true
    }

    /// USAGE on each domain, collation, sequence generator and type, and
    /// EXECUTE on each routine and embedding model, settled: whether any went.
    ///
    /// # Errors
    ///
    /// As `settled`.
    fn settle_usage(&mut self) -> Result<bool> {
        let before = every_grant(&*self.catalog).len();
        let cat = Arc::clone(&self.catalog);
        let roles = cat.roles();
        let mut next = cat.duplicate();
        for mut d in cat.domains().into_iter().cloned() {
            let (owner, owners) = (owner_of(&*cat, &d.name.auth), self.owner_grantable(&d)?);
            retained(&mut d.usage, &|held, a| holds(held, Who::sole(roles, a), (owner, owners)).1);
            next.put_domain(d);
        }
        for mut c in cat.collations().into_iter().cloned() {
            let owner = owner_of(&*cat, &c.name.auth);
            retained(&mut c.usage, &|held, a| usage_held(held, Who::sole(roles, a), owner).1);
            next.put_collation(c);
        }
        for mut s in cat.sequences().into_iter().cloned() {
            let owner = owner_of(&*cat, &s.name.auth);
            retained(&mut s.usage, &|held, a| usage_held(held, Who::sole(roles, a), owner).1);
            next.put_sequence(s);
        }
        for mut t in cat.user_types().into_iter().cloned() {
            let owner = owner_of(&*cat, &t.name.auth);
            let by = |held: &[UsageDesc], a: &str| usage_held(held, Who::sole(roles, a), owner).1;
            retained(&mut t.usage, &by);
            retained(&mut t.to_distinct.execute, &by);
            retained(&mut t.to_source.execute, &by);
            next.put_user_type(t);
        }
        for mut r in cat.routines().into_iter().cloned() {
            let by = |held: &[UsageDesc], a: &str| held.iter().any(|u| u.grantable && Who::sole(roles, a).holds(u.grantee.as_deref()));
            retained(&mut r.execute, &by);
            retained(&mut r.debug, &by);
            next.put_routine(r);
        }
        let mut embeddings = cat.embeddings().clone();
        for m in &mut embeddings.models {
            let owner = owner_of(&*cat, &m.name.auth);
            retained(&mut m.execute, &|held, a| usage_held(held, Who::sole(roles, a), owner).1);
        }
        next.set_embeddings(embeddings);
        self.catalog = Arc::from(next);
        Ok(every_grant(&*self.catalog).len() != before)
    }

    /// What `after`, these definitions as a change of role authorizations
    /// leaves them, abandons [ISO/IEC 9075-2:2023 §12.7 GR12]: what binds
    /// no more as its owner's (`abandoned_in`), the views whose owners
    /// hold SELECT no more and the constraints whose owners hold
    /// REFERENCES no more (`State::abandoned`), the types based on a type
    /// whose schema's owner holds USAGE on it no more, with the columns of
    /// them, and the collations whose USAGE a schema's owner no more holds.
    pub(super) fn lost_in(&self, after: &State, grant: Option<String>) -> Lost {
        let mut abandoned = self.abandoned_in(after, &|e| e.code == Code::Privilege);
        joined(&mut abandoned, self.abandoned(&*after.catalog));
        let mut types = Vec::new();
        for t in after.catalog.user_types() {
            let owner = owner_of(&*after.catalog, &t.name.auth);
            for qn in after.types_unheld(t, owner) {
                if !types.contains(&qn) {
                    types.push(qn);
                }
            }
            let unheld = |s: &str| !t.usage_of(Who::sole(after.catalog.roles(), owner_of(&*after.catalog, s)), owner).0;
            for table in self.catalog.tables().into_iter().filter(|table| unheld(&table.name.auth)) {
                let of = table.columns.iter().filter(|c| crate::db::structured::names_type(&c.data_type, &t.name));
                abandoned.columns.extend(of.map(|c| (table.name.clone(), c.name.clone())));
            }
        }
        Lost { grant, abandoned, types, collations: self.collations_unheld(after) }
    }

    /// Each collation a definition made, and the schemas of these
    /// definitions whose owners hold USAGE on it here but not in `after`.
    fn collations_unheld(&self, after: &State) -> Vec<(QualifiedName, Vec<String>)> {
        let mut found = Vec::new();
        for c in self.catalog.collations() {
            let Some(now) = after.catalog.collation(&c.name) else { continue };
            let owner = owner_of(&*self.catalog, &c.name.auth);
            let held = |st: &State, desc: &crate::catalog::CollationDesc, s: &str| {
                desc.usage_of(Who::sole(st.catalog.roles(), owner_of(&*st.catalog, s)), owner).0
            };
            let schemas: Vec<String> =
                self.catalog.schemas().into_iter().filter(|s| held(self, c, s) && !held(after, now, s)).map(str::to_owned).collect();
            if !schemas.is_empty() {
                found.push((c.name.clone(), schemas));
            }
        }
        found
    }
}

/// `more` added to `into`, each not there already.
pub(super) fn joined(into: &mut Abandoned, more: Abandoned) {
    fn add<T: PartialEq>(into: &mut Vec<T>, more: Vec<T>) {
        for m in more {
            if !into.contains(&m) {
                into.push(m);
            }
        }
    }
    add(&mut into.grants, more.grants);
    add(&mut into.views, more.views);
    add(&mut into.owners, more.owners);
    add(&mut into.constraints, more.constraints);
    add(&mut into.domain_constraints, more.domain_constraints);
    add(&mut into.assertions, more.assertions);
    add(&mut into.columns, more.columns);
    add(&mut into.triggers, more.triggers);
    add(&mut into.routines, more.routines);
}
