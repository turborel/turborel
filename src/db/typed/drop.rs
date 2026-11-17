//! [ISO/IEC 9075-2:2023 §11.31 SR6, GR1] DROP TABLE of a supertable:
//! refused with RESTRICT, and with CASCADE its direct subtables dropped
//! first, each with CASCADE, so the deepest go first.

use crate::catalog::{QualifiedName, proper_subtables};
use crate::db::ddl::Dropped;
use crate::db::{State, rule};
use crate::error::Result;

impl State {
    /// [ISO/IEC 9075-2:2023 §11.31 SR6, GR1] The proper subtables of the
    /// table `qn`, dropped, if `cascade`, as DROP TABLE of each direct
    /// subtable with CASCADE drops it: what they took away.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, not `cascade`, if `qn` has a proper subtable; as each
    /// drop's.
    pub(in crate::db) fn drop_subtables(&mut self, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        let subtables = proper_subtables(&*self.catalog, qn);
        if let (Some(first), false) = (subtables.first(), cascade) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.31 SR6] {first} is a subtable of {qn}, so {qn} is dropped only with CASCADE"
            )));
        }
        let mut dropped = Dropped::default();
        // Each direct subtable's drop takes its own subtables, which come
        // after it, so those are gone when met.
        for s in &subtables {
            if self.catalog.table(s).is_some() {
                dropped.absorb(self.drop_table(s, true)?);
            }
        }
        Ok(dropped)
    }
}
