//! \[DESIGN\] "R op ALL S" and "R op SOME S" [X3.135-1986 §5.16 GR2; ISO/IEC
//! 9075:1992 §8.7], IN among them [X3.135-1986 §5.13 GR3], R a row of one
//! value or more, nulls in R or in S's rows or not, answered without
//! comparing R with every row of S: S's rows are read once, the first time
//! a comparison asks, and what it needs kept of them. Each row of S gives
//! "R op r" true, false or unknown [ISO/IEC 9075:1992 §8.2 GR7]; ALL is
//! false if one gives false, else unknown if one gives unknown, else true;
//! SOME true if one gives true, else unknown if one does, else false
//! [X3.135-1986 §5.16 GR2].
//!
//! * = and <>: "R = r" is true if every value of R equals r's, false if a
//!   value of R differs from r's where neither is null, else unknown; "R <>
//!   r" is its negation [§8.2 GR7g, h], so <> ALL is NOT = SOME, and <> SOME
//!   NOT = ALL. = SOME (IN) asks whether a row agrees with R where neither
//!   is null: for each pattern of nulls S's rows have, its rows by the
//!   buckets of their values where neither is null, as a hash join's right
//!   operand (`exec/join.rs`), in memory or in a table of the spill area,
//!   made the first time a pattern of R asks. = ALL asks whether every row
//!   agrees with R: each column's least and greatest value not null equal
//!   R's, where R's is not null.
//! * <, <=, > and >=: only the values before the first null, of R or of r,
//!   decide "R op r" [§8.2 GR7c-f, i-l]: true or false at the first that
//!   differ, else unknown if there is a null, else they are equal. S's rows
//!   are kept by the place of their first null, each place's least and
//!   greatest values before it: those decide which of less, greater, equal
//!   and unknown its rows give. A place whose least is below R and greatest
//!   above may give equal or not, but it gives both less and greater, which
//!   decide ALL and SOME alone.
//!
//! Only a comparison of one row (§5.11 GR1) is made with the row
//! (`exec/eval.rs`). Split from exec/subquery.rs (RULES.md R-SIZE-2).

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::super::join::{Built, Full, Right};
use super::super::work::Work;
use super::SubqueryRows;
use crate::error::Result;
use crate::logical::{Quantifier, Truth};
use crate::storage::Row;
use crate::sync::lock_builtin;
use crate::value::{Bucket, CompOp, Value, bucket, compare};

/// The hashes of a result's rows of each pattern of nulls, each on the
/// columns where neither those rows nor R are null, by the place of the
/// rows' pattern in `Summary::patterns`: for an R of no null, and by the
/// pattern of nulls of others.
#[derive(Default)]
pub(super) struct Agreeing {
    whole: Vec<Option<Built>>,
    nulled: HashMap<Vec<bool>, Vec<Option<Built>>>,
}

/// What a result is, for the comparisons of a row with each of its rows.
pub(super) struct Summary {
    rows: usize,
    /// The patterns of nulls of its rows, which values are null, each once,
    /// those with fewer nulls first.
    patterns: Vec<Vec<bool>>,
    /// Each column's least and greatest value not null.
    columns: Vec<Option<(Value, Value)>>,
    /// For each place of a row's first null, its degree for a row of none,
    /// the least and greatest of those rows' values before it.
    prefixes: Vec<Option<(Row, Row)>>,
}

impl SubqueryRows {
    /// "`xs` `op` `quantifier` S", S its rows.
    ///
    /// # Errors
    ///
    /// The rows', and the spill area's; SQLCODE -811 for a comparison of
    /// one row with more than one.
    pub fn quantified(&self, xs: &[Value], (op, quantifier): (CompOp, Quantifier)) -> Result<Truth> {
        if quantifier == Quantifier::One {
            return super::super::eval::quantified(xs, (op, quantifier), self.rows()?);
        }
        let summary = self.summary()?;
        if summary.rows == 0 {
            return Ok(truth(quantifier == Quantifier::All));
        }
        let all = quantifier == Quantifier::All;
        match op {
            CompOp::Eq | CompOp::Ne => {
                // = SOME and <> ALL ask whether a row agrees; = ALL and <> SOME
                // whether every row does.
                let t = if (op == CompOp::Eq) == all { summary.all_equal(xs) } else { self.some_equal(xs, &summary)? };
                Ok(if op == CompOp::Eq { t } else { negated(t) })
            }
            CompOp::Lt | CompOp::Le => Ok(summary.ordered(xs, (true, op == CompOp::Le), all)),
            CompOp::Gt | CompOp::Ge => Ok(summary.ordered(xs, (false, op == CompOp::Ge), all)),
        }
    }

