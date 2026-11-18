//! \[DESIGN\] The definitions as they are (RULES.md D25): what a checkpoint's
//! image records of them, so a database is opened from them, each table
//! and index reopened by its locator, and only the definitions logged since
//! replayed; not from every definition ever made. The schemas, domains,
//! assertions, tables with their generations, views, indexes, sequence
//! generators, user-defined types, privileges, triggers and SQL-invoked
//! routines of the catalog, the
//! database's settings, in the forms of `catalog/codec.rs`; its users and
//! owner (`db/users.rs`); and the counters of its sequence generators,
//! settled, with the last epoch given (`db/sequences/counters.rs`,
//! RULES.md D62); its roles, and the users who may create them.
//!
//! ```text
//! snapshot: schemas (count u32, each its name and its owner, str),
//!           domains (count u32, each a domain), collations (count u32,
//!           each a collation), collation versions (count u32, each a name
//!           and version), assertions (count u32, each a constraint),
//!           tables (count u32, each a table), views (likewise), indexes
//!           (likewise), privileges (likewise), sequence generators
//!           (likewise), user-defined types (likewise), settings (count
//!           u32, each str and value), users (count u32, each str and
//!           verifier bytes), owner (u8 1 and str, or u8 0 for none), the
//!           last epoch given (u64), counters (count u32, each a name, its
//!           epoch u64 and its base i128), triggers (count u32, each a
//!           trigger, in the order of their timestamps of creation),
//!           routines (count u32, each a routine, likewise), roles (as
//!           `catalog/codec/roles.rs` writes them), the users allowed to
//!           create roles (count u32, each str), SQL-server modules (count
//!           u32, each a module, in the order of their timestamps of
//!           creation), structured types (count u32, each a type),
//!           user-defined casts, orderings and transforms (likewise),
//!           property graphs (count u32, each a graph, by name),
//!           embedding models and vector pipelines (`catalog/embedding.rs`),
//!           graph types and GQL graphs (count u32 and each, by name),
//!           the spatial reference systems defined and the geometry
//!           columns registered (as `catalog/codec/spatial.rs` writes them)
//! ```

use std::sync::Arc;

use super::sequences::Settled;
use super::{State, api};
use crate::bytes::{Put, Reader};
use crate::catalog::codec::{
    cast, gql_graph, graph, graph_type, ordering, put_cast, put_gql_graph, put_graph, put_graph_type, put_ordering, put_structured_type,
    put_transforms, structured_type, transforms,
};
use crate::catalog::codec::{
    collation, constraint, domain, index, module, privilege, put_collation, put_constraint, put_domain, put_index, put_module, put_name,
    put_privilege, put_roles, put_routine, put_sequence, put_table, put_trigger, put_user_type, put_view, qualified, roles, routine,
    sequence, table, trigger, user_type, view,
};
use crate::catalog::{
    CastDesc, Embeddings, GqlGraphDesc, GraphDesc, GraphTypeDesc, OrderingDesc, StructuredDesc, TransformGroupDesc, embeddings,
    put_embeddings,
};
use crate::catalog::{
    Catalog, CollationDesc, ConstraintDesc, DomainDesc, IndexDesc, ModuleDesc, PrivilegeDesc, QualifiedName, Roles, SequenceDesc,
    SqlRoutineDesc, TableDesc, TriggerDesc, UserTypeDesc, ViewDesc, module_of_schema,
};
use crate::error::Result;
use crate::settings;
use crate::value::Value;
use crate::value::codec::{put_value, value};

