//! [ISO/IEC 9075-2:2023 §4.27] The candidate keys of a query's result: the
//! primary key, or else the preferred candidate key, that DESCRIBE OUTPUT
//! gives as KEY_TYPE and KEY_MEMBER [§20.10 GR6d, GR7c i 3].
//!
//! A candidate key is a set of the result's columns that determines every
//! column, of which no proper subset does. One within a BPK-set is a BPK-set
//! by the deduction rule [§4.26.2], so the primary key is the left-most
//! candidate key within any BPK-set; the preferred candidate key, of none,
//! the left-most within a BUC-set or the grouping columns, a strong one, or
//! else the left-most of all. The sets of a set are searched in the order
//! the left-most rule gives, the first candidate key found the left-most.
//! A candidate key may be one of many, so the search stops after
//! `SEARCH_LIMIT` sets, and then no key is described.

use super::{Attrs, Known, Walk};
use crate::db::State;
use crate::logical::LogicalPlan;

/// The most sets of columns the search for a candidate key tests.
const SEARCH_LIMIT: usize = 4096;

/// [§20.10 GR6d] The key of a result DESCRIBE OUTPUT describes, each by the
/// positions of its columns, from 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DescribedKey {
    /// KEY_TYPE 1: its primary key.
    Primary(Vec<usize>),
    /// KEY_TYPE 2: its preferred candidate key.
    Preferred(Vec<usize>),
    /// KEY_TYPE 0: none found, the search having stopped.
    Unknown,
}

/// How a key found is described.
type Describe = fn(Vec<usize>) -> DescribedKey;

/// What a search for a candidate key within some columns found.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Found {
    /// The left-most candidate key within them.
    Key(Vec<usize>),
    /// No candidate key is within them.
    None,
    /// The search stopped at `SEARCH_LIMIT` sets.
    Stopped,
}

/// [§4.27] The primary key or the preferred candidate key of the rows of
/// `plan`, of the database `db`.
pub(crate) fn described_key(db: &State, plan: &LogicalPlan) -> DescribedKey {
    super::known(&Walk { db, next: std::cell::Cell::new(0) }, plan).described()
}

impl Known {
    /// [§4.27] Its primary key, else its preferred candidate key: the
    /// left-most within a BPK-set, else within a BUC-set or the grouping
    /// columns, else of all its columns.
    pub(super) fn described(&self) -> DescribedKey {
        let mut budget = SEARCH_LIMIT;
        let primary: Vec<&Attrs> = self.keys.iter().filter(|(_, primary)| *primary).map(|(key, _)| key).collect();
        let mut strong: Vec<&Attrs> = self.keys.iter().map(|(key, _)| key).collect();
        strong.extend(&self.grouping);
        let every: Attrs = self.columns.iter().copied().collect();
        let tiers: [(&[&Attrs], Describe); 3] =
            [(&primary, DescribedKey::Primary), (&strong, DescribedKey::Preferred), (&[&every], DescribedKey::Preferred)];
        for (sets, described) in tiers {
            match self.leftmost_within(sets, &mut budget) {
                Found::Key(key) => return described(key),
                Found::None => {}
                Found::Stopped => return DescribedKey::Unknown,
            }
        }
        DescribedKey::Unknown
    }
}

/// Whether the columns at positions `a` are left of those at `b` by the
/// left-most rule [§4.27]: of their column numbers in order, the shorter
/// extended with zeros, the first that differ the smaller.
pub(super) fn left_of(a: &[usize], b: &[usize]) -> bool {
    let number = |s: &[usize], i: usize| s.get(i).map_or(0, |p| p.saturating_add(1));
    let differs = (0..a.len().max(b.len())).find(|&i| number(a, i) != number(b, i));
    differs.is_some_and(|i| number(a, i) < number(b, i))
}

impl Known {
    /// The left-most candidate key within any of the attribute sets
    /// `sets`, if one is.
    fn leftmost_within(&self, sets: &[&Attrs], budget: &mut usize) -> Found {
        let mut best: Option<Vec<usize>> = None;
        for set in sets {
            let within: Vec<usize> = (0..self.columns.len()).filter(|&p| self.columns.get(p).is_some_and(|a| set.contains(a))).collect();
            match self.leftmost(&within, budget) {
                Found::Key(key) => {
                    if best.as_ref().is_none_or(|b| left_of(&key, b)) {
                        best = Some(key);
                    }
                }
                Found::None => {}
                Found::Stopped => return Found::Stopped,
            }
        }
        best.map_or(Found::None, Found::Key)
    }

    /// The left-most candidate key of the columns at positions `within`,
    /// in order: their sets tested as the left-most rule orders them, each
    /// before those it begins, a set that cannot be made a key, with all
    /// those after it, passed over with them.
    pub(super) fn leftmost(&self, within: &[usize], budget: &mut usize) -> Found {
        let mut chosen: Vec<usize> = Vec::new();
        let mut next = 0;
        loop {
            let Some(spent) = budget.checked_sub(1) else { return Found::Stopped };
            *budget = spent;
            let rest = within.get(next..).unwrap_or_default();
            let mut reach: Vec<usize> = chosen.iter().filter_map(|&i| within.get(i).copied()).collect();
            reach.extend(rest);
            if !self.superkey(&reach) {
                let Some(last) = chosen.pop() else { return Found::None };
                next = last.saturating_add(1);
                continue;
            }
            chosen.push(next);
            let set: Vec<usize> = chosen.iter().filter_map(|&i| within.get(i).copied()).collect();
            if self.superkey(&set) {
                if self.minimal(&set) {
                    return Found::Key(set);
                }
                chosen.pop();
            }
            next = next.saturating_add(1);
        }
    }

    /// Whether the columns at `positions` determine every column.
    fn superkey(&self, positions: &[usize]) -> bool {
        let from: Attrs = positions.iter().filter_map(|&p| self.attribute_of(p)).collect();
        let reached = self.closure(from);
        self.columns.iter().all(|a| reached.contains(a))
    }

    /// Whether no proper subset of the columns at `positions`, a superkey,
    /// determines every column: none of them left out does.
    fn minimal(&self, positions: &[usize]) -> bool {
        (0..positions.len()).all(|i| {
            let less: Vec<usize> = positions.iter().enumerate().filter(|&(j, _)| j != i).map(|(_, &p)| p).collect();
            !self.superkey(&less)
        })
    }
}
