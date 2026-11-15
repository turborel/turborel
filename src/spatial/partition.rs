//! \[DESIGN\] R* subtree selection and split distributions (Beckmann et al.,
//! SIGMOD 1990, sections 4.1 and 4.2). All costs use one coordinate scale
//! per decision: finite input near f64's extremes must not make infinity
//! minus infinity choose a branch. Scaling affects quality at underflow,
//! never the bounds retained in the tree or the rows a search finds.

use std::cmp::Ordering;

use super::Bounds;
use crate::error::{Code, Result, SqlError};

/// The paper's overlap shortlist, chosen by area enlargement. Overlap is
/// still compared with every sibling, not just the shortlisted siblings.
const OVERLAP_CANDIDATES: usize = 32;
/// The paper's experimentally selected minimum occupancy, 40 percent.
const FILL_NUMERATOR: usize = 2;
const FILL_DENOMINATOR: usize = 5;

/// The entry to descend into, minimizing overlap enlargement just above
/// leaves and area enlargement at higher levels; area breaks ties.
pub(crate) fn choose(bounds: &[Bounds], added: Bounds, above_leaves: bool) -> Result<usize> {
    let scaled = Scale::of(bounds.iter().copied().chain([added]));
    let added = scaled.bounds(added);
    let bounds: Vec<_> = bounds.iter().map(|&b| scaled.bounds(b)).collect();
    let mut choices: Vec<_> = bounds.iter().enumerate().map(|(i, &b)| (i, area(b.union(added)) - area(b), area(b))).collect();
    choices.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.2.total_cmp(&b.2)).then(a.0.cmp(&b.0)));
    if above_leaves {
        choices.truncate(OVERLAP_CANDIDATES);
        let best = choices
            .iter()
            .map(|c| (c, overlap_growth(&bounds, c.0, added)))
            .min_by(|(a, x), (b, y)| x.total_cmp(y).then(a.1.total_cmp(&b.1)).then(a.2.total_cmp(&b.2)).then(a.0.cmp(&b.0)));
        return best.map(|(c, _)| c.0).ok_or_else(|| invalid("cannot choose a child of an empty node"));
    }
    choices.first().map(|c| c.0).ok_or_else(|| invalid("cannot choose a child of an empty node"))
}

/// A split's ordering and the number of entries placed in its left node.
pub(crate) struct Split {
    pub(crate) order: Vec<usize>,
    pub(crate) at: usize,
}

/// Chooses the axis by the sum of both groups' margins over every legal
/// distribution in both lower- and upper-coordinate order. On that axis,
/// chooses minimum overlap, then minimum combined area.
pub(crate) fn split(bounds: &[Bounds]) -> Result<Split> {
    let minimum = bounds.len().saturating_mul(FILL_NUMERATOR).checked_div(FILL_DENOMINATOR).unwrap_or(0).max(1);
    let last = bounds.len().checked_sub(minimum).filter(|&last| last >= minimum);
    let last = last.ok_or_else(|| invalid("a split needs at least two entries"))?;
    let scale = Scale::of(bounds.iter().copied());
    let scaled: Vec<_> = bounds.iter().map(|&b| scale.bounds(b)).collect();
    let x = axis(&scaled, Axis::X, minimum..=last)?;
    let y = axis(&scaled, Axis::Y, minimum..=last)?;
    Ok(if x.margin <= y.margin { x.split } else { y.split })
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
}

struct Distribution {
    split: Split,
    margin: f64,
    overlap: f64,
    area: f64,
}

fn axis(bounds: &[Bounds], axis: Axis, range: std::ops::RangeInclusive<usize>) -> Result<Distribution> {
    let mut best: Option<Distribution> = None;
    let mut margin = 0.0_f64;
    for upper in [false, true] {
        let mut order: Vec<_> = (0..bounds.len()).collect();
        order.sort_by(|&a, &b| coordinate(bounds.get(a), axis, upper).total_cmp(&coordinate(bounds.get(b), axis, upper)).then(a.cmp(&b)));
        let suffix = suffixes(bounds, &order)?;
        let mut left = None;
        for (position, &index) in order.iter().enumerate() {
            let entry = bounds.get(index).copied().ok_or_else(|| invalid("a split entry is missing"))?;
            let merged = left.map_or(entry, |b: Bounds| b.union(entry));
            left = Some(merged);
            let at = position.saturating_add(1);
            if range.contains(&at) {
                let right = *suffix.get(at).ok_or_else(|| invalid("a split suffix is missing"))?;
                margin += edge_sum(merged) + edge_sum(right);
                let overlap = overlap(merged, right);
                let area = area(merged) + area(right);
                let better = best.as_ref().is_none_or(|b| overlap.total_cmp(&b.overlap).then(area.total_cmp(&b.area)) == Ordering::Less);
                if better {
                    best = Some(Distribution { split: Split { order: order.clone(), at }, margin: 0.0_f64, overlap, area });
                }
            }
        }
    }
    let mut best = best.ok_or_else(|| invalid("a split has no legal distribution"))?;
    best.margin = margin;
    Ok(best)
}

fn suffixes(bounds: &[Bounds], order: &[usize]) -> Result<Vec<Bounds>> {
    let mut suffix = Vec::with_capacity(order.len());
    let mut accumulated = None;
    for &index in order.iter().rev() {
        let entry = bounds.get(index).copied().ok_or_else(|| invalid("a split entry is missing"))?;
        let merged = accumulated.map_or(entry, |b: Bounds| b.union(entry));
        accumulated = Some(merged);
        suffix.push(merged);
    }
    suffix.reverse();
    Ok(suffix)
}

fn coordinate(bounds: Option<&Bounds>, axis: Axis, upper: bool) -> f64 {
    let Some(b) = bounds else { return 0.0_f64 };
    match (axis, upper) {
        (Axis::X, false) => b.min_x,
        (Axis::X, true) => b.max_x,
        (Axis::Y, false) => b.min_y,
        (Axis::Y, true) => b.max_y,
    }
}

fn overlap_growth(bounds: &[Bounds], index: usize, added: Bounds) -> f64 {
    let Some(&before) = bounds.get(index) else { return f64::INFINITY };
    let after = before.union(added);
    bounds.iter().enumerate().filter(|&(i, _)| i != index).map(|(_, &b)| overlap(after, b) - overlap(before, b)).sum()
}

fn overlap(a: Bounds, b: Bounds) -> f64 {
    a.intersection(b).map_or(0.0_f64, area)
}

fn area(b: Bounds) -> f64 {
    (b.max_x - b.min_x) * (b.max_y - b.min_y)
}

fn edge_sum(b: Bounds) -> f64 {
    b.max_x - b.min_x + b.max_y - b.min_y
}

struct Scale(f64);

impl Scale {
    fn of(bounds: impl Iterator<Item = Bounds>) -> Scale {
        Scale(bounds.flat_map(|b| [b.min_x, b.min_y, b.max_x, b.max_y]).map(f64::abs).fold(f64::MIN_POSITIVE, f64::max))
    }

    fn bounds(&self, b: Bounds) -> Bounds {
        Bounds { min_x: b.min_x / self.0, min_y: b.min_y / self.0, max_x: b.max_x / self.0, max_y: b.max_y / self.0 }
    }
}

fn invalid(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] R-tree {why}"))
}

#[cfg(test)]
mod tests;
