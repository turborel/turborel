//! [X3.135-1986 §5.11, §5.13, §5.16, §5.17, §5.24] Running subqueries and
//! applying the predicates over them. Split from exec.rs (RULES.md
//! R-SIZE-2).
//!
//! A subquery is "effectively executed for each row" (or group) its
//! predicate is applied to [X3.135-1986 §5.21 GR3, §5.23 GR4]. \[DESIGN\]
//! Its result depends only on the values of its outer references, so the
//! `Runner` keeps the rows it gave for each, and runs it once for each
//! distinct set of values: once in all, for a subquery with none. Each run
//! binds the values into the subquery's plan (`Subquery::bound`), so they
//! are constants, and optimizes and plans it as any query: a correlated
//! comparison becomes one an index can answer.
//!
//! A subquery run EXECUTOR.DECORRELATE_AFTER times, for as many sets of values,
//! is run once more instead without its equalities with outer references,
//! if it can be (`Subquery::decorrelated`): its rows are kept by those
//! columns' values (`value::bucket`), and the rows for any outer values
//! are those whose columns equal them [X3.135-1986 §5.11]. Until then, a
//! run per set of values, an index probe if it has one, costs less.
//!
//! \[DESIGN\] RULES.md D38: a subquery's rows outgrowing the work area are
//! spilled to a run (`exec/join/rescan.rs`), and a decorrelated subquery's
//! to a table keyed by bucket (`exec/join.rs`); the rows of the runs kept
//! for their outer values are kept while they fit the work area in all.

mod quantified;

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use super::join::{Built, Full, Rescanned, Right};
use super::work::{Bag, Work};
use super::{RowIter, broken, execute};
use crate::db::State;
use crate::error::Result;
use crate::logical::{Decorrelated, LogicalPlan, Subquery};
use crate::optimizer::RuleContext;
use crate::physical::PhysicalPlanner;
use crate::storage::Row;
use crate::sync::lock_builtin;
use crate::txn::TxId;
use crate::value::{Bucket, NormKey, Value, bucket, compare};

/// \[DESIGN\] The default of EXECUTOR.DECORRELATE_AFTER: the runs of a
/// subquery, for as many sets of outer values, after which it is
/// decorrelated, if it can be.
pub const DEFAULT_DECORRELATE_AFTER: usize = 8;

/// \[DESIGN\] The rows a subquery gave, read from the first each time: in
/// memory, or, once they outgrew the work area, in a run of its spill area
/// (RULES.md D38).
#[derive(Clone)]
pub struct SubqueryRows {
    rows: Arc<Mutex<Rescanned>>,
    /// The database, in whose work areas what is made of the rows is kept.
    db: Arc<State>,
    /// Whether the rows spilled.
    spilled: bool,
    /// What the comparisons of a value with every row ask of them, made the
    /// first time one does (`subquery/quantified.rs`).
    summary: Arc<Mutex<Option<Arc<quantified::Summary>>>>,
    agreeing: Arc<Mutex<quantified::Agreeing>>,
}

impl SubqueryRows {
    /// The rows `rows`, of a spill area if `spilled`, of `db`.
    fn of(rows: Rescanned, db: &Arc<State>) -> SubqueryRows {
        let spilled = matches!(rows, Rescanned::Spilled(_));
        let (summary, agreeing) = (Arc::new(Mutex::new(None)), Arc::new(Mutex::new(quantified::Agreeing::default())));
        SubqueryRows { rows: Arc::new(Mutex::new(rows)), db: Arc::clone(db), spilled, summary, agreeing }
    }

    /// Its rows, from the first, in order.
    ///
    /// # Errors
    ///
    /// The spill area's, reading them.
    pub fn rows(&self) -> Result<RowIter> {
        lock_builtin(&self.rows).pass()
    }

    /// Its first `n` rows, or as many as it has.
    ///
    /// # Errors
    ///
    /// As `rows`.
    pub fn first(&self, n: usize) -> Result<Vec<Row>> {
        self.rows()?.take(n).collect()
    }

