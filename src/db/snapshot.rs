//! \[DESIGN\] The definitions as they are (RULES.md D25): what a checkpoint's
//! image records of them, so a database is opened from them, each table
//! and index reopened by its locator, and only the definitions logged since
//! replayed; not from every definition ever made. The schemas, domains,
//! assertions, tables with their generations, views, indexes, sequence
//! generators and privileges of the catalog, the database's settings, in
//! the forms of `catalog/codec.rs`; its users and owner (`db/users.rs`);
//! and the counters of its sequence generators, settled, with the last
//! epoch given (`db/sequences/counters.rs`, RULES.md D62).
//!
//! ```text
//! snapshot: schemas (count u32, each its name and its owner, str),
//!           domains (count u32, each a domain), collations (count u32,
//!           each a collation), collation versions (count u32, each a name
//!           and version), assertions (count u32, each a constraint),
//!           tables (count u32, each a table), views (likewise), indexes
//!           (likewise), privileges (likewise), sequence generators
//!           (likewise), settings (count u32, each str and value), users
//!           (count u32, each str and verifier bytes), owner (u8 1 and
//!           str, or u8 0 for none), the last epoch given (u64), counters
//!           (count u32, each a name, its epoch u64 and its base i128)
//! ```

use std::sync::Arc;

use super::sequences::Settled;
use super::{State, api};
use crate::bytes::{Put, Reader};
use crate::catalog::codec::{
    collation, constraint, domain, index, privilege, put_collation, put_constraint, put_domain, put_index, put_name, put_privilege,
    put_sequence, put_table, put_view, qualified, sequence, table, view,
};
use crate::catalog::{CollationDesc, ConstraintDesc, DomainDesc, IndexDesc, PrivilegeDesc, SequenceDesc, TableDesc, ViewDesc};
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
    settings: Vec<(String, Value)>,
    users: Vec<(String, Vec<u8>)>,
    owner: Option<String>,
    /// The last epoch of a sequence generator's counter given.
    epochs: u64,
    /// The counters, settled: each but one of a generator's earlier epoch,
    /// which no statement draws from once the database reopens.
    counters: Vec<Settled>,
}

impl Snapshot {
    /// The definitions of `st`, in an order that does not depend on how
    /// they are held: each kind by name. The counters of its sequence
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
        let owner = st.users.owner().map(str::to_owned);
        let domains = st.catalog.domains().into_iter().cloned().collect();
        let collations = st.catalog.collations().into_iter().cloned().collect();
        let collation_versions = st.collation_versions();
        let assertions = st.catalog.assertions().into_iter().cloned().collect();
        let sequences: Vec<SequenceDesc> = st.catalog.sequences().into_iter().cloned().collect();
        let (epochs, mut counters) = st.sequences.settle()?;
        counters.retain(|(name, epoch, _)| sequences.iter().all(|s| s.name != *name || s.epoch <= *epoch));
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
            settings: st.settings.database_values(),
            users,
            owner,
            epochs,
            counters,
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
    }

    /// The snapshot `put` wrote.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it is not one.
    pub(super) fn get(r: &mut Reader<'_>) -> Result<Snapshot> {
        let schemas = (0..r.u32()?).map(|_| Ok((r.str()?, r.str()?))).collect::<Result<_>>()?;
        let domains = (0..r.u32()?).map(|_| domain(r)).collect::<Result<_>>()?;
        let collations = (0..r.u32()?).map(|_| collation(r)).collect::<Result<_>>()?;
        let collation_versions = (0..r.u32()?).map(|_| Ok((r.str()?, r.str()?))).collect::<Result<_>>()?;
        let assertions = (0..r.u32()?).map(|_| constraint(r)).collect::<Result<_>>()?;
        let tables = (0..r.u32()?).map(|_| table(r)).collect::<Result<_>>()?;
        let views = (0..r.u32()?).map(|_| view(r)).collect::<Result<_>>()?;
        let indexes = (0..r.u32()?).map(|_| index(r)).collect::<Result<_>>()?;
        let privileges = (0..r.u32()?).map(|_| privilege(r)).collect::<Result<_>>()?;
        let sequences = (0..r.u32()?).map(|_| sequence(r)).collect::<Result<_>>()?;
        let settings = (0..r.u32()?).map(|_| Ok((r.str()?, value(r)?))).collect::<Result<_>>()?;
        let users = (0..r.u32()?).map(|_| Ok((r.str()?, r.bytes()?.to_vec()))).collect::<Result<_>>()?;
        let owner = if r.u8()? == 0 { None } else { Some(r.str()?) };
        let epochs = r.u64()?;
        let counters = (0..r.u32()?).map(|_| Ok((qualified(r)?, r.u64()?, r.i128()?))).collect::<Result<_>>()?;
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
            settings,
            users,
            owner,
            epochs,
            counters,
        })
    }
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
        snapshot.domains.iter().cloned().for_each(|d| catalog.put_domain(d));
        snapshot.collations.iter().cloned().for_each(|c| catalog.put_collation(c));
        snapshot.sequences.iter().cloned().for_each(|s| catalog.put_sequence(s));
        self.install_counters(snapshot)?;
        // Before the indexes, whose keys are by them.
        self.catalog = Arc::from(catalog.duplicate());
        snapshot.assertions.iter().cloned().for_each(|a| catalog.put_assertion(a));
        catalog.add_views(snapshot.views.clone());
        catalog.grant(snapshot.privileges.clone());
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
            let ix = self.with_key_collations(ix.clone(), desc)?;
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
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::QualifiedName;
    use crate::db::Database;

    /// Verifies: [DESIGN] RULES.md D25, D43, D62, a snapshot of the
    /// definitions: of every kind of table, column, constraint, view, index,
    /// sequence generator, privilege, setting, user and owner there is, and
    /// of the counters, it reads back exactly as written.
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
               GRANT USAGE ON SEQUENCE Q TO B",
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
