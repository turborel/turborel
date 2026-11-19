//! \[EXTENSION\] Partitioning's descriptors (RULES.md D153): of each table
//! that is partitioned, how its rows are divided; of each partition, its
//! parent and the rows it holds. A partitioned table and its partitions
//! are base tables of the catalog; these descriptors are kept beside them,
//! so a `TableDesc` says nothing of partitioning.

use std::collections::BTreeMap;

use super::QualifiedName;
use crate::value::Value;

/// \[DESIGN\] The most levels a partitioning tree has below its root: what
/// bounds every walk up or down it (RULES.md R-CF-6).
pub const MAX_PARTITION_DEPTH: usize = 16;

/// \[DESIGN\] What separates a template's name from a partition's id in the
/// name of the partition's index made of it: a NUL, which no `<identifier>`
/// holds (CREATE INDEX refuses one), so the name meets no name a user
/// writes (RULES.md D158).
pub const DERIVED_SEPARATOR: char = '\u{0}';

/// A value of a range bound: a value of the key part's type, or below or
/// above every value.
#[derive(Debug, Clone)]
pub enum RangeDatum {
    /// A value.
    Value(Value),
    /// MINVALUE: below every value.
    MinValue,
    /// MAXVALUE: above every value.
    MaxValue,
}

/// The rows a partition holds, as its FOR VALUES clause gives them.
#[derive(Debug, Clone)]
pub enum PartitionBound {
    /// DEFAULT: the rows no sibling holds.
    Default,
    /// FROM (...) TO (...): the keys from the first, included, to the
    /// second, excluded, compared part by part.
    Range {
        /// The lower bound, one datum per key part.
        from: Vec<RangeDatum>,
        /// The upper bound, one datum per key part.
        to: Vec<RangeDatum>,
    },
    /// IN (...): the keys listed, each one value per key part.
    List(Vec<Vec<Value>>),
    /// WITH (...): options a strategy reads, as HASH's MODULUS and
    /// REMAINDER, each a name and a value, in the order written.
    With(Vec<(String, Value)>),
}

impl PartitionBound {
    /// The bound as its FOR VALUES clause writes it, or DEFAULT.
    pub fn text(&self) -> String {
        let values = |vs: &[Value]| vs.iter().map(Value::to_literal).collect::<Vec<_>>().join(", ");
        let datum = |d: &RangeDatum| match d {
            RangeDatum::Value(v) => v.to_literal(),
            RangeDatum::MinValue => "MINVALUE".to_owned(),
            RangeDatum::MaxValue => "MAXVALUE".to_owned(),
        };
        let datums = |ds: &[RangeDatum]| ds.iter().map(datum).collect::<Vec<_>>().join(", ");
        match self {
            PartitionBound::Default => "DEFAULT".to_owned(),
            PartitionBound::Range { from, to } => format!("FOR VALUES FROM ({}) TO ({})", datums(from), datums(to)),
            PartitionBound::List(keys) => {
                let listed: Vec<String> = keys.iter().map(|k| if k.len() == 1 { values(k) } else { format!("({})", values(k)) }).collect();
                format!("FOR VALUES IN ({})", listed.join(", "))
            }
            PartitionBound::With(options) => {
                let given: Vec<String> = options.iter().map(|(n, v)| format!("{n} {}", v.to_literal())).collect();
                format!("FOR VALUES WITH ({})", given.join(", "))
            }
        }
    }
}

/// An index of a partitioned table made again on each of its partitions,
/// present and future (RULES.md D158).
#[derive(Debug, Clone)]
pub struct IndexTemplate {
    /// Its name: the name each partition's index is derived from.
    pub name: String,
    /// The positions of its key columns, `EXPRESSION_PART` of an
    /// expression.
    pub columns: Vec<usize>,
    /// Its index method.
    pub method: String,
    /// Whether it enforces a UNIQUE constraint of the partitioned table.
    pub unique: bool,
    /// The method's options it sets.
    pub options: Vec<(String, Value)>,
    /// Its key parts that are expressions: each its place and its text.
    pub expressions: Vec<(usize, String)>,
    /// Whether it holds keys with a null (`IndexDesc::nulls`): of a
    /// UNIQUE NULLS NOT DISTINCT constraint.
    pub nulls: bool,
}

