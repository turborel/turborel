//! \[EXTENSION\] [GS §16 R16.1, R16.2] Expand and ExpandInto (RULES.md
//! D113): an index join probing an adjacency index is a graph's Expand,
//! each left row's vertex's edges read of its list. Of the source's
//! columns it is Expand, of the destination's, by the reverse entries an
//! index keeping both ways holds, Expand walked the other way, and of both
//! ends ExpandInto, the edges between two vertices. The join order chooses
//! which end a pattern starts from; this lets either end's edges be read
//! from the index, so neither way falls back to a hash join of every edge.

use std::borrow::Cow;

use super::OfKind;
use crate::catalog::{ColumnDesc, IndexDesc};
use crate::index::IndexMethod;
use crate::index::adjacency::{ADJACENCY_METHOD, planned_ends};
use crate::logical::{Cond, Expr};
use crate::value::{CompOp, DataType};

/// EXPLAIN's name of an index join walking an edge from its source.
pub(super) const EXPAND: &str = "Expand";
/// EXPLAIN's name of one walking an edge from its destination.
pub(super) const EXPAND_REVERSE: &str = "Expand (reverse)";
/// EXPLAIN's name of one reading the edges between two vertices.
pub(super) const EXPAND_INTO: &str = "ExpandInto";

/// The method EXPLAIN shows of a probe of index `ix` giving its first
/// `probed` columns, or its destination's alone if `reverse`: of an
/// adjacency index, the method and which Expand it is.
pub(super) fn method_shown(ix: &IndexDesc, probed: usize, reverse: bool) -> String {
    if ix.method != ADJACENCY_METHOD {
        return ix.method.clone();
    }
    let (source, _, _) = planned_ends(ix);
    let expand = if reverse {
        EXPAND_REVERSE
    } else if probed > source.len() {
        EXPAND_INTO
    } else {
        EXPAND
    };
    format!("{ADJACENCY_METHOD} {expand}")
}

/// Of each column probed, its place among the equalities, its place in the
/// index's key, and its type.
type Probed = (Vec<usize>, Vec<usize>, Vec<DataType>);

/// Of index `ix` on `columns`, of a method of `kind`, what the equalities
/// `keys` probe, their values of their kinds (`of_kind`): the place among
/// `keys` of each column probed, its place in the key, and its type; and
/// whether they are an adjacency index's destination's, read in reverse.
pub(super) fn probed(
    ix: &IndexDesc,
    columns: &[ColumnDesc],
    (keys, of_kind): (&[(usize, usize)], OfKind<'_>),
    kind: &dyn IndexMethod,
) -> Option<(Probed, bool)> {
    if let Some((used, types)) = super::probed_columns(ix, columns, (keys, of_kind), kind) {
        let at = (0..used.len()).collect();
        return Some(((used, at, types), false));
    }
    reversed(ix, columns, (keys, of_kind), kind).map(|probed| (probed, true))
}

/// Of an adjacency index `ix` on `columns`, of a method of `kind`, keeping
/// reverse entries, whose destination's columns the equalities `keys`
/// give values of their kinds (`of_kind`): the place among `keys` of each,
/// the destination's places in the key, and their types; none if a column
/// it does not probe may be null, whose rows it holds no entry of.
fn reversed(
    ix: &IndexDesc,
    columns: &[ColumnDesc],
    (keys, of_kind): (&[(usize, usize)], OfKind<'_>),
    kind: &dyn IndexMethod,
) -> Option<Probed> {
    let (_, destination, reverse) = planned_ends(ix);
    if ix.method != ADJACENCY_METHOD || !reverse || destination.is_empty() {
        return None;
    }
    let (mut used, mut types) = (Vec::new(), Vec::new());
    for at in destination.clone() {
        let column = *ix.columns.get(at)?;
        let data_type = columns.get(column)?.data_type.clone();
        used.push(keys.iter().position(|&(l, r)| r == column && of_kind(l, &data_type))?);
        types.push(data_type);
    }
    // An index holds no key with a null: the columns not probed are NOT NULL.
    let untested = ix.columns.iter().enumerate().filter(|(at, _)| !destination.contains(at));
    if untested.into_iter().any(|(_, &c)| columns.get(c).is_none_or(|d| !d.not_null)) {
        return None;
    }
    let compared: Vec<(usize, CompOp)> = destination.clone().map(|at| (at, CompOp::Eq)).collect();
    kind.supports_columns(ix.columns.len(), &compared).then(|| (used, destination.collect(), types))
}

/// `columns`, each one a conjunct IS NOT NULL of `filter` keeps no null of
/// marked NOT NULL: of a graph's edge joined, its ends
/// (`binder/graph/joins.rs`), so an adjacency index, holding no entry of
/// a null end, may be probed of either; `columns` itself if none is.
pub(super) fn kept<'a>(columns: &'a [ColumnDesc], filter: Option<&Cond>) -> Cow<'a, [ColumnDesc]> {
    let mut found = Vec::new();
    if let Some(cond) = filter {
        not_null(cond, &mut found);
    }
    let mut kept = Cow::Borrowed(columns);
    for column in found {
        if let Some(desc) = kept.to_mut().get_mut(column) {
            desc.not_null = true;
        }
    }
    kept
}

/// The columns of the conjuncts IS NOT NULL of `cond`, into `found`.
fn not_null(cond: &Cond, found: &mut Vec<usize>) {
    if let Cond::And(a, b) = cond {
        not_null(a, found);
        not_null(b, found);
    } else if let Cond::IsNull(Expr::Column(column), true) = cond {
        found.push(*column);
    }
}
