//! Random queries [DESIGN]: a generator of SQL-92 query expressions over a
//! fixed schema, each query run with every optimizer rule on and off, read
//! by batches and row by row (EXECUTOR.BATCHES), the four outcomes
//! compared. The standard governs results, not plans (RULES.md D15): no
//! rule and no way of reading may change a query's rows, their order where
//! ORDER BY sets one, or its error.
//!
//! The generator is pseudo-random from a fixed seed, so every run makes the
//! same queries (RULES.md R-TEST-5). `TURBOREL_QUERY_SEED` gives another
//! seed and `TURBOREL_QUERY_COUNT` another number of queries, to search
//! further by hand; a failure names the seed of its query, which makes it
//! again as the first.

use std::env;

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::threads::JobPolicy;

use crate::common::{exec, rows, set_all_rules};

/// The seed of a run when `SEED_VAR` gives none.
const DEFAULT_SEED: u64 = 0x5EED_1992_0000_0001;
/// The environment variable that gives another seed.
const SEED_VAR: &str = "TURBOREL_QUERY_SEED";
/// The queries of a run when `COUNT_VAR` gives no number: enough to reach
/// each form many times, few enough to keep the suite fast.
const DEFAULT_COUNT: u64 = 250;
/// The environment variable that gives another number of queries.
const COUNT_VAR: &str = "TURBOREL_QUERY_COUNT";

/// xorshift64*: a small generator, enough to pick among forms and values.
struct Rng(u64);

impl Rng {
    /// A generator from `seed`; xorshift needs a state not zero.
    fn new(seed: u64) -> Rng {
        Rng(seed.max(1_u64))
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12_u32;
        self.0 ^= self.0 << 25_u32;
        self.0 ^= self.0 >> 27_u32;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D_u64)
    }

    /// A number in `0..n`.
    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n).unwrap()).unwrap()
    }

    /// True `percent` times in a hundred.
    fn chance(&mut self, percent: usize) -> bool {
        self.below(100_usize) < percent
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }

    /// A small integer, as SQL text, negative ones among them.
    fn number(&mut self) -> String {
        (i64::try_from(self.below(21_usize)).unwrap() - 6_i64).to_string()
    }
}

/// The tables, each of the columns `NUMBERS` and `STRINGS`: A of the row
/// store with a B-link and a hash index, B of the column store, C of the
/// row store with no index and few rows, so plans of every kind are made.
const TABLES: [&str; 3] = ["A", "B", "C"];
const NUMBERS: [&str; 4] = ["K", "G", "N", "V"];
const STRINGS: [&str; 1] = ["S"];
const COMPARISONS: [&str; 6] = ["=", "<>", "<", "<=", ">", ">="];
const PATTERNS: [&str; 7] = ["'A%'", "'%B%'", "'_B%'", "'A_  '", "'%'", "'B%A%'", "'AB  '"];
const STRING_VALUES: [&str; 5] = ["'A'", "'AB'", "'BA'", "'B'", "'ABC'"];

/// Database with schema Q: tables A, B and C of `TABLES`, K a key, G a
/// few groups, N signed, S short strings, V decimals; nulls in all but K.
fn schema() -> Database {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION Q
           CREATE TABLE A (K INTEGER NOT NULL PRIMARY KEY, G INTEGER, N INTEGER, S CHAR(4), V DECIMAL(5,1))
           CREATE TABLE B (K INTEGER NOT NULL PRIMARY KEY, G INTEGER, N INTEGER, S CHAR(4), V DECIMAL(5,1)) USING columnstore
           CREATE TABLE C (K INTEGER NOT NULL, G INTEGER, N INTEGER, S CHAR(4), V DECIMAL(5,1))",
    )
    .unwrap();
    db.create_index("A_G", "Q.A", &["G"], "btree").unwrap();
    db.create_index("A_S", "Q.A", &["S"], "hash").unwrap();
    let s = db.session("Q");
    for (table, count, step) in [("A", 60_i64, 1_i64), ("B", 45_i64, 2_i64), ("C", 8_i64, 3_i64)] {
        let values: Vec<String> = (1_i64..=count).map(|k| row(k, step)).collect();
        s.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
    }
    s.commit().unwrap();
    db
}

