//! [ISO/IEC 9075-2:2023 §9.25] The execution of an invocation of a
//! polymorphic table function: its table arguments' rows partitioned, a
//! virtual processor for each partition run in turn on a copy of the PTF
//! data area compilation left, its cursors' rows those of its partition,
//! and each row its component procedures pipe a row of the result
//! [§20.28].
//!
//! The partitions are as GR13 to GR16 find them: of each co-partitioned
//! set of table arguments, the distinct values of their partitioning
//! columns, those of every table as one, a pruned table's absent dropped
//! [GR13b vii]; of each other partitioned table, its distinct values; the
//! product of them all. Values are alike as IS NOT DISTINCT FROM finds
//! them, by their collations' sort keys. A table with row semantics is
//! read whole on each, one virtual processor for each partition (UA056).

use std::collections::HashMap;

use super::super::dynamic::ptf::of_row;
use super::area::{Component, DataArea, cursor, cursor_row};
use super::compile::{Compiled, Procedure, TableInput, invoke, raised, surrogate_type};
use crate::catalog::TableSemantics;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::exec::{Subqueries, compare_rows};
use crate::storage::Row;
use crate::value::{Decimal, NormKey, Value};

/// What an execution runs: the invocation, compiled, and its table
/// arguments' bindings; its start, fulfill and finish component
/// procedures; and its arguments' values, of each parameter.
pub(crate) struct Execution<'a> {
    /// The invocation, compiled.
    pub(crate) compiled: &'a Compiled,
    /// Its table arguments, bound, of its generic table parameters in
    /// order.
    pub(crate) tables: &'a [TableInput],
    /// Its PTF start component procedure, if it has one.
    pub(crate) start: Option<&'a Procedure>,
    /// Its PTF fulfill component procedure.
    pub(crate) fulfill: &'a Procedure,
    /// Its PTF finish component procedure, if it has one.
    pub(crate) finish: Option<&'a Procedure>,
    /// The values of its pre-compilation argument list, of each parameter;
    /// null of a generic table parameter, and of a descriptor parameter
    /// whose argument is not null, some value not null.
    pub(crate) arguments: Vec<Value>,
}

/// One set of partitioned tables' partitions: each the values of their
/// partitioning columns its rows have, and of each table whether it has
/// rows of them.
struct Partitions {
    /// The tables, by their places among the table arguments.
    tables: Vec<usize>,
    /// Each partition's common values, as compared and as given, and of
    /// each table whether it is present.
    keys: Vec<(Vec<NormKey>, Vec<Value>, Vec<bool>)>,
}

/// The key the partitioning of `input` compares `row` by.
fn key_of(input: &TableInput, row: &Row) -> Vec<NormKey> {
    input.keys.iter().map(|&k| row.get(k).map_or(NormKey::Null, Value::norm)).collect()
}

/// The values of the partitioning columns of `input` in `row`.
fn values_of(input: &TableInput, row: &Row) -> Vec<Value> {
    input.partition.iter().map(|&c| row.get(c).cloned().unwrap_or(Value::Null)).collect()
}

/// [§9.25 GR13b vi, GR14c] The partitions of the tables `members` of
/// `inputs`, of rows `rows`: the distinct keys of all, a pruned table's
/// absent dropped; of all empty, one of nulls, every table present.
fn partitions(members: Vec<usize>, inputs: &[TableInput], rows: &[Vec<Row>]) -> Partitions {
    let width = members.first().and_then(|&t| inputs.get(t)).map_or(0, |i| i.partition.len());
    let mut keys: Vec<(Vec<NormKey>, Vec<Value>, Vec<bool>)> = Vec::new();
    let mut seen: HashMap<Vec<NormKey>, usize> = HashMap::new();
    for (m, &t) in members.iter().enumerate() {
        let (Some(input), Some(found)) = (inputs.get(t), rows.get(t)) else { continue };
        for row in found {
            let key = key_of(input, row);
            let at = *seen.entry(key.clone()).or_insert_with(|| {
                keys.push((key, values_of(input, row), vec![false; members.len()]));
                keys.len().saturating_sub(1)
            });
            if let Some(present) = keys.get_mut(at).and_then(|(_, _, p)| p.get_mut(m)) {
                *present = true;
            }
        }
    }
    if keys.is_empty() {
        keys.push((vec![NormKey::Null; width], vec![Value::Null; width], vec![true; members.len()]));
    }
    keys.retain(|(_, _, present)| members.iter().zip(present).all(|(&t, &p)| p || !inputs.get(t).is_some_and(|i| i.pruned)));
    Partitions { tables: members, keys }
}

