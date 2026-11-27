//! \[DESIGN\] (RULES.md D1089) What one statement's planning reads of the
//! tables it plans: each table's statistics and rows, and each index's
//! distinct keys, and what a table's sample shows of an expression
//! (D1279), asked the first time a step of the planning wants them and
//! kept for every step after. The optimizer's rules, the physical
//! planner, the parts marked to run in parallel, predicate transfer and
//! what a prepared statement's plan learned is measured from all read
//! them here. The maintenance thread may gather a table's statistics at
//! any moment, and a statement planned partly before and partly after
//! would weigh one plan by two states of one table.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use crate::catalog::QualifiedName;
use crate::db::State;
use crate::hash::xxhash::Xxh3Map;
use crate::index::Index;
use crate::logical::Expr;
use crate::stats::Statistics;

/// One statement's view of its tables' statistics, rows and index keys,
/// each asked once (`Seen::statistics`, `Seen::row_count`,
/// `Seen::index_keys`).
#[derive(Default)]
pub(crate) struct Seen {
    stats: RefCell<Xxh3Map<QualifiedName, Option<Arc<dyn Statistics>>>>,
    rows: RefCell<Xxh3Map<QualifiedName, Option<usize>>>,
    /// Each index's distinct keys, by its name, as its sketch counts them.
    keys: RefCell<HashMap<String, Option<f64>>>,
    /// What a table's sample showed of each expression asked of it
    /// (`cardinality/shown.rs`, RULES.md D1279), by the question's key,
    /// with the expression. It is kept so that no function it calls is
    /// freed, and its address taken by another, while the key made of
    /// it is.
    shown: RefCell<Xxh3Map<u64, (Expr, Option<f64>)>>,
}

impl Seen {
    /// `table`'s statistics in `db`, asked of its engine the first time.
    /// The cache's borrow is released before the engine is asked
    /// (RULES.md R-ERR-4).
    pub(crate) fn statistics(&self, db: &State, table: &QualifiedName) -> Option<Arc<dyn Statistics>> {
        let known = self.stats.borrow().get(table).cloned();
        if let Some(stats) = known {
            return stats;
        }
        let asked = db.statistics(table);
        self.stats.borrow_mut().insert(table.clone(), asked.clone());
        asked
    }

    /// `table`'s rows in `db`, counted by its engine the first time.
    pub(crate) fn row_count(&self, db: &State, table: &QualifiedName) -> Option<usize> {
        let known = self.rows.borrow().get(table).copied();
        if let Some(rows) = known {
            return rows;
        }
        let counted = db.row_count(table);
        self.rows.borrow_mut().insert(table.clone(), counted);
        counted
    }

    /// The distinct keys the index `name`, `index`, counts, asked of it
    /// the first time: a sketch's count is an estimate made anew at each
    /// asking.
    pub(crate) fn index_keys(&self, name: &str, index: &dyn Index) -> Option<f64> {
        self.keys_of(name, || index.distinct_keys())
    }

    /// The distinct keys the index `name` of `table`, `index`, counts, as
    /// `index_keys` asks them, its table noted for the maintenance if a
    /// count is due (`State::distinct_keys`).
    pub(crate) fn table_keys(&self, db: &State, (table, name): (&QualifiedName, &str), index: &dyn Index) -> Option<f64> {
        self.keys_of(name, || db.distinct_keys(table, index))
    }

    /// \[DESIGN\] (RULES.md D1279) What a table's sample shows of the
    /// expression `e`, the question of key `key`, found by `find` the
    /// first time. Evaluating the sample's rows costs more than the rest
    /// of a planning, and each plan weighed asks.
    pub(super) fn shown(&self, key: u64, e: &Expr, find: impl FnOnce() -> Option<f64>) -> Option<f64> {
        let known = self.shown.borrow().get(&key).map(|&(_, shown)| shown);
        if let Some(shown) = known {
            return shown;
        }
        let found = find();
        self.shown.borrow_mut().insert(key, (e.clone(), found));
        found
    }

    /// The distinct keys of the index `name`, counted by `count` the first
    /// time.
    fn keys_of(&self, name: &str, count: impl FnOnce() -> Option<f64>) -> Option<f64> {
        let known = self.keys.borrow().get(name).copied();
        if let Some(keys) = known {
            return keys;
        }
        let counted = count();
        self.keys.borrow_mut().insert(name.to_owned(), counted);
        counted
    }
}

/// A database whose jobs run only when asked, of each table `(name, rows,
/// keys)` of `tables` in the schema X, its column K of `keys` values and
/// V of a value a row, none of its statistics gathered; with its state.
#[cfg(test)]
pub(crate) fn ungathered(tables: &[(&str, usize, usize)]) -> (crate::db::Database, Arc<State>) {
    let db = crate::db::Database::new();
    db.set_job_policy(crate::threads::JobPolicy::Manual).unwrap();
    let made: Vec<String> = tables.iter().map(|(t, ..)| format!("CREATE TABLE {t} (K INTEGER NOT NULL, V INTEGER NOT NULL)")).collect();
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION X {}", made.join(" "))).unwrap();
    let s = db.session("X");
    for &(t, rows, keys) in tables {
        let values: Vec<String> = (0..rows).map(|i| format!("({}, {i})", i.checked_rem(keys).unwrap_or(0))).collect();
        for part in values.chunks(500) {
            s.run(&format!("INSERT INTO {t} VALUES {}", part.join(", "))).unwrap();
        }
    }
    s.commit().unwrap();
    let (st, _) = s.optimized("VALUES (1)").unwrap();
    for (t, ..) in tables {
        assert!(st.statistics(&named(t)).is_none(), "{t} gathered before its test asks");
    }
    (db, st)
}

/// The table `t` of the schema X.
#[cfg(test)]
pub(crate) fn named(t: &str) -> QualifiedName {
    QualifiedName { auth: "X".to_owned(), ident: t.to_owned() }
}

/// A scan of the columns K and V of X.`t`.
#[cfg(test)]
pub(crate) fn scanned(t: &str) -> crate::physical::Plan {
    let (rows, cost, order, most) = (0, 0.0_f64, Vec::new(), None);
    crate::physical::Plan::SeqScan { table: named(t), columns: vec![0, 1], rows, cost, order, backward: false, most, located: false }
}

// Unit tests: one statement's statistics asked once (RULES.md R-TEST-1,
// R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D1089: a table's statistics and rows
    /// are asked of the state the first time and kept: none before the
    /// table is gathered stay none after, and the rows counted stay the
    /// rows counted, while a statement that asks afresh sees the
    /// statistics gathered and the rows written since.
    #[test]
    fn a_statement_s_statistics_are_those_it_first_asked() {
        let (db, st) = ungathered(&[("T", 100, 10)]);
        let (seen, t) = (Seen::default(), named("T"));
        assert!(seen.statistics(&st, &t).is_none());
        assert_eq!(seen.row_count(&st, &t), Some(100));
        db.analyze("X.T").unwrap();
        let s = db.session("X");
        s.run("INSERT INTO T VALUES (1, 1)").unwrap();
        s.commit().unwrap();
        assert!(seen.statistics(&st, &t).is_none(), "gathered since the statement asked");
        assert_eq!(seen.row_count(&st, &t), Some(100), "counted as first asked");
        let fresh = Seen::default();
        assert!(fresh.statistics(&st, &t).is_some() && fresh.row_count(&st, &t) == Some(101), "a statement asking afresh");
    }
}
