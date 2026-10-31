//! Settings a plug-in of every kind declares and reads as an engine does
//! (RULES.md D36): declared under its own name as it is registered, their
//! values told it as they are published, from a snapshot, by one hook
//! (`Configurable`); shown in INFORMATION_SCHEMA.SETTINGS. The engines'
//! own are tested in `engine_settings.rs`.

use std::sync::Arc;

use turborel::catalog::ColumnDesc;
use turborel::clock::Clock;
use turborel::db::{CatalogFiles, CatalogStorage, CatalogStore, Credentials, Database, FileLogs, LogStorage, OpenOptions, OpenedLog};
use turborel::db::{ServedConnections, StorePlace};
use turborel::diagnostics::{Diagnostics, Event};
use turborel::disk::encoding::{Encoding, EncodingId};
use turborel::error::{Code, Result, SqlError};
use turborel::functions::{Accumulator, AggregateFunction, ScalarFunction};
use turborel::hash::{Digest, HashFunction, HashId};
use turborel::index::{Index, IndexMethod};
use turborel::log::{Log, LogCapabilities, Lsn, MemoryLog, Records};
use turborel::memory::MemoryBudget;
use turborel::settings::{Configurable, Effect};
use turborel::spill::{Area, PagedSpill, Spill, SpillArea, TempFiles};
use turborel::storage::Row;
use turborel::system::{Seen, SystemTable};
use turborel::value::{CompOp, DataType, Value};

use crate::common::{Noted, Scratch, after_crash, rows, whole};
use crate::extension_types::Named;

/// An index method of no index, "notedix".
struct Method(Noted);

impl IndexMethod for Method {
    fn name(&self) -> &'static str {
        "notedix"
    }
    fn configurable(&self) -> Option<&dyn Configurable> {
        Some(&self.0)
    }
    fn supports(&self, _: CompOp) -> bool {
        false
    }
    fn create(&self, _: &[DataType]) -> Result<Box<dyn Index>> {
        Err(SqlError::new(Code::Api, "no index"))
    }
}

/// A scalar function of no value, NOTEDFN.
struct Scalar(Noted);

impl ScalarFunction for Scalar {
    fn name(&self) -> &'static str {
        "NOTEDFN"
    }
    fn configurable(&self) -> Option<&dyn Configurable> {
        Some(&self.0)
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Character { length: 4 })
    }
    fn call(&self, _: &[Value]) -> Result<Value> {
        Ok(Value::Null)
    }
}

/// An aggregate of no value, NOTEDAGG.
struct Aggregate(Noted);

/// Its accumulator.
struct Nothing;

impl Accumulator for Nothing {
    fn update(&mut self, _: &Value) -> Result<()> {
        Ok(())
    }
    fn finish(&self) -> Result<Value> {
        Ok(Value::Null)
    }
}

impl AggregateFunction for Aggregate {
    fn name(&self) -> &'static str {
        "NOTEDAGG"
    }
    fn configurable(&self) -> Option<&dyn Configurable> {
        Some(&self.0)
    }
    fn result_type(&self, _: Option<DataType>) -> Result<DataType> {
        Ok(DataType::Character { length: 4 })
    }
    fn accumulator(&self, _: DataType) -> Box<dyn Accumulator> {
        Box::new(Nothing)
    }
}

/// A system table of no row, NOTEDTAB.
struct Table(Noted);

impl SystemTable for Table {
    fn name(&self) -> &'static str {
        "NOTEDTAB"
    }
    fn configurable(&self) -> Option<&dyn Configurable> {
        Some(&self.0)
    }
    fn columns(&self) -> Vec<ColumnDesc> {
        vec![ColumnDesc { name: "K".into(), data_type: DataType::Character { length: 4 }, not_null: false, default: None, domain: None }]
    }
    fn rows(&self, _: &Seen<'_>) -> Result<Vec<Row>> {
        Ok(Vec::new())
    }
}

/// The built-in catalog files, as "notedcat".
struct Catalogs(Noted);

impl CatalogStorage for Catalogs {
    fn name(&self) -> &'static str {
        "notedcat"
    }
    fn configurable(&self) -> Option<&dyn Configurable> {
        Some(&self.0)
    }
    fn open(&self, place: &StorePlace<'_>) -> Result<Option<Box<dyn CatalogStore>>> {
        CatalogFiles.open(place)
    }
    fn create(&self, place: &StorePlace<'_>) -> Result<Box<dyn CatalogStore>> {
        CatalogFiles.create(place)
    }
}

