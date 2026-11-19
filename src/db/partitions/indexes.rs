//! \[EXTENSION\] The indexes of a partitioned table (RULES.md D157, D158):
//! templates, each made again as a real index on every partition that
//! holds rows, present and future, of any index method. A template of a
//! UNIQUE constraint whose key holds the partition key of every level
//! makes a unique index on each partition, which then enforces the key
//! for the whole tree; one whose key does not makes a plain index, and the
//! key is checked across the partitions (`db/integrity/keys.rs`).

use std::sync::Arc;

use super::super::alter::Made;
use super::super::{State, api};
use crate::catalog::{
    Catalog, ConstraintKind, IndexDesc, IndexTemplate, KeyExpression, PartitionKey, QualifiedName, TableDesc, derived_index_name,
    template_of,
};
use crate::error::Result;
use crate::index::Index;
use crate::txn::Txn;

impl State {
    /// The templates of the UNIQUE constraints of the partitioned table
    /// `desc`, each made on its partitions.
    pub(in crate::db) fn constraint_templates(&self, desc: &TableDesc) -> Vec<IndexTemplate> {
        let mut out = Vec::new();
        for c in &desc.constraints {
            let ConstraintKind::Unique { columns, period, .. } = &c.kind else { continue };
            let columns = desc.key_columns(columns, period.as_deref());
            out.push(IndexTemplate {
                name: format!("{}.{}.{}", desc.name.auth, desc.name.ident, c.name.ident),
                nulls: desc.nulls_not_distinct(&columns),
                columns,
                method: self.constraint_method().to_owned(),
                unique: true,
                options: Vec::new(),
                expressions: Vec::new(),
            });
        }
        out
    }

    /// The templates of the partitioned table `qn` made again of its UNIQUE
    /// constraints, as ALTER TABLE left them, and each partition's indexes
    /// of them: one of a template taken away dropped, one of a template
    /// added built of its rows (`made`'s if given), noted in `built`. What
    /// it dropped.
    ///
    /// # Errors
    ///
    /// As `give_templates`.
    pub(in crate::db) fn sync_templates(
        &mut self,
        qn: &QualifiedName,
        (built, made, tx): (&mut Made, Option<&Made>, Option<&Txn>),
    ) -> Result<Vec<Arc<dyn Index>>> {
        let (Some(desc), Some(mut d)) = (self.catalog.table(qn).cloned(), self.catalog.partitioning().get(qn).cloned()) else {
            return Ok(Vec::new());
        };
        let Some(key) = d.key.as_mut() else { return Ok(Vec::new()) };
        key.templates.retain(|t| !t.unique);
        key.templates.extend(self.constraint_templates(&desc));
        let mut catalog = self.catalog.duplicate();
        let mut dropped = Vec::new();
        // Its own keys are its partitions' (RULES.md D157).
        let own: Vec<String> = catalog.indexes(qn).into_iter().filter(|ix| ix.unique).map(|ix| ix.name.clone()).collect();
        catalog.partitioning_mut().put(d);
        for leaf in catalog.partitioning().leaves(qn) {
            let expected: Vec<String> = derived_of(&*catalog, &leaf).into_iter().map(|ix| ix.name).collect();
            let gone: Vec<String> = catalog
                .indexes(&leaf)
                .into_iter()
                .filter(|ix| template_of(&ix.name).is_some() && !expected.contains(&ix.name))
                .map(|ix| ix.name.clone())
                .collect();
            for name in &gone {
                catalog.remove_index(name);
                dropped.extend(self.indexes.remove(name));
            }
            self.give_templates(&mut *catalog, &leaf, (built, made))?;
        }
        for name in &own {
            catalog.remove_index(name);
            dropped.extend(self.indexes.remove(name));
        }
        dropped.extend(self.sync_globals(&mut *catalog, qn, (built, made, tx))?);
        self.catalog = Arc::from(catalog);
        Ok(dropped)
    }

