//! \[DESIGN\] Which versions own a table's large objects, and when each is
//! freed (RULES.md D79). Every object belongs to one row, and the versions
//! that name it are one run of that row's history, unbroken: a version
//! names an object only if it made it or the version it replaces names it
//! too. So a value of another row, an older version, a variable or another
//! table is copied into objects of its own as it is written (`keep_lobs`),
//! and a value written back unchanged, or an append's old segments, are
//! shared without counting who holds them.
//!
//! What a change drops, the objects the version it replaced names and the
//! new one does not, is found as it is written and kept with the change.
//! At commit it is garbage with the version it was dropped from, freed once
//! no snapshot reads that version (`mvcc/gc.rs`); a change undone frees
//! what its version named and the one put back does not. Each object a
//! transaction makes is one of its changes, so a rollback frees it, and one
//! no version it wrote names, made for an attempt that then failed, is
//! freed at its commit.

use super::{IN_RECORD_OCTETS, directory_body, segment_body};
use crate::engine::rows::RowTable;
use crate::engine::rows::ids::key;
use crate::engine::rows::version::Version;
use crate::error::{Code, Result, SqlError};
use crate::storage::{LobLog, Row, RowId};
use crate::txn::{NO_TRANSACTION, TxId};
use crate::value::Value;
use crate::value::lob::{CHUNK_OCTETS, Chunk, Lob, LobKind, count_chars};
use crate::value::xml::XmlValue;

impl RowTable {
    /// The directories of the large objects the record `record`, of row
    /// `id`, names.
    pub(in crate::engine::rows) fn objects(&self, id: RowId, record: &[u8]) -> Result<Vec<u64>> {
        if self.lobs.columns().is_empty() {
            return Ok(Vec::new());
        }
        let plain = self.sealing.plain_record(id, record)?;
        let mut fetch = |reference| self.get_value(id, reference);
        let mut named = Vec::new();
        for &(c, _) in self.lobs.columns() {
            if let Some(l) = self.format.column_with(&plain, c, &mut fetch)?.as_lob()
                && let [crate::value::lob::Part::Reference(object)] = l.parts()
            {
                named.push(*object);
            }
        }
        Ok(named)
    }

    /// Of row `id`'s version of record `old` replaced by one of record
    /// `new`, or by a deletion: the objects `old` names and `new` does not,
    /// and those `new` names and `old` does not.
    pub(in crate::engine::rows) fn owned_change(&self, id: RowId, old: &[u8], new: Option<&[u8]>) -> Result<(Vec<u64>, Vec<u64>)> {
        if self.lobs.columns().is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let before = self.objects(id, old)?;
        let after = new.map(|record| self.objects(id, record)).transpose()?.unwrap_or_default();
        let dropped = before.iter().filter(|o| !after.contains(o)).copied().collect();
        let added = after.into_iter().filter(|o| !before.contains(o)).collect();
        Ok((dropped, added))
    }

    /// Frees the objects `ids` name, each a directory and its segments.
    pub(in crate::engine::rows) fn free_objects(&self, ids: &[u64]) {
        for &id in ids {
            self.lobs.free(id);
        }
    }

