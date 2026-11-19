//! \[EXTENSION\] The object of each host kind a starter project serves
//! (RULES.md D229, D263): a host plug-in's, which the node names in
//! PLUGINS.HOST. The kinds a call or two serves do something small and
//! whole; a kind that keeps what the database relies on (the log, the
//! catalog, spill, files, connections, keys, replication and the
//! archive) refuses each call until its author writes it, which stops
//! the node, so a starter never pretends to keep data.

use super::kinds::Starter;

/// The source of a host object of replication's or the archive's, served
/// by its operations' codes until the SDK's trait is written.
macro_rules! raw {
    ($name:literal, $ty:literal, $what:literal) => {
        concat!(
            "/// ", $name, ": ", $what, ", every operation refused until it is written.\nstruct ", $ty, ";\n\n",
            "impl HostObject for ", $ty, " {\n    fn answer(&mut self, _cx: &mut Cx<'_>, op: u8, _payload: &[u8]) -> Answer<Vec<u8>> {\n        ",
            "Err(PluginError::external(&format!(\"", $name, " does not serve operation {op} yet\")))\n    }\n}\n"
        )
    };
}

/// The paths a raw host object uses.
const RAW: &[&str] = &["turborel_plugin::Cx", "turborel_plugin::HostObject", "turborel_plugin::PluginError"];

