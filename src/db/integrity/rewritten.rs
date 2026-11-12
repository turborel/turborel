//! \[DESIGN\] The tables a statement wrote again whole, or took away the
//! rows of at once: ALTER TABLE, copying a table's rows to a new generation
//! of its storage or checking each against a constraint added
//! (`db/ddl.rs`), and TRUNCATE TABLE (`db/truncate.rs`).
//!
//! What such a statement did tells nothing of an assertion's change: rows
//! copied are none added, and rows taken at once are none it saw. So each
//! CHECK with a subquery, and each assertion, that reads such a table is
//! checked whole [ISO/IEC 9075:1992 §11.6 GR3, §11.34 GR2], not from the
//! rows (`db/assertions/changes.rs`). Of one deferred, the checks of rows
//! of earlier statements waiting are forgotten, as its whole check waits in
//! their stead, and a later statement's rows wait no more beside it. The
//! totals an assertion keeps are put right as the whole check is made, of
//! its tables as they are then (`db/assertions/totals.rs`): no row written
//! since tells their change.

use std::sync::Arc;

use super::Effects;
use crate::catalog::{ConstraintKind, QualifiedName};
use crate::db::State;
use crate::error::Result;
use crate::txn::Txn;

impl Effects<'_> {
    /// The rows of `table` written again whole, or taken away at once.
    pub(in crate::db) fn rewritten(&mut self, table: &QualifiedName) {
        if !self.rewritten.contains(table) {
            self.rewritten.push(table.clone());
        }
    }

    /// The tables whose rows it wrote again whole, or took away at once.
    pub(super) fn rewritten_tables(&self) -> &[QualifiedName] {
        &self.rewritten
    }

    /// The assertions that read a table whose rows it wrote again whole,
    /// or took away at once, as `st` holds them: each to be checked whole
    /// (`State::check_assertion_whole`). The checks waiting of each that is
    /// deferred now are forgotten, as its whole check waits instead.
    ///
    /// # Errors
    ///
    /// The spill area's, reading back the checks waiting.
    pub(super) fn whole_assertions(&self, st: &State) -> Result<Vec<QualifiedName>> {
        let mut whole = Vec::new();
        if self.rewritten.is_empty() {
            return Ok(whole);
        }
        for a in st.catalog.assertions() {
            let ConstraintKind::Check { reads, .. } = &a.kind else { continue };
            if !reads.iter().any(|t| self.rewritten.contains(t)) {
                continue;
            }
            if self.deferral.borrow().deferred(a) {
                self.deferral.borrow_mut().forget(&a.name, self.space.clone())?;
            }
            whole.push(a.name.clone());
        }
        Ok(whole)
    }
}

impl State {
    /// That immediate assertion `name` is not false after a statement that
    /// did `effects`: checked whole if `whole`, as of a table whose rows it
    /// wrote again whole or took away at once; else from its change.
    ///
    /// # Errors
    ///
    /// SQLCODE -201, SQLSTATE 23000, about the assertion, if it is false.
    pub(super) fn assertion_after(
        self: &Arc<Self>,
        tx: &Txn,
        (name, whole): (&QualifiedName, bool),
        effects: &mut Effects<'_>,
    ) -> Result<()> {
        if whole { self.check_assertion_whole(tx, name) } else { self.check_assertion_after(tx, name, effects) }
    }
}