/// Row `k` of a table whose values move by `step`: each column null at its
/// own period, so the tables differ and nulls meet nulls in joins.
fn row(k: i64, step: i64) -> String {
    let or_null = |period: i64, value: String| if k % period == 0 { "NULL".to_owned() } else { value };
    let strings = ["A", "AB", "BA", "B", "ABC", "BAB"];
    let s = strings[usize::try_from((k * step) % 6_i64).unwrap()];
    format!(
        "({k}, {}, {}, {}, {})",
        or_null(7_i64, ((k * step) % 5_i64).to_string()),
        or_null(11_i64, ((k * 13_i64 * step) % 17_i64 - 8_i64).to_string()),
        or_null(9_i64, format!("'{s}'")),
        or_null(13_i64, format!("{}.5", (k * step) % 9_i64 - 3_i64)),
    )
}

/// The correlation names in scope, and how many subqueries deep the
/// generator is: it nests one, named S, and no deeper.
struct Scope {
    names: Vec<String>,
    depth: usize,
}

impl Scope {
    fn number(&self, rng: &mut Rng) -> String {
        format!("{}.{}", rng.pick(&self.names.iter().map(String::as_str).collect::<Vec<_>>()), rng.pick(&NUMBERS))
    }

    fn string(&self, rng: &mut Rng) -> String {
        format!("{}.{}", rng.pick(&self.names.iter().map(String::as_str).collect::<Vec<_>>()), rng.pick(&STRINGS))
    }

    /// What to divide by: mostly G + 1, never zero; at times N, zero in a
    /// row of 17 or so, so a division raises an error now and then (on
    /// rows that, RULES.md D15 says, the plan does not choose).
    fn divisor(&self, rng: &mut Rng) -> String {
        let name = rng.pick(&self.names.iter().map(String::as_str).collect::<Vec<_>>());
        if rng.chance(15_usize) { format!("{name}.N") } else { format!("({name}.G + 1)") }
    }

    /// A column of either kind.
    fn column(&self, rng: &mut Rng) -> String {
        if rng.chance(20_usize) { self.string(rng) } else { self.number(rng) }
    }
}

/// A `<from clause>` and the scope it makes: one table, a product, an
/// inner, outer or full join, or three tables.
fn from_clause(rng: &mut Rng) -> (String, Scope) {
    let (x, y, z) = (rng.pick(&TABLES), rng.pick(&TABLES), rng.pick(&TABLES));
    let names = |list: &[&str]| list.iter().map(|n| (*n).to_owned()).collect();
    let on = |rng: &mut Rng| format!("X.{0} = Y.{0}", rng.pick(&NUMBERS));
    let (from, scope) = match rng.below(7_usize) {
        0 | 1 => (format!("{x} X"), names(&["X"])),
        2 => (format!("{x} X, {y} Y"), names(&["X", "Y"])),
        3 => (format!("{x} X JOIN {y} Y ON {}", on(rng)), names(&["X", "Y"])),
        4 => (format!("{x} X {} JOIN {y} Y ON {}", rng.pick(&["LEFT", "RIGHT", "FULL"]), on(rng)), names(&["X", "Y"])),
        5 => (format!("{x} X LEFT JOIN {y} Y ON {} AND Y.N > {}", on(rng), rng.number()), names(&["X", "Y"])),
        _ => (format!("{x} X JOIN {y} Y ON {} JOIN {z} Z ON Y.G = Z.G", on(rng)), names(&["X", "Y", "Z"])),
    };
    (from, Scope { names: scope, depth: 0 })
}

/// A `<search condition>` of at most `depth` levels of AND, OR and NOT.
fn condition(rng: &mut Rng, scope: &Scope, depth: usize) -> String {
    if depth == 0 || rng.chance(40_usize) {
        return predicate(rng, scope);
    }
    match rng.below(3_usize) {
        0 => format!("({} AND {})", condition(rng, scope, depth - 1), condition(rng, scope, depth - 1)),
        1 => format!("({} OR {})", condition(rng, scope, depth - 1), condition(rng, scope, depth - 1)),
        _ => format!("NOT ({})", condition(rng, scope, depth - 1)),
    }
}

/// A `<predicate>`: a comparison with a constant, a column or a quotient,
/// IN, BETWEEN, LIKE, IS NULL, or one with a subquery.
fn predicate(rng: &mut Rng, scope: &Scope) -> String {
    let not = if rng.chance(25_usize) { "NOT " } else { "" };
    match rng.below(11_usize) {
        0 | 1 => format!("{} {} {}", scope.number(rng), rng.pick(&COMPARISONS), rng.number()),
        2 => format!("{} {} {}", scope.number(rng), rng.pick(&COMPARISONS), scope.number(rng)),
        3 => format!("{} {not}IN ({}, {}, {})", scope.number(rng), rng.number(), rng.number(), rng.number()),
        4 => format!("{} {not}BETWEEN {} AND {}", scope.number(rng), rng.number(), rng.number()),
        5 => format!("{} {not}LIKE {}", scope.string(rng), rng.pick(&PATTERNS)),
        6 => format!("{} IS {not}NULL", scope.column(rng)),
        7 => format!("{} {} {}", scope.string(rng), rng.pick(&COMPARISONS), rng.pick(&STRING_VALUES)),
        8 => format!("{} / {} {} {}", scope.number(rng), scope.divisor(rng), rng.pick(&COMPARISONS), rng.number()),
        _ => subquery_predicate(rng, scope, not),
    }
}

