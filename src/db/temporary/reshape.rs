//! [ISO/IEC 9075:1992 §11.10, §11.11 GR2, §11.15 GR4, §11.16 GR2] ALTER
//! TABLE of a temporary table while sessions' instances of it hold rows
//! (RULES.md D49). Split from db/temporary.rs (RULES.md R-SIZE-2).
//!
//! A column added or dropped gives each instance that holds rows a
//! successor of the new generation, its rows copied there by the altering
//! transaction, each with the column added, of its default [§11.11 GR2],
//! or removed [§11.15 GR4], as `db/ddl.rs` copies a persistent table's.
//! Each session takes its successor once the transaction commits
//! (`Uses::published`). Any other action changes no row: the table's one
//! definition is every session's at once; a constraint added is checked
//! of each session's rows [§11.16 GR2].
//!
//! Each session's rows are checked as that session sees the database
//! (`Session::seen_by`): each temporary table its own instance, as its
//! constraints read them. No other transaction writes them meanwhile
//! (`Uses::alter`), so another session's rows are checked at once.

use std::cell::RefCell;
use std::sync::{Arc, Weak};

use super::{Instance, Instances};
use crate::ast::AlterAction;
use crate::catalog::{IndexDesc, QualifiedName, TableDesc, TemporaryKind};
use crate::db::alter::Reshape;
use crate::db::ddl::{copy_rows, validate_rows};
use crate::db::integrity::{Cursored, Deferral, Effects};
use crate::db::{Session, State, api};
use crate::error::Result;
use crate::txn::Txn;

impl Session {
    /// [ISO/IEC 9075:1992 §11.10] Transaction `tx`'s ALTER TABLE `action`
    /// of temporary table `name`, its rows to change as `reshape` says,
    /// from `st` to `next`: each instance's rows, of every session, copied
    /// to a successor.
    ///
    /// # Errors
    ///
    /// SQLCODE -201 if a row breaks a constraint; -911 if another
    /// transaction has the rows of an instance a constraint reads open; the
    /// copy's.
    pub(in crate::db) fn reshape_instances(
        &self,
        (st, next): (&Arc<State>, &Arc<State>),
        tx: &Txn,
        (name, action, reshape): (&QualifiedName, &AlterAction, Reshape),
    ) -> Result<()> {
        let was = st.catalog.table(name).ok_or_else(|| api(format!("[DESIGN] no table {name}")))?;
        let desc = next.catalog.table(name).cloned().ok_or_else(|| api(format!("[DESIGN] no table {name}")))?;
        let indexes: Vec<IndexDesc> = next.catalog.indexes(name).into_iter().cloned().collect();
        let own = Arc::downgrade(&self.temporaries.made);
        let delete_rows = desc.temporary.is_some_and(|t| t.delete_rows);
        let mut made = Vec::new();
        for (session, source) in st.temporaries.instances_of(name, was.generation) {
            let mine = session.ptr_eq(&own);
            // Another session's rows of ON COMMIT DELETE ROWS are gone as
            // its transaction ended, which none has open.
            if source.rows.made().is_none() || (delete_rows && !mine) {
                continue;
            }
            let successor = Instance::of(&desc, (&self.instance_place(st), &st.temporaries));
            successor.rows.add_empty(&indexes);
            if mine {
                let to = successor.on(next, name, true);
                copy_rows(&source.on(st, name, false), &to, tx, name, (action, reshape, self.effects(&to)))?;
            } else {
                let from = self.seen_by(st, &session, (&[], tx))?;
                let to = successor.on(&*self.seen_by(next, &session, (&[], tx))?, name, true);
                self.at_once(&to, |effects| copy_rows(&from, &to, tx, name, (action, reshape, effects)))?;
            }
            made.push((session, successor, mine));
        }
        // Each made once every copy is: a refusal leaves the successors an
        // earlier ALTER TABLE of the transaction made.
        for (session, successor, mine) in made {
            st.temporaries.succeed(name, &session, successor.clone());
            if mine {
                crate::sync::lock_builtin(&self.temporaries.made).insert((name.clone(), desc.generation), successor);
            }
        }
        Ok(())
    }

