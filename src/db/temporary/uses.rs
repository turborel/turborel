//! \[DESIGN\] The database's instances of its temporary tables, of every
//! session, and what ALTER TABLE of one does to them (RULES.md D49). Split
//! from db/temporary.rs (RULES.md R-SIZE-2).
//!
//! Each session's instances are noted, by session. An ALTER TABLE of a
//! temporary table marks it altered by its transaction, so no other
//! transaction begins on an instance of it until that one ends; one that
//! adds or drops a column gives each instance that holds rows a successor,
//! of the new definition, its rows copied there, which the session takes
//! in its place once the transaction commits, or which is forgotten if it
//! rolls back. Any other action changes no row, so leaves each instance in
//! place.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};

use super::storage::Rows_;
use super::{Instance, Instances};
use crate::catalog::QualifiedName;
use crate::error::{Code, Result, SqlError};
use crate::sync::lock_builtin;
use crate::txn::{Timestamp, TxId};

/// \[DESIGN\] The sessions' instances, and the temporary tables being
/// altered (RULES.md D49).
#[derive(Default)]
pub(crate) struct Uses(Mutex<Registry>);

/// The instances of each session, and how each temporary table is used.
#[derive(Default)]
struct Registry {
    sessions: Vec<Weak<Instances>>,
    tables: HashMap<QualifiedName, Use>,
}

/// A temporary table's altering transaction, and the successors it made
/// of the instances, each with its session.
#[derive(Default)]
struct Use {
    altering: Option<TxId>,
    successors: Vec<(Weak<Instances>, Instance)>,
}

/// A conflict with another transaction (SQLCODE -911).
fn conflict(message: String) -> SqlError {
    SqlError::new(Code::Conflict, message)
}

impl Registry {
    /// The sessions still there, with their instances.
    fn sessions(&mut self) -> Vec<Arc<Instances>> {
        self.sessions.retain(|w| w.strong_count() > 0);
        self.sessions.iter().filter_map(Weak::upgrade).collect()
    }

    /// The instance of temporary table `name` of `session`, `weak` it
    /// downgraded: the successor the transaction altering it made, or the
    /// instance of generation `generation`.
    fn instance_of(&self, session: &Instances, weak: &Weak<Instances>, (name, generation): (&QualifiedName, u32)) -> Option<Instance> {
        let succeeded = self.tables.get(name).and_then(|u| u.successors.iter().find(|(s, _)| s.ptr_eq(weak)));
        succeeded.map(|(_, i)| i.clone()).or_else(|| lock_builtin(session).get(&(name.clone(), generation)).cloned())
    }
}

