//! [ISO/IEC 9075:1992 §4.10.1, §14.2] The constraint mode of each
//! constraint in a session's transaction, and the checks of deferred
//! constraints waiting for it to become immediate (`Deferral`).

use std::collections::{HashMap, HashSet};

use super::{Check, mode_name};
use crate::catalog::{ConstraintDesc, QualifiedName};
use crate::error::Result;
use crate::exec::HeldRows;
use crate::spill::ListSpace;

/// A check over all a constraint reads (`Check::whole`): its table, none of
/// an assertion's, and the constraint.
pub(in crate::db) type Whole = (Option<QualifiedName>, QualifiedName);

/// [ISO/IEC 9075:1992 §4.28, §14.2] The constraint mode of each constraint
/// in a session's transaction, as SET CONSTRAINTS left it, and the checks
/// of deferred constraints not yet made, held as a list that spills, until
/// they are made or the transaction ends (RULES.md D38).
pub(in crate::db) struct Deferral {
    /// The mode SET CONSTRAINTS ALL gave every deferrable constraint, last:
    /// deferred or not.
    pub(in crate::db) all: Option<bool>,
    /// The mode SET CONSTRAINTS gave each constraint it named since.
    pub(in crate::db) named: HashMap<QualifiedName, bool>,
    /// The checks waiting: a check over all a constraint reads only once.
    pub(in crate::db) pending: HeldRows<Check>,
    /// Which checks over all a constraint reads are waiting, so a statement
    /// that asks for one again adds none.
    pub(in crate::db) wholes: HashSet<Whole>,
}

impl Default for Deferral {
    /// None waiting, and no mode set: as a transaction begins.
    fn default() -> Deferral {
        Deferral { all: None, named: HashMap::new(), pending: HeldRows::new(ListSpace::default()), wholes: HashSet::new() }
    }
}

impl Deferral {
    /// Whether no check waits: none is pending, and none over all a
    /// constraint reads.
    pub(in crate::db) fn nothing_waits(&self) -> bool {
        self.pending.is_empty() && self.wholes.is_empty()
    }

    /// [§10.6 GR1, GR2; §14.2 GR2, GR3] Whether constraint `c` is deferred:
    /// deferrable, and so set, or initially deferred.
    pub(in crate::db) fn deferred(&self, c: &ConstraintDesc) -> bool {
        c.attributes.deferrable && self.named.get(mode_name(c)).copied().or(self.all).unwrap_or(c.attributes.initially_deferred)
    }

    /// Whether `check` is one over all a constraint reads, waiting already.
    pub(in crate::db) fn waits(&self, check: &Check) -> bool {
        check.whole().is_some_and(|w| self.wholes.contains(&w))
    }

    /// `more` checks to wait, after those waiting; `wholes`, those of them
    /// over all a constraint reads, none waiting already.
    pub(in crate::db) fn defer(&mut self, more: HeldRows<Check>, wholes: Vec<Whole>) {
        self.wholes.extend(wholes);
        if self.pending.is_empty() {
            self.pending = more;
        } else {
            self.pending.append(more);
        }
    }

    /// Forgets the checks waiting of constraint `name`, made again and
    /// checked whole as it is made, the rows they wait for among what it
    /// checked (`db/assertions/kept.rs`); the rest kept, in a list of
    /// `space`.
    ///
    /// # Errors
    ///
    /// The spill area's, reading back the checks waiting.
    pub(in crate::db) fn forget(&mut self, name: &QualifiedName, space: ListSpace) -> Result<()> {
        let mut kept = HeldRows::new(space);
        self.pending.visit(&mut |c| if c.constraint() == name { Ok(()) } else { kept.push(c.clone()) })?;
        self.pending = kept;
        self.wholes.retain(|(_, constraint)| constraint != name);
        Ok(())
    }
}