    /// [ISO/IEC 9075:1992 §11.16 GR2] Transaction `tx`'s ALTER TABLE
    /// `action` of temporary table `name`, from `st` to `next`, which
    /// changes no row: the session's own rows satisfy the table's
    /// constraints, as `validate_rows` checks a persistent table's; and,
    /// for a constraint added, each other session's rows, at once, as it
    /// sees the database. Each instance checked is given entries of each
    /// index added, of its rows (`Rows_::entries_for`).
    ///
    /// # Errors
    ///
    /// SQLCODE -201 if a row breaks a constraint; -911 if another
    /// transaction has the rows of an instance a constraint reads open; the
    /// reads'.
    pub(in crate::db) fn validate_instances(
        &self,
        (st, next): (&Arc<State>, &Arc<State>),
        tx: &Txn,
        (name, action): (&QualifiedName, &AlterAction),
    ) -> Result<()> {
        let had = st.catalog.indexes(name);
        let added: Vec<String> =
            next.catalog.indexes(name).into_iter().filter(|ix| !had.iter().any(|h| h.name == ix.name)).map(|ix| ix.name.clone()).collect();
        let own = Arc::downgrade(&self.temporaries.made);
        let mine = self.seen_by(next, &own, (&added, tx))?;
        validate_rows(&mine, tx, name, self.effects(&mine))?;
        // As for a copy: rows of ON COMMIT DELETE ROWS are another session's
        // only while its transaction is open, which none is.
        let delete_rows = next.catalog.table(name).is_some_and(|t| t.temporary.is_some_and(|k| k.delete_rows));
        if delete_rows || !matches!(action, AlterAction::AddConstraint(_)) {
            return Ok(());
        }
        for (session, instance) in st.temporaries.instances_of(name, st.generation_of(name)) {
            if session.ptr_eq(&own) || instance.rows.made().is_none() {
                continue;
            }
            let theirs = self.seen_by(next, &session, (&added, tx))?;
            self.at_once(&theirs, |effects| validate_rows(&theirs, tx, name, effects))?;
        }
        Ok(())
    }

    /// `st` as `session` sees it, for transaction `tx` to check its rows:
    /// each global and created local temporary table its instance, the
    /// successor `tx` made of it if any, with its entries of each index
    /// `st` gives the table, those of `added` made of the rows `tx` sees
    /// (`Rows_::entries_for`); empty if it has none.
    /// Its constraints read no declared local temporary table [§11.8 SR4,
    /// §11.9 SR3].
    ///
    /// # Errors
    ///
    /// The reads' and index's, making entries.
    fn seen_by(&self, st: &Arc<State>, session: &Weak<Instances>, (added, tx): (&[String], &Txn)) -> Result<Arc<State>> {
        let place = self.instance_place(st);
        let temporary: Vec<TableDesc> =
            st.catalog.tables().into_iter().filter(|t| t.temporary.is_some_and(|k| k.kind != TemporaryKind::Declared)).cloned().collect();
        let mut copy = State::clone(st);
        for desc in temporary {
            let instance = st
                .temporaries
                .instance_of(session, &desc.name, desc.generation)
                .unwrap_or_else(|| Instance::of(&desc, (&place, &st.temporaries)));
            let indexes: Vec<IndexDesc> = st.catalog.indexes(&desc.name).into_iter().cloned().collect();
            let entries = instance.rows.entries_for(&indexes, (added, tx.id))?;
            instance.put(&mut copy, &desc.name, entries);
        }
        Ok(Arc::new(copy))
    }

    /// `run`'s, given what a statement of another session does in `st`: the
    /// constraints checked at once, none deferred to this session's commit,
    /// which reads its own instances.
    fn at_once<T>(&self, st: &State, run: impl FnOnce(Effects<'_>) -> Result<T>) -> Result<T> {
        let deferral = RefCell::new(Deferral { all: Some(false), ..Deferral::default() });
        let conflict = RefCell::new(None);
        let cursors = Cursored { rows: Vec::new(), conflict: &conflict };
        let mut effects = Effects::new(&self.auth(), &deferral, cursors, st.list_space());
        effects.now = self.now.get();
        run(effects)
    }
}
