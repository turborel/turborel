//! \[DESIGN\] The table an INSERT puts rows in [X3.135-1986 §8.7 SR1-SR4],
//! in two steps: resolved, its table or view found, through an updatable
//! view its base table's [§6.5 GR2], its object columns and its
//! privileges checked; then made of each statement, its defaults, its
//! override checked and what it generates.
//!
//! A session keeps what it resolved for the INSERTs it runs directly, by
//! the table and the columns they name as written, while what a prepared
//! statement's plan is kept with holds (`prepared::Key`, RULES.md D45):
//! the snapshot's generation and catalog, so every change of definitions,
//! privileges granted and revoked among them; the authorization, its role
//! and the session user; the default schema, which qualifies the name;
//! the session's own settings. Any of them changed, it resolves again, so
//! its Syntax and Access Rules are checked against what holds then. A
//! table resolved through a view that reads the statement's instant is
//! not kept, as such a binding holds a value. So INSERTs differing in
//! their values alone, as the texts run directly with their values
//! written in them are, resolve their table once.

use std::cell::Cell;
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::Arc;

use super::super::defaults::default_row;
use super::super::generated::{Generating, Inserted, check_override};
use super::super::prepared::Key;
use super::super::{Session, State};
use super::{INSERT_RULES, InsertInto, Target, object_columns};
use crate::ast::InsertStatement;
use crate::binder::{self, ViewCheck, qualify};
use crate::catalog::{Actions, Principal, QualifiedName, TableDesc, WHOLE};
use crate::error::{At, Result};
use crate::hash::xxhash::{Xxh3BuildHasher, Xxh3Map};
use crate::value::datetime::Now;

/// \[DESIGN\] The INSERTs' tables a session keeps resolved: past them all
/// go at once, as `Activity`'s texts do, since a session inserting into
/// that many tables in turn gains little from keeping them.
const TARGETS_KEPT: usize = 256;

/// An INSERT's table, resolved, before what each statement makes of it.
#[derive(Debug, Clone)]
struct Resolved {
    /// The base table's name.
    name: QualifiedName,
    base: Arc<TableDesc>,
    /// The object columns, as columns of the base table.
    object: Vec<usize>,
    checks: Vec<ViewCheck>,
}

/// The table and the columns an INSERT names, as written.
#[derive(Debug)]
struct Shape {
    auth: Option<String>,
    ident: String,
    columns: Option<Vec<String>>,
}

impl Shape {
    /// What `stmt` names.
    fn of(stmt: &InsertStatement) -> Shape {
        let columns = stmt.columns.as_ref().map(|list| list.iter().map(|c| c.name.clone()).collect());
        Shape { auth: stmt.table.auth.clone(), ident: stmt.table.ident.clone(), columns }
    }

    /// Whether `stmt` names it.
    fn is(&self, stmt: &InsertStatement) -> bool {
        let columns = match (self.columns.as_deref(), stmt.columns.as_deref()) {
            (None, None) => true,
            (Some(kept), Some(list)) => kept.iter().eq(list.iter().map(|c| &c.name)),
            (None, Some(_)) | (Some(_), None) => false,
        };
        columns && self.ident == stmt.table.ident && self.auth == stmt.table.auth
    }

    /// The hash of what `stmt` names, as a `Shape` of it would hash.
    fn hash_of(stmt: &InsertStatement) -> u64 {
        let mut h = Xxh3BuildHasher::default().build_hasher();
        stmt.table.auth.hash(&mut h);
        stmt.table.ident.hash(&mut h);
        stmt.columns.as_ref().map(Vec::len).hash(&mut h);
        for column in stmt.columns.iter().flatten() {
            column.name.hash(&mut h);
        }
        h.finish()
    }
}

/// The INSERTs' tables a session resolved, and what they were resolved
/// with.
#[derive(Debug, Default)]
pub(in crate::db) struct Targets {
    key: Option<Key>,
    by: Xxh3Map<u64, (Shape, Resolved)>,
}

impl Targets {
    /// What `stmt`'s table was resolved as, if it was and what it was
    /// resolved with holds now for `s` in `st`; all forgotten if it holds
    /// no more.
    fn found(&mut self, (st, s): (&State, &Session), hash: u64, stmt: &InsertStatement) -> Option<Resolved> {
        if !self.key.as_ref().is_some_and(|k| k.holds(st, s)) {
            self.key = None;
            self.by.clear();
            return None;
        }
        self.by.get(&hash).filter(|(shape, _)| shape.is(stmt)).map(|(_, resolved)| resolved.clone())
    }