/// IN of a subquery, EXISTS of a correlated one, or a comparison with a
/// scalar subquery of a set function; past one level deep, a comparison.
fn subquery_predicate(rng: &mut Rng, scope: &Scope, not: &str) -> String {
    if scope.depth > 0 {
        return format!("{} {} {}", scope.number(rng), rng.pick(&COMPARISONS), rng.number());
    }
    let name = "S";
    let inner = Scope { names: vec![name.to_owned()], depth: scope.depth + 1 };
    let table = rng.pick(&TABLES);
    let column = rng.pick(&NUMBERS);
    let filter = condition(rng, &inner, 1_usize);
    match rng.below(3_usize) {
        0 => format!("{} {not}IN (SELECT {name}.{column} FROM {table} {name} WHERE {filter})", scope.number(rng)),
        1 => format!("{not}EXISTS (SELECT * FROM {table} {name} WHERE {name}.{column} = {} AND {filter})", scope.number(rng)),
        _ => {
            let function = rng.pick(&["MAX", "MIN", "COUNT", "SUM"]);
            format!(
                "{} {} (SELECT {function}({name}.{column}) FROM {table} {name} WHERE {filter})",
                scope.number(rng),
                rng.pick(&COMPARISONS)
            )
        }
    }
}

/// A `<set function specification>` over the scope's columns.
fn set_function(rng: &mut Rng, scope: &Scope) -> String {
    match rng.below(8_usize) {
        0 => "COUNT(*)".to_owned(),
        1 => format!("COUNT({})", scope.column(rng)),
        2 => format!("COUNT(DISTINCT {})", scope.column(rng)),
        3 => format!("SUM({})", scope.number(rng)),
        4 => format!("SUM(DISTINCT {})", scope.number(rng)),
        5 => format!("AVG({})", scope.number(rng)),
        6 => format!("MIN({})", scope.column(rng)),
        _ => format!("MAX({})", scope.column(rng)),
    }
}

/// A `<value expression>` of a row: a column, or a number column with
/// arithmetic, a quotient among it.
fn value(rng: &mut Rng, scope: &Scope) -> String {
    match rng.below(5_usize) {
        0 => format!("{} + {}", scope.number(rng), rng.number()),
        1 => format!("{} * {}", scope.number(rng), scope.number(rng)),
        2 => format!("{} / {}", scope.number(rng), scope.divisor(rng)),
        _ => scope.column(rng),
    }
}

/// A `<query specification>` with an optional WHERE: of rows, with or
/// without DISTINCT; of set functions; or grouped, with or without HAVING.
fn specification(rng: &mut Rng) -> String {
    let (from, scope) = from_clause(rng);
    let filter = if rng.chance(75_usize) { format!(" WHERE {}", condition(rng, &scope, 2_usize)) } else { String::new() };
    let list = |rng: &mut Rng, item: fn(&mut Rng, &Scope) -> String| {
        let n = 1_usize + rng.below(3_usize);
        (0_usize..n).map(|_| item(rng, &scope)).collect::<Vec<_>>().join(", ")
    };
    match rng.below(4_usize) {
        0 | 1 => {
            let distinct = if rng.chance(30_usize) { "DISTINCT " } else { "" };
            format!("SELECT {distinct}{} FROM {from}{filter}", list(rng, value))
        }
        2 => format!("SELECT {} FROM {from}{filter}", list(rng, set_function)),
        _ => {
            let key = scope.column(rng);
            let having = if rng.chance(50_usize) {
                let function = rng.pick(&["SUM", "AVG", "MIN", "MAX", "COUNT"]);
                format!(" HAVING {function}({}) {} {}", scope.number(rng), rng.pick(&COMPARISONS), rng.number())
            } else {
                String::new()
            };
            format!("SELECT {key}, {} FROM {from}{filter} GROUP BY {key}{having}", list(rng, set_function))
        }
    }
}

/// A query of one column: a column or a set function grouped by one.
fn one_column(rng: &mut Rng) -> String {
    let (from, scope) = from_clause(rng);
    let filter = condition(rng, &scope, 1_usize);
    let column = scope.number(rng);
    if rng.chance(30_usize) {
        format!("SELECT COUNT(*) FROM {from} WHERE {filter} GROUP BY {column}")
    } else {
        format!("SELECT {column} FROM {from} WHERE {filter}")
    }
}