    /// "R = SOME S": true if a row equals R, else unknown if a row agrees
    /// with it where neither is null, else false. A row of no null is
    /// looked for first, as the patterns are in order of their nulls.
    fn some_equal(&self, xs: &[Value], summary: &Summary) -> Result<Truth> {
        let mut agreeing = lock_builtin(&self.agreeing);
        let Agreeing { whole, nulled } = &mut *agreeing;
        let of_r = nulls(xs);
        let hashes = if of_r.contains(&true) { nulled.entry(of_r.clone()).or_default() } else { whole };
        hashes.resize_with(summary.patterns.len(), || None);
        for (pattern, hash) in summary.patterns.iter().zip(hashes.iter_mut()) {
            let on: Vec<usize> =
                (0..xs.len()).filter(|&i| !of_r.get(i).copied().unwrap_or(true) && !pattern.get(i).copied().unwrap_or(true)).collect();
            if self.agrees(hash, xs, (pattern, &on))? {
                return Ok(if on.len() == xs.len() { Truth::True } else { Truth::Unknown });
            }
        }
        Ok(Truth::False)
    }

    /// Whether a row of nulls `pattern` has the values of `xs` on columns
    /// `on`, of `hash`, of those rows on those columns, made the first time.
    fn agrees(&self, hash: &mut Option<Built>, xs: &[Value], (pattern, on): (&[bool], &[usize])) -> Result<bool> {
        if on.is_empty() {
            return Ok(true);
        }
        let Some(key) = on.iter().map(|&i| xs.get(i).and_then(bucket)).collect::<Option<Vec<Bucket>>>() else { return Ok(false) };
        if hash.is_none() {
            let of_pattern = pattern.to_vec();
            let rows = self.rows()?.filter(move |r| !r.as_ref().is_ok_and(|row| nulls(row) != of_pattern));
            let keys = on.iter().map(|&i| (i, i)).collect();
            let Right::Held(built) = Built::of(rows, keys, (&mut Work::new(&self.db), Full::Table))? else {
                return Err(super::broken("a subquery's rows were partitioned"));
            };
            *hash = Some(built);
        }
        let Some(built) = hash.as_ref() else { return Err(super::broken("a hash of a subquery's rows is gone")) };
        let equal = |r: &Row| on.iter().all(|&i| xs.get(i).zip(r.get(i)).is_some_and(|(x, y)| compare(x, y) == Some(Ordering::Equal)));
        Ok(built.bucket_rows(&key)?.iter().any(equal))
    }

    /// Its summary, made the first time it is asked for.
    fn summary(&self) -> Result<Arc<Summary>> {
        let mut summary = lock_builtin(&self.summary);
        if let Some(made) = &*summary {
            return Ok(Arc::clone(made));
        }
        let mut made = Summary { rows: 0, patterns: Vec::new(), columns: Vec::new(), prefixes: Vec::new() };
        let mut patterns = HashSet::new();
        for r in self.rows()? {
            let row = r?;
            made.add(&row);
            patterns.insert(nulls(&row));
        }
        made.patterns = patterns.into_iter().collect();
        made.patterns.sort_by_key(|p| p.iter().filter(|&&null| null).count());
        let made = Arc::new(made);
        *summary = Some(Arc::clone(&made));
        Ok(made)
    }
}

/// Which values of `row` are null.
fn nulls(row: &[Value]) -> Vec<bool> {
    row.iter().map(Value::is_null).collect()
}

/// The order of rows `a` and `b` of no null, value by value in turn.
fn order(a: &[Value], b: &[Value]) -> Ordering {
    a.iter().zip(b).map(|(x, y)| compare(x, y).unwrap_or(Ordering::Equal)).find(|o| o.is_ne()).unwrap_or(Ordering::Equal)
}

