//! \[EXTENSION\] The host kinds that keep the database's data (RULES.md
//! D248, D250): log storage, catalog storage and spill, each a trait
//! whose methods are its operations. Each store is named by the
//! database's directory, as the host was given it.

use turborel::bytes::{Put, Reader};
use turborel::plugin::manifest::HostKind;
use turborel::plugin::payload::hosted as h;

use super::super::registry::Plugin;
use super::super::serve::Cx;
use super::super::{Answer, PluginError};
use super::HostObject;

/// Records, each with its position.
pub type Records = Vec<(u64, Vec<u8>)>;

/// Kind 23, log storage. Positions are records' numbers from 1, each
/// record appended at the position the database gives it.
pub trait LogStore: Send {
    /// Whether a log was made for `dir`.
    ///
    /// # Errors
    ///
    /// The store's, which stops the node.
    fn exists(&mut self, dir: &str) -> Answer<bool>;
    /// The log of `dir` opened: the position after its last record, none
    /// if there is no log.
    ///
    /// # Errors
    ///
    /// As `exists`.
    fn open(&mut self, dir: &str) -> Answer<Option<u64>>;
    /// A new, empty log for `dir`.
    ///
    /// # Errors
    ///
    /// As `exists`.
    fn create(&mut self, dir: &str) -> Answer<()>;
    /// `records` appended, durable before it answers.
    ///
    /// # Errors
    ///
    /// As `exists`: nothing it held durable before is lost.
    fn append(&mut self, dir: &str, records: &[(u64, Vec<u8>)]) -> Answer<()>;
    /// At most `max` records from `from`, in order, and whether more follow.
    ///
    /// # Errors
    ///
    /// As `exists`.
    fn read(&mut self, dir: &str, from: u64, max: u32) -> Answer<(Records, bool)>;
    /// Records before `before` dropped, as far as it likes.
    ///
    /// # Errors
    ///
    /// As `exists`.
    fn truncate(&mut self, dir: &str, before: u64) -> Answer<()>;
}

/// Kind 24, catalog storage: the last image of the catalog recorded.
pub trait CatalogKeeper: Send {
    /// The store of `dir`, if one was made: its last image, if any.
    ///
    /// # Errors
    ///
    /// The store's, which stops the node.
    fn open(&mut self, dir: &str) -> Answer<Option<Option<Vec<u8>>>>;
    /// A new store for `dir`.
    ///
    /// # Errors
    ///
    /// As `open`.
    fn create(&mut self, dir: &str) -> Answer<()>;
    /// `image` recorded as the last, durable before it answers;
    /// `compact_after` records kept before it compacts, as it likes.
    ///
    /// # Errors
    ///
    /// As `open`.
    fn record(&mut self, dir: &str, image: &[u8], compact_after: u64) -> Answer<()>;
}

/// Kind 25, spill: tables and runs, each named by the id it gives, kept
/// in the one process an area keeps.
pub trait SpillStore: Send {
    /// A new table: its id.
    ///
    /// # Errors
    ///
    /// The store's, which stops the node.
    fn table(&mut self) -> Answer<u64>;
    /// A new run: its id.
    ///
    /// # Errors
    ///
    /// As `table`.
    fn run(&mut self) -> Answer<u64>;
    /// `records` pushed onto the run `id`.
    ///
    /// # Errors
    ///
    /// As `table`.
    fn push(&mut self, id: u64, records: Vec<Vec<u8>>) -> Answer<()>;
    /// At most `max` of the run's records from the `from`th, and whether
    /// more follow.
    ///
    /// # Errors
    ///
    /// As `table`.
    fn read(&mut self, id: u64, from: u64, max: u32) -> Answer<(Vec<Vec<u8>>, bool)>;
    /// Each key and value of `pairs` added to the table `id`.
    ///
    /// # Errors
    ///
    /// As `table`.
    fn insert(&mut self, id: u64, pairs: Vec<(Vec<u8>, Vec<u8>)>) -> Answer<()>;
    /// The values under `key`, in the order added.
    ///
    /// # Errors
    ///
    /// As `table`.
    fn get(&mut self, id: u64, key: &[u8]) -> Answer<Vec<Vec<u8>>>;
    /// The first `value` under `key` removed: whether there was one.
    ///
    /// # Errors
    ///
    /// As `table`.
    fn remove(&mut self, id: u64, key: &[u8], value: &[u8]) -> Answer<bool>;
    /// The table or run `id` dropped.
    fn drop_id(&mut self, id: u64);
}

/// An operation no kind has.
fn unknown(op: u8) -> PluginError {
    PluginError::external(&format!("no host operation {op}"))
}

/// Log storage served.
struct Logged<T>(T);

