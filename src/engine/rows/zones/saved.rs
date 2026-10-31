//! \[DESIGN\] A row store table's zone maps kept with its pages (RULES.md
//! D31): saved at each checkpoint, with writers held, so the pages a
//! checkpoint captures hold zones no narrower than the rows they hold;
//! read as the table is opened, so it needs no pass over its rows to know
//! them. Recovery then widens them for every change it writes, as a write
//! does (`TableStorage::apply`).
//!
//! Zones are saved by segments of `SEGMENT_ZONES`, each a page of any
//! length (the pager's overflow), sealed as the table's rows are, its page
//! number the context: a checkpoint writes again only the segments a
//! change touched since the last. A segment that cannot be sealed, its
//! encodings not registered, is saved empty, and one that cannot be
//! opened is read as empty: its zones are then not known, and are summed
//! up after the table opens, as are those of row ids no segment holds.
//!
//! ```text
//! directory page:  each segment's page u64, in order
//! segment page:    columns kept u32 | zones u32 | each zone: rows u32 |
//!                  flags u8 (KNOWN, STALE) | each column's null u8 |
//!                  least values, record | greatest values, record
//! record:          length u32 | the record of the columns kept
//!                  (`record.rs`), NULL where a column holds no value
//! ```
//!
//! Integers are little-endian.

use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, Weak};

use super::{Map, ZONE_ROWS, Zone, Zones};
use crate::bytes::{Put, Reader};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::engine::rows::TableFiles;
use crate::engine::rows::sealing::Sealing;
use crate::error::{Code, Result};
use crate::storage::RowId;
use crate::sync::{lock, lock_builtin, read, write};
use crate::value::Value;

/// \[DESIGN\] Zones a segment saves: a page each, written again only as
/// one of its zones changes.
pub(super) const SEGMENT_ZONES: usize = 64;
/// The bit of a segment's sealing context setting it apart from a row's
/// and a frozen group's page's.
const ZONED: u64 = 1 << 62;
/// A zone's flag: it is known.
const KNOWN: u8 = 1;
/// A zone's flag: it may be wider than its rows.
const STALE: u8 = 2;
/// Bytes of a page number in the directory.
const WORD: usize = 8;

/// \[DESIGN\] Where a table's zones are saved: its file, how its rows are
/// sealed, the page listing the segments' pages, and those pages.
pub(in crate::engine::rows) struct Place {
    pub(in crate::engine::rows) pool: Arc<BufferPool>,
    pub(in crate::engine::rows) file: FileId,
    pub(in crate::engine::rows) sealing: Arc<Sealing>,
    pub(in crate::engine::rows) directory: PageId,
    /// Each segment's page, read from the directory as the table opens.
    pub(in crate::engine::rows) segments: Vec<PageId>,
}

impl TableFiles {
    /// Saves the zones of each table open of its files, a part of a
    /// checkpoint (`StorageEngine::checkpoint`).
    ///
    /// # Errors
    ///
    /// The buffer pool's: the checkpoint fails, and every zone of the
    /// table is saved again by the next.
    pub(in crate::engine::rows) fn save_zones(&self) -> Result<()> {
        let open: Vec<Arc<Zones>> = {
            let mut tables = lock(&self.zones)?;
            tables.retain(|z| z.strong_count() > 0);
            tables.iter().filter_map(Weak::upgrade).collect()
        };
        open.iter().try_for_each(|zones| zones.save())
    }

    /// Notes `zones`, of a table made or opened, to be saved at each
    /// checkpoint.
    pub(in crate::engine::rows) fn note_zones(&self, zones: &Arc<Zones>) -> Result<()> {
        lock(&self.zones)?.push(Arc::downgrade(zones));
        Ok(())
    }
}

impl Zones {
    /// Makes its zones saved as `sealing` seals.
    pub(in crate::engine::rows) fn seal_with(&self, sealing: &Arc<Sealing>) {
        if let Ok(mut place) = lock(&self.place)
            && let Some(place) = place.as_mut()
        {
            place.sealing = Arc::clone(sealing);
        }
    }

    /// Saves the segments changed since last saved.
    fn save(&self) -> Result<()> {
        let mut place = lock(&self.place)?;
        let Some(place) = place.as_mut() else { return Ok(()) };
        let saved = self.save_to(place);
        if saved.is_err() {
            // Saved again whole by the next checkpoint.
            read(&self.map)?.dirty.iter().for_each(|flag| flag.store(true, AtomicOrdering::Release));
        }
        saved
    }

    /// `save`'s work, at `place`.
    fn save_to(&self, place: &mut Place) -> Result<()> {
        let changed = self.changed_segments()?;
        let listed = place.segments.len();
        for (segment, content) in changed {
            while place.segments.len() <= segment {
                let (page, _) = place.pool.new_page(place.file)?;
                place.segments.push(page);
            }
            let Some(&page) = place.segments.get(segment) else { continue };
            let mut stored = Vec::with_capacity(content.len());
            // Its zones are not known after a restart if it cannot be sealed.
            if place.sealing.value(page.0 | ZONED, &content, &mut stored).is_err() {
                stored.clear();
            }
            *place.pool.write(place.file, page)? = stored;
        }
        if place.segments.len() != listed {
            let mut directory = Vec::with_capacity(place.segments.len().saturating_mul(WORD));
            place.segments.iter().for_each(|page| directory.put_u64(page.0));
            *place.pool.write(place.file, place.directory)? = directory;
        }
        Ok(())
    }

