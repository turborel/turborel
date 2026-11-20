//! \[DESIGN\] Joins on several columns, and on keys (RULES.md D384).
//!
//! The classes of equalities (`cardinality/joins.rs`) take each class as
//! independent of the others, so two relations joined on two columns keep
//! one pair in as many as the product of each column's larger distinct
//! values. Where the columns go together, as a part and its supplier do
//! in an offer of a part by a supplier, that is the pairs the columns
//! could make, not those they hold, and a join of every line item to its
//! one offer is estimated thousands of times too few.
//!
//! So each pair of relations the classes join gets a correction, applied
//! whenever both are in a set, which keeps every way of joining the set
//! at the same rows (D302): the product over the pair's classes of each
//! class's larger distinct values, over the larger of the two relations'
//! joint distinct values of their columns. A relation's joint distinct
//! values are, in order:
//!
//! * of columns holding a key of it (`cardinality/keys.rs`), the key's;
//! * of one column, its distinct values;
//! * of several, as its statistics count them together, an index whose
//!   key they are among them (`Statistics::group_distinct`), or an index
//!   of exactly them asked itself;
//! * else guessed, the product of each column's, at most the table's
//!   rows.
//!
//! A guess of one side is at most the other side's key, as the values a
//! join matches to a key are taken to be among the key's, the assumption
//! of containment the formula of distinct values already makes. A
//! referential constraint makes it exact: the referencing columns' values
//! are among the referenced key's, so their join keeps one pair in as many
//! as the referenced table has rows. The correction never lowers an
//! estimate.

use super::lineage::{Over, one_table, origin};
use super::{Classes, Estimates};
use crate::logical::LogicalPlan;

/// A column of a class: its relation, its place among the relation's
/// columns, and its distinct values.
#[derive(Debug, Clone, Copy)]
pub(in crate::physical) struct Member {
    pub(in crate::physical) rel: usize,
    pub(in crate::physical) column: usize,
    pub(in crate::physical) distinct: f64,
}

/// A relation's joint distinct values of some of its columns, and how
/// they are known.
#[derive(Debug, Clone, Copy)]
enum Joint {
    /// The columns hold a key: as many as the key's rows.
    Key(f64),
    /// Counted by the statistics or an index.
    Counted(f64),
    /// Guessed from each column's.
    Guessed(f64),
}

impl Joint {
    /// The distinct values.
    fn value(self) -> f64 {
        match self {
            Joint::Key(d) | Joint::Counted(d) | Joint::Guessed(d) => d,
        }
    }

    /// It, a guess at most `most`.
    fn guess_within(self, most: f64) -> Joint {
        match self {
            Joint::Guessed(d) => Joint::Guessed(d.min(most)),
            Joint::Key(_) | Joint::Counted(_) => self,
        }
    }
}

/// Of the joint distinct values of each side, a guess of one at most a key
/// of the other.
fn contained(r: Joint, s: Joint) -> (Joint, Joint) {
    match (r, s) {
        (Joint::Key(k), Joint::Guessed(_)) => (r, s.guess_within(k)),
        (Joint::Guessed(_), Joint::Key(k)) => (r.guess_within(k), s),
        (Joint::Key(_) | Joint::Counted(_) | Joint::Guessed(_), Joint::Key(_) | Joint::Counted(_) | Joint::Guessed(_)) => (r, s),
    }
}

/// The pairs of relations `members`' classes join, each the lesser
/// first, once.
fn linked(members: &[Vec<Member>]) -> Vec<(usize, usize)> {
    let mut linked: Vec<(usize, usize)> = Vec::new();
    let pairs = members.iter().flat_map(|class| {
        class.iter().enumerate().flat_map(move |(i, a)| class.iter().skip(i.saturating_add(1)).map(move |b| (a.rel.min(b.rel), a.rel.max(b.rel))))
    });
    for pair in pairs {
        if pair.0 != pair.1 && !linked.contains(&pair) {
            linked.push(pair);
        }
    }
    linked
}

