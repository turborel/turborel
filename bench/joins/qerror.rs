//! The q-error report of the join-order workload (RULES.md D376, D377),
//! two modes of `bench/joins.rs` run by `cargo bench --bench joins --
//! estimates` and `-- true-cardinalities`, each in place of the timings.
//!
//! `estimates` runs each query by EXPLAIN ANALYZE (`Session::analyzed`)
//! and finds, for each join operator, the q-error of its estimate,
//! max(e/a, a/e) with both at least one row, as Moerkotte et al. define
//! it. They are grouped by the tables below the join and shown as Leis et
//! al., "How Good Are Query Optimizers, Really?" (2015), Fig. 3 does:
//! median, 90th and 95th percentile and largest, and the share of
//! estimates below the true rows.
//!
//! `true-cardinalities` counts each connected set of each query's
//! relations, up to PLANNER.DP_BUDGET's default of them, by `SELECT
//! COUNT(*)` of their join with their filters, gives the planner those
//! rows in place of its estimates (`Session::with_cardinalities`), and
//! times the workload run under them against it run on estimates, as that
//! paper's §4 does. Both print figures and assert nothing (R-BENCH-2).

use std::collections::{BTreeMap, BTreeSet};

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::physical::Injected;

use crate::harness::{Bench, Case, note};
use crate::imdb;

/// The mode that reports the estimates' q-errors.
pub const ESTIMATES: &str = "estimates";
/// The mode that runs the workload on its true cardinalities.
pub const TRUE_CARDINALITIES: &str = "true-cardinalities";
/// The connected sets counted of one query at most: PLANNER.DP_BUDGET's
/// default.
const BUDGET: usize = 10_000;
/// The percentiles shown, the median, 90th, 95th and the largest.
const PERCENTILES: [usize; 4] = [50, 90, 95, 100];

/// Runs the mode named among the arguments, if one is: whether one was.
pub fn run(titles: u64) -> bool {
    let args: Vec<String> = std::env::args().collect();
    let wants = |mode: &str| args.iter().any(|a| a == mode);
    if wants(ESTIMATES) {
        estimates(&imdb::load(titles));
    } else if wants(TRUE_CARDINALITIES) {
        true_cardinalities(&imdb::load(titles));
    } else {
        return false;
    }
    true
}

/// The q-error of each join operator of each query, by the tables below
/// it, each with whether its estimate was below the true rows.
fn estimates(db: &Database) {
    let s = db.session("IMDB");
    let mut found: BTreeMap<usize, Vec<(f64, bool)>> = BTreeMap::new();
    for (_, sql) in imdb::queries() {
        let analyzed = s.analyzed(sql, &Params::new()).unwrap();
        s.commit().unwrap();
        for op in analyzed.operators.iter().filter(|o| o.join && o.loops > 0) {
            // The estimate is of one opening, the rows of all.
            let actual = (rows(op.rows) / rows(op.loops)).max(1.0_f64);
            let estimate = op.estimate.unwrap_or(1.0_f64).max(1.0_f64);
            found.entry(op.tables).or_default().push(((estimate / actual).max(actual / estimate), estimate < actual));
        }
    }
    note("q-error of each join's estimate, by the tables below it");
    note("tables  joins  median  90th  95th  max  under");
    for (tables, mut errors) in found {
        errors.sort_by(|a, b| a.0.total_cmp(&b.0));
        let shown: Vec<String> = PERCENTILES.iter().map(|&p| format!("{:.1}", errors[(errors.len() - 1) * p / 100].0)).collect();
        let under = errors.iter().filter(|e| e.1).count() * 100 / errors.len();
        note(&format!("{tables:>6}  {:>5}  {}  {under}%", errors.len(), shown.join("  ")));
    }
}

/// `n` as a float, at most `u32::MAX`, more than any query here counts.
fn rows(n: u64) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

/// The workload run on estimates and on the true rows of every connected
/// set of each query's relations.
fn true_cardinalities(db: &Database) {
    let s = db.session("IMDB");
    let queries = imdb::queries();
    let given: Vec<Injected> = queries.iter().map(|(_, sql)| counted(&s, sql)).collect();
    let bench = Bench::new("joins");
    let run = |injected: bool| {
        for ((_, sql), true_rows) in queries.iter().zip(&given) {
            s.with_cardinalities(if injected { true_rows.clone() } else { Injected::new() });
            let mut cursor = s.open_cursor(sql).unwrap();
            while cursor.fetch().unwrap().is_some() {}
            drop(cursor);
            s.commit().unwrap();
        }
        queries.len() as u64
    };
    bench.measure(Case::new("join-order workload, 100 queries run, estimated", "queries"), || run(false));
    bench.measure(Case::new("join-order workload, 100 queries run, true cardinalities", "queries"), || run(true));
    s.with_cardinalities(Injected::new());
}

/// The true rows of each connected set of `sql`'s relations, by the
/// correlation names EXPLAIN shows.
fn counted(s: &Session, sql: &str) -> Injected {
    let relations = imdb::relations(sql);
    let conjuncts: Vec<(String, BTreeSet<usize>)> = imdb::conjuncts(sql)
        .into_iter()
        .map(|c| {
            let reads = (0..relations.len()).filter(|&r| imdb::reads(&c, relations[r].1)).collect();
            (c, reads)
        })
        .collect();
    let mut given = Injected::new();
    for set in connected(relations.len(), &conjuncts) {
        let from: Vec<String> = set.iter().map(|&r| format!("{} AS {}", relations[r].0, relations[r].1)).collect();
        let filters: Vec<&str> = conjuncts.iter().filter(|(_, reads)| reads.is_subset(&set)).map(|(c, _)| c.as_str()).collect();
        let filter = if filters.is_empty() { String::new() } else { format!(" WHERE {}", filters.join(" AND ")) };
        let mut cursor = s.open_cursor(&format!("SELECT COUNT(*) FROM {}{filter}", from.join(", "))).unwrap();
        let count = cursor.fetch().unwrap().unwrap()[0].clone();
        drop(cursor);
        s.commit().unwrap();
        let names: Vec<String> = set.iter().map(|&r| relations[r].1.to_uppercase()).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        given = given.with(&names, count.to_string().parse::<f64>().unwrap());
    }
    given
}

/// The connected sets of `n` relations joined by `conjuncts` of two or
/// more, each relation alone among them, at most `BUDGET`.
fn connected(n: usize, conjuncts: &[(String, BTreeSet<usize>)]) -> BTreeSet<BTreeSet<usize>> {
    let edges: Vec<&BTreeSet<usize>> = conjuncts.iter().map(|(_, reads)| reads).filter(|r| r.len() >= 2).collect();
    let mut found: BTreeSet<BTreeSet<usize>> = (0..n).map(|r| BTreeSet::from([r])).collect();
    let mut grown: Vec<BTreeSet<usize>> = found.iter().cloned().collect();
    while let Some(set) = grown.pop() {
        for edge in &edges {
            if found.len() >= BUDGET || edge.is_disjoint(&set) || edge.is_subset(&set) {
                continue;
            }
            let bigger: BTreeSet<usize> = set.union(edge).copied().collect();
            if found.insert(bigger.clone()) {
                grown.push(bigger);
            }
        }
    }
    found
}
