//! \[EXTENSION\] The plan-space test (RULES.md D377): each query of the
//! join-order workload, and of the random queries (`random_queries.rs`),
//! run under
//! cardinalities given in place of the planner's estimates, drawn from a
//! fixed seed, which force other join orders and methods, and each gives
//! the rows it gives with none given [X3.135-1986 §5.20 GR2b, §5.22]. A
//! plan is right whatever rows the planner believes, so every plan it may
//! choose is checked against another, as Rigger and Su's differential
//! testing checks one engine against a reference.

use turborel::db::Session;
use turborel::error::Code;
use turborel::params::Params;
use turborel::physical::Injected;

use crate::common::{Rng, exec, rows, sorted};
use crate::join_order_workload::imdb;
use crate::random_queries::{DEFAULT_SEED, Outcome, agree, query, schema};

/// The random queries run, fewer than their own test runs, as each runs
/// under three sets of cardinalities.
const RANDOM_QUERIES: u64 = 120;
/// The correlation names the random queries give their tables.
const RANDOM_NAMES: [&str; 3] = ["X", "Y", "Z"];
/// The pairs of them their joins' conditions join.
const RANDOM_PAIRS: [(usize, usize); 2] = [(0, 1), (1, 2)];

/// The milliseconds a query of the workload may run under cardinalities
/// given, a guard against a hang alone: every plan they make runs well
/// within it.
const TIMEOUT_MS: u64 = 60_000;

/// Titles of the small database.
const TITLES: u64 = 100;
/// Titles of the database of a test of one query's plans.
const FEW_TITLES: u64 = 20;
/// The seeds of the cardinalities given, one run of each query each.
const SEEDS: [u64; 2] = [0x5EED_0001, 0x5EED_0002];
/// The powers of ten a set's rows are drawn from, -3 to 3, the titles'
/// rows multiplied by one of them: each set's rows under- and
/// overestimated by up to a thousand times.
const POWERS: [i32; 7] = [-3, -2, -1, 0, 1, 2, 3];
/// The rows a set given is drawn from, the database's titles.
const BASE_ROWS: f64 = 100.0_f64;

/// splitmix64, from a seed.
struct Draws(u64);

impl Draws {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30_u32)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27_u32)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31_u32)
    }

    /// Rows `BASE_ROWS` multiplied by 10^k, k in -3..=3, at least one.
    fn rows(&mut self) -> f64 {
        let at = usize::try_from(self.next() % 7).unwrap_or_default();
        (BASE_ROWS * 10.0_f64.powi(POWERS.get(at).copied().unwrap_or_default())).max(1.0_f64)
    }
}

/// Cardinalities drawn from `seed` for every relation of `names` and
/// each of `pairs` of them, each named as EXPLAIN names it. A pair no
/// condition joins is left out.
fn drawn(names: &[String], pairs: &[(usize, usize)], seed: u64) -> Injected {
    let mut draws = Draws(seed);
    let mut given = Injected::new();
    for a in names {
        given = given.with(&[a], draws.rows());
    }
    for &(a, b) in pairs {
        given = given.with(&[&names[a], &names[b]], draws.rows());
    }
    given
}

/// Each pair of `sql`'s relations a conjunct of its WHERE joins.
fn joined_pairs(sql: &str) -> Vec<(usize, usize)> {
    let relations = imdb::relations(sql);
    let mut pairs = Vec::new();
    for conjunct in imdb::conjuncts(sql) {
        let reads: Vec<usize> = (0..relations.len()).filter(|&r| imdb::reads(&conjunct, relations[r].1)).collect();
        if let [a, b] = reads[..]
            && !pairs.contains(&(a, b))
        {
            pairs.push((a, b));
        }
    }
    pairs
}

/// The plan and the rows of `sql` under `given`.
fn under(s: &Session, sql: &str, given: Injected) -> (String, Vec<String>) {
    s.with_cardinalities(given);
    (s.physical_plan(sql, &Params::new()).unwrap(), sorted(s, sql))
}

