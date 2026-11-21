//! \[DESIGN\] The join tree of a chain's relations, found by GYO ear
//! removal (Graham 1979; Yu and Ozsoyoglu 1979), restricted to the
//! relations no output reads (RULES.md D542).
//!
//! A relation is an ear of a parent when every conjunct that reads it and
//! another relation left reads the parent too: a conjunct of the two
//! alone, or an equality of a column of it with one of a third relation
//! that the parent's equalities with both imply, the three columns of one
//! numeric type, so the implication holds of every value. An ear's
//! conjuncts with its parent are its semi join's condition, and those with
//! a third are implied, so dropped. A relation of no conjunct with any
//! other is an ear of any: its semi join keeps its parent's rows if it has
//! a row, as the product did.

use crate::logical::{Cond, Expr};
use crate::value::{CompOp, DataType};

/// A conjunct of the chain: the relations it reads, as a bit set, and,
/// of an equality of a column of each of two, the columns.
pub(super) struct Edge {
    /// The relations it reads.
    pub(super) rels: u64,
    /// Of `a = b`, columns of two relations, the two.
    pub(super) equal: Option<(usize, usize)>,
}

impl Edge {
    /// The edge of `c`, reading the relations `rels`.
    pub(super) fn of(c: &Cond, rels: u64) -> Edge {
        let equal = match c {
            Cond::Compare(Expr::Column(a), CompOp::Eq, Expr::Column(b)) => Some((*a, *b)),
            Cond::Compare(..)
            | Cond::Const(_)
            | Cond::Distinct(..)
            | Cond::IsNull(..)
            | Cond::Like(..)
            | Cond::In(..)
            | Cond::Value(_)
            | Cond::And(..)
            | Cond::Or(..)
            | Cond::Not(_)
            | Cond::Is(..)
            | Cond::Exists(_)
            | Cond::Unique(..)
            | Cond::Quantified(..)
            | Cond::Match { .. } => None,
        };
        Edge { rels: if rels.count_ones() > 1 { rels } else { 0 }, equal }
    }
}

/// What the ears are found over: the conjuncts' edges, the relation of
/// each column, and each column's type.
pub(super) struct Graph<'a> {
    pub(super) edges: &'a [Edge],
    pub(super) rel_of: &'a dyn Fn(usize) -> Option<usize>,
    pub(super) type_of: &'a dyn Fn(usize) -> Option<DataType>,
}

/// One ear removed: the relation, its parent, the edges that are its
/// semi join's condition and those implied.
pub(super) struct Ear {
    pub(super) rel: usize,
    pub(super) parent: usize,
    pub(super) joined: Vec<usize>,
    pub(super) implied: Vec<usize>,
}

/// The bit of relation `r`.
fn bit(r: usize) -> u64 {
    u32::try_from(r).ok().and_then(|r| 1_u64.checked_shl(r)).unwrap_or(0)
}

impl Graph<'_> {
    /// The ears, in the order removed, of the relations `left`, none of
    /// those of `kept`; edges `used` already taken are passed over.
    pub(super) fn ears(&self, mut left: u64, kept: u64) -> Vec<Ear> {
        let mut used = vec![false; self.edges.len()];
        let mut out = Vec::new();
        while let Some(ear) = self.next_ear(left, kept, &used) {
            for &e in ear.joined.iter().chain(&ear.implied) {
                if let Some(u) = used.get_mut(e) {
                    *u = true;
                }
            }
            left &= !bit(ear.rel);
            out.push(ear);
        }
        out
    }

    /// The first ear of `left` not of `kept`, if one is.
    fn next_ear(&self, left: u64, kept: u64, used: &[bool]) -> Option<Ear> {
        let rels = (0..64).filter(|&r| left & bit(r) != 0);
        for rel in rels.clone().filter(|&r| kept & bit(r) == 0) {
            for parent in rels.clone().filter(|&p| p != rel) {
                if let Some(ear) = self.ear_of(rel, parent, (left, used)) {
                    return Some(ear);
                }
            }
        }
        None
    }

    /// `rel` as an ear of `parent` among the relations `left`, if it is.
    fn ear_of(&self, rel: usize, parent: usize, (left, used): (u64, &[bool])) -> Option<Ear> {
        let (r, p) = (bit(rel), bit(parent));
        let mut ear = Ear { rel, parent, joined: Vec::new(), implied: Vec::new() };
        for (i, e) in self.edges.iter().enumerate() {
            let live = e.rels & left;
            if used.get(i).copied().unwrap_or(true) || live & r == 0 || live.count_ones() < 2 {
                continue;
            }
            if live & !(r | p) == 0 {
                ear.joined.push(i);
            } else if self.implied(e, rel, parent, used) {
                ear.implied.push(i);
            } else {
                return None;
            }
        }
        Some(ear)
    }

    /// Whether the equality `e` of a column of `rel` with one of a third
    /// relation is implied by equalities of a column of the parent with
    /// both, the three of one numeric type.
    fn implied(&self, e: &Edge, rel: usize, parent: usize, used: &[bool]) -> bool {
        let Some((a, b)) = e.equal else { return false };
        let (own, other) = if (self.rel_of)(a) == Some(rel) { (a, b) } else { (b, a) };
        let Some(ty) = (self.type_of)(own).filter(DataType::is_numeric) else { return false };
        let alike = |c: usize| (self.type_of)(c).as_ref() == Some(&ty);
        let live = || self.edges.iter().zip(used).filter(|(_, u)| !**u).filter_map(|(f, _)| f.equal);
        let equal = |x: usize, y: usize| live().any(|pair| pair == (x, y) || pair == (y, x));
        let partner = |(m, n): (usize, usize)| {
            if m == own {
                Some(n)
            } else if n == own {
                Some(m)
            } else {
                None
            }
        };
        let through = |c: usize| (self.rel_of)(c) == Some(parent) && alike(c) && equal(c, other);
        alike(other) && live().filter_map(partner).any(through)
    }
}