/// The built-in file log, as "notedlog".
struct Logs(Noted);

impl LogStorage for Logs {
    fn name(&self) -> &'static str {
        "notedlog"
    }
    fn configurable(&self) -> Option<&dyn Configurable> {
        Some(&self.0)
    }
    fn open(&self, place: &StorePlace<'_>) -> Result<Option<OpenedLog>> {
        FileLogs::default().open(place)
    }
    fn exists(&self, place: &StorePlace<'_>) -> Result<bool> {
        FileLogs::default().exists(place)
    }
    fn create(&self, place: &StorePlace<'_>) -> Result<Arc<dyn Log>> {
        FileLogs::default().create(place)
    }
}

/// A hash function of digests of zeros, "notedhash".
struct Hash(Noted);

impl HashFunction for Hash {
    fn id(&self) -> HashId {
        HashId(9001)
    }
    fn name(&self) -> &'static str {
        "notedhash"
    }
    fn configurable(&self) -> Option<&dyn Configurable> {
        Some(&self.0)
    }
    fn digest_len(&self) -> usize {
        4
    }
    fn hash_parts(&self, _: &[&[u8]]) -> Digest {
        Digest::new(&[0_u8; 4]).unwrap()
    }
}

/// An encoding that copies its input, "notedenc".
struct Copying(Noted);

impl Encoding for Copying {
    fn id(&self) -> EncodingId {
        EncodingId(9001)
    }
    fn name(&self) -> &'static str {
        "notedenc"
    }
    fn configurable(&self) -> Option<&dyn Configurable> {
        Some(&self.0)
    }
    fn overhead(&self, _: &str) -> usize {
        0
    }
    fn encode(&self, _: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        out.extend_from_slice(input);
        Ok(())
    }
    fn decode(&self, _: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        out.extend_from_slice(input);
        Ok(())
    }
}

/// Credentials whose verifier is the password, "notedcred".
struct Plain(Noted);

impl Credentials for Plain {
    fn name(&self) -> &'static str {
        "notedcred"
    }
    fn configurable(&self) -> Option<&dyn Configurable> {
        Some(&self.0)
    }
    fn verifier(&self, _: &str, password: &str) -> Result<Vec<u8>> {
        Ok(password.as_bytes().to_vec())
    }
}

/// A server of no connection, "notedserved".
struct Served(Noted);

impl ServedConnections for Served {
    fn name(&self) -> &'static str {
        "notedserved"
    }
    fn configurable(&self) -> Option<&dyn Configurable> {
        Some(&self.0)
    }
    fn end(&self, _: u64) -> bool {
        false
    }
    fn cancel(&self, _: u64) -> bool {
        false
    }
}

/// The built-in spill, under the name it is given.
struct Spilled(&'static str, Noted);

impl Spill for Spilled {
    fn name(&self) -> &'static str {
        self.0
    }
    fn configurable(&self) -> Option<&dyn Configurable> {
        Some(&self.1)
    }
    fn area(&self, memory: &Arc<MemoryBudget>, temp: &Arc<TempFiles>, area: Area) -> Result<Box<dyn SpillArea>> {
        PagedSpill.area(memory, temp, area)
    }
}

/// A log in memory, "notedwal".
struct Logged(MemoryLog, Noted);

impl Log for Logged {
    fn name(&self) -> &'static str {
        "notedwal"
    }
    fn configurable(&self) -> Option<&dyn Configurable> {
        Some(&self.1)
    }
    fn capabilities(&self) -> LogCapabilities {
        self.0.capabilities()
    }
    fn append(&self, record: &[u8]) -> Result<Lsn> {
        self.0.append(record)
    }
    fn sync(&self, through: Lsn) -> Result<()> {
        self.0.sync(through)
    }
    fn read_from(&self, from: Lsn) -> Records<'_> {
        self.0.read_from(from)
    }
    fn truncate_before(&self, before: Lsn) -> Result<()> {
        self.0.truncate_before(before)
    }
    fn end(&self) -> Lsn {
        self.0.end()
    }
}

/// A diagnostics sink that drops every event, "notedsink".
struct Sink(Noted);

