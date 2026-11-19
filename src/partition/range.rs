//! \[EXTENSION\] PARTITION BY RANGE (RULES.md D154): each partition holds
//! the keys from its lower bound, included, to its upper bound, excluded,
//! compared part by part as rows are [ISO/IEC 9075:1992 §8.2 GR7]. A part
//! of MINVALUE or MAXVALUE is below or above every value, and the parts
//! after it are MINVALUE or MAXVALUE too, as they no longer matter. A null
//! key part is in no range: such a row belongs to the DEFAULT partition.

use std::cmp::Ordering;

use super::{PartitionStrategy, RANGE, Restriction, refused, scalar_key};
use crate::catalog::{PartitionBound, RangeDatum};
use crate::error::Result;
use crate::value::{DataType, Value, assign, compare};

/// The built-in RANGE strategy.
#[derive(Debug, Clone, Copy, Default)]
pub struct RangeStrategy;

impl PartitionStrategy for RangeStrategy {
    fn name(&self) -> &str {
        RANGE
    }

    fn check_key(&self, types: &[DataType]) -> Result<()> {
        scalar_key(RANGE, types)
    }

    fn bound(&self, types: &[DataType], spec: PartitionBound) -> Result<PartitionBound> {
        let PartitionBound::Range { from, to } = spec else {
            return refused("[EXTENSION] a partition of PARTITION BY RANGE is FOR VALUES FROM (...) TO (...) or DEFAULT");
        };
        let (from, to) = (typed_datums(from, types)?, typed_datums(to, types)?);
        if order(&from, &to) != Ordering::Less {
            return refused("[EXTENSION] a range partition's FROM bound is not below its TO bound, so it holds no row");
        }
        Ok(PartitionBound::Range { from, to })
    }

    fn overlaps(&self, a: &PartitionBound, b: &PartitionBound) -> Result<bool> {
        let (Some((a_from, a_to)), Some((b_from, b_to))) = (ends(a), ends(b)) else { return Ok(false) };
        Ok(order(a_from, b_to) == Ordering::Less && order(b_from, a_to) == Ordering::Less)
    }

    fn contains(&self, bound: &PartitionBound, key: &[Value]) -> Result<bool> {
        let Some((from, to)) = ends(bound) else { return Ok(false) };
        let (Some(low), Some(high)) = (against(key, from), against(key, to)) else { return Ok(false) };
        Ok(low != Ordering::Less && high == Ordering::Less)
    }

    fn admits(&self, bound: &PartitionBound, restrictions: &[Option<Restriction>]) -> Result<bool> {
        let Some((from, to)) = ends(bound) else { return Ok(false) };
        // A part restricted to null is in no range.
        if restrictions.iter().any(|r| matches!(r, Some(Restriction::Null))) {
            return Ok(false);
        }
        Ok(spans(restrictions).iter().any(|(low, high)| order(low, to) == Ordering::Less && order(from, high) != Ordering::Greater))
    }
}

/// The datums of a bound assigned to the key's types, MINVALUE and
/// MAXVALUE kept, every part after one of them one of them too.
fn typed_datums(datums: Vec<RangeDatum>, types: &[DataType]) -> Result<Vec<RangeDatum>> {
    if datums.len() != types.len() {
        return refused(format!("[EXTENSION] a range bound of {} values for a key of {} parts", datums.len(), types.len()));
    }
    let mut unbounded = false;
    let mut out = Vec::with_capacity(datums.len());
    for (d, t) in datums.into_iter().zip(types) {
        match d {
            RangeDatum::Value(v) => {
                if unbounded {
                    return refused("[EXTENSION] a range bound's part after MINVALUE or MAXVALUE is MINVALUE or MAXVALUE too");
                }
                if v.is_null() {
                    return refused("[EXTENSION] a range bound holds no null");
                }
                out.push(RangeDatum::Value(assign(v, t, "EXTENSION")?));
            }
            RangeDatum::MinValue | RangeDatum::MaxValue => {
                unbounded = true;
                out.push(d);
            }
        }
    }
    Ok(out)
}

