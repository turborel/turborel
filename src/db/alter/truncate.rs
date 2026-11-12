//! \[DESIGN\] TRUNCATE TABLE's change of a table [ISO/IEC 9075-2:2023
//! §14.10 GR5, GR8], as the log keeps it (`AlterAction::Truncate`): a
//! persistent table given a new generation of its storage, made empty, so
//! its rows go at once, none deleted one by one; its indexes made new and
//! empty with it. Readers of earlier snapshots go on reading the old
//! generation, as after ALTER TABLE (`db/alter.rs`). A temporary table's
//! rows are each session's, and its own are deleted by the statement
//! (`db/truncate.rs`), so its descriptor alone changes here.
//!
//! With RESTART IDENTITY, the table's identity column's sequence generator
//! is restarted at its start value in the same change, as the ALTER COLUMN
//! RESTART WITH that GR8 effectively executes restarts it, so the rows and
//! the generator go together or not at all.

use super::Reshape;
use crate::ast::{AlterAction, SequenceOptions};
use crate::catalog::{Generated, QualifiedName};
use crate::db::State;
use crate::db::alter::Made;
use crate::db::ddl::Dropped;
use crate::db::sequences::Epochs;
use crate::error::Result;

impl State {
    /// [ISO/IEC 9075-2:2023 §14.10 GR5, GR8] Table `qn` with no row, if
    /// persistent, in a new generation of its storage (`made`'s, if given),
    /// and its identity column's generator restarted at its start value if
    /// `restart`, its epoch from `epochs`. What it took away, and what it
    /// made: its rows change in no way a copy would.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the start value is beyond the generator's bounds
    /// now [§9.37 SR12]; the engine's or method's.
    pub(super) fn truncate_rows(
        &mut self,
        qn: &QualifiedName,
        restart: bool,
        (action, made, epochs): (&AlterAction, Option<&Made>, &mut Epochs),
    ) -> Result<(Dropped, Made, Option<Reshape>)> {
        let desc = self.base_table(qn)?;
        let mut next = desc.clone();
        if restart {
            // [§14.10 GR8b] ALTER COLUMN CN RESTART WITH SV: a RESTART of no
            // value restarts at the start value [§9.37 SR9c].
            let options = SequenceOptions { restart: Some(None), ..SequenceOptions::default() };
            for c in &mut next.columns {
                if let Some(Generated::Identity(identity)) = &mut c.generated {
                    identity.sequence = self.altered(identity.sequence.clone(), &options, (0, epochs))?;
                }
            }
        }
        if desc.temporary.is_some() {
            let mut catalog = self.catalog.duplicate();
            catalog.replace_table(next);
            self.catalog = std::sync::Arc::from(catalog);
            return Ok((Dropped::default(), Made::default(), None));
        }
        next.generation = desc.generation.saturating_add(1);
        let (dropped, built) = self.regenerate(qn, (&desc, next), (Reshape::Keep, Vec::new()), (made, action))?;
        Ok((dropped, built, None))
    }
}