/// Every host kind, in the order of their codes.
pub(in crate::extension) const HOST_STARTERS: [Starter; 17] = [
    Starter {
        word: "encoding",
        code: 20,
        declaration: "HOST ENCODING PLAIN (ID = 60001)",
        uses: &["turborel_plugin::host::PageEncoding"],
        source: "/// PLAIN: each page as it is, for an encoding to grow from.
struct Plain;

impl PageEncoding for Plain {
    fn encode(&mut self, _context: u64, _argument: &str, page: &[u8]) -> Answer<Vec<u8>> {
        Ok(page.to_vec())
    }
    fn decode(&mut self, _context: u64, page: &[u8]) -> Answer<Vec<u8>> {
        Ok(page.to_vec())
    }
}
",
        added: ".host_encoding(\"PLAIN\", Plain)",
    },
    Starter {
        word: "hash-function",
        code: 21,
        declaration: "HOST HASH FUNCTION FNV (ID = 60001, LENGTH = 8)",
        uses: &["turborel_plugin::host::Checksum"],
        source: "/// FNV: FNV-1a of 64 bits, its digest eight bytes.
struct Fnv;

impl Checksum for Fnv {
    fn hash(&mut self, parts: &[Vec<u8>]) -> Vec<u8> {
        let digest = parts.iter().flatten().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3));
        digest.to_le_bytes().to_vec()
    }
}
",
        added: ".host_checksum(\"FNV\", Fnv)",
    },
    Starter {
        word: "clock",
        code: 22,
        declaration: "HOST CLOCK SYSTEM (EVERY = 10)",
        uses: &["turborel_plugin::host::TimeSource"],
        source: "/// SYSTEM: the system's time, in microseconds since 1970.
struct System;

impl TimeSource for System {
    fn now(&mut self) -> i64 {
        let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        i64::try_from(since.as_micros()).unwrap_or(i64::MAX)
    }
}
",
        added: ".host_clock(\"SYSTEM\", System)",
    },
    Starter {
        word: "log-storage",
        code: 23,
        declaration: "HOST LOG STORAGE LOG (BATCH = 256)",
        uses: &["turborel_plugin::PluginError", "turborel_plugin::host::LogStore", "turborel_plugin::host::Records"],
        source: "/// LOG: the database's log, every call refused until it is written.
struct Log;

impl LogStore for Log {
    fn exists(&mut self, _dir: &str) -> Answer<bool> {
        Err(PluginError::external(\"not written yet\"))
    }
    fn open(&mut self, _dir: &str) -> Answer<Option<u64>> {
        Err(PluginError::external(\"not written yet\"))
    }
    fn create(&mut self, _dir: &str) -> Answer<()> {
        Err(PluginError::external(\"not written yet\"))
    }
    fn append(&mut self, _dir: &str, _records: &[(u64, Vec<u8>)]) -> Answer<()> {
        Err(PluginError::external(\"not written yet\"))
    }
    fn read(&mut self, _dir: &str, _from: u64, _max: u32) -> Answer<(Records, bool)> {
        Err(PluginError::external(\"not written yet\"))
    }
    fn truncate(&mut self, _dir: &str, _before: u64) -> Answer<()> {
        Err(PluginError::external(\"not written yet\"))
    }
}
",
        added: ".host_log(\"LOG\", Log)",
    },
    Starter {
        word: "catalog-storage",
        code: 24,
        declaration: "HOST CATALOG STORAGE CATALOG",
        uses: &["turborel_plugin::PluginError", "turborel_plugin::host::CatalogKeeper"],
        source: "/// CATALOG: the catalog's images, every call refused until it is written.
struct Catalog;

impl CatalogKeeper for Catalog {
    fn open(&mut self, _dir: &str) -> Answer<Option<Option<Vec<u8>>>> {
        Err(PluginError::external(\"not written yet\"))
    }
    fn create(&mut self, _dir: &str) -> Answer<()> {
        Err(PluginError::external(\"not written yet\"))
    }
    fn record(&mut self, _dir: &str, _image: &[u8], _compact_after: u64) -> Answer<()> {
        Err(PluginError::external(\"not written yet\"))
    }
}
",
        added: ".host_catalog(\"CATALOG\", Catalog)",
    },
    Starter {
        word: "spill",
        code: 25,
        declaration: "HOST SPILL AREAS",
        uses: &["turborel_plugin::PluginError", "turborel_plugin::host::SpillStore"],
        source: "/// AREAS: spill's tables and runs, every call refused until it is written.
struct Areas;

impl SpillStore for Areas {
    fn table(&mut self) -> Answer<u64> {
        Err(PluginError::external(\"not written yet\"))
    }
    fn run(&mut self) -> Answer<u64> {
        Err(PluginError::external(\"not written yet\"))
    }
    fn push(&mut self, _id: u64, _records: Vec<Vec<u8>>) -> Answer<()> {
        Err(PluginError::external(\"not written yet\"))
    }
    fn read(&mut self, _id: u64, _from: u64, _max: u32) -> Answer<(Vec<Vec<u8>>, bool)> {
        Err(PluginError::external(\"not written yet\"))
    }
    fn insert(&mut self, _id: u64, _pairs: Vec<(Vec<u8>, Vec<u8>)>) -> Answer<()> {
        Err(PluginError::external(\"not written yet\"))
    }
    fn get(&mut self, _id: u64, _key: &[u8]) -> Answer<Vec<Vec<u8>>> {
        Err(PluginError::external(\"not written yet\"))
    }
    fn remove(&mut self, _id: u64, _key: &[u8], _value: &[u8]) -> Answer<bool> {
        Err(PluginError::external(\"not written yet\"))
    }
    fn drop_id(&mut self, _id: u64) {}
}
",
        added: ".host_spill(\"AREAS\", Areas)",
    },
    Starter {
        word: "file-system",
        code: 26,
        declaration: "HOST FILE SYSTEM FILES",
        uses: &["turborel_plugin::host::Files"],
        source: FILES,
        added: ".host_files(\"FILES\", Store)",
    },
    Starter {
        word: "served-connections",
        code: 27,
        declaration: "HOST SERVED CONNECTIONS SERVED",
        uses: &["turborel_plugin::host::Connections"],
        source: "/// SERVED: connections it ends and cancels none of.
struct Served;

impl Connections for Served {
    fn end(&mut self, _id: u64) -> bool {
        false
    }
    fn cancel(&mut self, _id: u64) -> bool {
        false
    }
}
",
        added: ".host_connections(\"SERVED\", Served)",
    },
    Starter {
        word: "listener",
        code: 28,
        declaration: "HOST LISTENER DOOR",
        uses: &["turborel_plugin::PluginError", "turborel_plugin::host::Accept"],
        source: "/// DOOR: where clients come from, no client taken until it is written.
struct Door;

impl Accept for Door {
    fn accept(&mut self, _wait_ms: u32) -> Answer<Option<String>> {
        Ok(None)
    }
    fn relay(&mut self, _wait_ms: u32, _out: &[u8]) -> Answer<(bool, Vec<u8>)> {
        Err(PluginError::external(\"not written yet\"))
    }
}
",
        added: ".host_listener(\"DOOR\", Door)",
    },
    Starter {
        word: "package-crypto",
        code: 29,
        declaration: "HOST PACKAGE CRYPTO SEALS",
        uses: &["turborel_plugin::host::Crypto"],
        source: "/// SEALS: package cryptography that matches no digest and verifies no
/// signature until it is written, so it installs no package.
struct Seals;

impl Crypto for Seals {
    fn sha256(&mut self, _bytes: &[u8]) -> [u8; 32] {
        [0; 32]
    }
    fn verify(&mut self, _key: &[u8; 32], _message: &[u8], _signature: &[u8; 64]) -> bool {
        false
    }
}
",
        added: ".host_crypto(\"SEALS\", Seals)",
    },
    Starter {
        word: "keyring",
        code: 30,
        declaration: "HOST KEYRING KEYS",
        uses: &["turborel_plugin::host::Keys"],
        source: "/// KEYS: a keyring holding no key until it is written.
struct Ring;

impl Keys for Ring {
    fn key(&mut self, _name: &str) -> Answer<Option<[u8; 32]>> {
        Ok(None)
    }
}
",
        added: ".host_keyring(\"KEYS\", Ring)",
    },
    Starter {
        word: "background-job",
        code: 31,
        declaration: "HOST BACKGROUND JOB TIDY",
        uses: &["turborel_plugin::host::Job"],
        source: "/// TIDY: a job doing nothing yet, run again a minute after.
struct Tidy;

impl Job for Tidy {
    fn run(&mut self) -> Answer<Option<u64>> {
        Ok(Some(60_000))
    }
}
",
        added: ".host_job(\"TIDY\", Tidy)",
    },
    Starter {
        word: "replica-transport",
        code: 32,
        declaration: "HOST REPLICA TRANSPORT WIRE",
        uses: RAW,
        source: raw!("WIRE", "Wire", "how replicas reach each other"),
        added: ".host(32, \"WIRE\", Wire)",
    },
    Starter {
        word: "journal",
        code: 33,
        declaration: "HOST JOURNAL BOOK",
        uses: RAW,
        source: raw!("BOOK", "Book", "the consensus journal's storage"),
        added: ".host(33, \"BOOK\", Book)",
    },
    Starter {
        word: "state-transfer",
        code: 34,
        declaration: "HOST STATE TRANSFER SHIP",
        uses: RAW,
        source: raw!("SHIP", "Ship", "how a replica's state is shipped"),
        added: ".host(34, \"SHIP\", Ship)",
    },
    Starter {
        word: "consensus",
        code: 35,
        declaration: "HOST CONSENSUS VOTE",
        uses: RAW,
        source: raw!("VOTE", "Vote", "the replication protocol"),
        added: ".host(35, \"VOTE\", Vote)",
    },
    Starter {
        word: "archive",
        code: 36,
        declaration: "HOST ARCHIVE VAULT",
        uses: RAW,
        source: raw!("VAULT", "Vault", "where the log is archived and backups kept"),
        added: ".host(36, \"VAULT\", Vault)",
    },
];

/// FILES: the database's files, every call refused until it is written.
const FILES: &str = "/// FILES: the database's files, every call refused until it is written.
struct Store;

/// Refused, as nothing is written yet.
fn unwritten<T>() -> Answer<T> {
    Err(turborel_plugin::PluginError::external(\"not written yet\"))
}

impl Files for Store {
    fn open(&mut self, _path: &str, _mode: turborel_plugin::host::Mode) -> Answer<()> {
        unwritten()
    }
    fn read(&mut self, _path: &str, _offset: u64, _max: u32) -> Answer<Vec<u8>> {
        unwritten()
    }
    fn write(&mut self, _path: &str, _offset: u64, _bytes: &[u8]) -> Answer<()> {
        unwritten()
    }
    fn size(&mut self, _path: &str) -> Answer<u64> {
        unwritten()
    }
    fn set_size(&mut self, _path: &str, _size: u64) -> Answer<()> {
        unwritten()
    }
    fn sync(&mut self, _path: &str, _durability: u8) -> Answer<()> {
        unwritten()
    }
    fn remove(&mut self, _path: &str) -> Answer<()> {
        unwritten()
    }
    fn exists(&mut self, _path: &str) -> Answer<bool> {
        unwritten()
    }
    fn create_dirs(&mut self, _path: &str) -> Answer<()> {
        unwritten()
    }
    fn read_dir(&mut self, _path: &str) -> Answer<Vec<String>> {
        unwritten()
    }
    fn sync_dir(&mut self, _path: &str) -> Answer<()> {
        unwritten()
    }
    fn lock(&mut self, _path: &str) -> Answer<u64> {
        unwritten()
    }
    fn unlock(&mut self, _token: u64) {}
    fn rename(&mut self, _from: &str, _to: &str) -> Answer<()> {
        unwritten()
    }
    fn remove_tree(&mut self, _path: &str) -> Answer<()> {
        unwritten()
    }
}
";

/// Whether `starter` is of a host kind.
pub(in crate::extension) fn is_host(starter: &Starter) -> bool {
    starter.code >= 20
}