/// The two ends of a range bound; none of another bound.
fn ends(bound: &PartitionBound) -> Option<(&[RangeDatum], &[RangeDatum])> {
    match bound {
        PartitionBound::Range { from, to } => Some((from, to)),
        PartitionBound::Default | PartitionBound::List(_) | PartitionBound::With(_) => None,
    }
}

/// The order of one datum against another: MINVALUE below every value,
/// MAXVALUE above.
fn datum_order(a: &RangeDatum, b: &RangeDatum) -> Ordering {
    match (a, b) {
        (RangeDatum::MinValue, RangeDatum::MinValue) | (RangeDatum::MaxValue, RangeDatum::MaxValue) => Ordering::Equal,
        (RangeDatum::MinValue, RangeDatum::Value(_) | RangeDatum::MaxValue) | (RangeDatum::Value(_), RangeDatum::MaxValue) => {
            Ordering::Less
        }
        (RangeDatum::MaxValue, RangeDatum::Value(_) | RangeDatum::MinValue) | (RangeDatum::Value(_), RangeDatum::MinValue) => {
            Ordering::Greater
        }
        (RangeDatum::Value(x), RangeDatum::Value(y)) => compare(x, y).unwrap_or(Ordering::Equal),
    }
}

/// Two bounds' order, part by part.
fn order(a: &[RangeDatum], b: &[RangeDatum]) -> Ordering {
    a.iter().zip(b).map(|(x, y)| datum_order(x, y)).find(|o| *o != Ordering::Equal).unwrap_or(Ordering::Equal)
}

/// A key's order against a bound, part by part; none if a part of the
/// key is null, which compares to nothing.
fn against(key: &[Value], bound: &[RangeDatum]) -> Option<Ordering> {
    for (v, d) in key.iter().zip(bound) {
        let o = match d {
            RangeDatum::MinValue => Ordering::Greater,
            RangeDatum::MaxValue => Ordering::Less,
            RangeDatum::Value(x) => compare(v, x)?,
        };
        if o != Ordering::Equal {
            return Some(o);
        }
    }
    Some(Ordering::Equal)
}

/// The spans of keys meeting the restrictions, each its lowest and
/// highest key as bounds, both included: the parts equal to one value
/// make a prefix, the next part's restriction its ends, MINVALUE and
/// MAXVALUE the rest. A part equal to several values gives a span each.
fn spans(restrictions: &[Option<Restriction>]) -> Vec<(Vec<RangeDatum>, Vec<RangeDatum>)> {
    let mut prefix = Vec::new();
    for (at, r) in restrictions.iter().enumerate() {
        let rest = restrictions.len().saturating_sub(at.saturating_add(1));
        let span = |low: RangeDatum, high: RangeDatum| {
            let (mut l, mut h) = (prefix.clone(), prefix.clone());
            l.push(low);
            h.push(high);
            l.extend(std::iter::repeat_n(RangeDatum::MinValue, rest));
            h.extend(std::iter::repeat_n(RangeDatum::MaxValue, rest));
            (l, h)
        };
        match r {
            Some(Restriction::Equal(values)) => {
                if let [v] = values.as_slice() {
                    prefix.push(RangeDatum::Value(v.clone()));
                } else {
                    return values.iter().map(|v| span(RangeDatum::Value(v.clone()), RangeDatum::Value(v.clone()))).collect();
                }
            }
            Some(Restriction::Range { low, high }) => {
                let end = |e: &Option<(Value, bool)>, open: RangeDatum| e.as_ref().map_or(open, |(v, _)| RangeDatum::Value(v.clone()));
                return vec![span(end(low, RangeDatum::MinValue), end(high, RangeDatum::MaxValue))];
            }
            Some(Restriction::Null) | None => return vec![span(RangeDatum::MinValue, RangeDatum::MaxValue)],
        }
    }
    vec![(prefix.clone(), prefix)]
}
