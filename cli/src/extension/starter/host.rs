//! \[EXTENSION\] The object of each host kind a starter project serves
//! (RULES.md D229, D263): a host plug-in's, which the node names in
//! PLUGINS.HOST. The kinds a call or two serves do something small and
//! whole; a kind that keeps what the database relies on (the log, the
//! catalog, spill, files, connections, keys, replication and the
//! archive) refuses each call until its author writes it, which stops
//! the node, so a starter never pretends to keep data.

use super::kinds::Starter;

/// A method of signature `sig` of a starter's object `name`, refusing
/// each call until it is written.
macro_rules! refused {
    ($name:literal, $sig:literal) => {
        concat!("    fn ", $sig, " {\n        Err(PluginError::external(\"", $name, " is not written yet\"))\n    }\n")
    };
}

/// WIRE: a replica transport refusing each call.
const WIRE: &str = concat!(
    "/// WIRE: how replicas reach each other, every call refused until it is written.\nstruct Wire;\n\n",
    "impl ReplicaTransport for Wire {\n",
    refused!("WIRE", "open(&mut self, _me: u32, _configuration: &[Member]) -> Answer<Vec<u32>>"),
    refused!("WIRE", "send(&mut self, _to: u32, _frame: &[u8]) -> Answer<bool>"),
    refused!("WIRE", "receive(&mut self) -> Answer<Vec<(u32, Vec<u8>)>>"),
    refused!("WIRE", "reconfigure(&mut self, _configuration: &[Member]) -> Answer<Vec<u32>>"),
    "}\n"
);

/// BOOK: a journal refusing each call.
const BOOK: &str = concat!(
    "/// BOOK: the consensus journal's storage, every call refused until it is written.\nstruct Book;\n\n",
    "impl Journal for Book {\n",
    refused!("BOOK", "open(&mut self) -> Answer<Held>"),
    refused!("BOOK", "persist(&mut self, _at: u64, _record: &[u8]) -> Answer<()>"),
    refused!("BOOK", "truncate(&mut self, _after: u64) -> Answer<()>"),
    refused!("BOOK", "retain(&mut self, _through: u64) -> Answer<()>"),
    refused!("BOOK", "mark(&mut self, _at: u64, _epoch: u64, _members: &[Member]) -> Answer<()>"),
    refused!("BOOK", "persist_state(&mut self, _state: &[u8]) -> Answer<()>"),
    refused!("BOOK", "sync(&mut self) -> Answer<Vec<u64>>"),
    refused!("BOOK", "flush(&mut self) -> Answer<Vec<u64>>"),
    "}\n"
);

/// SHIP: a state transfer refusing each call.
const SHIP: &str = concat!(
    "/// SHIP: how a replica's state is shipped, every call refused until it is written.\nstruct Ship;\n\n",
    "impl StateTransfer for Ship {\n",
    refused!("SHIP", "begin(&mut self, _from: u32) -> Answer<(u64, Option<Vec<u8>>)>"),
    refused!("SHIP", "take(&mut self, _fetch: u64, _frame: &[u8]) -> Answer<(bool, Option<Vec<u8>>)>"),
    refused!("SHIP", "fetched(&mut self, _fetch: u64) -> Answer<Manifest>"),
    refused!("SHIP", "chunk(&mut self, _fetch: u64, _file: u32, _at: u64, _max: u32) -> Answer<Vec<u8>>"),
    refused!("SHIP", "export(&mut self, _export: u64, _manifest: Manifest) -> Answer<()>"),
    refused!("SHIP", "serve(&mut self, _to_export: (u32, u64), _request: &[u8], _given: Option<(u32, u64, Vec<u8>)>) -> Answer<Served>"),
    "}\n"
);

/// VOTE: a consensus refusing each step.
const VOTE: &str = concat!(
    "/// VOTE: the replication protocol, every step refused until it is written.\nstruct Vote;\n\n",
    "impl Consensus for Vote {\n",
    refused!("VOTE", "open(&mut self, _me: u32, _configuration: &[Member], _recovered: Option<Held>, _nonce: u64) -> Answer<()>"),
    refused!("VOTE", "step(&mut self, _input: ReplicaInput) -> Answer<Vec<ReplicaOutput>>"),
    "    fn describe(&mut self) -> String {\n        \"VOTE, not written yet\".to_owned()\n    }\n}\n"
);