/// The definitions as a checkpoint captured them.
#[derive(Debug, Clone, Default)]
pub(super) struct Snapshot {
    /// Each schema's name, and its owner.
    schemas: Vec<(String, String)>,
    domains: Vec<DomainDesc>,
    /// The `<collation definition>`s made.
    collations: Vec<CollationDesc>,
    /// Each built-in collation's version as this was taken, by name.
    collation_versions: Vec<(String, String)>,
    assertions: Vec<ConstraintDesc>,
    tables: Vec<TableDesc>,
    views: Vec<ViewDesc>,
    indexes: Vec<IndexDesc>,
    privileges: Vec<PrivilegeDesc>,
    sequences: Vec<SequenceDesc>,
    user_types: Vec<UserTypeDesc>,
    settings: Vec<(String, Value)>,
    users: Vec<(String, Vec<u8>)>,
    owner: Option<String>,
    /// The last epoch of a sequence generator's counter given.
    epochs: u64,
    /// The counters, settled: each but one of a generator's earlier epoch,
    /// which no statement draws from once the database reopens.
    counters: Vec<Settled>,
    /// The triggers, oldest first.
    triggers: Vec<TriggerDesc>,
    /// The SQL-invoked routines, oldest first.
    routines: Vec<SqlRoutineDesc>,
    /// The roles and role authorizations, in the order made.
    roles: Roles,
    /// The users allowed to create roles, by name.
    creators: Vec<String>,
    /// The SQL-server modules, oldest first.
    modules: Vec<ModuleDesc>,
    /// The structured types, by name.
    structured_types: Vec<StructuredDesc>,
    /// The user-defined casts, in the order made.
    casts: Vec<CastDesc>,
    /// The user-defined orderings, by their types' names.
    orderings: Vec<OrderingDesc>,
    /// The types' transform groups, by the types' names.
    transforms: Vec<(QualifiedName, Vec<TransformGroupDesc>)>,
    /// The property graphs, by name.
    graphs: Vec<GraphDesc>,
    /// \[EXTENSION\] The embedding models and vector pipelines.
    embeddings: Embeddings,
    /// The graph types, by name.
    graph_types: Vec<GraphTypeDesc>,
    /// The GQL graphs, by name.
    gql_graphs: Vec<GqlGraphDesc>,
    /// The spatial reference systems defined and the geometry columns
    /// registered.
    spatial: crate::catalog::SpatialCatalog,
}

impl Snapshot {
    /// The definitions of `st`, in an order that does not depend on how
    /// they are held: each kind by name, the triggers and routines by their
    /// timestamps of creation. The counters of its sequence
    /// generators settled.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a thread panicked holding the counters.
    pub(super) fn of(st: &State) -> Result<Snapshot> {
        let mut tables: Vec<TableDesc> = st.catalog.tables().into_iter().cloned().collect();
        tables.sort_by_key(|t| t.name.to_string());
        let mut views: Vec<ViewDesc> = st.catalog.views().into_iter().cloned().collect();
        views.sort_by_key(|v| v.name.to_string());
        let indexes = tables.iter().flat_map(|t| st.catalog.indexes(&t.name).into_iter().cloned()).collect();
        let named = tables.iter().map(|t| &t.name).chain(views.iter().map(|v| &v.name));
        let privileges = named.flat_map(|n| st.catalog.privileges(n).into_iter().cloned()).collect();
        let with_owner = |s: &str| (s.to_owned(), st.catalog.schema_owner(s).unwrap_or(s).to_owned());
        let mut schemas: Vec<(String, String)> = st.catalog.schemas().into_iter().map(with_owner).collect();
        schemas.sort();
        let users = st.users.entries().map(|(n, v)| (n.clone(), v.clone())).collect();
        let domains = st.catalog.domains().into_iter().cloned().collect();
        let collations = st.catalog.collations().into_iter().cloned().collect();
        let collation_versions = st.collation_versions();
        let assertions = st.catalog.assertions().into_iter().cloned().collect();
        let sequences: Vec<SequenceDesc> = st.catalog.sequences().into_iter().cloned().collect();
        let user_types = st.catalog.user_types().into_iter().cloned().collect();
        let (epochs, counters) = settled_counters(st)?;
        Ok(Snapshot {
            schemas,
            domains,
            collations,
            collation_versions,
            assertions,
            tables,
            views,
            indexes,
            privileges,
            sequences,
            user_types,
            settings: st.settings.database_values(),
            users,
            owner: st.users.owner().map(str::to_owned),
            epochs,
            counters,
            triggers: st.catalog.triggers().into_iter().cloned().collect(),
            routines: st.catalog.routines().into_iter().cloned().collect(),
            roles: st.catalog.roles().clone(),
            creators: st.users.creators().cloned().collect(),
            modules: st.catalog.modules().into_iter().cloned().collect(),
            structured_types: st.catalog.structured_types().into_iter().cloned().collect(),
            casts: st.catalog.casts().into_iter().cloned().collect(),
            orderings: st.catalog.orderings().into_iter().cloned().collect(),
            transforms: st.catalog.all_transforms().into_iter().map(|(n, g)| (n.clone(), g.to_vec())).collect(),
            graphs: st.catalog.graphs().into_iter().cloned().collect(),
            embeddings: st.catalog.embeddings().clone(),
            graph_types: st.catalog.gql().graph_types().into_iter().cloned().collect(),
            gql_graphs: st.catalog.gql().gql_graphs().into_iter().cloned().collect(),
            spatial: st.catalog.spatial().clone(),
        })
    }

