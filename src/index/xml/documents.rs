//! \[DESIGN\] RULES.md D106: the values an XML index holds, each with its
//! row, and the entries made of them.
//!
//! An index is given a row's value whole as it adds its entries, but may
//! not be able to read it as it takes them back: garbage collection frees
//! a version's large objects before it takes back the version's entries,
//! and recovery and a transaction's undo, once spilled, name an object by
//! its id (RULES.md D79). So each (value, row) pair held is marked, by the
//! value's identity: of a value its row holds itself, the XXH3-128 of
//! its octets, which are there to derive its entries again; of one held
//! as a large object, the object's id, which its table never gives twice,
//! under which the index keeps the entries it made, by row, to take them
//! back with no read of the value, or to add them again. The marker
//! counts the pair's additions, as each entry counts its own, one for
//! each version of a row holding it (`Index`).
//!
//! ```text
//! marker:    5 | 0, then the XXH3-128 of the octets, or 1, then
//!            the object's id u64 | row u64 -> count u32
//! document:  6 | object's id u64 | row u64 | ORDPATH -> count u32, path's
//!            id, kind, the string value, the number value
//! version:   8 | row u64 | identity, as a marker's -> count u32: the
//!            primary's, of the values of a row's versions
//! other:     10 | row u64 | identity, as a marker's -> count u32: of a
//!            value that is not one document node
//! ```
//!
//! A value that is not one document node, of several items, of an atomic
//! value, of a node of another kind, or of none, as XML(SEQUENCE) holds,
//! is marked too, by row and identity: a query of it as its context item
//! may raise an error, or start from no document, where its entries alone
//! would not say so [ISO/IEC 9075-14:2006 draft §6.18 GR1a ii 2], so
//! every lookup gives its row, which its caller checks (`search.rs`).

use std::ops::Bound;

use super::derive::{Derived, Keyed, derive};
use super::dictionary::Dictionary;
use super::keys::{
    Cursor, DOCUMENTS, ELEMENT, Indexed, MARKS, MOST_ORDINAL, OTHERS, ROW, VERSIONS, after, corrupt, count_of, counted, entries_of,
    put_ordinal, values_of,
};
use super::{Method, XmlIndex, too_long};
use crate::engine::tree::{Direction, Update};
use crate::error::{Code, Result, SqlError};
use crate::hash::xxhash::xxh3_128_halves;
use crate::storage::RowId;
use crate::sync::lock_builtin;
use crate::value::Value;
use crate::value::lob::{Lob, Part};
use crate::xml::MAX_STORED;

/// The identity of a value its row holds.
const OCTETS: u8 = 0;
/// The identity of a value held as a large object.
const OBJECT: u8 = 1;
/// Bytes of a count before a document entry's body.
const COUNT: usize = 4;
/// Octets of a document entry's key and count but its ORDPATH: its space,
/// object, row, ORDPATH's end and count.
const DOCUMENT_KEY: usize = 1 + 8 + ROW + 1 + COUNT;
/// How many document entries are read before they are changed.
const BATCH: usize = 256;

/// How a value is held: its octets, or the large object it is, readable
/// if its table made it a slice it reads.
enum Held {
    Octets(Vec<u8>),
    Object { id: u64, lob: Option<Lob> },
}

/// Whether a change adds an entry or takes one back.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Change {
    Add,
    TakeBack,
}

/// How `value`, an XML value, is held.
fn held(value: &Value) -> Result<Held> {
    let Value::Xml(xml) = value else {
        return Err(SqlError::new(Code::Api, "[DESIGN] an XML index is given a value that is not XML (RULES.md D106)"));
    };
    let lob = xml.lob()?;
    if let Some(octets) = lob.inline() {
        return Ok(Held::Octets(octets.to_vec()));
    }
    if let [Part::Reference(id)] = lob.parts() {
        return Ok(Held::Object { id: *id, lob: None });
    }
    if let [Part::Slice(slice)] = lob.parts()
        && slice.whole()
        && let Some(kept) = slice.source.kept()
    {
        return Ok(Held::Object { id: kept.id, lob: Some(lob) });
    }
    Ok(Held::Octets(octets_of(&lob)?))
}

/// The octets of `lob`, a stored form, read whole.
fn octets_of(lob: &Lob) -> Result<Vec<u8>> {
    let most = u64::try_from(MAX_STORED).unwrap_or(u64::MAX);
    lob.bounded(most)?.ok_or_else(|| {
        SqlError::new(Code::Corrupt, format!("[DESIGN] an XML value stored is longer than {MAX_STORED} octets (RULES.md D106)"))
    })
}

