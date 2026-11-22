//! \[DESIGN\] What a search of an index gives: its entries, with their
//! keys as values or as the bytes the index holds, and the rows of a key.

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

/// \[DESIGN\] (RULES.md D821) The entries a search finds, lent one at a
/// time, each key as the bytes `engine::key::encode_key` makes of its
/// values over the key's types (`Index::search_encoded`). A caller
/// compares a row's key with an entry's as those bytes, and copies and
/// decodes none it need not.
pub type Encoded = Box<dyn EncodedEntries>;

/// \[DESIGN\] (RULES.md D821) A search's entries, each lent until the next
/// is asked for.
pub trait EncodedEntries: Send {
    /// The next entry, in the search's order; an error of the index's
    /// storage is an entry, after which there are none.
    fn next_encoded(&mut self) -> Option<Result<EncodedEntry<'_>>>;
}

/// \[DESIGN\] (RULES.md D821) One entry of `EncodedEntries`, lent.
pub struct EncodedEntry<'a> {
    /// Its key's bytes.
    pub key: &'a [u8],
    /// Its row.
    pub row: RowId,
    /// The types of the key's columns, which its bytes are of.
    pub types: &'a [DataType],
}
