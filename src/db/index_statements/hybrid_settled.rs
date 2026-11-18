//! \[EXTENSION\] A hybrid index that embeds its text, left half made or
//! half dropped by a database that stopped between the steps of its
//! statement (`hybrid_embedded.rs`), settled as the database opens to
//! serve: each such index whose HYBRID.STATE is not READY is dropped with
//! what it keeps beside it, as its statement, never told done, would have
//! dropped it had it failed. No table of chunks, pipeline or vector index
//! is left that no index keeps.

use super::super::Database;
use super::hybrid_embedded::text_of;
use crate::catalog::{IndexDesc, owner_of};
use crate::error::Result;
use crate::index::vector::hybrid::{STATE_OPTION, STATES, embeds};

impl Database {
    /// Each hybrid index that embeds its text and is not READY dropped,
    /// with what it keeps, by a session of its schema's owner.
    ///
    /// # Errors
    ///
    /// Those of dropping them.
    pub(in crate::db) fn hybrid_settled(&self) -> Result<()> {
        let st = self.snapshot()?;
        let unsettled: Vec<IndexDesc> = st
            .catalog
            .tables()
            .into_iter()
            .flat_map(|t| st.catalog.indexes(&t.name))
            .filter(|ix| embeds(&ix.method, &ix.options) && text_of(&ix.options, STATE_OPTION).is_some_and(|state| state != STATES[0]))
            .cloned()
            .collect();
        for ix in unsettled {
            let owner = owner_of(&*st.catalog, &ix.table.auth).to_owned();
            self.session(&owner).kept_undone(&ix.name, &ix.table.auth)?;
        }
        Ok(())
    }
}