    /// [ISO/IEC 9075:1992 §8.9 GR2] Whether no two of its rows, neither with
    /// a null, are equal; the keys seen kept in memory, or, of a result
    /// that spilled, in a bag of its database's work area, which spills too.
    ///
    /// # Errors
    ///
    /// As `rows`; the work area's.
    pub fn unique(&self) -> Result<bool> {
        let rows = self.rows()?;
        if self.spilled {
            let mut seen = Bag::new(Work::new(&self.db));
            return super::eval::unique(rows, &mut |key| seen.add_new(&key));
        }
        let mut seen = HashSet::new();
        super::eval::unique(rows, &mut |key| Ok(seen.insert(key)))
    }
}

/// \[DESIGN\] What runs the subqueries of the conditions evaluated.
pub trait Subqueries {
    /// The rows `subquery` gives when its outer references have `outer`;
    /// at most `limit` of them, if given.
    ///
    /// # Errors
    ///
    /// The subquery's own.
    fn rows(&self, subquery: &Subquery, outer: &[Value], limit: Option<usize>) -> Result<SubqueryRows>;
}

/// Where no subquery can be: refuses to run one (SQLCODE -910).
pub struct NoSubqueries;

impl Subqueries for NoSubqueries {
    fn rows(&self, _: &Subquery, _: &[Value], _: Option<usize>) -> Result<SubqueryRows> {
        Err(broken("a subquery where none can be run"))
    }
}

/// The key a subquery's rows are kept under: the subquery, by address, and
/// its outer values as compared [X3.135-1986 §5.11 GR7].
type Run = (usize, Vec<NormKey>);

/// The rows of a decorrelated subquery: each, its subquery's columns
/// first, by the buckets of its key columns' values, in memory or in a
/// table of the spill area (`exec/join.rs`); rows with a null key are
/// none's.
struct Keyed {
    how: Decorrelated,
    rows: Mutex<Built>,
}

/// Whether a subquery was decorrelated.
#[derive(Default, Clone)]
enum Decorrelation {
    /// Not yet tried.
    #[default]
    Untried,
    /// It cannot be.
    Refused,
    /// Its rows by key.
    Keyed(Arc<Keyed>),
}

/// What the runner knows of a subquery: how many times it ran, and whether
/// it was decorrelated.
#[derive(Default)]
struct Seen {
    runs: usize,
    decorrelation: Decorrelation,
}

/// \[DESIGN\] Runs subqueries for transaction `tx`, keeping their rows (see
/// the module's documentation): those of each run while what it keeps in
/// memory fits the work area, a spilled run's always, as it holds them on
/// disk.
pub struct Runner {
    db: Arc<State>,
    tx: TxId,
    done: RefCell<HashMap<Run, SubqueryRows>>,
    /// The bytes the rows kept in `done` hold in memory.
    kept: Cell<usize>,
    seen: RefCell<HashMap<usize, Seen>>,
}

impl Runner {
    /// A runner for transaction `tx` in `db`.
    pub fn new(db: &Arc<State>, tx: TxId) -> Runner {
        let (done, seen) = (RefCell::new(HashMap::new()), RefCell::new(HashMap::new()));
        Runner { db: Arc::clone(db), tx, done, kept: Cell::new(0), seen }
    }

    /// The rows of `plan`, optimized and planned.
    fn run(&self, plan: LogicalPlan) -> RowIter {
        let (optimized, _) = self.db.optimizer.optimize(plan, &RuleContext { db: &self.db });
        execute(PhysicalPlanner::new(&self.db).plan(&optimized), &self.db, self.tx)
    }

    /// The rows of `subquery`, at `at`, by key, once it ran
    /// EXECUTOR.DECORRELATE_AFTER times, if it can be decorrelated; this run is
    /// counted.
    fn keyed(&self, subquery: &Subquery, at: usize) -> Result<Option<Arc<Keyed>>> {
        let runs = {
            let mut seen = self.seen.borrow_mut();
            let s = seen.entry(at).or_default();
            match &s.decorrelation {
                Decorrelation::Keyed(keyed) => return Ok(Some(Arc::clone(keyed))),
                Decorrelation::Refused => return Ok(None),
                Decorrelation::Untried => {}
            }
            s.runs = s.runs.saturating_add(1);
            s.runs
        };
        if runs <= self.db.settings.count(crate::settings::DECORRELATE_AFTER) {
            return Ok(None);
        }
        let keyed = match subquery.decorrelated() {
            None => None,
            Some(how) => {
                // Its key columns follow its own, each equal to an outer slot's value.
                let keys = (0..how.slots.len()).map(|n| (n, how.width.saturating_add(n))).collect();
                let rows = self.run(how.plan.clone());
                let Right::Held(rows) = Built::of(rows, keys, (&mut Work::new(&self.db), Full::Table))? else {
                    return Err(broken("a subquery's rows were partitioned"));
                };
                Some(Arc::new(Keyed { how, rows: Mutex::new(rows) }))
            }
        };
        let decorrelation = keyed.as_ref().map_or(Decorrelation::Refused, |k| Decorrelation::Keyed(Arc::clone(k)));
        self.seen.borrow_mut().entry(at).or_default().decorrelation = decorrelation;
        Ok(keyed)
    }