/// A `<query expression>`: a query specification, or UNION, EXCEPT or
/// INTERSECT of two of one number column; ORDER BY its first column, at
/// times.
fn query(rng: &mut Rng) -> String {
    let body = if rng.chance(25_usize) {
        let operator = rng.pick(&["UNION", "UNION ALL", "EXCEPT", "INTERSECT"]);
        format!("{} {operator} {}", one_column(rng), one_column(rng))
    } else {
        specification(rng)
    };
    if rng.chance(40_usize) { format!("{body} ORDER BY 1{}", rng.pick(&["", " DESC"])) } else { body }
}

/// What a query gave: its rows in the order read, or its error.
type Outcome = Result<Vec<String>, Code>;

/// The outcome of `sql` in `s`, read by batches or not.
fn run(s: &Session, sql: &str, batches: bool) -> Outcome {
    exec(s, &format!("SET EXECUTOR.BATCHES = '{}'", if batches { "ON" } else { "OFF" })).unwrap();
    rows(s, sql)
}

/// Whether two outcomes agree: the same error, or the same rows; in the
/// same order of first columns when the query is ordered (rows equal in
/// it may come in any order).
fn agree(sql: &str, a: &Outcome, b: &Outcome) -> bool {
    let (Ok(a), Ok(b)) = (a, b) else { return a == b };
    let first = |rows: &[String]| rows.iter().map(|r| r.split(',').next().unwrap_or_default().to_owned()).collect::<Vec<_>>();
    if sql.contains("ORDER BY") && first(a) != first(b) {
        return false;
    }
    let (mut sorted_a, mut sorted_b) = (a.clone(), b.clone());
    sorted_a.sort();
    sorted_b.sort();
    sorted_a == sorted_b
}

/// The seed and number of queries of this run.
fn plan() -> (u64, u64) {
    let read = |var: &str, default: u64| env::var(var).map_or(default, |v| v.parse().unwrap_or_else(|_| panic!("{var}={v} is no number")));
    (read(SEED_VAR, DEFAULT_SEED), read(COUNT_VAR, DEFAULT_COUNT))
}

/// Verifies: [DESIGN] RULES.md D15, D31: random queries of joins (inner,
/// outer, full, products), WHERE of AND, OR, NOT, comparisons, IN,
/// BETWEEN, LIKE and IS NULL, subqueries (IN, EXISTS correlated, scalar),
/// set functions with and without DISTINCT, GROUP BY and HAVING, DISTINCT,
/// UNION, EXCEPT and INTERSECT, and ORDER BY, over tables of both stores,
/// give the same outcome with every optimizer rule on and off, read by
/// batches and row by row. Each query is made from a seed of its own;
/// a failure prints it and the SQL.
#[test]
fn random_queries_answer_alike_with_and_without_rules() {
    let db = schema();
    let (seed, count) = plan();
    // The first query's seed is the run's, so a failing query's own seed
    // and a count of 1 make it again.
    let mut seeds = Rng::new(seed);
    let queries: Vec<(u64, String)> = (0_u64..count)
        .map(|i| {
            let own = if i == 0 { seed } else { seeds.next() };
            (own, query(&mut Rng::new(own)))
        })
        .collect();
    let s = db.session("Q");
    let with_rules: Vec<[Outcome; 2]> = queries.iter().map(|(_, sql)| [run(&s, sql, true), run(&s, sql, false)]).collect();
    drop(s);
    set_all_rules(&db, false);
    let s = db.session("Q");
    let mut refused = 0_usize;
    for ((own, sql), [batched, row_by_row]) in queries.iter().zip(&with_rules) {
        let unoptimized = [run(&s, sql, true), run(&s, sql, false)];
        let all = [batched, row_by_row, &unoptimized[0], &unoptimized[1]];
        let same = all.iter().all(|o| agree(sql, batched, o));
        assert!(
            same,
            "{SEED_VAR}={own} {COUNT_VAR}=1: {sql}\nrules on, by batches: {batched:?}\nrules on, row by row: {row_by_row:?}\nrules off: {unoptimized:?}"
        );
        assert!(
            !matches!(batched, Err(Code::Syntax | Code::SyntaxRule)),
            "{SEED_VAR}={own} {COUNT_VAR}=1: the generator made {sql}: {batched:?}"
        );
        refused += usize::from(batched.is_err());
    }
    assert!(refused * 10_usize <= queries.len(), "{refused} of {} queries failed", queries.len());
}