    /// The database's value of the setting `name`, if it set one.
    pub(super) fn setting(&self, name: &str) -> Option<&Value> {
        self.settings.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    /// It in bytes.
    pub(super) fn put(&self, out: &mut Vec<u8>) {
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        out.put_u32(count(self.schemas.len()));
        for (schema, owner) in &self.schemas {
            out.put_str(schema);
            out.put_str(owner);
        }
        out.put_u32(count(self.domains.len()));
        self.domains.iter().for_each(|d| put_domain(out, d));
        out.put_u32(count(self.collations.len()));
        self.collations.iter().for_each(|c| put_collation(out, c));
        out.put_u32(count(self.collation_versions.len()));
        for (name, version) in &self.collation_versions {
            out.put_str(name);
            out.put_str(version);
        }
        out.put_u32(count(self.assertions.len()));
        self.assertions.iter().for_each(|a| put_constraint(out, a));
        out.put_u32(count(self.tables.len()));
        self.tables.iter().for_each(|t| put_table(out, t));
        out.put_u32(count(self.views.len()));
        self.views.iter().for_each(|v| put_view(out, v));
        out.put_u32(count(self.indexes.len()));
        self.indexes.iter().for_each(|i| put_index(out, i));
        out.put_u32(count(self.privileges.len()));
        self.privileges.iter().for_each(|p| put_privilege(out, p));
        out.put_u32(count(self.sequences.len()));
        self.sequences.iter().for_each(|s| put_sequence(out, s));
        out.put_u32(count(self.user_types.len()));
        self.user_types.iter().for_each(|t| put_user_type(out, t));
        out.put_u32(count(self.settings.len()));
        for (name, v) in &self.settings {
            out.put_str(name);
            put_value(out, v);
        }
        out.put_u32(count(self.users.len()));
        for (name, verifier) in &self.users {
            out.put_str(name);
            out.put_bytes(verifier);
        }
        self.put_rest(out);
    }

    /// Its owner, epochs, counters, triggers and routines in bytes, the last of
    /// what `put` writes.
    fn put_rest(&self, out: &mut Vec<u8>) {
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        out.put_u8(u8::from(self.owner.is_some()));
        if let Some(owner) = &self.owner {
            out.put_str(owner);
        }
        out.put_u64(self.epochs);
        out.put_u32(count(self.counters.len()));
        for (name, epoch, base) in &self.counters {
            put_name(out, name);
            out.put_u64(*epoch);
            out.put_i128(*base);
        }
        out.put_u32(count(self.triggers.len()));
        self.triggers.iter().for_each(|t| put_trigger(out, t));
        out.put_u32(count(self.routines.len()));
        self.routines.iter().for_each(|t| put_routine(out, t));
        put_roles(out, &self.roles);
        out.put_u32(count(self.creators.len()));
        self.creators.iter().for_each(|c| out.put_str(c));
        out.put_u32(count(self.modules.len()));
        self.modules.iter().for_each(|m| put_module(out, m));
        out.put_u32(count(self.structured_types.len()));
        self.structured_types.iter().for_each(|t| put_structured_type(out, t));
        out.put_u32(count(self.casts.len()));
        self.casts.iter().for_each(|c| put_cast(out, c));
        out.put_u32(count(self.orderings.len()));
        self.orderings.iter().for_each(|o| put_ordering(out, o));
        out.put_u32(count(self.transforms.len()));
        self.transforms.iter().for_each(|(n, g)| put_transforms(out, (n, g)));
        out.put_u32(count(self.graphs.len()));
        self.graphs.iter().for_each(|g| put_graph(out, g));
        put_embeddings(out, &self.embeddings);
        out.put_u32(count(self.graph_types.len()));
        self.graph_types.iter().for_each(|t| put_graph_type(out, t));
        out.put_u32(count(self.gql_graphs.len()));
        self.gql_graphs.iter().for_each(|g| put_gql_graph(out, g));
        crate::catalog::codec::put_spatial(out, &self.spatial);
    }

    /// The snapshot `put` wrote.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it is not one.
    pub(super) fn get(r: &mut Reader<'_>) -> Result<Snapshot> {
        // Each read in the order `put` wrote it, as a struct expression's
        // fields are evaluated in the order written.
        Ok(Snapshot {
            schemas: (0..r.u32()?).map(|_| Ok((r.str()?, r.str()?))).collect::<Result<_>>()?,
            domains: (0..r.u32()?).map(|_| domain(r)).collect::<Result<_>>()?,
            collations: (0..r.u32()?).map(|_| collation(r)).collect::<Result<_>>()?,
            collation_versions: (0..r.u32()?).map(|_| Ok((r.str()?, r.str()?))).collect::<Result<_>>()?,
            assertions: (0..r.u32()?).map(|_| constraint(r)).collect::<Result<_>>()?,
            tables: (0..r.u32()?).map(|_| table(r)).collect::<Result<_>>()?,
            views: (0..r.u32()?).map(|_| view(r)).collect::<Result<_>>()?,
            indexes: (0..r.u32()?).map(|_| index(r)).collect::<Result<_>>()?,
            privileges: (0..r.u32()?).map(|_| privilege(r)).collect::<Result<_>>()?,
            sequences: (0..r.u32()?).map(|_| sequence(r)).collect::<Result<_>>()?,
            user_types: (0..r.u32()?).map(|_| user_type(r)).collect::<Result<_>>()?,
            settings: (0..r.u32()?).map(|_| Ok((r.str()?, value(r)?))).collect::<Result<_>>()?,
            users: (0..r.u32()?).map(|_| Ok((r.str()?, r.bytes()?.to_vec()))).collect::<Result<_>>()?,
            owner: if r.u8()? == 0 { None } else { Some(r.str()?) },
            epochs: r.u64()?,
            counters: (0..r.u32()?).map(|_| Ok((qualified(r)?, r.u64()?, r.i128()?))).collect::<Result<_>>()?,
            triggers: (0..r.u32()?).map(|_| trigger(r)).collect::<Result<_>>()?,
            routines: (0..r.u32()?).map(|_| routine(r)).collect::<Result<_>>()?,
            roles: roles(r)?,
            creators: (0..r.u32()?).map(|_| r.str()).collect::<Result<_>>()?,
            modules: (0..r.u32()?).map(|_| module(r)).collect::<Result<_>>()?,
            structured_types: (0..r.u32()?).map(|_| structured_type(r)).collect::<Result<_>>()?,
            casts: (0..r.u32()?).map(|_| cast(r)).collect::<Result<_>>()?,
            orderings: (0..r.u32()?).map(|_| ordering(r)).collect::<Result<_>>()?,
            transforms: (0..r.u32()?).map(|_| transforms(r)).collect::<Result<_>>()?,
            graphs: (0..r.u32()?).map(|_| graph(r)).collect::<Result<_>>()?,
            embeddings: embeddings(r)?,
            graph_types: (0..r.u32()?).map(|_| graph_type(r)).collect::<Result<_>>()?,
            gql_graphs: (0..r.u32()?).map(|_| gql_graph(r)).collect::<Result<_>>()?,
            spatial: crate::catalog::codec::spatial(r)?,
        })
    }
}

/// The generators' epochs and counters of `st`, settled: a generator's
/// counter of its own epoch kept, though another of its name, of a schema
/// or a table, is of a later one (RULES.md D63).
///
/// # Errors
///
/// SQLCODE -910 if a thread panicked holding the counters.
fn settled_counters(st: &State) -> Result<(u64, Vec<Settled>)> {
    let (epochs, mut counters) = st.sequences.settle()?;
    let published = st.generators();
    let kept = |name: &QualifiedName, epoch: u64| {
        published.iter().any(|s| s.name == *name && s.epoch == epoch) || published.iter().all(|s| s.name != *name || s.epoch <= epoch)
    };
    counters.retain(|(name, epoch, _)| kept(name, *epoch));
    Ok((epochs, counters))
}

impl State {
    /// Makes the definitions of `snapshot`, into a state that has none:
    /// each table's storage and each index's structure reopened by its
    /// locator while the database is reopened, else made (`table_storage`,
    /// `index_structure`).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if an engine or index method it names is not
    /// registered; their errors, reopening or making them; a setting's,
    /// set.
    pub(super) fn install(&mut self, snapshot: &Snapshot) -> Result<()> {
        let mut catalog = self.catalog.duplicate();
        for (schema, owner) in &snapshot.schemas {
            catalog.create_schema(schema, owner, snapshot.tables.iter().filter(|t| t.name.auth == *schema).cloned().collect());
        }
        put_module_tables(&mut *catalog, snapshot);
        put_described(&mut *catalog, snapshot);
        self.install_counters(snapshot)?;
        // Before the indexes, whose keys are by them.
        self.catalog = Arc::from(catalog.duplicate());
        put_dependent(&mut *catalog, snapshot);
        self.check_collation_versions(&*catalog, &snapshot.collation_versions)?;
        for desc in &snapshot.tables {
            let engine =
                Arc::clone(self.engines.get(&desc.engine).ok_or_else(|| api(format!("[DESIGN] no storage engine {}", desc.engine)))?);
            let storage = self.table_storage(&*engine, desc)?;
            self.storage.insert(Arc::new(desc.name.clone()), storage);
            self.locks.insert(desc.name.clone(), Arc::default());
        }
        for ix in &snapshot.indexes {
            let method = Arc::clone(self.methods.get(&ix.method).ok_or_else(|| api(format!("[DESIGN] no index method {}", ix.method)))?);
            let desc = snapshot
                .tables
                .iter()
                .find(|t| t.name == ix.table)
                .ok_or_else(|| api(format!("[DESIGN] index {} has no table", ix.name)))?;
            let ix = self.with_derived(ix.clone(), desc)?;
            let types = ix.key_types(desc).ok_or_else(|| api(format!("[DESIGN] index {} is on a column its table lacks", ix.name)))?;
            let (structure, _) = self.index_structure(&*method, (&ix, desc), &types)?;
            self.indexes.insert(ix.name.clone(), structure);
            catalog.add_index(ix);
        }
        for (name, v) in &snapshot.settings {
            // One read as the database opened, given before it did, is kept
            // over the one recorded (`settings::read_as_opened`).
            let given = self.settings.key(name).and_then(|key| {
                let d = self.settings.definition(key)?;
                Some(settings::read_as_opened(d) && self.settings.effective(key, &settings::Own::default()).1 == settings::Source::Database)
            });
            if given != Some(true) {
                self.settings.set(name, Some(v))?;
            }
        }
        for (name, verifier) in &snapshot.users {
            self.users.replay_user(name, Some(verifier.clone()));
        }
        if let Some(owner) = &snapshot.owner {
            self.users.replay_owner(owner);
        }
        snapshot.creators.iter().for_each(|c| self.users.replay_creator(c, true));
        catalog.set_roles(snapshot.roles.clone());
        self.catalog = Arc::from(catalog);
        Ok(())
    }
    /// The counters of `snapshot`'s sequence generators, each set at its
    /// base, and the epochs past its last (RULES.md D62).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a thread panicked holding the counters.
    fn install_counters(&self, snapshot: &Snapshot) -> Result<()> {
        self.sequences.advance_past(snapshot.epochs)?;
        for (name, epoch, base) in &snapshot.counters {
            self.sequences.replay(name, *epoch, *base)?;
        }
        Ok(())
    }
}

// Unit tests: a snapshot reads back as written (RULES.md R-TEST-1).

/// The domains, collations, sequence generators and user-defined types of
/// `snapshot`, put in `catalog`: none of them depends on a table.
fn put_described(catalog: &mut dyn Catalog, snapshot: &Snapshot) {
    snapshot.domains.iter().cloned().for_each(|d| catalog.put_domain(d));
    snapshot.collations.iter().cloned().for_each(|c| catalog.put_collation(c));
    snapshot.sequences.iter().cloned().for_each(|s| catalog.put_sequence(s));
    snapshot.user_types.iter().cloned().for_each(|t| catalog.put_user_type(t));
    snapshot.structured_types.iter().cloned().for_each(|t| catalog.put_structured_type(t));
    snapshot.casts.iter().cloned().for_each(|c| catalog.put_cast(c));
    snapshot.orderings.iter().cloned().for_each(|o| catalog.put_ordering(o));
    snapshot.transforms.iter().for_each(|(n, g)| catalog.put_transforms(n, g.clone()));
}

/// The assertions, views, privileges, triggers, SQL-invoked routines,
/// modules and property graphs of `snapshot`, put in `catalog`: each
/// depends on a table, so they follow the tables.
fn put_dependent(catalog: &mut dyn Catalog, snapshot: &Snapshot) {
    snapshot.assertions.iter().cloned().for_each(|a| catalog.put_assertion(a));
    catalog.add_views(snapshot.views.clone());
    catalog.grant(snapshot.privileges.clone());
    snapshot.triggers.iter().cloned().for_each(|t| catalog.put_trigger(t));
    snapshot.routines.iter().cloned().for_each(|r| catalog.put_routine(r));
    snapshot.modules.iter().cloned().for_each(|m| catalog.put_module(m));
    snapshot.graphs.iter().cloned().for_each(|g| catalog.put_graph(g));
    catalog.set_embeddings(snapshot.embeddings.clone());
    snapshot.graph_types.iter().cloned().for_each(|t| catalog.gql_mut().put_graph_type(t));
    snapshot.gql_graphs.iter().cloned().for_each(|g| catalog.gql_mut().put_gql_graph(g));
    catalog.set_spatial(snapshot.spatial.clone());
}

/// [ISO/IEC 9075-4:2006 draft §12.8] Puts in `catalog` the declared tables
/// of the modules of `snapshot`, each in its module's own schema.
fn put_module_tables(catalog: &mut dyn Catalog, snapshot: &Snapshot) {
    for t in snapshot.tables.iter().filter(|t| module_of_schema(&t.name.auth).is_some()) {
        catalog.add_table(t.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::QualifiedName;
    use crate::db::Database;

    /// Verifies: [DESIGN] RULES.md D25, D43, D62, a snapshot of the
    /// definitions: of every kind of table, column, constraint, view, index,
    /// sequence generator, privilege, SQL-invoked routine, property graph,
    /// setting, user and
    /// owner there is, and of the counters, it reads back exactly as written.
    #[test]
    fn a_snapshot_reads_back_as_written() {
        let db = Database::new();
        db.schema(
            "CREATE SCHEMA AUTHORIZATION A
               CREATE TABLE P (K INTEGER NOT NULL PRIMARY KEY, N CHARACTER(128) DEFAULT USER, F REAL DEFAULT 1.5)
               CREATE TABLE C (K INTEGER REFERENCES P MATCH FULL ON DELETE CASCADE ON UPDATE SET NULL,
                               V NUMERIC(9, 2) DEFAULT NULL CHECK (V > 0),
                               UNIQUE (K, V),
                               CONSTRAINT D CHECK (K IN (SELECT K FROM P)) DEFERRABLE INITIALLY DEFERRED)
               CREATE VIEW W (K) AS SELECT K FROM C WHERE V > 1 WITH CHECK OPTION
               CREATE VIEW G (K, N) AS SELECT K, COUNT(*) FROM C GROUP BY K
               GRANT SELECT, UPDATE (V) ON C TO B WITH GRANT OPTION
               GRANT REFERENCES (K) ON P TO PUBLIC
               CREATE SEQUENCE Q AS SMALLINT START WITH 5 INCREMENT BY -2 MINVALUE -9 MAXVALUE 9 CYCLE
               GRANT USAGE ON SEQUENCE Q TO B
               CREATE FUNCTION TOP (X NUMERIC(9, 2)) RETURNS INTEGER SPECIFIC TOP_ONE
                 DETERMINISTIC READS SQL DATA RETURNS NULL ON NULL INPUT RETURN (SELECT MAX(K) FROM C WHERE V > X)
               CREATE PROCEDURE PUT (IN K INTEGER, OUT N INTEGER, INOUT M INTEGER) MODIFIES SQL DATA
                 INSERT INTO P (K) VALUES (K)
               GRANT EXECUTE ON FUNCTION TOP TO B WITH GRANT OPTION
               CREATE PROPERTY GRAPH N VERTEX TABLES (P PROPERTIES (K, N AS WHO))
                 EDGE TABLES (C KEY (K, V) UNDIRECTED SOURCE KEY (K) REFERENCES P DESTINATION P
                   LABEL PAID PROPERTIES (V * 2 AS TWICE, TOP(V) AS T)) OPTIONS (TRUSTED MODE) WITH INVOKER RIGHTS
               GRANT SELECT ON PROPERTY GRAPH N TO B WITH GRANT OPTION",
        )
        .unwrap();
        let a = db.session("A");
        let mut drawn = a.open_cursor("SELECT NEXT VALUE FOR Q FROM (VALUES (0)) AS X (Z)").unwrap();
        assert!(drawn.fetch().unwrap().is_some());
        db.create_index("C_V", "A.C", &["V", "K"], "hash").unwrap();
        db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(crate::value::Decimal::new(1 << 20, 0))).unwrap();
        db.create_user("ANN", &[0, 255, 7]).unwrap();
        db.create_user("bob", &[]).unwrap();
        db.set_owner("ANN").unwrap();
        let snapshot = Snapshot::of(&db.snapshot().unwrap()).unwrap();
        assert!(!snapshot.tables.is_empty() && snapshot.views.len() == 2 && snapshot.privileges.len() >= 2);
        assert_eq!(
            snapshot.graphs.iter().map(|g| (g.elements.len(), g.select.len(), g.horizon.routines.clone())).collect::<Vec<_>>(),
            [(2, 1, Some(vec![QualifiedName { auth: "A".into(), ident: "TOP_ONE".into() }]))]
        );
        assert_eq!(snapshot.routines.iter().map(|r| r.execute.len()).collect::<Vec<_>>(), [2, 1]);
        assert_eq!((snapshot.users.len(), snapshot.owner.as_deref()), (2, Some("ANN")));
        let q = QualifiedName { auth: "A".into(), ident: "Q".into() };
        assert_eq!((snapshot.sequences.len(), snapshot.counters.clone()), (1, vec![(q, snapshot.epochs, 3)]));
        let mut bytes = Vec::new();
        snapshot.put(&mut bytes);
        let mut r = Reader::new(&bytes, "snapshot");
        let again = Snapshot::get(&mut r).unwrap();
        r.finish().unwrap();
        assert_eq!(format!("{again:?}"), format!("{snapshot:?}"));
        assert!(Snapshot::get(&mut Reader::new(&bytes[..bytes.len() - 1], "snapshot")).is_err(), "cut short");
    }
}
