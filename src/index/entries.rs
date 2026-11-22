//! \[DESIGN\] What a search of an index gives: its entries, with their
//! keys as values or as the bytes the index holds, and the rows of a key.

use std::sync::Arc;

use super::Key;
use crate::error::Result;
use crate::storage::RowId;
use crate::value::DataType;

/// \[DESIGN\] The entries a search finds, (key, row) pairs, read as they
/// are taken (`Index::search`).
pub type Entries = Box<dyn Iterator<Item = Result<(Key, RowId)>> + Send>;

/// \[DESIGN\] The rows with an entry under one key, read as they are taken
/// (`Index::key_rows`).
pub type RowIds = Box<dyn Iterator<Item = Result<RowId>> + Send>;

/// \[DESIGN\] (RULES.md D821) The entries a search finds, each key as the
/// bytes `engine::key::encode_key` makes of its values over `types`, read
/// as they are taken (`Index::search_encoded`). A caller compares a row's
/// key with an entry's as those bytes, and decodes none it need not.
pub struct Encoded {
    /// The (key, row) pairs, in the search's order.
    pub entries: Box<dyn Iterator<Item = Result<(Vec<u8>, RowId)>> + Send>,
    /// The types of the key's columns, which its bytes are of.
    pub types: Arc<[DataType]>,
}