/// One side of a pair of relations: its plan and its columns the pair's
/// classes join, with their distinct values.
struct Side<'p> {
    plan: &'p LogicalPlan,
    columns: Vec<usize>,
    distinct: Vec<f64>,
}

impl Estimates<'_> {
    /// The classes `members` of the relations `leaves`, each pair of
    /// relations they join with its correction.
    pub(in crate::physical) fn keyed_classes(&self, leaves: &[&LogicalPlan], members: &[Vec<Member>]) -> Classes {
        let mut pairs = Vec::new();
        for (r, s) in linked(members) {
            let (Some(&r_plan), Some(&s_plan)) = (leaves.get(r), leaves.get(s)) else { continue };
            let shared: Vec<(Member, Member)> = members
                .iter()
                .filter_map(|class| Some((*class.iter().find(|m| m.rel == r)?, *class.iter().find(|m| m.rel == s)?)))
                .collect();
            let side = |plan, of: &dyn Fn(&(Member, Member)) -> Member| Side {
                plan,
                columns: shared.iter().map(|m| of(m).column).collect(),
                distinct: shared.iter().map(|m| of(m).distinct).collect(),
            };
            let factor = self.pair_factor(&side(r_plan, &|m| m.0), &side(s_plan, &|m| m.1));
            if factor > 1.0_f64 {
                pairs.push((r, s, factor));
            }
        }
        let classes = members.iter().map(|class| class.iter().map(|m| (m.rel, m.distinct)).collect()).collect();
        Classes::new(classes).with_pairs(pairs)
    }

    /// The correction of the join of `r` and `s`: their classes' larger
    /// distinct values multiplied, over the larger joint distinct values,
    /// at least 1.
    fn pair_factor(&self, r: &Side<'_>, s: &Side<'_>) -> f64 {
        let classes: f64 = r.distinct.iter().zip(&s.distinct).map(|(a, b)| a.max(*b).max(1.0_f64)).product();
        let (jr, js) = (self.joint(r), self.joint(s));
        let (jr, js) = if self.referencing(r, s) {
            (Joint::Counted(jr.value().min(js.value())), js)
        } else if self.referencing(s, r) {
            (jr, Joint::Counted(js.value().min(jr.value())))
        } else {
            contained(jr, js)
        };
        (classes / jr.value().max(js.value()).max(1.0_f64)).max(1.0_f64)
    }

    /// The joint distinct values of `side`'s columns, as the module says.
    fn joint(&self, side: &Side<'_>) -> Joint {
        if let Some(key) = self.key_of(side.plan, &side.columns) {
            return Joint::Key(key);
        }
        let guess = side.distinct.iter().map(|d| d.max(1.0_f64)).product::<f64>();
        if let [column] = side.columns.as_slice() {
            let counted = origin(side.plan, *column).and_then(|o| self.statistics(o.table)?.distinct(o.column));
            return if counted.is_some() { Joint::Counted(guess) } else { Joint::Guessed(guess) };
        }
        let Some((table, places)) = one_table(Over::One(side.plan), &side.columns) else { return Joint::Guessed(guess) };
        let mut keys: Vec<usize> = places.iter().map(|&(_, place)| place).collect();
        keys.sort_unstable();
        keys.dedup();
        let rows = self.table_rows(table).unwrap_or(f64::INFINITY);
        let counted = self.statistics(table).and_then(|s| s.group_distinct(&keys)).or_else(|| self.sketched(table, &keys));
        counted.map_or(Joint::Guessed(guess.min(rows)), |d| Joint::Counted(d.min(rows)))
    }

    /// Whether a referential constraint of `from`'s table references
    /// `to`'s on exactly their columns, each read of its table.
    fn referencing(&self, from: &Side<'_>, to: &Side<'_>) -> bool {
        let (Some((from_table, from_places)), Some((to_table, to_places))) =
            (one_table(Over::One(from.plan), &from.columns), one_table(Over::One(to.plan), &to.columns))
        else {
            return false;
        };
        let pairs: Vec<(usize, usize)> = from_places.iter().zip(&to_places).map(|(&(_, a), &(_, b))| (a, b)).collect();
        self.table_keys(from_table).references(to_table, &pairs)
    }
}
