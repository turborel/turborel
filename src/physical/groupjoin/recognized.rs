//! \[DESIGN\] (RULES.md D490) A grouping of a join recognized as a
//! groupjoin, as Moerkotte and Neumann, "Accelerating Queries with
//! Group-By and Join by Groupjoin" (PVLDB 2011), find one: the join an
//! inner hash join or a LEFT outer join of no condition but its keys,
//! under projections of columns alone; one operand, the group operand G,
//! keyed uniquely by its key columns, which the grouping columns hold;
//! every other grouping column of G, or a key column of the other
//! operand, P, equal to its G column in value and in type; every set
//! function of P's columns alone, none DISTINCT, filtered or an inverse
//! distribution function. So each group is one G row and its join's
//! rows, and each P row joins one group at most, and a group's set
//! functions take exactly the rows of P the join gave it, in the order
//! it gave them (their Eq. 11 and 12). Of a LEFT join G is its left
//! operand, every row of which is a group. The keys' columns are of types
//! whose values the executor's table finds equal exactly as the join
//! does: exact numbers, dates or truth values.
//!
//! A set function may be filtered by a marker alone, a column of the
//! probe never null but of the row of nulls a LEFT join gives a group no
//! row joined, as unnesting a subquery filters one (`optimizer/unnesting`):
//! of every row of the probe it is true, so the filter is dropped and that
//! function is fed no row of nulls.

use super::columns::{Unique, column_type, found_once, unique, width};
use crate::db::State;
use crate::logical::{AggCall, Cond, Expr, OuterJoin, expr_columns, map_expr_columns};
use crate::physical::{JoinKey, Plan};
use crate::value::DataType;

/// A grouping of a join that a groupjoin can compute: its operands, the
/// group operand's keys paired with the other's, the set functions over
/// the other operand's columns, the group operand's column each grouping
/// column is, whether a group no row joined is kept, of a LEFT join, and
/// whether the group operand was the join's right, built.
pub(super) struct Found<'a> {
    pub(super) build: &'a Plan,
    pub(super) probe: &'a Plan,
    /// Each pair of keys, the build's column `left`, the probe's `right`.
    pub(super) keys: Vec<JoinKey>,
    pub(super) aggs: Vec<AggCall>,
    /// Of each set function, whether it was filtered by a marker, so a
    /// LEFT join's row of nulls feeds it nothing.
    pub(super) marked: Vec<bool>,
    pub(super) columns: Vec<usize>,
    pub(super) outer: bool,
    pub(super) by_first: bool,
    pub(super) widths: (usize, usize),
    /// The join's description, as EXPLAIN shows it.
    pub(super) joined: &'a str,
}

/// A join a groupjoin may stand for: its operands, keys, whether it is a
/// LEFT one, and its description.
struct Join<'a> {
    left: &'a Plan,
    right: &'a Plan,
    keys: &'a [JoinKey],
    outer: bool,
    detail: &'a str,
}

/// The join `plan` is, under projections of its columns alone, and the
/// column of the join each column of `plan` is.
fn join_under<'a>(plan: &'a Plan, db: &State) -> Option<(Join<'a>, Vec<usize>)> {
    let mut map: Option<Vec<usize>> = None;
    let mut at = plan;
    while let Plan::Project { exprs, input, .. } = at {
        let columns = exprs.iter().map(|e| if let Expr::Column(c) = e { Some(*c) } else { None }).collect::<Option<Vec<_>>>()?;
        map = Some(match map {
            Some(outer) => outer.iter().map(|&c| columns.get(c).copied()).collect::<Option<_>>()?,
            None => columns,
        });
        at = input;
    }
    let join = joined(at)?;
    let map = match map {
        Some(map) => map,
        None => (0..width(at, db)?).collect(),
    };
    Some((join, map))
}

/// Whether `plan`, under projections, is a join a groupjoin can stand
/// over (`joined`), as a grouping of it may be one.
pub(in crate::physical) fn over_join(plan: &Plan) -> bool {
    let mut at = plan;
    while let Plan::Project { input, .. } = at {
        at = input;
    }
    joined(at).is_some()
}

/// The join `plan` is: an inner hash join, or a LEFT outer join on keys of
/// no other condition.
fn joined(plan: &Plan) -> Option<Join<'_>> {
    if let Plan::HashJoin { left, right, keys, detail, .. } = plan {
        return Some(Join { left, right, keys, outer: false, detail });
    }
    let Plan::OuterJoin { kind: OuterJoin::Left, left, right, keys, cond: None, own: None, detail, .. } = plan else { return None };
    (!keys.is_empty()).then_some(Join { left, right, keys, outer: true, detail })
}

/// Whether values of the types `a` and `b` are matched by the executor's
/// table exactly as a join matches them: exact numbers of any scale, as
/// their units and scales normalized compare; dates; truth values.
fn matched_alike(a: &DataType, b: &DataType) -> bool {
    matches!(
        (a, b),
        (DataType::Exact { .. }, DataType::Exact { .. }) | (DataType::Date, DataType::Date) | (DataType::Boolean, DataType::Boolean)
    )
}

/// Whether a value of a column of type `b` equal to one of type `a` is
/// the same value: integers alike in type, or dates.
fn same_value(a: &DataType, b: &DataType) -> bool {
    let integer = matches!(a, DataType::Exact { scale: 0, .. });
    a == b && (integer || matches!(a, DataType::Date))
}