    /// The rows `rows`, at most `limit`, in memory or spilled.
    fn gathered(&self, rows: RowIter, limit: Option<usize>) -> Result<SubqueryRows> {
        let rows: RowIter = match limit {
            Some(n) => Box::new(rows.take(n)),
            None => rows,
        };
        let (rows, _) = Rescanned::of(rows, &mut Work::new(&self.db))?;
        Ok(SubqueryRows::of(rows, &self.db))
    }
}

/// The rows of a decorrelated subquery for `outer` values: those whose key
/// columns equal the values of their slots, at most `limit`; none if one is
/// null [X3.135-1986 §5.11 GR2].
fn rows_for(keyed: &Keyed, outer: &[Value], limit: Option<usize>) -> Result<Vec<Row>> {
    let values: Option<Vec<&Value>> = keyed.how.slots.iter().map(|&slot| outer.get(slot)).collect();
    let Some(values) = values else { return Ok(Vec::new()) };
    let Some(key) = values.iter().map(|v| bucket(v)).collect::<Option<Vec<Bucket>>>() else { return Ok(Vec::new()) };
    let equal = |row: &Row| {
        let columns = row.get(keyed.how.width..).unwrap_or_default();
        columns.len() == values.len() && columns.iter().zip(&values).all(|(c, v)| compare(c, v) == Some(Ordering::Equal))
    };
    let width = keyed.how.width;
    let found = lock_builtin(&keyed.rows).bucket(&key)?.into_iter().filter(equal);
    Ok(found.take(limit.unwrap_or(usize::MAX)).map(|row| row.get(..width).unwrap_or_default().to_vec()).collect())
}

impl Subqueries for Runner {
    fn rows(&self, subquery: &Subquery, outer: &[Value], limit: Option<usize>) -> Result<SubqueryRows> {
        let at = std::ptr::from_ref(subquery).addr();
        let key = (at, outer.iter().map(Value::norm).collect());
        if let Some(rows) = self.done.borrow().get(&key) {
            return Ok(rows.clone());
        }
        let found = match self.keyed(subquery, at)? {
            Some(keyed) => SubqueryRows::of(Rescanned::Held(Arc::new(rows_for(&keyed, outer, limit)?)), &self.db),
            None => self.gathered(self.run(subquery.bound(outer)), limit)?,
        };
        // Kept while what is kept in memory fits the work area; past that,
        // run again for each set of outer values.
        let bytes = lock_builtin(&found.rows).held_bytes();
        if self.kept.get().saturating_add(bytes) <= Work::new(&self.db).limit() {
            self.kept.set(self.kept.get().saturating_add(bytes));
            self.done.borrow_mut().insert(key, found.clone());
        }
        Ok(found)
    }
}

// Unit tests: the guard where no subquery can be (RULES.md R-TEST-2
// robustness).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::QualifiedName;
    use crate::logical::LogicalPlan;

    /// Verifies: [DESIGN] where no subquery can be run, running one reports
    /// SQLCODE -910 instead of a panic.
    #[test]
    fn no_subqueries_refuses_to_run_one() {
        let table = QualifiedName { auth: "A".into(), ident: "T".into() };
        let s = Subquery { plan: LogicalPlan::Scan { table, names: vec![], needed: vec![] }, outer: vec![] };
        assert_eq!(NoSubqueries.rows(&s, &[], None).err().map(|e| e.code), Some(crate::error::Code::Api));
    }
}