impl Diagnostics for Sink {
    fn name(&self) -> &'static str {
        "notedsink"
    }
    fn configurable(&self) -> Option<&dyn Configurable> {
        Some(&self.0)
    }
    fn event(&self, _: &Event) {}
}

/// A clock standing at 1970-01-01, "notedclock".
struct Still(Noted);

impl Clock for Still {
    fn name(&self) -> &'static str {
        "notedclock"
    }
    fn configurable(&self) -> Option<&dyn Configurable> {
        Some(&self.0)
    }
    fn now(&self) -> i64 {
        0
    }
}

/// Registers a plug-in, its hook the one given, as the database takes it.
type Register = fn(&Database, Noted) -> Result<()>;

/// A plug-in of each kind but an engine, by the setting it declares, of
/// its name, and how it is registered.
fn kinds() -> Vec<(&'static str, Register)> {
    vec![
        ("NOTEDIX.DEPTH", |db, n| db.register_index_method(Box::new(Method(n)))),
        ("NOTEDFN.DEPTH", |db, n| db.register_scalar_function(Box::new(Scalar(n)))),
        ("NOTEDAGG.DEPTH", |db, n| db.register_aggregate_function(Box::new(Aggregate(n)))),
        ("NOTEDTAB.DEPTH", |db, n| db.register_system_table(Box::new(Table(n)))),
        ("NOTEDCAT.DEPTH", |db, n| db.set_catalog_storage(Box::new(Catalogs(n)))),
        ("NOTEDLOG.DEPTH", |db, n| db.set_log_storage(Box::new(Logs(n)))),
        ("NOTEDHASH.DEPTH", |db, n| db.register_hash_function(Box::new(Hash(n)))),
        ("NOTEDENC.DEPTH", |db, n| db.register_encoding(Box::new(Copying(n)))),
        ("NOTEDCRED.DEPTH", |db, n| db.set_credentials(Box::new(Plain(n)))),
        ("NOTEDSERVED.DEPTH", |db, n| db.set_served_connections(Box::new(Served(n)))),
        ("NOTEDSPILL.DEPTH", |db, n| db.set_spill(Box::new(Spilled("notedspill", n)))),
        ("NOTEDTYPE.DEPTH", |db, n| db.register_type(Box::new(Named("NOTEDTYPE", Some(n))))),
        ("NOTEDWAL.DEPTH", |db, n| db.set_log(Arc::new(Logged(MemoryLog::new(), n)))),
        ("NOTEDSINK.DEPTH", |db, n| db.set_diagnostics(Box::new(Sink(n)))),
        ("NOTEDCLOCK.DEPTH", |db, n| db.set_clock(Arc::new(Still(n)))),
    ]
}

/// The setting `setting` as INFORMATION_SCHEMA.SETTINGS shows it: its
/// value, source, effect, started value and whether a change waits.
fn shown(db: &Database, setting: &str) -> Vec<String> {
    let sql = format!(
        "SELECT TRIM(SETTING_VALUE), TRIM(VALUE_SOURCE), TRIM(TAKES_EFFECT), STARTED_VALUE, IS_PENDING \
         FROM INFORMATION_SCHEMA.SETTINGS WHERE SETTING_NAME = '{setting}'"
    );
    rows(&db.session("ANYONE"), &sql).unwrap()
}

/// Verifies: [DESIGN] RULES.md D36, R-SAFE-3: a plug-in of every kind (an
/// index method, a scalar and an aggregate function, a system table, a
/// catalog's and a log's storage, a hash function, an encoding,
/// credentials, a server's connections, a spill, a data type, a log, a
/// diagnostics sink, a clock) declares its setting as it is registered
/// and is told its value: its default at once, each value the database is
/// given after, its default again once reset; INFORMATION_SCHEMA.SETTINGS
/// shows it.
#[test]
fn a_plug_in_of_every_kind_reads_its_settings_as_they_are_published() {
    for (setting, register) in kinds() {
        let db = Database::new();
        let noted = Noted::new(setting);
        register(&db, noted.clone()).unwrap();
        assert_eq!(noted.told(), 1, "{setting}: its default");
        assert_eq!(shown(&db, setting), ["1,DEFAULT,STATEMENT,NULL,NO"], "{setting}");
        db.set_setting(&setting.to_ascii_lowercase(), &whole(4)).unwrap();
        assert_eq!(noted.told(), 4, "{setting}");
        assert_eq!(shown(&db, setting), ["4,DATABASE,STATEMENT,NULL,NO"], "{setting}");
        db.reset_setting(setting).unwrap();
        assert_eq!(noted.told(), 1, "{setting}: its default again");
    }
}

