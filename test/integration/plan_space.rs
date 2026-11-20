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

/// The milliseconds a query of the workload runs under cardinalities
/// given before it gives up. A plan they make bad enough may run for
/// minutes, and gives its rows whatever its speed. About one run in ten
/// gives up, most of them where the join enumeration joins two relations
/// whose columns the query's equalities make equal by a product.
const TIMEOUT_MS: u64 = 1_500;
/// At most one run in this many gives up, a slower machine giving up
/// more.
const GAVE_UP_SHARE: usize = 4;

/// Titles of the small database.
const TITLES: u64 = 100;
/// The seeds of the cardinalities given, one run of each query each.
const SEEDS: [u64; 2] = [0x5EED_0001, 0x5EED_0002];
/// The powers of ten a set's rows are drawn from, 0 to 3, the titles'
/// rows multiplied by one of them.
const POWERS: u64 = 4;
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

    /// Rows `BASE_ROWS` multiplied by 10^k, k in 0..=3.
    fn rows(&mut self) -> f64 {
        BASE_ROWS * 10.0_f64.powi(i32::try_from(self.next() % POWERS).unwrap_or_default())
    }
}

/// Cardinalities drawn from `seed` for every relation of `names` and
/// each of `pairs` of them, each named as EXPLAIN names it. A pair no
/// condition joins is left out, and no set is given fewer rows than the
/// titles. A set given too few makes a product with it look cheap
/// whatever it holds, and such a plan runs for minutes or past the
/// memory a query may take.
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

/// The plan and the outcome of `sql` under `given`, a plan that takes
/// longer than STATEMENT.TIMEOUT_MS or more memory than a query may
/// giving up.
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

/// Verifies: [EXTENSION] RULES.md D377, D302, D308: every query of the
/// workload gives its rows under cardinalities drawn from each seed, or
/// gives up as its plan takes too long or too much memory, which a few do;
/// and they plan a third of the queries otherwise at least once.
#[test]
fn every_plan_the_workload_may_take_gives_its_rows() {
    let db = imdb::load(TITLES);
    let s = db.session("IMDB");
    let (mut changed, mut gave_up) = (0, 0);
    for (name, sql) in imdb::queries() {
        let names: Vec<String> = imdb::relations(sql).iter().map(|(_, alias)| alias.to_uppercase()).collect();
        assert!(names.len() >= 2, "{name}");
        let pairs = joined_pairs(sql);
        let (plan, rows) = under(&s, sql, Injected::new());
        exec(&s, &format!("SET STATEMENT.TIMEOUT_MS = {TIMEOUT_MS}")).unwrap();
        let mut other = false;
        for seed in SEEDS {
            let (injected, found) = tried(&s, sql, drawn(&names, &pairs, seed));
            match found {
                Ok(found) => assert_eq!(found, rows, "{name} under seed {seed:#x}:\n{injected}"),
                Err(Code::Cancelled | Code::Resources) => gave_up += 1,
                Err(code) => panic!("{name} under seed {seed:#x}: {code:?}\n{injected}"),
            }
            other |= injected != plan;
        }
        exec(&s, "SET STATEMENT.TIMEOUT_MS = 0").unwrap();
        changed += usize::from(other);
    }
    s.with_cardinalities(Injected::new());
    assert!(changed >= 33, "{changed} of 100 queries planned otherwise");
    assert!(gave_up * GAVE_UP_SHARE <= 2 * 100, "{gave_up} of 200 runs gave up");
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