/// The plan and the outcome of `sql` under `given`.
fn tried(s: &Session, sql: &str, given: Injected) -> (String, Result<Vec<String>, Code>) {
    s.with_cardinalities(given);
    let plan = s.physical_plan(sql, &Params::new()).unwrap();
    let found = rows(s, sql).map(|mut r| {
        r.sort();
        r
    });
    s.rollback().unwrap();
    (plan, found)
}

/// Verifies: [EXTENSION] RULES.md D377, D387, D302: every query of the
/// workload gives its rows under cardinalities drawn from each seed,
/// under- and overestimates, each plan they make in well under a minute;
/// and they plan a third of the queries otherwise at least once.
#[test]
fn every_plan_the_workload_may_take_gives_its_rows() {
    let db = imdb::load(TITLES);
    let s = db.session("IMDB");
    let mut changed = 0;
    for (name, sql) in imdb::queries() {
        let names: Vec<String> = imdb::relations(sql).iter().map(|(_, alias)| alias.to_uppercase()).collect();
        assert!(names.len() >= 2, "{name}");
        let pairs = joined_pairs(sql);
        let (plan, rows) = under(&s, sql, Injected::new());
        exec(&s, &format!("SET STATEMENT.TIMEOUT_MS = {TIMEOUT_MS}")).unwrap();
        let mut other = false;
        for seed in SEEDS {
            let (injected, found) = tried(&s, sql, drawn(&names, &pairs, seed));
            assert_eq!(found, Ok(rows.clone()), "{name} under seed {seed:#x}:\n{injected}");
            other |= injected != plan;
        }
        exec(&s, "SET STATEMENT.TIMEOUT_MS = 0").unwrap();
        changed += usize::from(other);
    }
    s.with_cardinalities(Injected::new());
    assert!(changed >= 33, "{changed} of 100 queries planned otherwise");
}

/// Verifies: [EXTENSION] RULES.md D377: a join of three tables and a self
/// join, their sets given each way round, give the rows they give with
/// none given, and a self join's two correlation names are two relations.
#[test]
fn small_joins_give_their_rows_under_any_cardinalities() {
    let db = imdb::load(TITLES);
    let s = db.session("IMDB");
    let queries = [
        (
            "SELECT t.id, mk.keyword_id FROM title AS t, movie_keyword AS mk, keyword AS k\n\
             WHERE mk.movie_id = t.id\n  AND mk.keyword_id = k.id\n  AND t.production_year > 2000",
            ["T", "MK", "K"],
        ),
        (
            "SELECT a.id, b.id FROM title AS a, title AS b, kind_type AS kt\n\
             WHERE a.kind_id = b.kind_id\n  AND b.kind_id = kt.id\n  AND a.id < 40",
            ["A", "B", "KT"],
        ),
    ];
    for (sql, names) in queries {
        let names: Vec<String> = names.iter().map(|&n| n.to_owned()).collect();
        let pairs = joined_pairs(sql);
        assert_eq!(pairs.len(), 2, "{sql}");
        let (_, rows) = under(&s, sql, Injected::new());
        assert!(!rows.is_empty(), "{sql}");
        for seed in 0..16_u64 {
            assert_eq!(under(&s, sql, drawn(&names, &pairs, seed)).1, rows, "{sql} under seed {seed}");
        }
    }
}

