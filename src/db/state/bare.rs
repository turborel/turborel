//! The state of nothing registered or attached, which `with_builtins`
//! gives the built-ins.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::super::sealing::DATABASE_OPTIONS;
use super::super::users::Users;
use super::State;
use crate::catalog::Catalog;
use crate::engine::MemoryPages;
use crate::functions::Registry;
use crate::hash::murmur::MurmurMap;
use crate::memory::MemoryBudget;
use crate::optimizer::Optimizer;
use crate::settings::Settings;
use crate::spill::WorkAreas;
use crate::txn::Transactions;
use crate::value::types::TypeRegistry;

impl State {
    /// The state over `catalog`, of `memory` and its work areas `work`,
    /// with nothing registered or attached.
    pub(super) fn bare(catalog: Box<dyn Catalog>, (memory, work): (&Arc<MemoryBudget>, Arc<WorkAreas>)) -> State {
        State {
            generation: 0,
            catalog: Arc::from(catalog),
            engines: HashMap::new(),
            table_options: vec![(DATABASE_OPTIONS.to_owned(), crate::storage::encodings_option())],
            seals: Arc::default(),
            index_options: Vec::new(),
            methods: HashMap::new(),
            placement: HashMap::new(),
            hashes: crate::hash::crc::builtin(),
            collations: crate::value::collation::builtin().into_iter().map(|c| (c.name().to_owned(), c)).collect(),
            encodings: Arc::default(),
            created: false,
            retired: HashMap::new(),
            stamp: crate::value::datetime::Now::default(),
            settings: Settings::default(),
            users: Users::default(),
            system: crate::system::builtin(),
            storage: MurmurMap::default(),
            locks: MurmurMap::default(),
            indexes: HashMap::new(),
            functions: Registry::with_set_functions(),
            xquery_functions: crate::xquery::Library::builtin(),
            types: TypeRegistry::builtin(),
            rankings: super::super::rankings::SharedRankings::default(),
            strategies: super::super::partitions::SharedStrategies::default(),
            vectors: super::super::vectors::SharedVectors::default(),
            embedders: Arc::new(crate::vector::embed::Embedders::built_in()),
            extensions: Arc::default(),
            extension_host: Arc::new(crate::plugin::Host::new(Arc::clone(memory))),
            clock: Arc::new(crate::clock::SystemClock),
            uuids: Arc::default(),
            optimizer: Optimizer::with_builtin_rules(),
            tx: Arc::new(Transactions::default()),
            memory: Arc::clone(memory),
            pages: MemoryPages::new(crate::db::POOL_BYTES),
            temporaries: Arc::default(),
            sequences: Arc::default(),
            work: Arc::clone(&work),
            temp_lobs: Arc::new(crate::spill::TempLobStore::new(work)),
            jobs: Arc::default(),
            graph_runs: Arc::default(),
            ahead: crate::disk::buffer::AheadReader::new(),
            eviction: Arc::default(),
            rekey: Arc::default(),
            files: Arc::default(),
            plugins: Arc::new(crate::files::plugins::PluginFiles::in_memory(Arc::clone(memory))),
            log: None,
            reopening: None,
            building: HashSet::new(),
        }
    }
}
