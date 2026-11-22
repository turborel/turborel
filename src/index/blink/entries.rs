//! \[DESIGN\] The entries an index's search reads (RULES.md D501, D786,
//! D809). They are those of a range of its tree, each borrowed from the
//! scan and checked against the probe, its key decoded once per run of
//! equal keys by the types of the index the search holds.

use std::ops::Deref;
use std::sync::Arc;

use super::value::noted;
use super::{BLinkIndex, Probe, Range, split};
use crate::engine::key::decode_key;
use crate::engine::tree::{BLinkTree, Direction, scan_of};
use crate::error::Result;
use crate::index::Key;
use crate::storage::RowId;
use crate::value::DataType;

impl BLinkIndex {
    /// The entries `probe` selects, read through `tree`, its own, in
    /// `direction`. Their keys are decoded if `decoded` or checked, by the
    /// types `tree` reaches. What it gives holds `tree` and its own copy
    /// of the probe, so it lives as long as `tree` does.
    pub(super) fn entries<T: Keyed>(
        &self,
        tree: T,
        probe: &Probe,
        (decoded, direction): (bool, Direction),
    ) -> impl Iterator<Item = Result<(Option<Key>, RowId)>> + use<T> {
        Self::entries_within(tree, (probe, self.range(probe)), (decoded, direction))
    }

    /// The entries of `range` of `tree`, each of `probe` if it is checked,
    /// decoded if `decoded`, in `direction`.
    pub(super) fn entries_within<T: Keyed>(
        tree: T,
        (probe, range): (&Probe, Option<Range>),
        (decoded, direction): (bool, Direction),
    ) -> impl Iterator<Item = Result<(Option<Key>, RowId)>> + use<T> {
        let checked = range.as_ref().is_some_and(|r| r.checked);
        let mut scan = range.map(|r| scan_of(tree, r.lower.as_ref().map(Vec::as_slice), r.upper.as_ref().map(Vec::as_slice), direction));
        // The probe is copied only to check entries by, so an exact probe's
        // search copies none. The types are read through the tree the scan
        // holds, so no reference count every search shares is written.
        let probe = checked.then(|| probe.clone());
        let mut last = Decoded::default();
        // Each entry borrowed from the scan, none copied (RULES.md D809).
        std::iter::from_fn(move || {
            loop {
                let found = scan.as_mut()?.next_entry()?.and_then(|(held, (entry, value))| {
                    let (key, row) = split(entry)?;
                    noted(row, value)?;
                    let key = (decoded || checked).then(|| last.of(key, held.types())).transpose()?;
                    Ok((key, row))
                });
                let (key, row) = match found {
                    Ok(found) => found,
                    Err(e) => return Some(Err(e)),
                };
                let answers = |p: &Probe| key.as_ref().is_some_and(|k| p.answers(&k.0));
                if probe.as_ref().is_none_or(answers) {
                    return Some(Ok((key, row)));
                }
            }
        })
    }
}

/// The last key a search decoded, by its bytes: entries of one key, as an
/// equality probe of a key not unique finds, are decoded once
/// (RULES.md D501). A key is kept only once a second entry has its bytes,
/// so the entries of a unique key, each its own, are decoded and never
/// copied (RULES.md D786).
#[derive(Default)]
pub(super) struct Decoded {
    /// The bytes of the last key decoded, once one is.
    pub(super) bytes: Option<Vec<u8>>,
    /// That key, once a second entry had its bytes.
    pub(super) key: Option<Key>,
}

impl Decoded {
    /// The key whose encoding is `bytes`, of columns of `types`.
    pub(super) fn of(&mut self, bytes: &[u8], types: &[DataType]) -> Result<Key> {
        let again = self.bytes.as_deref() == Some(bytes);
        if let Some(key) = self.key.as_ref().filter(|_| again) {
            return Ok(key.clone());
        }
        let key = Key(decode_key(bytes, types)?);
        if again {
            self.key = Some(key.clone());
            return Ok(key);
        }
        self.key = None;
        let kept = self.bytes.get_or_insert_with(Vec::new);
        kept.clear();
        kept.extend_from_slice(bytes);
        Ok(key)
    }
}

/// A tree an index's search reads, and the types of the index's key.
pub(super) trait Keyed: Deref<Target = BLinkTree> {
    /// The types its keys are decoded by.
    fn types(&self) -> &[DataType];
}

/// An index's tree, reached through the index it holds: what a search
/// reads, so the index outlives the search.
pub(super) struct Held(pub(super) Arc<BLinkIndex>);

impl Deref for Held {
    type Target = BLinkTree;
    fn deref(&self) -> &BLinkTree {
        &self.0.tree
    }
}

impl Keyed for Held {
    fn types(&self) -> &[DataType] {
        &self.0.types
    }
}

/// An index's tree and its key's types, borrowed: what a count of its
/// entries in place reads.
pub(super) struct Borrowed<'a>(pub(super) &'a BLinkTree, pub(super) &'a [DataType]);

impl Deref for Borrowed<'_> {
    type Target = BLinkTree;
    fn deref(&self) -> &BLinkTree {
        self.0
    }
}

impl Keyed for Borrowed<'_> {
    fn types(&self) -> &[DataType] {
        self.1
    }
}
