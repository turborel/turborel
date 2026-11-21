//! The example's host stores (RULES.md D248, D250): the log storage
//! FILELOG and the catalog storage FILECAT, a file each in the plug-in's
//! directory named by the database's directory, and the spill MEMORY, in
//! the process an area keeps, noting each table and run it makes in
//! `spill.log`. `crash.append` makes the process die
//! halfway through writing an append, as a commit's records are made
//! durable; `crash.read` as recovery reads the log.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};

use turborel_plugin::host::{CatalogKeeper, LogStore, Records, SpillStore};
use turborel_plugin::{Answer, Plugin, PluginError};

use super::host_small::{crash_if, note, xxh64_of};

/// The plug-in, with these objects.
pub fn with(plugin: Plugin) -> Plugin {
    plugin.host_log("FILELOG", FileLog).host_catalog("FILECAT", FileCatalog).host_spill("MEMORY", Memory::default())
}

/// The file of `dir`'s store of `what`.
fn file_of(dir: &str, what: &str) -> String {
    format!("{what}-{:016x}", xxh64_of(&[dir.as_bytes()]))
}

/// An I/O error, as the program's.
fn failed(e: &std::io::Error) -> PluginError {
    PluginError::new("58030", &e.to_string())
}

/// The bytes of the file `path`, none if it is not there.
fn bytes_of(path: &str) -> Answer<Option<Vec<u8>>> {
    match File::open(path) {
        Ok(mut f) => {
            let mut bytes = Vec::new();
            f.read_to_end(&mut bytes).map_err(|e| failed(&e))?;
            Ok(Some(bytes))
        }
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(failed(&e))
            }
        }
    }
}

/// The whole records of a log file's bytes, and the bytes they take: a
/// record is its position, its length and its bytes.
fn parsed(bytes: &[u8]) -> (Records, usize) {
    let mut records = Vec::new();
    let mut at = 0_usize;
    loop {
        let head = bytes.get(at..at.saturating_add(12));
        let Some(head) = head else { return (records, at) };
        let (lsn, len) = head.split_at(8);
        let lsn = u64::from_le_bytes(lsn.try_into().unwrap_or_default());
        let len = usize::try_from(u32::from_le_bytes(len.try_into().unwrap_or_default())).unwrap_or(usize::MAX);
        let end = at.saturating_add(12).saturating_add(len);
        let Some(body) = bytes.get(at.saturating_add(12)..end) else { return (records, at) };
        records.push((lsn, body.to_vec()));
        at = end;
    }
}

/// A log kept in a file, each append synced before it answers.
struct FileLog;

impl FileLog {
    /// The records of `dir`'s log, the torn tail a crash left cut off.
    fn records(dir: &str) -> Answer<Option<Records>> {
        let path = file_of(dir, "log");
        let Some(bytes) = bytes_of(&path)? else { return Ok(None) };
        let (records, whole) = parsed(&bytes);
        if whole < bytes.len() {
            let f = OpenOptions::new().write(true).open(&path).map_err(|e| failed(&e))?;
            f.set_len(u64::try_from(whole).unwrap_or(u64::MAX)).map_err(|e| failed(&e))?;
            f.sync_all().map_err(|e| failed(&e))?;
        }
        Ok(Some(records))
    }
}

impl LogStore for FileLog {
    fn exists(&mut self, dir: &str) -> Answer<bool> {
        Ok(std::path::Path::new(&file_of(dir, "log")).exists())
    }
    fn open(&mut self, dir: &str) -> Answer<Option<u64>> {
        Ok(Self::records(dir)?.map(|r| r.last().map_or(1, |(lsn, _)| lsn.saturating_add(1))))
    }
    fn create(&mut self, dir: &str) -> Answer<()> {
        File::create(file_of(dir, "log")).and_then(|f| f.sync_all()).map_err(|e| failed(&e))
    }
    fn append(&mut self, dir: &str, records: &[(u64, Vec<u8>)]) -> Answer<()> {
        let mut bytes = Vec::new();
        for (lsn, record) in records {
            bytes.extend_from_slice(&lsn.to_le_bytes());
            bytes.extend_from_slice(&u32::try_from(record.len()).unwrap_or(u32::MAX).to_le_bytes());
            bytes.extend_from_slice(record);
        }
        let mut f = OpenOptions::new().append(true).open(file_of(dir, "log")).map_err(|e| failed(&e))?;
        if std::path::Path::new("crash.append").exists() {
            let half = bytes.get(..bytes.len().checked_div(2).unwrap_or(0)).unwrap_or_default();
            drop(f.write_all(half).and_then(|()| f.sync_all()));
            std::process::abort();
        }
        f.write_all(&bytes).and_then(|()| f.sync_all()).map_err(|e| failed(&e))
    }
    fn read(&mut self, dir: &str, from: u64, max: u32) -> Answer<(Records, bool)> {
        crash_if("read");
        let all = Self::records(dir)?.unwrap_or_default();
        let mut after = all.into_iter().filter(|(lsn, _)| *lsn >= from).peekable();
        let page: Records = after.by_ref().take(usize::try_from(max).unwrap_or(usize::MAX)).collect();
        Ok((page, after.peek().is_some()))
    }
    fn truncate(&mut self, _dir: &str, _before: u64) -> Answer<()> {
        // It keeps every record: a log may drop as few as it likes.
        Ok(())
    }
}