/// VAULT: an archive refusing each call.
const VAULT: &str = concat!(
    "/// VAULT: where the log is archived and backups kept, refused until it is written.\nstruct Vault;\n\n",
    "impl Target for Vault {\n",
    refused!("VAULT", "open(&mut self, _location: &str) -> Answer<Box<dyn TargetStore>>"),
    "}\n"
);

/// Every host kind, in the order of their codes.
pub(in crate::extension) const HOST_STARTERS: [Starter; 18] = [
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
struct Sessions;

impl Connections for Sessions {
    fn end(&mut self, _id: u64) -> bool {
        false
    }
    fn cancel(&mut self, _id: u64) -> bool {
        false
    }
}
",
        added: ".host_connections(\"SERVED\", Sessions)",
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
        uses: &["turborel_plugin::ReplicaTransport", "turborel_plugin::Member", "turborel_plugin::PluginError"],
        source: WIRE,
        added: ".replica_transport(\"WIRE\", Wire)",
    },
    Starter {
        word: "journal",
        code: 33,
        declaration: "HOST JOURNAL BOOK",
        uses: &["turborel_plugin::Journal", "turborel_plugin::Held", "turborel_plugin::Member", "turborel_plugin::PluginError"],
        source: BOOK,
        added: ".journal(\"BOOK\", Book)",
    },
    Starter {
        word: "state-transfer",
        code: 34,
        declaration: "HOST STATE TRANSFER SHIP",
        uses: &["turborel_plugin::StateTransfer", "turborel_plugin::Manifest", "turborel_plugin::Served", "turborel_plugin::PluginError"],
        source: SHIP,
        added: ".state_transfer(\"SHIP\", Ship)",
    },
    Starter {
        word: "consensus",
        code: 35,
        declaration: "HOST CONSENSUS VOTE",
        uses: &[
            "turborel_plugin::Consensus",
            "turborel_plugin::Held",
            "turborel_plugin::Member",
            "turborel_plugin::ReplicaInput",
            "turborel_plugin::ReplicaOutput",
            "turborel_plugin::PluginError",
        ],
        source: VOTE,
        added: ".consensus(\"VOTE\", Vote)",
    },
    Starter {
        word: "archive",
        code: 36,
        declaration: "HOST ARCHIVE VAULT",
        uses: &["turborel_plugin::Target", "turborel_plugin::TargetStore", "turborel_plugin::PluginError"],
        source: VAULT,
        added: ".archive(\"VAULT\", Vault)",
    },
    Starter {
        word: "buffer-eviction",
        code: 38,
        declaration: "HOST BUFFER EVICTION SCANS (EVERY = 1000);\nSETTING ONCE TYPE INTEGER MIN 0 MAX 1000 DEFAULT 4",
        uses: &["turborel_plugin::Cx", "turborel_plugin::host::EvictionAdvisor", "turborel_plugin::host::Policy", "turborel_plugin::host::Report", "turborel_plugin::host::policies"],
        source: SCANS,
        added: ".host_eviction(\"SCANS\", Scans)",
    },
];

/// SCANS: an eviction policy resisting scans, as its plug-in's setting
/// ONCE weighs a page read once.
const SCANS: &str = "/// SCANS: least recently used, a page read once evicted before one
/// used again by as many intervals as the plug-in's setting ONCE says.
struct Scans;

impl EvictionAdvisor for Scans {
    fn advise(&mut self, cx: &mut Cx<'_>, _report: &Report) -> Answer<Option<Policy>> {
        let once = cx.settings().iter().find(|(n, _)| n.ends_with(\".ONCE\")).map(|(n, _)| n.clone());
        let once = once.and_then(|n| cx.integer(&n)).and_then(|o| u32::try_from(o).ok()).unwrap_or(0);
        Ok(Some(policies::scan_resistant(once)))
    }
}
";

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