    /// Gives the table `table` of `catalog`, if it is a partition that
    /// holds rows, an index of each template of each table it is a
    /// partition of, empty, or `made`'s structure of its name, noted in
    /// `built`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a template's method is not registered; the
    /// method's, making the structure.
    pub(in crate::db) fn derive_indexes(
        &mut self,
        catalog: &mut dyn Catalog,
        table: &QualifiedName,
        (built, made): (&mut Made, Option<&Made>),
    ) -> Result<()> {
        let kept = |name: &str| made.and_then(|m| m.indexes.iter().find(|(n, _)| n == name)).map(|(_, s)| Arc::clone(s));
        let Some(desc) = catalog.table(table).cloned() else { return Ok(()) };
        for ix in derived_of(catalog, table) {
            let ix = self.bound_derived(ix, &desc)?;
            let structure = match kept(&ix.name) {
                Some(structure) => structure,
                None => self.derived_structure(&ix, &desc)?.0,
            };
            self.indexes.insert(ix.name.clone(), Arc::clone(&structure));
            built.indexes.push((ix.name.clone(), structure));
            catalog.add_index(ix);
        }
        Ok(())
    }

    /// The structure of the derived index `ix` of `desc`, empty unless
    /// reopened, and whether it was reopened.
    pub(in crate::db) fn derived_structure(&mut self, ix: &IndexDesc, desc: &TableDesc) -> Result<(Arc<dyn Index>, bool)> {
        let method = Arc::clone(self.methods.get(&ix.method).ok_or_else(|| api(format!("[EXTENSION] no index method {}", ix.method)))?);
        let types = ix.key_types(desc).ok_or_else(|| api(format!("[DESIGN] an index of {} is on a column it lacks", desc.name)))?;
        self.index_structure(&*method, (ix, desc), &types)
    }

    /// `ix` bound over `desc`'s row, its collations its columns'.
    pub(in crate::db) fn bound_derived(&self, ix: IndexDesc, desc: &TableDesc) -> Result<IndexDesc> {
        let nulls = ix.nulls;
        let ix = self.with_derived(ix, desc)?;
        Ok(IndexDesc { nulls: nulls || ix.nulls, ..ix })
    }
}

/// The indexes, unbound, `table` has of the templates of the tables it is
/// a partition of: none if it is not one, or holds no rows.
pub(in crate::db) fn derived_of(catalog: &dyn Catalog, table: &QualifiedName) -> Vec<IndexDesc> {
    let p = catalog.partitioning();
    if p.of(table).is_none() || p.is_partitioned(table) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for ancestor in p.ancestors(table) {
        let Some(key) = p.key(&ancestor) else { continue };
        for t in &key.templates {
            let unique = t.unique && covers(catalog, table, &ancestor, &t.columns);
            out.push(derived(t, table, unique));
        }
    }
    out
}

/// The index of `table` the template `t` makes, unique if `unique`.
pub(in crate::db) fn derived(t: &IndexTemplate, table: &QualifiedName, unique: bool) -> IndexDesc {
    IndexDesc {
        name: derived_index_name(&t.name, table),
        table: table.clone(),
        columns: t.columns.clone(),
        method: t.method.clone(),
        unique,
        options: t.options.clone(),
        collations: Vec::new(),
        nulls: t.nulls,
        expressions: t.expressions.iter().map(|(part, text)| KeyExpression::unbound(*part, text.clone())).collect(),
    }
}

/// Whether `columns`, a key of `owner`'s, hold every column of the
/// partition key of each table from `owner` down to the partition
/// `table`'s parent, none of them an expression: two rows of one key are
/// then always in one partition.
pub(in crate::db) fn covers(catalog: &dyn Catalog, table: &QualifiedName, owner: &QualifiedName, columns: &[usize]) -> bool {
    let p = catalog.partitioning();
    let mut levels = p.ancestors(table);
    if let Some(at) = levels.iter().position(|a| a == owner) {
        levels.truncate(at.saturating_add(1));
    }
    levels
        .iter()
        .filter_map(|a| p.key(a))
        .all(|key: &PartitionKey| key.expressions.is_empty() && key.columns.iter().all(|c| columns.contains(c)))
}