impl<T: LogStore> HostObject for Logged<T> {
    fn answer(&mut self, _: &mut Cx<'_>, op: u8, payload: &[u8]) -> Answer<Vec<u8>> {
        let mut r = Reader::received(payload, "host call");
        let dir = r.str()?;
        let mut out = Vec::new();
        match op {
            h::LOG_EXISTS => out.put_u8(u8::from(self.0.exists(&dir)?)),
            h::LOG_OPEN => {
                let end = self.0.open(&dir)?;
                out.put_u8(u8::from(end.is_some()));
                out.put_u64(end.unwrap_or(0));
            }
            h::LOG_CREATE => self.0.create(&dir)?,
            h::LOG_APPEND => self.0.append(&dir, &h::records(&mut r)?)?,
            h::LOG_READ => {
                let (records, more) = self.0.read(&dir, r.u64()?, r.u32()?)?;
                h::put_records(&mut out, &records);
                out.put_u8(u8::from(more));
            }
            h::LOG_TRUNCATE => self.0.truncate(&dir, r.u64()?)?,
            other => return Err(unknown(other)),
        }
        Ok(out)
    }
}

/// Catalog storage served.
struct Cataloged<T>(T);

impl<T: CatalogKeeper> HostObject for Cataloged<T> {
    fn answer(&mut self, _: &mut Cx<'_>, op: u8, payload: &[u8]) -> Answer<Vec<u8>> {
        let mut r = Reader::received(payload, "host call");
        let dir = r.str()?;
        let mut out = Vec::new();
        match op {
            h::CATALOG_OPEN => {
                let found = self.0.open(&dir)?;
                out.put_u8(u8::from(found.is_some()));
                let last = found.flatten();
                out.put_u8(u8::from(last.is_some()));
                if let Some(image) = last {
                    out.put_bytes(&image);
                }
            }
            h::CATALOG_CREATE => self.0.create(&dir)?,
            h::CATALOG_RECORD => {
                let image = r.bytes()?;
                self.0.record(&dir, image, r.u64()?)?;
            }
            other => return Err(unknown(other)),
        }
        Ok(out)
    }
}

/// Spill served.
struct Spilled<T>(T);

impl<T: SpillStore> Spilled<T> {
    /// The answer of an operation of the table or run `id`.
    fn of(&mut self, op: u8, id: u64, r: &mut Reader<'_>) -> Answer<Vec<u8>> {
        let mut out = Vec::new();
        match op {
            h::SPILL_PUSH => self.0.push(id, h::parts(r)?)?,
            h::SPILL_READ => {
                let (records, more) = self.0.read(id, r.u64()?, r.u32()?)?;
                let parts: Vec<&[u8]> = records.iter().map(Vec::as_slice).collect();
                h::put_parts(&mut out, &parts);
                out.put_u8(u8::from(more));
            }
            h::SPILL_INSERT => {
                let flat = h::parts(r)?;
                let pairs = flat.chunks(2).filter_map(|c| Some((c.first()?.clone(), c.get(1)?.clone()))).collect();
                self.0.insert(id, pairs)?;
            }
            h::SPILL_GET => {
                let got = self.0.get(id, r.bytes()?)?;
                let parts: Vec<&[u8]> = got.iter().map(Vec::as_slice).collect();
                h::put_parts(&mut out, &parts);
            }
            h::SPILL_REMOVE => {
                let key = r.bytes()?;
                out.put_u8(u8::from(self.0.remove(id, key, r.bytes()?)?));
            }
            h::SPILL_DROP => self.0.drop_id(id),
            other => return Err(unknown(other)),
        }
        Ok(out)
    }
}

impl<T: SpillStore> HostObject for Spilled<T> {
    fn answer(&mut self, _: &mut Cx<'_>, op: u8, payload: &[u8]) -> Answer<Vec<u8>> {
        let mut out = Vec::new();
        match op {
            h::SPILL_TABLE => out.put_u64(self.0.table()?),
            h::SPILL_RUN => out.put_u64(self.0.run()?),
            other => {
                let mut r = Reader::received(payload, "host call");
                let id = r.u64()?;
                return self.of(other, id, &mut r);
            }
        }
        Ok(out)
    }
}

impl Plugin {
    /// It, serving `store` as HOST LOG STORAGE `name`.
    #[must_use]
    pub fn host_log(self, name: &str, store: impl LogStore + 'static) -> Plugin {
        self.host(HostKind::LogStorage.code(), name, Logged(store))
    }
    /// It, serving `keeper` as HOST CATALOG STORAGE `name`.
    #[must_use]
    pub fn host_catalog(self, name: &str, keeper: impl CatalogKeeper + 'static) -> Plugin {
        self.host(HostKind::CatalogStorage.code(), name, Cataloged(keeper))
    }
    /// It, serving `store` as HOST SPILL `name`.
    #[must_use]
    pub fn host_spill(self, name: &str, store: impl SpillStore + 'static) -> Plugin {
        self.host(HostKind::Spill.code(), name, Spilled(store))
    }
}