/// [§9.25 GR6, GR7, GR13, GR14] The sets of partitioned tables: each
/// `<co-partition specification>`'s, then each other partitioned table
/// alone.
fn partitioned_sets(inputs: &[TableInput], rows: &[Vec<Row>]) -> Vec<Partitions> {
    let mut sets: Vec<(Option<usize>, Vec<usize>)> = Vec::new();
    for (t, input) in inputs.iter().enumerate().filter(|(_, i)| !i.partition.is_empty()) {
        match input.copartitioned {
            Some(s) => match sets.iter_mut().find(|(g, _)| *g == Some(s)) {
                Some((_, members)) => members.push(t),
                None => sets.push((Some(s), vec![t])),
            },
            None => sets.push((None, vec![t])),
        }
    }
    sets.into_iter().map(|(_, members)| partitions(members, inputs, rows)).collect()
}

/// [§9.25 GR16] The product of `sets`' partitions: each a choice of one
/// partition of each set, by its place among the set's.
fn product(sets: &[Partitions]) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = vec![Vec::new()];
    for set in sets {
        out = out.into_iter().flat_map(|chosen| (0..set.keys.len()).map(move |k| [chosen.clone(), vec![k]].concat())).collect();
    }
    out
}

/// One virtual processor's partition: of each table, its common values
/// if it is partitioned, and whether it is present.
struct Partition {
    /// Of each table, its common values and whether it is present, if it is
    /// partitioned.
    of: Vec<Option<Common>>,
}

/// A partitioned table's common values in a partition, as compared and as
/// given, and whether it is present.
type Common = (Vec<NormKey>, Vec<Value>, bool);

impl Partition {
    /// The partition `chosen` picks of `sets`, of `n` tables.
    fn of(sets: &[Partitions], chosen: &[usize], n: usize) -> Partition {
        let mut of = vec![None; n];
        for (set, &k) in sets.iter().zip(chosen) {
            let Some((key, values, present)) = set.keys.get(k) else { continue };
            for (m, &t) in set.tables.iter().enumerate() {
                if let Some(slot) = of.get_mut(t) {
                    *slot = Some((key.clone(), values.clone(), present.get(m).copied().unwrap_or(false)));
                }
            }
        }
        Partition { of }
    }

    /// [§9.25 GR16b iii, GR17, GR18] The rows of table `t`, `input`, of
    /// `rows`, its cursor reads: of a partitioned table, those of the
    /// partition's common values, none if it is absent; of another, all;
    /// in the order its ordering gives, each with its place among `rows`.
    fn rows<'r>(&self, t: usize, input: &TableInput, rows: &'r [Row]) -> Vec<(usize, &'r Row)> {
        let mut out: Vec<(usize, &Row)> = match self.of.get(t).and_then(Option::as_ref) {
            Some((_, _, false)) => Vec::new(),
            Some((key, _, true)) => rows.iter().enumerate().filter(|(_, r)| key_of(input, r) == *key).collect(),
            None => rows.iter().enumerate().collect(),
        };
        if !input.sort.is_empty() {
            out.sort_by(|(_, a), (_, b)| compare_rows(&input.sort, a, b));
        }
        out
    }
}

/// [ISO/IEC 9075-2:2023 §9.25] The result of the invocation `x`, of its
/// table arguments' rows `rows`, each with its extra key columns, where
/// `subqueries` runs a query's subqueries: the rows its component
/// procedures pipe, each a row of the `<PTF derived table>`: its proper
/// result columns, then of each table argument its columns of PASS
/// THROUGH, else its partitioning columns.
///
/// # Errors
///
/// An exception of a component procedure, or the status variable's
/// [GR20d iii, GR20i, GR20j iii]; SQLSTATE 0700G if a pass-through
/// surrogate value represents no row [§20.28 GR11b ii].
pub(crate) fn execute(x: &Execution<'_>, rows: &[Vec<Row>], subqueries: &dyn Subqueries) -> Result<Vec<Row>> {
    // [GR4] an empty table of row semantics, or a pruned one, gives none.
    for (input, found) in x.tables.iter().zip(rows) {
        let pruned = matches!(input.semantics, TableSemantics::Row) || input.pruned;
        if pruned && found.is_empty() {
            return Ok(Vec::new());
        }
    }
    let sets = partitioned_sets(x.tables, rows);
    let mut result = Vec::new();
    for chosen in product(&sets) {
        let partition = Partition::of(&sets, &chosen, x.tables.len());
        let piped = processor(x, &partition, rows, subqueries)?;
        for values in piped {
            result.push(result_row(x, &partition, rows, values)?);
        }
    }
    Ok(result)
}

