//! \[EXTENSION\] A host plug-in's log and catalog storage (RULES.md D248).
//!
//! The log's positions are its records' numbers from 1. An append is the
//! database's alone, and a sync sends every record appended and not yet
//! sent in one call, the program durable through them once it answers:
//! commits that sync together are made durable together, a group commit,
//! one call for as many as waited. A sync that fails stops the node, and
//! every later sync fails too, so a record the program may have lost is
//! never taken as durable. Recovery opens the log as the program left it
//! and reads it a page of BATCH records a call (256 if not written).
//!
//! A catalog store records each image in one call, durable once answered,
//! and keeps the last as the database reads it.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use crate::bytes::Put;
use crate::db::{CatalogStorage, CatalogStore, LogStorage, OpenedLog, StorePlace};
use crate::error::{Result, SqlError};
use crate::log::{Log, LogCapabilities, Lsn, Records};
use crate::plugin::bridge::{answer, byte, leaked};
use crate::plugin::errors::violation;
use crate::plugin::hosted::HostLink;
use crate::plugin::manifest::HostDeclared;
use crate::plugin::payload::hosted::{
    CATALOG_CREATE, CATALOG_OPEN, CATALOG_RECORD, LOG_APPEND, LOG_CREATE, LOG_EXISTS, LOG_OPEN, LOG_READ, LOG_TRUNCATE, put_records,
    records,
};

/// Records a page of a log's read holds, if BATCH is not written.
const PAGE: u64 = 256;

/// A payload of `place`'s directory.
fn of_place(place: &StorePlace<'_>) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.put_str(&place.dir.to_string_lossy());
    payload
}

/// A host plug-in's kind of log.
#[derive(Debug)]
pub struct PluginLogStorage {
    link: HostLink,
    name: &'static str,
    page: u32,
    durable: bool,
}

impl PluginLogStorage {
    /// The log storage `d` declares: BATCH records a page read, and
    /// DURABLE 0 if a sync does not survive a crash.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T06 if an option is not a number.
    pub fn new(d: &HostDeclared, link: HostLink) -> Result<PluginLogStorage> {
        let page = u32::try_from(d.number("BATCH", PAGE)?.max(1)).unwrap_or(u32::MAX);
        Ok(PluginLogStorage { link, name: leaked(&d.name), page, durable: d.number("DURABLE", 1)? != 0 })
    }

    /// The log of `place` the program keeps, its end `end`.
    fn log(&self, place: &StorePlace<'_>, end: u64) -> Arc<PluginLog> {
        let held = Held { next: end.max(1), sent: Vec::new(), durable: end.saturating_sub(1), sending: false, failed: None };
        let dir = place.dir.to_string_lossy().into_owned();
        Arc::new(PluginLog {
            link: self.link.clone(),
            dir,
            page: self.page,
            durable: self.durable,
            held: Mutex::new(held),
            given: Condvar::new(),
        })
    }
}

impl LogStorage for PluginLogStorage {
    fn name(&self) -> &'static str {
        self.name
    }
    fn open(&self, place: &StorePlace<'_>) -> Result<Option<OpenedLog>> {
        let got = self.link.call(LOG_OPEN, of_place(place))?;
        let (found, end) = answer(&got, |r| Ok((r.u8()? == 1, r.u64()?)))?;
        Ok(found.then(|| OpenedLog { log: self.log(place, end), cut: 0 }))
    }
    fn exists(&self, place: &StorePlace<'_>) -> Result<bool> {
        let got = self.link.call(LOG_EXISTS, of_place(place))?;
        Ok(answer(&got, byte)? == 1)
    }
    fn create(&self, place: &StorePlace<'_>) -> Result<Arc<dyn Log>> {
        self.link.call(LOG_CREATE, of_place(place))?;
        Ok(self.log(place, 1))
    }
}

/// What a host log holds of its own: the next position, the records not
/// yet sent, the position durable through, whether a sync is sending,
/// and how one failed, if one did: every later sync fails so too.
struct Held {
    next: u64,
    sent: Vec<(u64, Vec<u8>)>,
    durable: u64,
    sending: bool,
    failed: Option<SqlError>,
}

/// A log a host plug-in keeps.
struct PluginLog {
    link: HostLink,
    dir: String,
    page: u32,
    durable: bool,
    held: Mutex<Held>,
    given: Condvar,
}

impl PluginLog {
    /// What it holds.
    fn held(&self) -> MutexGuard<'_, Held> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// One APPEND of `records`, no lock held (R-ERR-4).
    fn send(&self, records: &[(u64, Vec<u8>)]) -> Result<()> {
        let mut payload = Vec::new();
        payload.put_str(&self.dir);
        put_records(&mut payload, records);
        self.link.call(LOG_APPEND, payload).map(drop)
    }
}