/// Verifies: [DESIGN] RULES.md D36: a setting a plug-in of any kind
/// declares outside its own name, or of a built-in setting's name, is
/// refused, the plug-in neither registered nor told; one of no settings
/// is registered as before.
#[test]
fn a_plug_ins_setting_outside_its_name_is_refused() {
    for (setting, register) in kinds() {
        let db = Database::new();
        let stray = Noted::new("OTHER.DEPTH");
        assert_eq!(register(&db, stray.clone()).map_err(|e| e.sqlcode()), Err(-910_i32), "{setting}");
        assert_eq!(stray.told(), -1, "{setting}: not told");
        assert!(db.setting("OTHER.DEPTH").is_err(), "{setting}: not declared");
    }
    let db = Database::new();
    let builtin = Noted::new("LOCKS.WAIT");
    assert_eq!(db.set_spill(Box::new(Spilled("locks", builtin.clone()))).map_err(|e| e.sqlcode()), Err(-910_i32));
    assert_eq!((builtin.told(), db.setting("LOCKS.WAIT").unwrap().to_string()), (-1, "OFF".to_owned()), "the built-in kept");
    db.set_spill(Box::new(PagedSpill)).unwrap();
    db.register_index_method(Box::new(turborel::index::HashMethod::in_memory())).unwrap();
}

/// Verifies: [DESIGN] RULES.md D36: a plug-in's setting that takes effect
/// as it starts (`Effect::Start`) is kept as it started, that value shown
/// as STARTED_VALUE: a change is shown waiting for a restart (IS_PENDING
/// YES) until the plug-in is registered again, which takes it.
#[test]
fn a_plug_ins_setting_taking_effect_at_its_start_waits_for_it() {
    let db = Database::new();
    let first = Noted::taking("NOTEDIX.DEPTH", Effect::Start);
    db.register_index_method(Box::new(Method(first.clone()))).unwrap();
    assert_eq!(first.told(), 1);
    db.set_setting("NOTEDIX.DEPTH", &whole(5)).unwrap();
    assert_eq!(first.told(), 1, "kept as it started");
    assert_eq!(shown(&db, "NOTEDIX.DEPTH"), ["5,DATABASE,START,1,YES"]);
    let again = Noted::taking("NOTEDIX.DEPTH", Effect::Start);
    db.register_index_method(Box::new(Method(again.clone()))).unwrap();
    assert_eq!(again.told(), 5, "started again");
    assert_eq!(shown(&db, "NOTEDIX.DEPTH"), ["5,DATABASE,START,5,NO"]);
}

/// Verifies: [DESIGN] RULES.md D25, D36: plug-ins' settings are kept with
/// the definitions: plug-ins registered again before the database is
/// opened again, a catalog's and a log's storage and a function, are told
/// their defaults, then the values given before as it opens, replayed.
#[test]
fn plug_ins_are_told_their_settings_kept_as_the_database_opens() {
    let scratch = Scratch::new("plugin-settings");
    let dir = scratch.path("db");
    let registered = |db: &Database| {
        let noted = [Noted::new("NOTEDCAT.DEPTH"), Noted::new("NOTEDLOG.DEPTH"), Noted::new("NOTEDFN.DEPTH")];
        db.set_catalog_storage(Box::new(Catalogs(noted[0].clone()))).unwrap();
        db.set_log_storage(Box::new(Logs(noted[1].clone()))).unwrap();
        db.register_scalar_function(Box::new(Scalar(noted[2].clone()))).unwrap();
        noted
    };
    let db = Database::new();
    registered(&db);
    db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    for (setting, value) in [("NOTEDCAT.DEPTH", 3), ("NOTEDLOG.DEPTH", 4), ("NOTEDFN.DEPTH", 5)] {
        db.set_setting(setting, &whole(value)).unwrap();
    }
    db.close().unwrap();
    drop(db);
    let again = Database::new();
    let noted = registered(&again);
    assert_eq!(noted.iter().map(Noted::told).collect::<Vec<_>>(), [1, 1, 1], "their defaults, until opened");
    again.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    assert_eq!(noted.iter().map(Noted::told).collect::<Vec<_>>(), [3, 4, 5], "kept");
}