/// The groupjoin the grouping `plan` is, if it is one (as the module
/// says): its group operand the join's left or right, the left first.
pub(super) fn recognized<'a>(plan: &'a Plan, db: &State) -> Option<Found<'a>> {
    let Plan::Aggregate { groups, collations, aggs, seeds, input, .. } = plan else { return None };
    // Groups made before any row, or by a collation, are not one row each.
    if groups.is_empty() || !collations.is_empty() || !seeds.is_empty() {
        return None;
    }
    let (join, map) = join_under(input, db)?;
    let grouping = Grouping { groups, aggs, map: &map };
    sided(&join, &grouping, true, db).or_else(|| (!join.outer).then(|| sided(&join, &grouping, false, db)).flatten())
}

/// A grouping's columns and set functions, and the join column each
/// column of its input is.
struct Grouping<'a> {
    groups: &'a [usize],
    aggs: &'a [AggCall],
    map: &'a [usize],
}

/// The groupjoin of `join` under `grouping`, its group operand the left
/// if `left`, else the right, if that operand is one's.
fn sided<'a>(join: &Join<'a>, grouping: &Grouping<'_>, left: bool, db: &State) -> Option<Found<'a>> {
    let lw = width(join.left, db)?;
    let (build, probe) = if left { (join.left, join.right) } else { (join.right, join.left) };
    // The build's columns among the join's, and the probe's.
    let (build_at, probe_at) = if left { (0, lw) } else { (lw, 0) };
    let widths = (width(build, db)?, width(probe, db)?);
    let keys: Vec<JoinKey> =
        join.keys.iter().map(|k| if left { *k } else { JoinKey { left: k.right, right: k.left, nulls: k.nulls } }).collect();
    let pair = |k: &JoinKey| Some((column_type(build, k.left, db)?, column_type(probe, k.right, db)?));
    let typed = keys.iter().map(pair).collect::<Option<Vec<_>>>()?;
    if !typed.iter().all(|(a, b)| matched_alike(a, b)) {
        return None;
    }
    let of_build = |j: usize| j.checked_sub(build_at).filter(|&c| c < widths.0);
    let of_probe = |j: usize| j.checked_sub(probe_at).filter(|&c| c < widths.1);
    let mut columns = Vec::with_capacity(grouping.groups.len());
    for &g in grouping.groups {
        let j = *grouping.map.get(g)?;
        let column = match of_build(j) {
            Some(c) => c,
            // A probe's key column under an inner join is its build key's
            // value, where that is the same value.
            None => substituted(of_probe(j)?, (&keys, &typed), join.outer)?,
        };
        columns.push(column);
    }
    let (aggs, marked) = probed(grouping.aggs, grouping.map, (&of_probe, probe))?;
    let sets = unique(build, db);
    if !keyed_once(&sets, &keys, &columns, join.outer) {
        return None;
    }
    Some(Found { build, probe, keys, aggs, marked, columns, outer: join.outer, by_first: !left, widths, joined: join.detail })
}

/// The build's column a grouping column of the probe's key column `p` is,
/// of an inner join: of the key of `p`, whose two columns' values are the
/// same value.
fn substituted(p: usize, (keys, typed): (&[JoinKey], &[(DataType, DataType)]), outer: bool) -> Option<usize> {
    if outer {
        return None;
    }
    keys.iter().zip(typed).find(|(k, (a, b))| k.right == p && same_value(a, b)).map(|(k, _)| k.left)
}

/// The set functions `aggs`, their columns the probe's, as each of the
/// join's maps to one by `of_probe`, if each reads the probe's alone, and
/// none is DISTINCT, filtered but by a marker of `probe`, or an inverse
/// distribution function; and of each, whether it was filtered by one.
fn probed(
    aggs: &[AggCall],
    map: &[usize],
    (of_probe, probe): (&dyn Fn(usize) -> Option<usize>, &Plan),
) -> Option<(Vec<AggCall>, Vec<bool>)> {
    let to_probe = |c: usize| map.get(c).copied().and_then(of_probe);
    aggs.iter()
        .map(|a| {
            let marked = match &a.filter {
                Some(filter) => marker(filter, &to_probe, probe).then_some(true)?,
                None => false,
            };
            if a.distinct || a.distribution.is_some() {
                return None;
            }
            let mut read = Vec::new();
            if let Some(arg) = &a.arg {
                expr_columns(arg, &mut read);
            }
            if !read.iter().all(|&c| to_probe(c).is_some()) {
                return None;
            }
            let moved = |c: usize| to_probe(c).unwrap_or(c);
            let unfiltered = AggCall { filter: None, ..a.clone() };
            Some((unfiltered.map_exprs(&mut |e| map_expr_columns(e, &moved)), marked))
        })
        .collect::<Option<Vec<_>>>()
        .map(|pairs| pairs.into_iter().unzip())
}

/// Whether `filter` is a marker's: IS NOT NULL of a column of `probe`
/// that is a constant not null, as `to_probe` maps the join's column to
/// it.
fn marker(filter: &Cond, to_probe: &dyn Fn(usize) -> Option<usize>, probe: &Plan) -> bool {
    let Cond::IsNull(Expr::Column(c), true) = filter else { return false };
    let Plan::Project { exprs, .. } = probe else { return false };
    let at = to_probe(*c).and_then(|p| exprs.get(p));
    let Some(Expr::Const(v)) = at else { return false };
    !v.is_null()
}

/// Whether the build's rows are one a group and a probe row's key finds
/// one at most: a unique set of the build among the grouping's `columns`
/// and its `keys`, null-safe, or, of an inner join, each column of it
/// matched by a key on which a null matches nothing, so a row of a null
/// there joins nothing and is no group.
fn keyed_once(sets: &[Unique], keys: &[JoinKey], columns: &[usize], outer: bool) -> bool {
    let grouped: Vec<Unique> = sets.iter().filter(|u| u.columns.iter().all(|c| columns.contains(c))).cloned().collect();
    let kept: Vec<Unique> = grouped.into_iter().filter(|u| u.null_safe || !outer).collect();
    found_once(&kept, keys, |k| k.left)
}
