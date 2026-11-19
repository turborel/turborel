//! \[EXTENSION\] CREATE TABLE ... PARTITION BY and CREATE TABLE ...
//! PARTITION OF (RULES.md D153): a partitioned table's key recorded, and
//! a partition described from its parent, its bound checked against its
//! siblings', and given an id.

use super::super::State;
use crate::ast::{BoundDatum, BoundSpec, Literal, PartitionBy, PartitionKeyPart, PartitionOfClause, TableDefinition};
use crate::binder::{datetime_literal, qualify};
use crate::catalog::{
    Catalog, EXPRESSION_PART, MAX_PARTITION_DEPTH, PartitionBound, PartitionDesc, PartitionKey, PartitionOf, QualifiedName, RangeDatum,
    TableDesc,
};
use crate::engine::rows::ROW_STORE;
use crate::error::{At, Result};
use crate::partition::refused;
use crate::txn::LockMode;
use crate::value::Value;

impl State {
    /// [EXTENSION] The descriptor of the partition `t` defines in schema
    /// `schema`, of `of`'s parent: the parent's columns, its own engine
    /// and options, a generation after `placed`'s or the last of its name.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the parent is no partitioned base table of the
    /// schema, or the two differ in being temporary.
    pub(in crate::db) fn partition_described(
        &self,
        schema: &str,
        (t, of): (&TableDefinition, &PartitionOfClause),
        placed: Option<&(String, u32)>,
    ) -> Result<TableDesc> {
        let qn = qualify(&t.name, schema);
        self.new_object(&qn, "§11.3 SR2").or_at(t.name.at)?;
        let parent = self.partition_parent(&qn, of)?;
        if parent.temporary != t.temporary {
            return refused(format!("[EXTENSION] {qn} and its parent {} are not both temporary, of one kind and ON COMMIT", parent.name))
                .or_at(t.name.at);
        }
        let engine = if t.temporary.is_some() {
            ROW_STORE.to_owned()
        } else {
            self.placement.get(&qn).map_or_else(|| self.table_engine().to_owned(), Clone::clone)
        };
        let desc = TableDesc {
            name: qn.clone(),
            columns: parent.columns.clone(),
            constraints: Vec::new(),
            engine,
            options: Vec::new(),
            generation: 0,
            temporary: t.temporary,
            typed: None,
            periods: parent.periods.clone(),
        };
        let mut desc = self.stored(&desc, &t.storage)?;
        desc.generation = placed.map_or_else(|| self.retired.get(&qn).map_or(0, |g| g.saturating_add(1)), |(_, g)| *g);
        self.check_new_objects(std::slice::from_ref(&desc))?;
        Ok(desc)
    }

    /// The parent `of` names, of the partition `qn`: a partitioned base
    /// table of its schema.
    fn partition_parent(&self, qn: &QualifiedName, of: &PartitionOfClause) -> Result<TableDesc> {
        let parent = qualify(&of.parent, &qn.auth);
        let desc = self.catalog.table(&parent).cloned();
        let Some(desc) = desc.filter(|_| self.catalog.partitioning().is_partitioned(&parent)) else {
            return refused(format!("[EXTENSION] {parent} is no partitioned table, so {qn} is no partition of it")).or_at(of.parent.at);
        };
        if parent.auth != qn.auth {
            return refused(format!("[EXTENSION] {qn} is not in the schema of its parent {parent}")).or_at(of.parent.at);
        }
        Ok(desc)
    }

    /// Records in `catalog` how the table `desc`, defined by `t` in schema
    /// `schema`, takes part in partitioning, its datetimes of displacement
    /// `zone`: its key, and of a partition its place. The locks it needs
    /// beside its own.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a rule its key or bound breaks.
    pub(in crate::db) fn define_partitioning(
        &self,
        catalog: &mut dyn Catalog,
        (schema, zone): (&str, i16),
        t: &TableDefinition,
        desc: &TableDesc,
    ) -> Result<Vec<(QualifiedName, LockMode)>> {
        let Some(p) = t.partitioning.as_deref() else { return Ok(Vec::new()) };
        if t.typed.is_some() || t.graph.is_some() {
            return refused(format!("[EXTENSION] {} is a typed table or a node or edge table, whose rows have identities of their own, so it is neither partitioned nor a partition (RULES.md D153)", desc.name)).or_at(t.name.at);
        }
        let key = p.by.as_ref().map(|by| self.key_of(desc, by)).transpose()?;
        let (of, locks) = match &p.of {
            Some(of) => {
                let parent = qualify(&of.parent, schema);
                let placed = self.place_partition(catalog, (&desc.name, &parent), (&of.bound, zone)).or_at(of.at)?;
                (Some(placed), vec![(parent, LockMode::Exclusive)])
            }
            None => (None, Vec::new()),
        };
        catalog.partitioning_mut().put(PartitionDesc { table: desc.name.clone(), key, of });
        Ok(locks)
    }