/// `bounds` widened to hold `v`, of the order `cmp`.
fn widen<T: Clone>(bounds: &mut Option<(T, T)>, v: &T, cmp: impl Fn(&T, &T) -> Ordering) {
    match bounds {
        Some((least, greatest)) => {
            if cmp(v, least) == Ordering::Less {
                *least = v.clone();
            }
            if cmp(v, greatest) == Ordering::Greater {
                *greatest = v.clone();
            }
        }
        None => *bounds = Some((v.clone(), v.clone())),
    }
}

/// True if `holds`, else false.
fn truth(holds: bool) -> Truth {
    if holds { Truth::True } else { Truth::False }
}

/// [X3.135-1986 §5.18 GR2] NOT of `t`.
fn negated(t: Truth) -> Truth {
    match t {
        Truth::True => Truth::False,
        Truth::False => Truth::True,
        Truth::Unknown => Truth::Unknown,
    }
}

impl Summary {
    /// Adds `row`.
    fn add(&mut self, row: &Row) {
        self.rows = self.rows.saturating_add(1);
        if self.columns.is_empty() {
            self.columns = vec![None; row.len()];
            self.prefixes = vec![None; row.len().saturating_add(1)];
        }
        for (bounds, v) in self.columns.iter_mut().zip(row) {
            if !v.is_null() {
                widen(bounds, v, |a, b| compare(a, b).unwrap_or(Ordering::Equal));
            }
        }
        let first_null = row.iter().position(Value::is_null).unwrap_or(row.len());
        if let Some(bounds) = self.prefixes.get_mut(first_null) {
            widen(bounds, &row.get(..first_null).unwrap_or_default().to_vec(), |a, b| order(a, b));
        }
    }

    /// "R = ALL S": true if every row equals R; false if one differs from
    /// it where neither is null, so a column's least or greatest value not
    /// null differs from R's; else unknown.
    fn all_equal(&self, xs: &[Value]) -> Truth {
        let same = |(bounds, x): (&Option<(Value, Value)>, &Value)| {
            x.is_null()
                || bounds.as_ref().is_none_or(|(l, g)| compare(l, x) == Some(Ordering::Equal) && compare(g, x) == Some(Ordering::Equal))
        };
        if !self.columns.iter().zip(xs).all(same) {
            return Truth::False;
        }
        let nulled = xs.iter().any(Value::is_null) || self.patterns.iter().flatten().any(|&null| null);
        if nulled { Truth::Unknown } else { Truth::True }
    }

    /// "R op ALL S" if `all`, else "R op SOME S", op < or <= if `less`,
    /// else > or >=, and <= or >= if `or_equal`: of what its rows give, by
    /// the place of their first null.
    fn ordered(&self, xs: &[Value], (less, or_equal): (bool, bool), all: bool) -> Truth {
        let at_null = xs.iter().position(Value::is_null).unwrap_or(xs.len());
        // Whether a row gives "R < r", "R > r", "R = r" and unknown.
        let (mut below, mut above, mut equal, mut unknown) = (false, false, false, false);
        for (place, bounds) in self.prefixes.iter().enumerate() {
            let Some((least, greatest)) = bounds else { continue };
            let k = at_null.min(place);
            let r = xs.get(..k).unwrap_or_default();
            let (to_least, to_greatest) = (order(r, least.get(..k).unwrap_or_default()), order(r, greatest.get(..k).unwrap_or_default()));
            below |= to_greatest == Ordering::Less;
            above |= to_least == Ordering::Greater;
            if to_least == Ordering::Equal || to_greatest == Ordering::Equal {
                if k == xs.len() { equal = true } else { unknown = true }
            }
        }
        let some_true = (below && less) || (above && !less) || (equal && or_equal);
        let some_false = (below && !less) || (above && less) || (equal && !or_equal);
        match (all, some_false, some_true, unknown) {
            (true, true, _, _) | (false, _, false, false) => Truth::False,
            (true, false, _, true) | (false, _, false, true) => Truth::Unknown,
            (true, false, _, false) | (false, _, true, _) => Truth::True,
        }
    }
}

// Unit tests: the answers against comparing R with every row (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests;