/// Verifies: [EXTENSION] RULES.md D377, D15: each random query of joins,
/// outer and full among them, subqueries, groups and set operators gives
/// the outcome under cardinalities drawn from its seed and the next that
/// it gives with none given: its rows, or its error.
#[test]
fn random_queries_answer_alike_under_any_cardinalities() {
    let db = schema();
    let s = db.session("Q");
    let names: Vec<String> = RANDOM_NAMES.iter().map(|&n| n.to_owned()).collect();
    let mut seeds = Rng::new(DEFAULT_SEED);
    for _ in 0..RANDOM_QUERIES {
        let own = seeds.next();
        let sql = query(&mut Rng::new(own));
        s.with_cardinalities(Injected::new());
        let alone: Outcome = rows(&s, &sql);
        s.rollback().unwrap();
        for seed in [own, own.wrapping_add(1)] {
            s.with_cardinalities(drawn(&names, &RANDOM_PAIRS, seed));
            let found = rows(&s, &sql);
            s.rollback().unwrap();
            assert!(agree(&sql, &alone, &found), "seed {own:#x}, drawn from {seed:#x}: {sql}\n{alone:?}\n{found:?}");
        }
    }
}

/// The query of the regression test of RULES.md D387: two relations
/// joined only through a third, adjacent in FROM.
const IMPLIED: &str = "SELECT akt.id, mi.id FROM aka_title AS akt, movie_info AS mi, title AS t\n\
     WHERE akt.movie_id = t.id\n  AND mi.movie_id = t.id";

/// Verifies: [DESIGN] RULES.md D387, D302: two relations of one class of
/// equalities, though no conjunct reads both, joined first as their rows
/// given make cheapest, are joined by the key their class implies, never
/// by a product, and give the rows they give with none given, in a time a
/// product of them would pass.
#[test]
fn relations_of_one_class_join_by_its_implied_key() {
    let db = imdb::load(TITLES);
    let s = db.session("IMDB");
    let (_, rows) = under(&s, IMPLIED, Injected::new());
    assert_ne!(rows, Vec::<String>::new());
    let given = Injected::new().with(&["T"], 1.0e7_f64).with(&["AKT", "MI"], 1.0_f64);
    exec(&s, "SET STATEMENT.TIMEOUT_MS = 20000").unwrap();
    let (plan, found) = tried(&s, IMPLIED, given);
    exec(&s, "SET STATEMENT.TIMEOUT_MS = 0").unwrap();
    s.with_cardinalities(Injected::new());
    assert!(!plan.contains("Cartesian product"), "{plan}");
    let keyed = |l: &str| l.contains("MI.MOVIE_ID = AKT.MOVIE_ID") || l.contains("AKT.MOVIE_ID = MI.MOVIE_ID");
    assert!(plan.lines().any(|l| l.contains("Join") && keyed(l) && !l.contains("T.ID")), "{plan}");
    assert_eq!(found, Ok(rows), "{plan}");
}

/// Verifies: [DESIGN] RULES.md D660, D377: a grouping over joins is
/// grouped first at most once, never again within its own plan grouped
/// first, under any cardinalities: query w020 of the workload under the
/// cardinalities of seed 0x5EED_0001, which once grouped its plan first
/// again and again, each part of a part, gives its rows, its grouping
/// shown grouped first on one line at most.
#[test]
fn a_grouping_is_grouped_first_once_under_any_cardinalities() {
    // The plan is made by the cardinalities given, not the data, so few
    // titles keep the test short.
    let db = imdb::load(FEW_TITLES);
    let s = db.session("IMDB");
    let (_, sql) = imdb::queries().into_iter().find(|(name, _)| *name == "w020").unwrap();
    let names: Vec<String> = imdb::relations(sql).iter().map(|(_, alias)| alias.to_uppercase()).collect();
    let (_, rows) = under(&s, sql, Injected::new());
    for seed in SEEDS {
        let (plan, found) = tried(&s, sql, drawn(&names, &joined_pairs(sql), seed));
        assert!(plan.lines().all(|l| l.matches("(eager:").count() <= 1), "seed {seed:#x}:\n{plan}");
        assert!(!plan.contains("PARTIAL PARTIAL") && !plan.contains("MERGED MERGED"), "seed {seed:#x}:\n{plan}");
        assert_eq!(found, Ok(rows.clone()), "seed {seed:#x}:\n{plan}");
    }
    s.with_cardinalities(Injected::new());
}
