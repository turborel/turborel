//! \[EXTENSION\] A table's rows sealed through the encodings its option
//! ENCODINGS names (RULES.md D30, D41, D48), such as a compression: its records
//! and its values out of line, each with the row's id as the context.
//!
//! ```text
//! record stored:  the record (`record.rs`), its header 0; or SEALED, then
//!                 the record sealed (`disk/encoding/registry.rs`)
//! value stored:   the value sealed, through no encoding if none is named
//! ```
//!
//! What is sealed names the encodings it went through, so a change of the
//! option is made in place: rows written after it pass through the new
//! ones, and those written before are read through their own. A record
//! sealed through none is stored as it is, so a table that names none pays
//! nothing for its records. Encodings the option names that are not
//! registered, as when a database is opened without its plug-in, refuse
//! writes, not the table's opening.

use std::borrow::Cow;
use std::sync::{Arc, RwLock};

use crate::disk::encoding::{Encodings, Pipeline};
use crate::error::{Code, Result, SqlError};
use crate::storage::RowId;
use crate::sync::{read, write};

/// The first byte of a record stored sealed: one its header never is.
const SEALED: u8 = 1;
/// Bytes of an encoding's id in sealed data.
const ID_BYTES: usize = 4;

/// \[EXTENSION\] How a table's rows are sealed.
pub(super) struct Sealing {
    encodings: Arc<Encodings>,
    /// The pipeline new rows pass through; or, if the encodings named are
    /// not all registered, why new rows are refused.
    current: RwLock<std::result::Result<Arc<Pipeline>, String>>,
}

impl Sealing {
    /// Rows passed through no encoding.
    pub(super) fn plain() -> Sealing {
        Sealing { encodings: Arc::default(), current: RwLock::new(Ok(Arc::new(Pipeline::none()))) }
    }

    /// Rows passed through the encodings `names` names, by commas, of
    /// `encodings`.
    pub(super) fn named(encodings: Arc<Encodings>, names: &str) -> Sealing {
        let current = RwLock::new(Self::resolved(&encodings, names));
        Sealing { encodings, current }
    }

    fn resolved(encodings: &Encodings, names: &str) -> std::result::Result<Arc<Pipeline>, String> {
        encodings.named(names).map(Arc::new).map_err(|e| format!("[EXTENSION] the table's option ENCODINGS = '{names}': {}", e.msg))
    }

    /// Makes rows written from now on pass through `names`' encodings.
    pub(super) fn rename(&self, names: &str) {
        let resolved = Self::resolved(&self.encodings, names);
        if let Ok(mut current) = write(&self.current) {
            *current = resolved;
        }
    }

    /// The pipeline new rows pass through.
    fn pipeline(&self) -> Result<Arc<Pipeline>> {
        read(&self.current)?.clone().map_err(|why| SqlError::new(Code::Api, why))
    }

    /// The most bytes a record takes more once stored: its mark, the
    /// pipeline's ids, and what its encodings add (`Pipeline::overhead`).
    pub(super) fn overhead(&self) -> usize {
        let Ok(current) = read(&self.current) else { return 0 };
        let Ok(pipeline) = current.as_ref() else { return 0 };
        if pipeline.is_empty() {
            return 0;
        }
        // The mark, the count, the ids, and what the encodings add.
        let ids = pipeline.ids().len().saturating_mul(ID_BYTES).saturating_add(1);
        ids.saturating_add(1).saturating_add(pipeline.overhead())
    }

    /// `record`, row `id`'s, as it is stored.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the encodings named are not registered; an
    /// encoding's errors.
    pub(super) fn record(&self, id: RowId, record: Vec<u8>) -> Result<Vec<u8>> {
        let pipeline = self.pipeline()?;
        if pipeline.is_empty() {
            return Ok(record);
        }
        let mut out = Vec::with_capacity(record.len().saturating_add(1));
        out.push(SEALED);
        pipeline.seal(id, &record, &mut out)?;
        Ok(out)
    }

    /// The record of row `id` that `stored` holds.
    ///
    /// # Errors
    ///
    /// As `Encodings::unseal`.
    pub(super) fn plain_record<'a>(&self, id: RowId, stored: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        match stored.split_first() {
            Some((&SEALED, sealed)) => self.encodings.unseal(id, sealed),
            Some(_) | None => Ok(Cow::Borrowed(stored)),
        }
    }

    /// Appends `value`, of row `id`, to `out` as it is stored.
    ///
    /// # Errors
    ///
    /// As `record`.
    pub(super) fn value(&self, id: RowId, value: &[u8], out: &mut Vec<u8>) -> Result<()> {
        self.pipeline()?.seal(id, value, out)
    }

    /// The value of row `id` that `stored` holds.
    ///
    /// # Errors
    ///
    /// As `Encodings::unseal`.
    pub(super) fn plain_value<'a>(&self, id: RowId, stored: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        self.encodings.unseal(id, stored)
    }
}