    /// The key `by` gives the table `desc`, checked: each part a column of
    /// it or an expression an index's key could be, of types its strategy
    /// divides by, none of a collation not the default.
    fn key_of(&self, desc: &TableDesc, by: &PartitionBy) -> Result<PartitionKey> {
        let (mut columns, mut expressions) = (Vec::new(), Vec::new());
        for (place, part) in by.parts.iter().enumerate() {
            match part {
                PartitionKeyPart::Column(name, at) => {
                    let Some((c, column)) = desc.column_desc(name) else {
                        return refused(format!("[EXTENSION] {} has no column {name} to partition by", desc.name)).or_at(*at);
                    };
                    if column.collation.is_some() {
                        return refused(format!("[EXTENSION] column {name} is of a collation, so {} is not partitioned by it", desc.name))
                            .or_at(*at);
                    }
                    columns.push(c);
                }
                PartitionKeyPart::Expression(text, _) => {
                    columns.push(EXPRESSION_PART);
                    expressions.push((place, text.clone()));
                }
            }
        }
        let templates = self.constraint_templates(desc);
        let key = PartitionKey { strategy: by.strategy.clone(), columns, expressions, templates };
        let bound = self.bind_partition_key(desc, &key).or_at(by.at)?;
        bound.strategy.check_key(&bound.types).or_at(by.at)?;
        Ok(key)
    }

    /// The place in `catalog` of the partition `table` of `parent`, of the
    /// bound `spec`, its literals' datetimes of displacement `zone`: its
    /// bound checked against its siblings', given an id.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the parent is too deep, the bound is not its
    /// strategy's or overlaps a sibling's, or a second DEFAULT.
    pub(in crate::db) fn place_partition(
        &self,
        catalog: &mut dyn Catalog,
        (table, parent): (&QualifiedName, &QualifiedName),
        (spec, zone): (&BoundSpec, i16),
    ) -> Result<PartitionOf> {
        if catalog.partitioning().ancestors(parent).len().saturating_add(1) >= MAX_PARTITION_DEPTH {
            return refused(format!("[EXTENSION] {parent} is {MAX_PARTITION_DEPTH} levels deep, so it takes no partition"));
        }
        let Some(desc) = catalog.table(parent).cloned() else { return refused(format!("[EXTENSION] there is no table {parent}")) };
        let Some(key) = catalog.partitioning().key(parent).cloned() else {
            return refused(format!("[EXTENSION] {parent} is no partitioned table, so {table} is no partition of it"));
        };
        let bound = self.checked_bound(catalog, (&desc, &key), (spec, zone))?;
        Ok(PartitionOf { parent: parent.clone(), bound, id: catalog.partitioning_mut().take_id() })
    }

    /// The bound `spec` of a new partition of the table `desc` of key
    /// `key`, in its strategy's form, overlapping no sibling's.
    fn checked_bound(
        &self,
        catalog: &dyn Catalog,
        (desc, key): (&TableDesc, &PartitionKey),
        (spec, zone): (&BoundSpec, i16),
    ) -> Result<PartitionBound> {
        let bound_key = self.bind_partition_key(desc, key)?;
        let siblings: Vec<PartitionBound> =
            catalog.partitioning().children(&desc.name).iter().filter_map(|d| d.of.as_ref().map(|o| o.bound.clone())).collect();
        let bound = match bound_value(spec, zone) {
            PartitionBound::Default => {
                if !bound_key.strategy.takes_default() {
                    return refused(format!("[EXTENSION] PARTITION BY {} takes no DEFAULT partition", key.strategy));
                }
                if siblings.iter().any(|b| matches!(b, PartitionBound::Default)) {
                    return refused(format!("[EXTENSION] {} has a DEFAULT partition already", desc.name));
                }
                return Ok(PartitionBound::Default);
            }
            given @ (PartitionBound::Range { .. } | PartitionBound::List(_) | PartitionBound::With(_)) => {
                bound_key.strategy.bound(&bound_key.types, given)?
            }
        };
        for sibling in siblings.iter().filter(|b| !matches!(b, PartitionBound::Default)) {
            if bound_key.strategy.overlaps(&bound, sibling)? {
                return refused(format!("[EXTENSION] the bound overlaps that of another partition of {}", desc.name));
            }
        }
        Ok(bound)
    }
}

/// The bound `spec` writes, its literals made values, of displacement
/// `zone` where a datetime writes none; not yet of the key's types.
pub(in crate::db) fn bound_value(spec: &BoundSpec, zone: i16) -> PartitionBound {
    let value = |d: &BoundDatum| match d {
        BoundDatum::Literal(l) => literal(l, zone),
        BoundDatum::Null | BoundDatum::MinValue | BoundDatum::MaxValue => Value::Null,
    };
    let datum = |d: &BoundDatum| match d {
        BoundDatum::MinValue => RangeDatum::MinValue,
        BoundDatum::MaxValue => RangeDatum::MaxValue,
        BoundDatum::Literal(_) | BoundDatum::Null => RangeDatum::Value(value(d)),
    };
    match spec {
        BoundSpec::Default => PartitionBound::Default,
        BoundSpec::Range { from, to } => {
            PartitionBound::Range { from: from.iter().map(datum).collect(), to: to.iter().map(datum).collect() }
        }
        BoundSpec::List(keys) => PartitionBound::List(keys.iter().map(|k| k.iter().map(value).collect()).collect()),
        BoundSpec::With(options) => PartitionBound::With(options.iter().map(|(n, d)| (n.clone(), value(d))).collect()),
    }
}

/// The value of `l`, a datetime of displacement `zone` if it writes none.
fn literal(l: &Literal, zone: i16) -> Value {
    match l {
        Literal::Character(s) => Value::Char(s.clone()),
        Literal::Exact(d) => Value::Exact(*d),
        Literal::Approximate(f) => Value::Approx(*f),
        Literal::Binary(b) => Value::Binary(b.clone()),
        Literal::Datetime(w) => datetime_literal(w, zone),
        Literal::Interval(i) => Value::Interval(*i),
        Literal::Boolean(b) => b.map_or(Value::Null, Value::Bool),
    }
}