/// How a partitioned table's rows are divided.
#[derive(Debug, Clone)]
pub struct PartitionKey {
    /// The name of its strategy, as registered (`PartitionStrategy::name`).
    pub strategy: String,
    /// The positions of its key's columns, `EXPRESSION_PART` of an
    /// expression.
    pub columns: Vec<usize>,
    /// Its key parts that are expressions: each its place and its text.
    pub expressions: Vec<(usize, String)>,
    /// Its local indexes, made on each partition.
    pub templates: Vec<IndexTemplate>,
}

/// A partition's place in its tree.
#[derive(Debug, Clone)]
pub struct PartitionOf {
    /// Its parent, the partitioned table it is a partition of.
    pub parent: QualifiedName,
    /// The rows it holds.
    pub bound: PartitionBound,
    /// Its id, given as it was attached and never given another table of
    /// the database, which orders it among its siblings.
    pub id: u64,
}

/// How one table takes part in partitioning.
#[derive(Debug, Clone)]
pub struct PartitionDesc {
    /// The table.
    pub table: QualifiedName,
    /// Of a partitioned table, how its rows are divided.
    pub key: Option<PartitionKey>,
    /// Of a partition, its parent and bound.
    pub of: Option<PartitionOf>,
}

/// Every table's partitioning descriptor, by the table's name.
#[derive(Debug, Clone, Default)]
pub struct Partitioning {
    descs: BTreeMap<QualifiedName, PartitionDesc>,
    /// The id the next partition is given.
    next_id: u64,
}

impl Partitioning {
    /// The descriptor of `table`, if it is partitioned or a partition.
    pub fn get(&self, table: &QualifiedName) -> Option<&PartitionDesc> {
        self.descs.get(table)
    }

    /// Records `desc`, in place of any of its table; one with neither a
    /// key nor a parent is forgotten.
    pub fn put(&mut self, desc: PartitionDesc) {
        if desc.key.is_none() && desc.of.is_none() {
            self.descs.remove(&desc.table);
        } else {
            self.descs.insert(desc.table.clone(), desc);
        }
    }

