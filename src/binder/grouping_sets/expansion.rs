//! [ISO/IEC 9075-2:2023 §7.13 SR10, SR12 to SR16] A `<group by clause>`
//! taken to the grouping sets it stands for: a simple one, its parentheses
//! erased, to a primitive one; any other, its ROLLUPs and CUBEs to GROUPING
//! SETS, nested GROUPING SETS promoted, and its elements concatenated, to
//! one list of grouping sets, in the order the Syntax Rules give them.

use crate::ast::{GroupBy, GroupingColumn, GroupingElement};
use crate::error::{Code, Result, SqlError};
use crate::parser::MAX_NESTING_DEPTH;

/// \[DESIGN\] The most grouping sets a `<group by clause>` may stand for: a
/// CUBE of n sets stands for 2 to the n, and a concatenation for the
/// product of its elements' (RULES.md R-CONST).
pub const MAX_GROUPING_SETS: usize = 4096;

/// One grouping set: its `<grouping column reference>`s, in order.
pub(super) type Set<'g> = Vec<&'g GroupingColumn>;

/// What a `<group by clause>` stands for.
pub(super) enum Expansion<'g> {
    /// [SR10] A primitive `<group by clause>`: its grouping column
    /// references, none of GROUP BY ().
    Primitive(Set<'g>),
    /// [SR16, SR17] The grouping sets of one `<grouping sets
    /// specification>` of `<ordinary grouping set>`s and `<empty grouping
    /// set>`s.
    Sets(Vec<Set<'g>>),
}

/// The grouping sets `g` stands for.
pub(super) fn expand(g: &GroupBy) -> Result<Expansion<'_>> {
    // [SR9] simple: no <rollup list>, <cube list> or <grouping sets
    // specification>.
    let simple = g.elements.iter().all(|e| matches!(e, GroupingElement::Ordinary(_) | GroupingElement::Empty));
    if simple {
        // [SR10] its parentheses erased, and its <empty grouping set>s.
        return Ok(Expansion::Primitive(g.elements.iter().flat_map(|e| ordinary(e).iter()).collect()));
    }
    // [SR16b to SR16d] each element a <grouping sets specification>, the
    // first two replaced by their concatenation until one is left.
    let mut sets = vec![Vec::new()];
    for e in &g.elements {
        sets = concatenated(&sets, &sets_of(e, 0)?)?;
    }
    Ok(Expansion::Sets(sets))
}

/// The `<grouping column reference>`s of `e` if it is an `<ordinary
/// grouping set>`; none of any other.
fn ordinary(e: &GroupingElement) -> &[GroupingColumn] {
    match e {
        GroupingElement::Ordinary(columns) => columns,
        GroupingElement::Empty | GroupingElement::Rollup(_) | GroupingElement::Cube(_) | GroupingElement::Sets(_) => &[],
    }
}

/// The grouping sets of the `<grouping set>` `e`, nested `depth` deep in
/// GROUPING SETS: an `<ordinary grouping set>`'s or the `<empty grouping
/// set>`'s itself; a ROLLUP's [SR13] or a CUBE's [SR14]; a GROUPING SETS'
/// each of its own, promoted [SR15].
fn sets_of(e: &GroupingElement, depth: usize) -> Result<Vec<Set<'_>>> {
    if depth > MAX_NESTING_DEPTH {
        return Err(SqlError::new(
            Code::Unsupported,
            format!("[DESIGN] GROUPING SETS nests deeper than this implementation's limit of {MAX_NESTING_DEPTH} levels"),
        ));
    }
    match e {
        GroupingElement::Ordinary(columns) => Ok(vec![columns.iter().collect()]),
        GroupingElement::Empty => Ok(vec![Vec::new()]),
        GroupingElement::Rollup(sets) => rollup(sets),
        GroupingElement::Cube(sets) => cube(sets),
        GroupingElement::Sets(inner) => {
            let mut all = Vec::new();
            for i in inner {
                all.extend(sets_of(i, depth.saturating_add(1))?);
                limited(all.len())?;
            }
            Ok(all)
        }
    }
}

/// [SR13] ROLLUP of `sets`: the concatenation of each initial sublist of
/// them, the longest first, down to the `<empty grouping set>`.
fn rollup(sets: &[Vec<GroupingColumn>]) -> Result<Vec<Set<'_>>> {
    limited(sets.len().saturating_add(1))?;
    Ok((0..=sets.len()).rev().map(|i| sets.iter().take(i).flatten().collect()).collect())
}

/// [SR14] CUBE of `sets`: of each binary number BSL of as many bits, from
/// 2 to the n, less 1, down to 0, the concatenation of the sets whose bit,
/// counted from the left, is 1.
fn cube(sets: &[Vec<GroupingColumn>]) -> Result<Vec<Set<'_>>> {
    let n = u32::try_from(sets.len()).unwrap_or(u32::MAX);
    let count = 1_usize.checked_shl(n).filter(|&c| c <= MAX_GROUPING_SETS).ok_or_else(too_many)?;
    let bit = |i: usize, j: usize| (i >> sets.len().saturating_sub(j).saturating_sub(1)) & 1 == 1;
    Ok((0..count).rev().map(|i| sets.iter().enumerate().filter(|(j, _)| bit(i, *j)).flat_map(|(_, s)| s).collect()).collect())
}

/// [SR16c v, vi] Each set of `xs` followed by each of `ys`: of each of
/// `xs`, in order, its concatenation with each of `ys`, in order.
fn concatenated<'g>(xs: &[Set<'g>], ys: &[Set<'g>]) -> Result<Vec<Set<'g>>> {
    limited(xs.len().saturating_mul(ys.len()))?;
    Ok(xs.iter().flat_map(|x| ys.iter().map(move |y| x.iter().chain(y).copied().collect())).collect())
}

/// Refused if `count` grouping sets are more than `MAX_GROUPING_SETS`.
fn limited(count: usize) -> Result<()> {
    if count > MAX_GROUPING_SETS { Err(too_many()) } else { Ok(()) }
}

/// The error of a `<group by clause>` of more than `MAX_GROUPING_SETS`.
fn too_many() -> SqlError {
    SqlError::new(
        Code::Unsupported,
        format!(
            "[ISO/IEC 9075-2:2023 §7.13 SR13, SR14, SR16] GROUP BY stands for more than this implementation's limit of {MAX_GROUPING_SETS} grouping sets"
        ),
    )
}

// Unit tests: the transformations of the Syntax Rules (RULES.md R-TEST-1,
// R-TEST-8).
#[cfg(test)]
mod tests;
