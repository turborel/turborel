//! [ISO/IEC 9075-2:2023 §11.33 SR4, GR1] DROP VIEW of a superview: refused
//! with RESTRICT, and with CASCADE its direct subviews dropped first, each
//! with CASCADE, so the deepest go first.

use crate::catalog::{QualifiedName, proper_subtables};
use crate::db::ddl::Dropped;
use crate::db::{State, rule};
use crate::error::Result;

impl State {
    /// [ISO/IEC 9075-2:2023 §11.33 SR4, GR1] The proper subviews of the
    /// view `qn`, dropped, if `cascade`, as DROP VIEW of each direct
    /// subview with CASCADE drops it: what they took away.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, not `cascade`, if `qn` has a proper subview; as each
    /// drop's.
    pub(in crate::db) fn drop_subviews(&mut self, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        let subviews = proper_subtables(&*self.catalog, qn);
        if let (Some(first), false) = (subviews.first(), cascade) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.33 SR4] {first} is a subview of {qn}, so {qn} is dropped only with CASCADE"
            )));
        }
        let mut dropped = Dropped::default();
        // Each direct subview's drop takes its own subviews, which come
        // after it, so those are gone when met.
        for s in &subviews {
            if self.catalog.view(s).is_some() {
                dropped.absorb(self.drop_view(s, true)?);
            }
        }
        Ok(dropped)
    }
}