    /// A partition's id, never given before.
    pub fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = id.saturating_add(1);
        id
    }

    /// The id the next partition is given, as an image keeps it.
    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    /// Sets the id the next partition is given, as an image kept it.
    pub fn set_next_id(&mut self, id: u64) {
        self.next_id = id;
    }

    /// Forgets the descriptor of `table`.
    pub fn remove(&mut self, table: &QualifiedName) {
        self.descs.remove(table);
    }

    /// Every descriptor, in the order of the tables' names.
    pub fn all(&self) -> impl Iterator<Item = &PartitionDesc> {
        self.descs.values()
    }

    /// Whether there is none, so no table is partitioned.
    pub fn is_empty(&self) -> bool {
        self.descs.is_empty()
    }

    /// The key of `table`, if it is partitioned.
    pub fn key(&self, table: &QualifiedName) -> Option<&PartitionKey> {
        self.descs.get(table).and_then(|d| d.key.as_ref())
    }

    /// Whether `table` is partitioned.
    pub fn is_partitioned(&self, table: &QualifiedName) -> bool {
        self.key(table).is_some()
    }

    /// Where `table` is a partition, if it is one.
    pub fn of(&self, table: &QualifiedName) -> Option<&PartitionOf> {
        self.descs.get(table).and_then(|d| d.of.as_ref())
    }

    /// The partitions of `table`, in the order of their ids.
    pub fn children(&self, table: &QualifiedName) -> Vec<&PartitionDesc> {
        let mut children: Vec<&PartitionDesc> = self.descs.values().filter(|d| d.of.as_ref().is_some_and(|o| o.parent == *table)).collect();
        children.sort_by_key(|d| d.of.as_ref().map_or(0, |o| o.id));
        children
    }

    /// The tables `table` is a partition of, its parent first, at most
    /// `MAX_PARTITION_DEPTH`.
    pub fn ancestors(&self, table: &QualifiedName) -> Vec<QualifiedName> {
        let mut out = Vec::new();
        let mut at = self.of(table);
        while let Some(of) = at {
            if out.len() >= MAX_PARTITION_DEPTH {
                break;
            }
            out.push(of.parent.clone());
            at = self.of(&of.parent);
        }
        out
    }

    /// The root of the tree `table` is in: the last of its ancestors, or
    /// itself.
    pub fn root(&self, table: &QualifiedName) -> QualifiedName {
        self.ancestors(table).pop().unwrap_or_else(|| table.clone())
    }

    /// The partitions below `table`, at every level, each before its own,
    /// at most `MAX_PARTITION_DEPTH` levels down.
    pub fn descendants(&self, table: &QualifiedName) -> Vec<QualifiedName> {
        let mut out = Vec::new();
        let mut stack: Vec<(QualifiedName, usize)> = self.children(table).into_iter().rev().map(|d| (d.table.clone(), 1)).collect();
        while let Some((t, depth)) = stack.pop() {
            if depth < MAX_PARTITION_DEPTH {
                stack.extend(self.children(&t).into_iter().rev().map(|d| (d.table.clone(), depth.saturating_add(1))));
            }
            out.push(t);
        }
        out
    }

    /// The tables that hold `table`'s rows: its partitions that are not
    /// partitioned, at every level; of a table not partitioned, itself.
    pub fn leaves(&self, table: &QualifiedName) -> Vec<QualifiedName> {
        if !self.is_partitioned(table) {
            return vec![table.clone()];
        }
        self.descendants(table).into_iter().filter(|t| !self.is_partitioned(t)).collect()
    }

    /// The partition whose id is `id`, if one has it.
    pub fn by_id(&self, id: u64) -> Option<&PartitionDesc> {
        self.descs.values().find(|d| d.of.as_ref().is_some_and(|o| o.id == id))
    }
}

/// The tables whose rows `table`'s rows are too: its proper supertables,
/// the nearest first, then the tables it is a partition of, its parent
/// first (RULES.md D81, D153).
pub fn tables_above(catalog: &dyn super::Catalog, table: &QualifiedName) -> Vec<QualifiedName> {
    let mut all = super::proper_supertables(catalog, table);
    all.extend(catalog.partitioning().ancestors(table));
    all
}

/// The name of the index of the partition `table` made of the template
/// `template` (RULES.md D158): unique, as the template's name and the
/// table's are.
pub fn derived_index_name(template: &str, table: &QualifiedName) -> String {
    format!("{template}{DERIVED_SEPARATOR}{}{DERIVED_SEPARATOR}{}", table.auth, table.ident)
}

/// \[EXTENSION\] The name of the global index of the template `template`,
/// over every partition of its table (RULES.md D157): the template's,
/// then two NULs and GLOBAL, so no derived index's, whose second part is
/// a schema's name and never empty.
pub fn global_index_name(template: &str) -> String {
    format!("{template}{DERIVED_SEPARATOR}{DERIVED_SEPARATOR}GLOBAL")
}

/// Whether `index` names a global index (`global_index_name`).
pub fn is_global_index(index: &str) -> bool {
    index.split(DERIVED_SEPARATOR).nth(1).is_some_and(str::is_empty)
}

/// Of the name of an index made of a template, the template's name; none
/// of another index's.
pub fn template_of(index: &str) -> Option<&str> {
    index.split_once(DERIVED_SEPARATOR).map(|(template, _)| template)
}

#[cfg(test)]
mod tests;