/// [§9.25 GR20] Virtual processor of `partition`: its PTF data area a copy
/// of compilation's, its cursors' rows its partition's, its start, fulfill
/// and finish component procedures run; the rows they piped.
fn processor(x: &Execution<'_>, partition: &Partition, rows: &[Vec<Row>], subqueries: &dyn Subqueries) -> Result<Vec<Vec<Value>>> {
    let mut area: DataArea = x.compiled.area.clone();
    let layout = &x.compiled.layout;
    for (t, input) in x.tables.iter().enumerate() {
        let found = rows.get(t).map_or(&[][..], Vec::as_slice);
        let requested = x.compiled.requested.get(t).map_or(&[][..], Vec::as_slice);
        let mut columns: Vec<(String, crate::value::DataType)> = requested.iter().filter_map(|&c| input.columns.get(c).cloned()).collect();
        let surrogate = x.compiled.surrogates.get(t).and_then(Option::as_ref);
        if let Some(name) = surrogate {
            columns.push((name.clone(), surrogate_type()));
        }
        let read: Vec<Row> = partition
            .rows(t, input, found)
            .into_iter()
            .map(|(n, row)| {
                let mut r: Row = requested.iter().map(|&c| row.get(c).cloned().unwrap_or(Value::Null)).collect();
                if surrogate.is_some() {
                    r.push(Value::Exact(Decimal::new(i128::try_from(n).unwrap_or(i128::MAX), 0)));
                }
                r
            })
            .collect();
        let ordinal = t.saturating_add(1);
        area.descriptors.insert(cursor_row(ordinal), of_row(&columns, 0, &area.catalog));
        let types = columns.into_iter().map(|(_, ty)| ty).collect();
        area.cursors.insert(cursor(ordinal), super::area::Cursor { rows: read, types, next: None });
    }
    if let Some(start) = x.start {
        invoke((start, Component::Start), layout, &x.arguments, (&mut area, subqueries))?;
        if let Some(e) = raised(&area) {
            return Err(e);
        }
    }
    // [GR20e] each cursor opened.
    area.cursors.values_mut().for_each(|c| c.next = Some(0));
    let fulfilled = invoke((x.fulfill, Component::Fulfill), layout, &x.arguments, (&mut area, subqueries));
    // [GR20h] each closed, and destroyed.
    area.cursors.clear();
    fulfilled?;
    if let Some(e) = raised(&area) {
        return Err(e);
    }
    if let Some(finish) = x.finish {
        invoke((finish, Component::Finish), layout, &x.arguments, (&mut area, subqueries))?;
        if let Some(e) = raised(&area) {
            return Err(e);
        }
    }
    Ok(area.piped)
}

/// [ISO/IEC 9075-2:2023 §20.28 GR7 to GR11] The row of the `<PTF derived
/// table>` the piped `values` give, on the virtual processor of
/// `partition`: its proper result columns theirs; of each table argument
/// of PASS THROUGH, its columns those of the row its pass-through output
/// surrogate value represents, or null of a null one, its partitioning
/// columns the partition's common values; of another, its partitioning
/// columns those.
///
/// # Errors
///
/// SQLSTATE 0700G if a surrogate value represents no row of its table.
fn result_row(x: &Execution<'_>, partition: &Partition, rows: &[Vec<Row>], values: Vec<Value>) -> Result<Row> {
    let proper = x.compiled.proper.len();
    let mut piped = values.into_iter();
    let mut row: Row = piped.by_ref().take(proper).collect();
    row.resize(proper, Value::Null);
    for (t, input) in x.tables.iter().enumerate() {
        let common = partition.of.get(t).and_then(Option::as_ref).map(|(_, v, _)| v.as_slice());
        let partitioning = |c: usize| input.partition.iter().position(|&p| p == c);
        if input.pass_through {
            let surrogate = piped.next().unwrap_or(Value::Null);
            let source = if surrogate.is_null() { None } else { Some(represented(&surrogate, rows.get(t))?) };
            for c in 0..input.columns.len() {
                let value = match (partitioning(c), common) {
                    (Some(k), Some(common)) => common.get(k).cloned().unwrap_or(Value::Null),
                    (Some(_) | None, _) => source.and_then(|r| r.get(c)).cloned().unwrap_or(Value::Null),
                };
                row.push(value);
            }
        } else if let Some(common) = common {
            let mut cols: Vec<usize> = input.partition.clone();
            cols.sort_unstable();
            for c in cols {
                row.push(partitioning(c).and_then(|k| common.get(k)).cloned().unwrap_or(Value::Null));
            }
        }
    }
    Ok(row)
}

/// [§20.28 GR11b ii, iii] The row of `rows` the pass-through surrogate
/// value `v` represents.
///
/// # Errors
///
/// SQLSTATE 0700G if it represents none.
fn represented<'r>(v: &Value, rows: Option<&'r Vec<Row>>) -> Result<&'r Row> {
    let n = if let Value::Exact(d) = v
        && d.scale == 0
    {
        usize::try_from(d.units).ok()
    } else {
        None
    };
    n.and_then(|n| rows.and_then(|r| r.get(n))).ok_or_else(|| {
        let msg =
            format!("[ISO/IEC 9075-2:2023 §20.28 GR11b ii] pass-through surrogate value {} represents no row of its table", v.to_literal());
        SqlError::new(Code::Dynamic, msg).with_state(SqlState::INVALID_SURROGATE)
    })
}