/// The identity of a value held so.
fn identity(held: &Held) -> Vec<u8> {
    match held {
        Held::Octets(octets) => {
            let (a, b) = xxh3_128_halves(octets, 0);
            [&[OCTETS][..], &a.to_be_bytes(), &b.to_be_bytes()].concat()
        }
        Held::Object { id, .. } => [&[OBJECT][..], &id.to_be_bytes()].concat(),
    }
}

/// The key of the marker of a value of identity `identity` and `row`.
fn mark_key(identity: &[u8], row: RowId) -> Vec<u8> {
    [&[MARKS][..], identity, &row.to_be_bytes()].concat()
}

/// The key of the mark of a value of identity `identity` of `row` that is
/// not one document node.
fn other_key(identity: &[u8], row: RowId) -> Vec<u8> {
    [&[OTHERS][..], &row.to_be_bytes(), identity].concat()
}

/// The kind of node `keyed` is, its path's first octet.
fn kind_of(keyed: &Keyed<'_>) -> u8 {
    keyed.path.first().copied().unwrap_or(ELEMENT)
}

/// The body of `node`'s document entry, after its count: its path's id,
/// its kind and its two values.
fn document_body(node: &Indexed<'_>) -> Vec<u8> {
    let [string, number] = node.values;
    let mut body = Vec::with_capacity(MOST_ORDINAL.saturating_add(string.len()).saturating_add(number.len()).saturating_add(1));
    put_ordinal(&mut body, node.path);
    body.push(node.kind);
    body.extend_from_slice(string);
    body.extend_from_slice(number);
    body
}

/// The prefix of the document entries of object `id` and `row`.
fn documents_of(id: u64, row: RowId) -> Vec<u8> {
    [&[DOCUMENTS][..], &id.to_be_bytes(), &row.to_be_bytes()].concat()
}

/// What `change` makes of an entry counting `old`, or absent: its value
/// of the count, `value` gives; removed once it counts none.
fn updated(old: Option<u32>, change: Change, value: impl FnOnce(u32) -> Vec<u8>) -> Update {
    let new = match change {
        Change::Add => Some(old.unwrap_or(0).saturating_add(1)),
        Change::TakeBack => old.map(|n| n.saturating_sub(1)).filter(|&n| n > 0),
    };
    let Some(n) = new else { return if old.is_some() { Update::Remove } else { Update::Keep } };
    Update::Put(value(n))
}