    /// Keeps `resolved`, of `stmt`, with what holds now.
    fn keep(&mut self, (st, s): (&State, &Session), hash: u64, (stmt, resolved): (&InsertStatement, Resolved)) {
        if !self.key.as_ref().is_some_and(|k| k.holds(st, s)) {
            self.key = Some(Key::now(st, s));
            self.by.clear();
        }
        if self.by.len() >= TARGETS_KEPT {
            self.by.clear();
        }
        self.by.insert(hash, (Shape::of(stmt), resolved));
    }
}

impl Session {
    /// The table the INSERT `stmt` run directly puts rows in, as
    /// `insert_target` gives it, every table seen whole: resolved once
    /// while what it was resolved with holds.
    pub(super) fn insert_target_kept(&self, st: &State, stmt: &InsertStatement) -> Result<Target> {
        let into = InsertInto {
            table: &stmt.table,
            columns: stmt.columns.as_deref(),
            given: (Inserted::Source(&stmt.source), stmt.overriding),
            rules: &INSERT_RULES,
            horizon: &WHOLE,
        };
        let authorization = self.authorization();
        let user = authorization.of(st.catalog.roles());
        let hash = Shape::hash_of(stmt);
        let found = self.targets.borrow_mut().found((st, self), hash, stmt);
        let resolved = if let Some(kept) = found {
            kept
        } else {
            let noted = Cell::new(false);
            let made = resolved(st, &into, (user, &self.default_schema()), Some(&noted))?;
            // One whose view read the statement's instant holds it: not kept.
            if !noted.get() {
                self.targets.borrow_mut().keep((st, self), hash, (stmt, made.clone()));
            }
            made
        };
        completed(st, &into, resolved, (user, self.now.get()))
    }
}

/// The table `into` inserts into, as `Session::insert_target_of` gives
/// it, of a statement `user`'s, its unqualified names in `schema`, its
/// defaults those of `now`.
pub(in crate::db) fn insert_target_in(
    st: &State,
    into: &InsertInto<'_>,
    (user, schema, now): (Principal<'_>, &str, Now),
    noted: Option<&Cell<bool>>,
) -> Result<Target> {
    completed(st, into, resolved(st, into, (user, schema), noted)?, (user, now))
}

/// The table `into` inserts into, resolved, of a statement `user`'s, its
/// unqualified names in `schema`. A view's read of the statement's
/// instant is noted in `noted`, if given.
fn resolved(st: &State, into: &InsertInto<'_>, (user, schema): (Principal<'_>, &str), noted: Option<&Cell<bool>>) -> Result<Resolved> {
    let rules = into.rules;
    let name = qualify(into.table, schema);
    let target = binder::write_target(st, &name, (rules.read_only, user), noted).or_at(into.table.at)?;
    let desc = &target.named;
    let object: Vec<usize> = match into.columns {
        // [EXTENSION] (§4.2) but a node or edge table's pseudo-columns.
        None => (0..into.horizon.width(&name).map_or(desc.columns.len(), |w| w.min(desc.columns.len())))
            .filter(|&k| desc.columns.get(k).is_some_and(|c| !c.hidden()))
            .collect(),
        Some(list) => object_columns(&name, &desc.columns, list, rules)?,
    };
    // [ISO/IEC 9075:1992 §13.8 AR1; ISO/IEC 9075-2:2023 §14.12 AR1b
    // iii] INSERT on each column named, or on each of the table's.
    let needed = Actions { insert: object.iter().copied().collect(), ..Actions::default() };
    binder::require(st, user.cell, desc, &needed, rules.privilege).or_at(into.table.at)?;
    // [X3.135-1986 §8.7 GR1a] the candidate row is one of the base table.
    let object: Vec<usize> = object.iter().filter_map(|&i| target.map.get(i).copied()).collect();
    Ok(Resolved { name: target.base.name.clone(), base: Arc::new(target.base), object, checks: target.checks })
}

/// The table `into` inserts into, `resolved`, made of its statement:
/// `user`'s defaults, those of `now`; its override checked; what it
/// generates.
fn completed(st: &State, into: &InsertInto<'_>, resolved: Resolved, (user, now): (Principal<'_>, Now)) -> Result<Target> {
    let Resolved { name, base, object, checks } = resolved;
    let defaults = default_row(&base, user, now)?;
    // [ISO/IEC 9075-2:2023 §14.11 SR12, SR13, §14.12 SR18f, SR18h] its
    // identity column and generated columns given values only as its
    // override says.
    let draws = check_override(&base, &object, into.given).or_at(into.table.at)?;
    let generating = Generating::of(st, &base, draws)?;
    Ok(Target { name, desc: base, object, checks, defaults, generating })
}
