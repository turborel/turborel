//! FILES, the example's storage engine: each table a file of the
//! extension's directory, its locator the file's name. The file is a log
//! of records, appended and made durable, each one write: the changes of
//! an APPLY, all or none, or a CHECKPOINT's position. A table is kept in
//! memory as the records read so far, and only the records appended
//! since, by this process or another of the extension's, are read again.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use turborel::bytes::{Put, Reader};
use turborel::plugin::values::{put_values, values};
use turborel::storage::{Row, RowId};
use turborel::value::{Value, compare};
use turborel_plugin::{Answer, ColumnAnswer, Engine, Opened, PluginError, TableAnswer};

/// A record of changes: count, then each id and its row or none.
const CHANGES: u8 = 1;
/// A record of a checkpoint: the position the table is durable through.
const DURABLE: u8 = 2;

/// The engine: each table read so far, by its locator.
#[derive(Default)]
pub struct Files {
    tables: HashMap<Vec<u8>, Table>,
}

/// A table: the bytes of its file read, the next id, the position it is
/// durable through, and its rows.
#[derive(Default)]
struct Table {
    read: u64,
    next: RowId,
    durable: u64,
    rows: BTreeMap<RowId, Row>,
}

/// An I/O failure, as SQLSTATE 58030.
fn io(e: &std::io::Error) -> PluginError {
    PluginError::new("58030", &e.to_string())
}

/// The file of `locator`.
fn path(locator: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(locator).into_owned())
}

/// `record` appended to the file of `locator`, durable, as one write.
fn append(locator: &[u8], record: &[u8]) -> Answer<()> {
    let mut framed = Vec::with_capacity(record.len().saturating_add(4));
    framed.put_bytes(record);
    let mut file = std::fs::OpenOptions::new().append(true).open(path(locator)).map_err(|e| io(&e))?;
    file.write_all(&framed).and_then(|()| file.sync_data()).map_err(|e| io(&e))
}

impl Table {
    /// The record `bytes` taken in.
    fn take(&mut self, bytes: &[u8]) -> Answer<()> {
        let mut r = Reader::new(bytes, "a FILES record");
        if r.u8()? == DURABLE {
            self.durable = self.durable.max(r.u64()?);
            return Ok(());
        }
        for _ in 0..r.u32()? {
            let id = r.u64()?;
            if r.u8()? == 0 {
                self.rows.remove(&id);
            } else {
                self.rows.insert(id, values(&mut r)?);
            }
            self.next = self.next.max(id.saturating_add(1));
        }
        Ok(())
    }
}

impl Files {
    /// The table of `locator`, the records appended since it was last read
    /// taken in; none if its file is not there.
    fn table(&mut self, locator: &[u8]) -> Answer<Option<&mut Table>> {
        let mut file = match std::fs::File::open(path(locator)) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.tables.remove(locator);
                return Ok(None);
            }
            Err(e) => return Err(io(&e)),
        };
        let t = self.tables.entry(locator.to_vec()).or_insert_with(|| Table { next: 1, ..Table::default() });
        let mut more = Vec::new();
        file.seek(SeekFrom::Start(t.read)).and_then(|_| file.read_to_end(&mut more)).map_err(|e| io(&e))?;
        let mut r = Reader::new(&more, "a FILES table");
        while let Ok(record) = r.bytes() {
            t.take(record)?;
            t.read = t.read.saturating_add(u64::try_from(record.len()).unwrap_or(0)).saturating_add(4);
        }
        Ok(Some(t))
    }

    /// The table of `locator`, which must be there.
    fn there(&mut self, locator: &[u8]) -> Answer<&mut Table> {
        self.table(locator)?.ok_or_else(|| PluginError::external("no such table"))
    }
}