impl XmlIndex {
    /// The dictionary of its tree.
    pub(super) fn dictionary(&self) -> Dictionary<'_> {
        Dictionary { tree: &self.tree, lock: &self.lock }
    }

    /// Adds the entries of `value` of `row`.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if the value is beyond the index's bounds; -916 if it
    /// is no stored form; -910 if it is a large object by id alone, not
    /// held; the tree's. A failure leaves no entry of it.
    pub(super) fn add(&self, value: &Value, row: RowId) -> Result<()> {
        let held = held(value)?;
        let identity = identity(&held);
        let (mark, other) = (mark_key(&identity, row), other_key(&identity, row));
        let document = match held {
            Held::Octets(octets) => {
                self.check(&octets)?;
                self.write_all(&octets, row, None)?
            }
            Held::Object { id, lob } => {
                let _object = lock_builtin(self.stripe(id));
                if self.count(&mark)? > 0 {
                    // Another version of the row holds the object: its mark
                    // of a value not one document node is there if it is one.
                    self.documents(id, row, Change::Add)?;
                    self.count(&other)? == 0
                } else {
                    let lob = lob.ok_or_else(|| {
                        SqlError::new(
                            Code::Api,
                            format!("[DESIGN] an XML index is given large object {id} by id alone, which it does not hold (RULES.md D106)"),
                        )
                    })?;
                    let octets = octets_of(&lob)?;
                    self.check(&octets)?;
                    self.write_all(&octets, row, Some(id))?
                }
            }
        };
        if !document {
            self.bump(&other, Change::Add)?;
        }
        self.bump(&mark, Change::Add)?;
        self.version(&identity, row, Change::Add)
    }

    /// Takes back one addition of `value` of `row`, if it holds one.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the value is no stored form; the tree's.
    pub(super) fn take_back(&self, value: &Value, row: RowId) -> Result<()> {
        let held = held(value)?;
        let identity = identity(&held);
        let mark = mark_key(&identity, row);
        match held {
            Held::Octets(octets) => {
                if self.count(&mark)? == 0 {
                    return Ok(());
                }
                let dictionary = self.dictionary();
                let mut names = |n: &_| dictionary.name(n, false);
                derive(&octets, self.method.prefix(), &mut names, &mut |keyed| self.write_node(keyed, (row, None), Change::TakeBack))?;
            }
            Held::Object { id, .. } => {
                let _object = lock_builtin(self.stripe(id));
                if self.count(&mark)? == 0 {
                    return Ok(());
                }
                self.documents(id, row, Change::TakeBack)?;
            }
        }
        // Of a value one document node, there is no such mark to take.
        self.bump(&other_key(&identity, row), Change::TakeBack)?;
        self.bump(&mark, Change::TakeBack)?;
        self.version(&identity, row, Change::TakeBack)
    }

    /// Of the primary, adds or takes back a version of `row` of a value of
    /// identity `identity`: its answer of a row is its entries' if they
    /// are of one value alone, which every version a reader may see has.
    fn version(&self, identity: &[u8], row: RowId, change: Change) -> Result<()> {
        if self.method != Method::Primary {
            return Ok(());
        }
        self.bump(&[&[VERSIONS][..], &row.to_be_bytes(), identity].concat(), change)
    }

    /// The rows it holds `value` of, each once: those marked with its
    /// identity, and, of a value its row holds, every entry of it there.
    ///
    /// # Errors
    ///
    /// As `take_back`.
    pub(super) fn rows_of(&self, value: &Value) -> Result<Vec<RowId>> {
        let held = held(value)?;
        let identity = identity(&held);
        let prefix = [&[MARKS][..], &identity].concat();
        let upper = after(&prefix);
        let mut rows = Vec::new();
        for found in self.tree.scan(Bound::Included(&prefix), upper.as_ref().map(Vec::as_slice), Direction::Forward) {
            let key = found?.0;
            let row = Cursor { bytes: &key, at: key.len().saturating_sub(ROW) }.row()?;
            let complete = match &held {
                Held::Octets(octets) => self.has_entries(octets, (row, &identity))?,
                Held::Object { .. } => true,
            };
            if complete {
                rows.push(row);
            }
        }
        Ok(rows)
    }

    /// Whether every entry of `octets` of `row` is there, and its mark if
    /// it is not one document node: else the row's marker is of another
    /// value of the same hash.
    fn has_entries(&self, octets: &[u8], (row, identity): (RowId, &[u8])) -> Result<bool> {
        let dictionary = self.dictionary();
        let mut all = true;
        let mut names = |n: &_| dictionary.name(n, false);
        let derived = derive(octets, self.method.prefix(), &mut names, &mut |keyed| {
            let Some(path) = dictionary.path(keyed.path, false)? else {
                all = false;
                return Ok(());
            };
            let [string, number] = values_of(keyed.text, keyed.whole);
            let node = Indexed { path, kind: kind_of(keyed), values: [&string, &number], ordpath: keyed.ordpath };
            for key in entries_of(self.method, &node, row) {
                all &= self.tree.get(&key)?.is_some();
            }
            Ok(())
        })?;
        Ok(all && (derived.document || self.tree.get(&other_key(identity, row))?.is_some()))
    }

    /// The lock of the values held as object `id`.
    fn stripe(&self, id: u64) -> &std::sync::Mutex<()> {
        let at =
            u64::try_from(self.stripes.len()).ok().and_then(|n| id.checked_rem(n)).and_then(|at| usize::try_from(at).ok()).unwrap_or(0);
        self.stripes.get(at).unwrap_or(&self.lock)
    }

    /// How many additions the marker `mark` counts.
    fn count(&self, mark: &[u8]) -> Result<u32> {
        self.tree.get(mark)?.map_or(Ok(0), |v| count_of(&v))
    }

    /// Changes the count of entry `key` by one.
    fn bump(&self, key: &[u8], change: Change) -> Result<()> {
        self.tree.update(key, |old| {
            let old = old.map(count_of).transpose()?;
            Ok(updated(old, change, counted))
        })
    }

    /// That every node of `octets` can be keyed: its names given tokens,
    /// so a derivation that writes gives only paths' ids; and its path,
    /// its entries and its document entry no longer than the tree takes.
    fn check(&self, octets: &[u8]) -> Result<()> {
        let dictionary = self.dictionary();
        let (most_key, most_entry) = (self.tree.max_key(), self.tree.max_entry());
        let mut names = |n: &_| dictionary.name(n, true);
        derive(octets, self.method.prefix(), &mut names, &mut |keyed| {
            // Of a path given no id yet, the longest an id takes.
            let [string, number] = values_of(keyed.text, keyed.whole);
            let node = Indexed { path: u64::MAX, kind: kind_of(keyed), values: [&string, &number], ordpath: keyed.ordpath };
            let mut sizes = vec![(keyed.path.len().saturating_add(1), most_key)];
            sizes.extend(entries_of(self.method, &node, RowId::MAX).into_iter().map(|key| (key.len(), most_key)));
            let document = DOCUMENT_KEY.saturating_add(keyed.ordpath.len()).saturating_add(document_body(&node).len());
            sizes.push((document, most_entry));
            match sizes.into_iter().find(|(n, most)| n > most) {
                Some((n, most)) => Err(too_long(n, most)),
                None => Ok(()),
            }
        })
        .map(drop)
    }

    /// Adds the entries of every node of `octets` of `row`, and its
    /// document entries if it is held as `object`; if one fails, takes
    /// back those added before it. Whether the value is one document node.
    fn write_all(&self, octets: &[u8], row: RowId, object: Option<u64>) -> Result<bool> {
        let dictionary = self.dictionary();
        let mut written = 0_usize;
        let mut names = |n: &_| dictionary.name(n, true);
        let done = derive(octets, self.method.prefix(), &mut names, &mut |keyed| {
            self.write_node(keyed, (row, object), Change::Add)?;
            written = written.saturating_add(1);
            Ok(())
        });
        let error = match done {
            Ok(Derived { document, .. }) => return Ok(document),
            Err(error) => error,
        };
        let mut taken = 0_usize;
        let mut known = |n: &_| dictionary.name(n, false);
        // The error is the first's: what the take-back meets is lost.
        let _ = derive(octets, self.method.prefix(), &mut known, &mut |keyed| {
            if taken < written {
                taken = taken.saturating_add(1);
                self.write_node(keyed, (row, object), Change::TakeBack)?;
            }
            Ok(())
        });
        Err(error)
    }

    /// Adds, or takes back, the entries of `keyed` of `row`, and its
    /// document entry if its value is held as `object`. Taken back, a node
    /// whose path has no id has no entry.
    fn write_node(&self, keyed: &Keyed<'_>, (row, object): (RowId, Option<u64>), change: Change) -> Result<()> {
        let Some(path) = self.dictionary().path(keyed.path, change == Change::Add)? else { return Ok(()) };
        let [string, number] = values_of(keyed.text, keyed.whole);
        let node = Indexed { path, kind: kind_of(keyed), values: [&string, &number], ordpath: keyed.ordpath };
        self.write_entries(&node, row, change)?;
        let Some(id) = object else { return Ok(()) };
        let key = [&documents_of(id, row)[..], keyed.ordpath, &[0]].concat();
        self.bump_document(&key, &document_body(&node), change)
    }

    /// Adds, or takes back, the entries of `node` of `row`.
    fn write_entries(&self, node: &Indexed<'_>, row: RowId, change: Change) -> Result<()> {
        entries_of(self.method, node, row).iter().try_for_each(|key| self.bump(key, change))
    }

    /// Changes the count of document entry `key`, of `body`, by one.
    fn bump_document(&self, key: &[u8], body: &[u8], change: Change) -> Result<()> {
        self.tree.update(key, |old| {
            let old = old.map(|v| v.get(..COUNT).map_or(Err(corrupt()), count_of)).transpose()?;
            Ok(updated(old, change, |n| [&n.to_le_bytes()[..], body].concat()))
        })
    }

    /// Adds, or takes back, the entries of the value held as object `id`
    /// of `row` once more, as its document entries list them, and those
    /// document entries: read a batch at a time, then changed.
    fn documents(&self, id: u64, row: RowId, change: Change) -> Result<()> {
        let prefix = documents_of(id, row);
        let upper = after(&prefix);
        let mut lower = Bound::Included(prefix.clone());
        loop {
            let scan = self.tree.scan(lower.as_ref().map(Vec::as_slice), upper.as_ref().map(Vec::as_slice), Direction::Forward);
            let batch: Vec<(Vec<u8>, Vec<u8>)> = scan.take(BATCH).collect::<Result<_>>()?;
            for (key, body) in &batch {
                self.document_entry((key, body), (prefix.len(), row), change)?;
            }
            let Some((last, _)) = batch.last().filter(|_| batch.len() == BATCH) else { return Ok(()) };
            lower = Bound::Excluded(last.clone());
        }
    }

    /// Adds, or takes back, the entries of document entry `key` of `body`,
    /// its ORDPATH after `at` octets, of `row`, and the document entry.
    fn document_entry(&self, (key, body): (&[u8], &[u8]), (at, row): (usize, RowId), change: Change) -> Result<()> {
        let ordpath = Cursor { bytes: key, at }.ordpath()?;
        let mut fields = Cursor { bytes: body, at: COUNT };
        let (path, kind) = (fields.ordinal()?, fields.byte()?);
        let values = [fields.value()?, fields.value()?];
        self.write_entries(&Indexed { path, kind, values, ordpath }, row, change)?;
        self.bump_document(key, body.get(COUNT..).ok_or_else(corrupt)?, change)
    }
}