impl Uses {
    /// Notes `made`, a session's instances, once.
    pub(super) fn enlist(&self, made: &Arc<Instances>) {
        let mut registry = lock_builtin(&self.0);
        let known = registry.sessions.iter().any(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(made)));
        if !known {
            registry.sessions.retain(|w| w.strong_count() > 0);
            registry.sessions.push(Arc::downgrade(made));
        }
    }

    /// The temporary tables a transaction is altering.
    pub(super) fn altering(&self) -> HashSet<QualifiedName> {
        lock_builtin(&self.0).tables.iter().filter(|(_, u)| u.altering.is_some()).map(|(name, _)| name.clone()).collect()
    }

    /// That transaction `tx` may begin on `rows`, an instance's: no other
    /// transaction is altering its table. It is noted as begun.
    ///
    /// # Errors
    ///
    /// SQLCODE -911 if one is.
    pub(super) fn may_write(&self, rows: &Rows_, tx: TxId) -> Result<()> {
        let registry = lock_builtin(&self.0);
        let name = &rows.desc.name;
        let altering = registry.tables.get(name).and_then(|u| u.altering);
        if altering.is_some_and(|other| other != tx) {
            return Err(conflict(format!(
                "[DESIGN] temporary table {name} is being altered: its rows are read and written once that transaction ends (RULES.md D49)"
            )));
        }
        let mut open = lock_builtin(&rows.open);
        if !open.contains(&tx) {
            open.push(tx);
        }
        Ok(())
    }

    /// [ISO/IEC 9075:1992 §11.10] Transaction `tx`, reading at `snapshot`,
    /// alters temporary table `name`: no other begins on an instance of it
    /// until it ends. Any session's instance may hold rows; none is open to
    /// another transaction, so none of its transactions reads the table as
    /// it was, and `tx` sees each one's rows as they are.
    ///
    /// # Errors
    ///
    /// SQLCODE -911 if another transaction has an instance's rows open, or
    /// wrote them and committed after `snapshot`.
    pub(in crate::db) fn alter(&self, name: &QualifiedName, (tx, snapshot): (TxId, Timestamp)) -> Result<()> {
        let mut registry = lock_builtin(&self.0);
        for session in registry.sessions() {
            for ((of, _), instance) in lock_builtin(&session).iter() {
                let rows = &instance.rows;
                if of != name || rows.made().is_none() {
                    continue;
                }
                if rows.open_to_other(tx) {
                    return Err(conflict(format!(
                        "[DESIGN] another transaction has the rows of a session's instance of temporary table {name} open: it is altered once that one ends (RULES.md D49)"
                    )));
                }
                if rows.written_after(snapshot) {
                    return Err(conflict(format!(
                        "[DESIGN] a session's instance of temporary table {name} has rows committed after this transaction began, which it does not see: it is altered by a transaction begun since (RULES.md D49)"
                    )));
                }
            }
        }
        registry.tables.entry(name.clone()).or_default().altering = Some(tx);
        Ok(())
    }

    /// The instance of temporary table `name` of each session that has
    /// one: the successor the transaction altering it made, or the
    /// instance of generation `generation`.
    pub(super) fn instances_of(&self, name: &QualifiedName, generation: u32) -> Vec<(Weak<Instances>, Instance)> {
        let mut registry = lock_builtin(&self.0);
        let sessions = registry.sessions();
        let mut out = Vec::new();
        for session in sessions {
            let weak = Arc::downgrade(&session);
            if let Some(instance) = registry.instance_of(&session, &weak, (name, generation)) {
                out.push((weak, instance));
            }
        }
        out
    }

    /// `session`'s instance of temporary table `name`, as `instances_of`
    /// gives it; none if it has none, or is gone.
    pub(super) fn instance_of(&self, session: &Weak<Instances>, name: &QualifiedName, generation: u32) -> Option<Instance> {
        let registry = lock_builtin(&self.0);
        let made = session.upgrade()?;
        registry.instance_of(&made, session, (name, generation))
    }

    /// `successor` takes the place of `session`'s instance of temporary
    /// table `name` if the transaction altering it commits.
    pub(super) fn succeed(&self, name: &QualifiedName, session: &Weak<Instances>, successor: Instance) {
        let mut registry = lock_builtin(&self.0);
        let used = registry.tables.entry(name.clone()).or_default();
        used.successors.retain(|(s, _)| !s.ptr_eq(session));
        used.successors.push((Weak::clone(session), successor));
    }

    /// ALTER TABLE of temporary table `name`, of a transaction that
    /// committed at `at`, published generation `generation` of it: each
    /// session's instance of an earlier one is read and written no more
    /// (`TableLock::check_generation`), and each successor takes its
    /// place.
    pub(in crate::db) fn published(&self, name: &QualifiedName, generation: u32, at: Timestamp) {
        let mut registry = lock_builtin(&self.0);
        let sessions = registry.sessions();
        for session in &sessions {
            for ((of, g), instance) in lock_builtin(session).iter() {
                if of == name && *g < generation {
                    instance.lock.replaced(generation, at);
                }
            }
        }
        let successors = registry.tables.get_mut(name).map(|u| std::mem::take(&mut u.successors)).unwrap_or_default();
        for (session, successor) in successors {
            let Some(session) = session.upgrade() else { continue };
            let made = successor.rows.desc.generation;
            successor.lock.replaced(made, at);
            lock_builtin(&session).insert((name.clone(), made), successor);
        }
    }

    /// Transaction `tx` ended: what it altered may be used again, and the
    /// successors it made and did not publish, its transaction rolled
    /// back, are forgotten, by their sessions too.
    pub(super) fn ended(&self, tx: TxId) {
        let mut registry = lock_builtin(&self.0);
        for (name, used) in &mut registry.tables {
            if used.altering != Some(tx) {
                continue;
            }
            used.altering = None;
            for (session, successor) in used.successors.drain(..) {
                let Some(session) = session.upgrade() else { continue };
                let mut made = lock_builtin(&session);
                let key = (name.clone(), successor.rows.desc.generation);
                if made.get(&key).is_some_and(|i| Arc::ptr_eq(&i.rows, &successor.rows)) {
                    made.remove(&key);
                }
            }
        }
    }
}
