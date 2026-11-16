//! [ISO/IEC 9075-2:2023 §9.24] The database and transaction of the
//! statement a session runs, kept while it runs, where a PTF describe
//! component procedure reads SQL-data as the statement is bound.

use std::sync::Arc;

use super::super::{Session, State};
use crate::binder::Compile;
use crate::txn::TxId;

/// While it lives, the statement of `tx` against `db` is the one a session
/// runs; as it drops, the one before is again.
pub(in crate::db) struct Compiling<'s> {
    session: &'s Session,
    before: Option<Compile>,
}

impl Drop for Compiling<'_> {
    fn drop(&mut self) {
        *self.session.compile.borrow_mut() = self.before.take();
    }
}

impl Session {
    /// [ISO/IEC 9075-2:2023 §9.24] The statement of `tx` against `st` made
    /// the one it runs, until what this gives drops.
    pub(in crate::db) fn compiling(&self, st: &Arc<State>, tx: TxId) -> Compiling<'_> {
        let now = Compile { db: Arc::clone(st), tx };
        let before = self.compile.borrow_mut().replace(now);
        Compiling { session: self, before }
    }
}