/// A catalog kept in a file, each image written whole and renamed into
/// place.
struct FileCatalog;

impl CatalogKeeper for FileCatalog {
    fn open(&mut self, dir: &str) -> Answer<Option<Option<Vec<u8>>>> {
        let Some(bytes) = bytes_of(&file_of(dir, "catalog"))? else { return Ok(None) };
        Ok(Some(bytes.split_first().and_then(|(has, image)| (*has == 1).then(|| image.to_vec()))))
    }
    fn create(&mut self, dir: &str) -> Answer<()> {
        File::create(file_of(dir, "catalog")).and_then(|mut f| f.write_all(&[0]).and_then(|()| f.sync_all())).map_err(|e| failed(&e))
    }
    fn record(&mut self, dir: &str, image: &[u8], _compact_after: u64) -> Answer<()> {
        let (path, staged) = (file_of(dir, "catalog"), file_of(dir, "catalog.new"));
        let mut f = File::create(&staged).map_err(|e| failed(&e))?;
        f.write_all(&[1]).and_then(|()| f.write_all(image)).and_then(|()| f.sync_all()).map_err(|e| failed(&e))?;
        std::fs::rename(&staged, &path).map_err(|e| failed(&e))
    }
}

/// Spill kept in the memory of the process an area keeps.
#[derive(Default)]
struct Memory {
    next: u64,
    runs: HashMap<u64, Vec<Vec<u8>>>,
    tables: HashMap<u64, HashMap<Vec<u8>, Vec<Vec<u8>>>>,
}

impl Memory {
    /// A new id.
    fn id(&mut self) -> u64 {
        self.next = self.next.saturating_add(1);
        self.next
    }
}

/// No table or run of `id`.
fn none(id: u64) -> PluginError {
    PluginError::external(&format!("no spill of id {id}"))
}

impl SpillStore for Memory {
    fn table(&mut self) -> Answer<u64> {
        note("spill.log", "table");
        let id = self.id();
        self.tables.insert(id, HashMap::new());
        Ok(id)
    }
    fn run(&mut self) -> Answer<u64> {
        note("spill.log", "run");
        let id = self.id();
        self.runs.insert(id, Vec::new());
        Ok(id)
    }
    fn push(&mut self, id: u64, records: Vec<Vec<u8>>) -> Answer<()> {
        self.runs.get_mut(&id).ok_or_else(|| none(id))?.extend(records);
        Ok(())
    }
    fn read(&mut self, id: u64, from: u64, max: u32) -> Answer<(Vec<Vec<u8>>, bool)> {
        let run = self.runs.get(&id).ok_or_else(|| none(id))?;
        let from = usize::try_from(from).unwrap_or(usize::MAX);
        let page: Vec<Vec<u8>> = run.iter().skip(from).take(usize::try_from(max).unwrap_or(usize::MAX)).cloned().collect();
        Ok((page.clone(), from.saturating_add(page.len()) < run.len()))
    }
    fn insert(&mut self, id: u64, pairs: Vec<(Vec<u8>, Vec<u8>)>) -> Answer<()> {
        let table = self.tables.get_mut(&id).ok_or_else(|| none(id))?;
        for (key, value) in pairs {
            table.entry(key).or_default().push(value);
        }
        Ok(())
    }
    fn get(&mut self, id: u64, key: &[u8]) -> Answer<Vec<Vec<u8>>> {
        Ok(self.tables.get(&id).ok_or_else(|| none(id))?.get(key).cloned().unwrap_or_default())
    }
    fn remove(&mut self, id: u64, key: &[u8], value: &[u8]) -> Answer<bool> {
        let values = self.tables.get_mut(&id).ok_or_else(|| none(id))?.get_mut(key);
        let Some(values) = values else { return Ok(false) };
        let at = values.iter().position(|v| v == value);
        Ok(at.map(|i| values.remove(i)).is_some())
    }
    fn drop_id(&mut self, id: u64) {
        self.runs.remove(&id);
        self.tables.remove(&id);
    }
}