impl Engine for Files {
    fn create(&mut self, table: &str, _columns: &[String]) -> Answer<Vec<u8>> {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let name: String = table.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        let locator = format!("{name}-{}-{stamp}.table", std::process::id()).into_bytes();
        std::fs::File::create(path(&locator)).and_then(|f| f.sync_all()).map_err(|e| io(&e))?;
        Ok(locator)
    }
    fn open(&mut self, locator: &[u8]) -> Answer<Opened> {
        Ok(self.table(locator)?.map_or_else(Opened::default, |t| Opened {
            found: true,
            next: t.next,
            rows: u64::try_from(t.rows.len()).unwrap_or(u64::MAX),
            durable: t.durable,
        }))
    }
    fn scan(&mut self, locator: &[u8], after: RowId, max: u32) -> Answer<(Vec<(RowId, Row)>, bool)> {
        let t = self.there(locator)?;
        let mut all = t.rows.range(after.saturating_add(1)..).map(|(id, row)| (*id, row.clone()));
        let page: Vec<(RowId, Row)> = all.by_ref().take(usize::try_from(max).unwrap_or(usize::MAX)).collect();
        Ok((page, all.next().is_some()))
    }
    fn get(&mut self, locator: &[u8], ids: &[RowId]) -> Answer<Vec<Option<Row>>> {
        let t = self.there(locator)?;
        Ok(ids.iter().map(|id| t.rows.get(id).cloned()).collect())
    }
    fn apply(&mut self, locator: &[u8], changes: &[(RowId, Option<Row>)]) -> Answer<Vec<Option<Row>>> {
        let t = self.there(locator)?;
        let before = changes.iter().map(|(id, _)| t.rows.get(id).cloned()).collect();
        let mut record = vec![CHANGES];
        record.put_u32(u32::try_from(changes.len()).unwrap_or(u32::MAX));
        for (id, row) in changes {
            record.put_u64(*id);
            record.put_u8(u8::from(row.is_some()));
            if let Some(row) = row {
                put_values(&mut record, row)?;
            }
        }
        append(locator, &record)?;
        self.there(locator)?;
        Ok(before)
    }
    fn checkpoint(&mut self, locator: &[u8], through: u64) -> Answer<()> {
        let mut record = vec![DURABLE];
        record.put_u64(through);
        append(locator, &record)
    }
    fn drop_table(&mut self, locator: &[u8]) -> Answer<()> {
        self.tables.remove(locator);
        std::fs::remove_file(path(locator)).map_err(|e| io(&e))
    }
    fn statistics(&mut self, locator: &[u8]) -> Answer<Option<TableAnswer>> {
        let t = self.there(locator)?;
        let width = t.rows.values().next().map_or(0, Vec::len);
        let columns = (0..width).map(|c| column(t.rows.values().filter_map(|row| row.get(c)))).collect();
        let rows = f64::from(u32::try_from(t.rows.len()).unwrap_or(u32::MAX));
        Ok(Some(TableAnswer { rows, width: -1.0, refresh: REFRESH, columns }))
    }
}

/// The rows written after which the database asks FILES its statistics
/// again.
const REFRESH: u64 = 100;

/// What FILES knows of a column of `values`: its distinct values, its
/// share of nulls, and its least and greatest.
fn column<'a>(values: impl Iterator<Item = &'a Value>) -> ColumnAnswer {
    let all: Vec<&Value> = values.collect();
    let present: Vec<&Value> = all.iter().copied().filter(|v| !v.is_null()).collect();
    let distinct: BTreeSet<String> = present.iter().map(ToString::to_string).collect();
    let least = |a: &&Value, b: &&Value| compare(a, b).unwrap_or(std::cmp::Ordering::Equal);
    let bounds = present.iter().copied().min_by(least).cloned().zip(present.iter().copied().max_by(least).cloned());
    let share = |n: usize| f64::from(u32::try_from(n).unwrap_or(u32::MAX));
    let nulls = if all.is_empty() { 0.0_f64 } else { (share(all.len()) - share(present.len())) / share(all.len()) };
    ColumnAnswer { distinct: share(distinct.len()), nulls, bounds, histogram: Vec::new(), frequencies: None }
}