impl Log for PluginLog {
    fn capabilities(&self) -> LogCapabilities {
        LogCapabilities { durable: self.durable }
    }
    fn append(&self, record: &[u8]) -> Result<Lsn> {
        self.link.running()?;
        let mut held = self.held();
        let at = held.next;
        held.next = at.checked_add(1).ok_or_else(|| violation("a host log's positions ran out"))?;
        held.sent.push((at, record.to_vec()));
        Ok(Lsn(at))
    }
    fn sync(&self, through: Lsn) -> Result<()> {
        let mut held = self.held();
        loop {
            if let Some(failed) = &held.failed {
                return Err(failed.clone());
            }
            if held.durable >= through.0 {
                return Ok(());
            }
            if held.sending {
                held = self.given.wait(held).unwrap_or_else(PoisonError::into_inner);
                continue;
            }
            let batch = std::mem::take(&mut held.sent);
            let last = batch.last().map_or(held.durable, |(at, _)| *at);
            held.sending = true;
            drop(held);
            let done = self.send(&batch);
            held = self.held();
            held.sending = false;
            match done {
                Ok(()) => held.durable = held.durable.max(last),
                Err(e) => held.failed = Some(e),
            }
            self.given.notify_all();
        }
    }
    fn read_from(&self, from: Lsn) -> Records<'_> {
        let end = self.end();
        if let Err(e) = self.sync(Lsn(end.0.saturating_sub(1))) {
            return Box::new(std::iter::once(Err(e)));
        }
        Box::new(Pages { log: self, from: from.0.max(1), end: end.0, page: Vec::new().into_iter(), done: false })
    }
    fn truncate_before(&self, before: Lsn) -> Result<()> {
        let mut payload = Vec::new();
        payload.put_str(&self.dir);
        payload.put_u64(before.0);
        self.link.call(LOG_TRUNCATE, payload).map(drop)
    }
    fn end(&self) -> Lsn {
        Lsn(self.held().next)
    }
}

/// A host log's records from a position, a page a call.
struct Pages<'a> {
    log: &'a PluginLog,
    from: u64,
    end: u64,
    page: std::vec::IntoIter<(u64, Vec<u8>)>,
    done: bool,
}

impl Pages<'_> {
    /// The next page, none at the end.
    fn more(&mut self) -> Result<()> {
        let mut payload = Vec::new();
        payload.put_str(&self.log.dir);
        payload.put_u64(self.from);
        payload.put_u32(self.log.page);
        let got = self.log.link.call(LOG_READ, payload)?;
        let (page, more) = answer(&got, |r| Ok((records(r)?, r.u8()? == 1)))?;
        if page.windows(2).any(|w| w.first().map(|f| f.0) >= w.get(1).map(|s| s.0)) {
            return Err(violation("a host log's records came out of order"));
        }
        self.from = page.last().map_or(self.end, |(at, _)| at.saturating_add(1));
        self.done = !more || page.is_empty();
        self.page = page.into_iter();
        Ok(())
    }
}

impl Iterator for Pages<'_> {
    type Item = Result<(Lsn, Vec<u8>)>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((at, record)) = self.page.next() {
                return Some(Ok((Lsn(at), record)));
            }
            if self.done || self.from >= self.end {
                return None;
            }
            if let Err(e) = self.more() {
                self.done = true;
                return Some(Err(e));
            }
        }
    }
}

/// A host plug-in's kind of catalog store.
#[derive(Debug)]
pub struct PluginCatalogStorage {
    link: HostLink,
    name: &'static str,
}

impl PluginCatalogStorage {
    /// The catalog storage `d` declares.
    pub fn new(d: &HostDeclared, link: HostLink) -> PluginCatalogStorage {
        PluginCatalogStorage { link, name: leaked(&d.name) }
    }

    /// The store of `place`, its last image `last`.
    fn store(&self, place: &StorePlace<'_>, last: Option<Vec<u8>>) -> Box<dyn CatalogStore> {
        Box::new(PluginCatalogStore { link: self.link.clone(), dir: place.dir.to_string_lossy().into_owned(), last: Mutex::new(last) })
    }
}

impl CatalogStorage for PluginCatalogStorage {
    fn name(&self) -> &'static str {
        self.name
    }
    fn open(&self, place: &StorePlace<'_>) -> Result<Option<Box<dyn CatalogStore>>> {
        let got = self.link.call(CATALOG_OPEN, of_place(place))?;
        let (found, last) = answer(&got, |r| {
            let found = r.u8()? == 1;
            let last = if r.u8()? == 1 { Some(r.bytes()?.to_vec()) } else { None };
            Ok((found, last))
        })?;
        Ok(found.then(|| self.store(place, last)))
    }
    fn create(&self, place: &StorePlace<'_>) -> Result<Box<dyn CatalogStore>> {
        self.link.call(CATALOG_CREATE, of_place(place))?;
        Ok(self.store(place, None))
    }
}

/// A catalog store a host plug-in keeps, and its last image.
struct PluginCatalogStore {
    link: HostLink,
    dir: String,
    last: Mutex<Option<Vec<u8>>>,
}

impl CatalogStore for PluginCatalogStore {
    fn record(&self, record: &[u8], compact_after: usize) -> Result<()> {
        let mut payload = Vec::with_capacity(record.len().saturating_add(32));
        payload.put_str(&self.dir);
        payload.put_bytes(record);
        payload.put_u64(u64::try_from(compact_after).unwrap_or(u64::MAX));
        self.link.call(CATALOG_RECORD, payload)?;
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) = Some(record.to_vec());
        Ok(())
    }
    fn last(&self) -> Option<Vec<u8>> {
        self.last.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}