    /// Makes each large object of `row` one this table keeps for it: one
    /// short enough is held in the record, one the version it replaces,
    /// the newest of row `replacing`, names is shared, and any other is
    /// copied into objects made for it, `log` given each to keep and log.
    pub(in crate::engine::rows) fn keep_lobs(&self, tx: TxId, replacing: Option<RowId>, row: &mut Row, log: LobLog<'_>) -> Result<()> {
        if self.lobs.columns().is_empty() {
            return Ok(());
        }
        let named = match replacing {
            Some(id) => match self.tree.get(&key(id))? {
                Some(newest) => {
                    let version = Version::parse(&newest)?;
                    if version.deleted { Vec::new() } else { self.objects(id, version.record)? }
                }
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        for &(c, kind) in self.lobs.columns() {
            if let Some(Value::Xml(x)) = row.get(c)
                && x.stored_lob().is_none()
            {
                let stored = Value::Xml(XmlValue::stored(x.lob()?));
                if let Some(slot) = row.get_mut(c) {
                    *slot = stored;
                }
            }
            let Some(l) = row.get(c).and_then(Value::as_lob) else { continue };
            let shared =
                self.lobs.kept_here(l).map(|id| named.contains(&id) || (tx != NO_TRANSACTION && self.made_by(tx, &[id]).unwrap_or(false)));
            if shared == Some(true) || (l.inline().is_some() && l.octets() <= IN_RECORD_OCTETS) {
                continue;
            }
            let kept = if l.octets() <= IN_RECORD_OCTETS { held(l, kind)? } else { self.write_lob(tx, l, &mut *log)? };
            if let Some(slot) = row.get_mut(c).and_then(Value::as_lob_mut) {
                *slot = kept;
            }
        }
        Ok(())
    }

    /// Copies `lob` into objects of this table, a chunk at a time, each
    /// given to `log` to keep and log: a value reading them.
    fn write_lob(&self, tx: TxId, lob: &Lob, log: LobLog<'_>) -> Result<Lob> {
        if self.lobs.make_tree()? {
            self.write_meta()?;
        }
        let kind = lob.kind();
        let mut reader = lob.reader();
        let (mut at, end) = (0_u64, lob.octets());
        let mut segments = Vec::new();
        let mut carried: Vec<u8> = Vec::new();
        while at < end || !carried.is_empty() {
            let want = u64::try_from(CHUNK_OCTETS.saturating_sub(carried.len())).unwrap_or(0).min(end.saturating_sub(at));
            carried.extend_from_slice(&reader.read(at, want)?);
            at = at.saturating_add(want);
            let cut = if kind == LobKind::Character && at < end {
                match crate::value::lob::char_cut(&carried, carried.len()) {
                    0 => carried.len(),
                    cut => cut,
                }
            } else {
                carried.len()
            };
            let piece: Vec<u8> = carried.drain(..cut).collect();
            if piece.is_empty() {
                return Err(SqlError::new(Code::Corrupt, "[DESIGN] a character large object's octets are not UTF-8"));
            }
            let chars = if kind == LobKind::Character { count_chars(&piece) } else { u64::try_from(piece.len()).unwrap_or(u64::MAX) };
            let size = Chunk { octets: u32::try_from(piece.len()).unwrap_or(u32::MAX), chars: u32::try_from(chars).unwrap_or(u32::MAX) };
            let id = self.new_object(tx, &segment_body(&piece), &mut *log)?;
            segments.push((id, size));
        }
        let directory = self.new_object(tx, &directory_body(kind, &segments)?, &mut *log)?;
        if tx != NO_TRANSACTION {
            self.note_parts(tx, &segments.iter().map(|(id, _)| *id).collect::<Vec<_>>())?;
        }
        Ok(self.lobs.stored(kind, directory, (lob.octets(), lob.chars())))
    }

    /// A new object of `body`, made by `tx`, kept and logged by `log`: its
    /// id.
    fn new_object(&self, tx: TxId, body: &[u8], log: LobLog<'_>) -> Result<u64> {
        let id = self.next_id()?;
        log(id, body, &mut || self.lobs.put(id, body))?;
        if tx != NO_TRANSACTION {
            self.note_made(tx, id)?;
        }
        Ok(id)
    }

    /// Of a version written by `tx` that names the objects `added` the one
    /// it replaces does not: refused unless `tx` made each, so no two
    /// rows' versions share one.
    pub(in crate::engine::rows) fn owns(&self, tx: TxId, added: &[u64]) -> Result<()> {
        if added.is_empty() || tx == NO_TRANSACTION {
            return Ok(());
        }
        if self.made_by(tx, added)? {
            return Ok(());
        }
        Err(SqlError::new(Code::Api, "[DESIGN] a version names a large object of another row or version, not copied for it"))
    }

    /// Keeps object `id` of `body`, read from the log as recovery replays
    /// it, if it is not kept: ids given from now on are past it.
    pub(in crate::engine::rows) fn apply_object(&self, id: u64, body: &[u8]) -> Result<()> {
        self.reserve_past(id)?;
        self.next.fetch_max(id.saturating_add(1), std::sync::atomic::Ordering::AcqRel);
        self.blocks.clear();
        if self.lobs.make_tree()? {
            self.write_meta()?;
        }
        self.lobs.put(id, body)?;
        self.lobs.note_candidates(&[id]);
        Ok(())
    }

    /// Frees the objects recovery may have left that no row's newest
    /// version names: those it made from the log, those named by versions
    /// it replaced, and those awaiting collection at the checkpoint it
    /// began from. Each is looked for in the newest versions of the rows
    /// it touched, as no other's names one, a directory naming its
    /// segments.
    pub(in crate::engine::rows) fn sweep_objects(&self) -> Result<()> {
        let (candidates, touched) = self.lobs.take_candidates();
        if candidates.is_empty() {
            return Ok(());
        }
        let mut named = crate::hash::xxhash::Xxh3Set::default();
        for id in touched {
            self.named_by(id, &mut named)?;
        }
        let freed: Vec<u64> = candidates.into_iter().filter(|o| !named.contains(o)).collect();
        self.free_objects(&freed);
        Ok(())
    }

    /// Adds to `named` the objects row `id`'s newest version names, its
    /// directories and their segments; none if it is deleted or gone.
    fn named_by(&self, id: RowId, named: &mut crate::hash::xxhash::Xxh3Set<u64>) -> Result<()> {
        let Some(newest) = self.tree.get(&key(id))? else { return Ok(()) };
        let version = Version::parse(&newest)?;
        if version.deleted {
            return Ok(());
        }
        for directory in self.objects(id, version.record)? {
            named.extend(self.lobs.segments(directory)?);
            named.insert(directory);
        }
        Ok(())
    }
}

/// `lob`, of `kind`, short, held in memory, as a record holds it.
fn held(lob: &Lob, kind: LobKind) -> Result<Lob> {
    let bytes = lob.reader().read(0, lob.octets())?;
    match kind {
        LobKind::Character => std::str::from_utf8(&bytes)
            .map(Lob::of_text)
            .map_err(|_| SqlError::new(Code::Corrupt, "[DESIGN] a character large object's octets are not UTF-8")),
        LobKind::Binary => Ok(Lob::of_octets(&bytes)),
    }
}

/// The candidates recovery may free, and the rows it touched.
#[derive(Default)]
pub(in crate::engine::rows) struct Candidates {
    objects: Vec<u64>,
    rows: Vec<RowId>,
}

impl super::LobSpace {
    /// Notes `ids` as objects recovery may free.
    pub(in crate::engine::rows) fn note_candidates(&self, ids: &[u64]) {
        if let Ok(mut c) = self.candidates.lock() {
            c.objects.extend_from_slice(ids);
        }
    }

    /// Notes row `id` as touched by recovery.
    pub(in crate::engine::rows) fn note_touched(&self, id: RowId) {
        if let Ok(mut c) = self.candidates.lock() {
            c.rows.push(id);
        }
    }

    /// The candidates and rows noted, then forgotten.
    fn take_candidates(&self) -> (Vec<u64>, Vec<RowId>) {
        self.candidates.lock().map(|mut c| (std::mem::take(&mut c.objects), std::mem::take(&mut c.rows))).unwrap_or_default()
    }
}