    /// The content of each segment changed since last saved, marked saved.
    fn changed_segments(&self) -> Result<Vec<(usize, Vec<u8>)>> {
        let map = read(&self.map)?;
        let mut changed = Vec::new();
        for (segment, flag) in map.dirty.iter().enumerate() {
            // Marked saved before it is read: a change meanwhile marks it again.
            if !flag.swap(false, AtomicOrdering::AcqRel) {
                continue;
            }
            let first = segment.saturating_mul(SEGMENT_ZONES);
            let zones = map.zones.get(first..first.saturating_add(SEGMENT_ZONES).min(map.zones.len())).unwrap_or_default();
            let mut content = Vec::new();
            content.put_u32(u32::try_from(self.kept.len()).unwrap_or(u32::MAX));
            content.put_u32(u32::try_from(zones.len()).unwrap_or(u32::MAX));
            for zone in zones {
                self.put_zone(&lock_builtin(zone), &mut content)?;
            }
            changed.push((segment, content));
        }
        Ok(changed)
    }

    /// Appends `zone` to `out`.
    fn put_zone(&self, zone: &Zone, out: &mut Vec<u8>) -> Result<()> {
        out.put_u32(zone.rows);
        out.put_u8(if zone.known { KNOWN } else { 0 } | if zone.stale { STALE } else { 0 });
        zone.nulls.iter().for_each(|&null| out.put_u8(u8::from(null)));
        for end in [true, false] {
            let values = zone
                .bounds
                .iter()
                .map(|b| b.as_ref().map_or(Value::Null, |(least, greatest)| (if end { least } else { greatest }).clone()));
            let mut record = Vec::new();
            self.format.encode(&values.collect(), &mut record)?;
            out.put_bytes(&record);
        }
        Ok(())
    }

    /// The zones of `content`, a segment's.
    fn zones_of(&self, content: &[u8]) -> Result<Vec<Zone>> {
        let mut reader = Reader::new(content, "segment of a table's zone maps");
        if usize::try_from(reader.u32()?).ok() != Some(self.kept.len()) {
            return Err(reader.corrupt("it keeps other columns than the table"));
        }
        let count = reader.u32()?;
        let mut zones = Vec::new();
        for _ in 0..count {
            let rows = reader.u32()?;
            let flags = reader.u8()?;
            let nulls = self.kept.iter().map(|_| reader.u8().map(|n| n != 0)).collect::<Result<Vec<bool>>>()?;
            let least = self.format.decode(reader.bytes()?)?;
            let greatest = self.format.decode(reader.bytes()?)?;
            let bounds = least.into_iter().zip(greatest).map(|(l, g)| (!l.is_null()).then_some((l, g))).collect();
            let known = flags & KNOWN != 0;
            zones.push(Zone { bounds, nulls, rows, changes: 0, stale: flags & STALE != 0 || !known, known });
        }
        reader.finish()?;
        Ok(zones)
    }

    /// Reads the zones saved, of a table opened whose rows' ids are below
    /// `below`: those of ids no segment holds not known until summed up.
    ///
    /// # Errors
    ///
    /// The buffer pool's; SQLCODE -916 if a segment is malformed.
    pub(in crate::engine::rows) fn load(&self, below: RowId) -> Result<()> {
        if self.kept.is_empty() {
            return Ok(());
        }
        let mut loaded = Vec::new();
        if let Some(place) = lock(&self.place)?.as_mut() {
            place.segments = segments_of(&place.pool.read(place.file, place.directory.0)?)?;
            for (segment, page) in place.segments.iter().enumerate() {
                let zones = self.segment(place, *page)?;
                let first = segment.saturating_mul(SEGMENT_ZONES);
                loaded.resize_with(first.max(loaded.len()), || Zone::empty(self.kept.len(), false));
                loaded.extend(zones);
            }
        }
        let count = usize::try_from(below.div_ceil(ZONE_ROWS)).unwrap_or(usize::MAX).max(loaded.len());
        loaded.resize_with(count, || Zone::empty(self.kept.len(), false));
        let stale = loaded.iter().filter(|z| z.stale).count();
        let mut map = write(&self.map)?;
        let segments = count.div_ceil(SEGMENT_ZONES);
        *map = Map {
            zones: loaded.into_iter().map(Mutex::new).collect(),
            unknown_below: below,
            dirty: (0..segments).map(|_| AtomicBool::new(false)).collect(),
        };
        self.stale.store(stale, AtomicOrdering::Release);
        Ok(())
    }

    /// The zones segment page `page` holds: none if it is empty, or cannot
    /// be opened as its encodings are not registered.
    fn segment(&self, place: &Place, page: PageId) -> Result<Vec<Zone>> {
        let stored = place.pool.read(place.file, page.0)?;
        if stored.is_empty() {
            return Ok(Vec::new());
        }
        match place.sealing.plain_value(page.0 | ZONED, &stored) {
            Ok(content) => self.zones_of(&content),
            Err(e) => {
                if matches!(e.code, Code::Api) {
                    Ok(Vec::new())
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Frees the pages its zones are saved in, of a table dropped; saved no
    /// more.
    pub(in crate::engine::rows) fn free_pages(&self) -> Result<()> {
        let Some(place) = lock(&self.place)?.take() else { return Ok(()) };
        for page in place.segments.iter().chain([&place.directory]) {
            place.pool.free(place.file, *page)?;
        }
        Ok(())
    }
}

/// The segments' pages a directory page's `content` lists.
fn segments_of(content: &[u8]) -> Result<Vec<PageId>> {
    let mut reader = Reader::new(content, "directory of a table's zone maps");
    (0..content.len().div_ceil(WORD)).map(|_| reader.u64().map(PageId)).collect()
}
