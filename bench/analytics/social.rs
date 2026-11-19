//! How fast GRAPH_TABLE queries run on a social network shaped as LDBC
//! SNB's is: people who know each other, the number each knows of a
//! power-law distribution, a few forums, posts and comments, who created
//! each (HAS_CREATOR), what each comment replies to (REPLY_OF) and who
//! likes which post (LIKES); a property graph over tables, made of a fixed
//! seed, of 1,000 people a unit of scale. Measured through SQL: friends
//! one and two KNOWS away (rigid patterns), friends of friends filtered by
//! city and birth year, ANY SHORTEST between two people, ANY SHORTEST from
//! the people of a city, many starts at once, and TRAIL of one to three
//! edges within MAX_PATH_LENGTH; and the rigid patterns joined: KNOWS
//! walked back from one person, every walk of one or two KNOWS, and the
//! KNOWS triangles of every person; and friends one and two away in
//! PLAIN, a TRUSTED graph whose copy of KNOWS has no adjacency index, so
//! its 2 joins of one whole scan are joined and its 8 of two searched
//! (RULES.md D147, R-BENCH-1).
//!
//! The scale is `SCALE_VAR`, 1 if unset; `WORKLOAD_VAR` runs one workload
//! alone. Each workload's rows are summed
//! into a checksum, printed before it is measured, that does not depend
//! on their order, so a change to the search is seen not to change them.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::default_numeric_fallback,
    reason = "benchmark code: the generator's figures are estimates (RULES.md R-DEV-3)"
)]

use std::collections::HashSet;
use std::env;

use super::harness::{Bench, Case, note, scatter};
use turborel::db::{Database, Session};
use turborel::params::Params;

/// The variable of the scale: thousands of people.
pub const SCALE_VAR: &str = "TURBOREL_BENCH_SOCIAL_SCALE";
/// The variable naming the one workload run, by a part of its name;
/// every workload if unset.
pub const WORKLOAD_VAR: &str = "TURBOREL_BENCH_SOCIAL_WORKLOAD";
/// The scale if `SCALE_VAR` is unset.
const DEFAULT_SCALE: u64 = 1;
/// People a unit of scale.
const PEOPLE_PER_SCALE: u64 = 1_000;
/// Cities people live in.
const CITIES: u64 = 10;
/// The most people one person knows.
const MOST_KNOWN: u64 = 400;
/// Rows each INSERT writes.
const PER_INSERT: usize = 500;
/// The seed of every choice the generator makes.
const SEED: u64 = 0x005E_ED50_C1A1;
/// People each run of a one-person workload starts from.
const ASKED: u64 = 20;

/// The tables of the network, and its graph SOCIAL.
const SCHEMA: &str = "CREATE SCHEMA AUTHORIZATION SNB
    CREATE TABLE PERSON (ID INTEGER PRIMARY KEY, FIRST_NAME VARCHAR(16), CITY INTEGER, BIRTH_YEAR INTEGER)
    CREATE TABLE KNOWS (ID INTEGER PRIMARY KEY, P1 INTEGER REFERENCES PERSON, P2 INTEGER REFERENCES PERSON, SINCE INTEGER)
    CREATE TABLE FORUM (ID INTEGER PRIMARY KEY, TITLE VARCHAR(32))
    CREATE TABLE POST (ID INTEGER PRIMARY KEY, LENGTH INTEGER)
    CREATE TABLE COMMENT (ID INTEGER PRIMARY KEY, LENGTH INTEGER)
    CREATE TABLE CONTAINER_OF (ID INTEGER PRIMARY KEY, FORUM INTEGER REFERENCES FORUM, POST INTEGER REFERENCES POST)
    CREATE TABLE POST_CREATOR (ID INTEGER PRIMARY KEY, POST INTEGER REFERENCES POST, PERSON INTEGER REFERENCES PERSON)
    CREATE TABLE COMMENT_CREATOR (ID INTEGER PRIMARY KEY, COMMENT INTEGER REFERENCES COMMENT, PERSON INTEGER REFERENCES PERSON)
    CREATE TABLE REPLY_OF (ID INTEGER PRIMARY KEY, COMMENT INTEGER REFERENCES COMMENT, POST INTEGER REFERENCES POST)
    CREATE TABLE LIKES (ID INTEGER PRIMARY KEY, PERSON INTEGER REFERENCES PERSON, POST INTEGER REFERENCES POST, CREATED INTEGER)";

/// The graph SOCIAL over the tables.
const GRAPH: &str = "CREATE PROPERTY GRAPH SOCIAL
    VERTEX TABLES (PERSON LABEL PERSON PROPERTIES (ID, FIRST_NAME, CITY, BIRTH_YEAR),
      FORUM LABEL FORUM PROPERTIES (ID, TITLE), POST LABEL POST PROPERTIES (ID, LENGTH),
      COMMENT LABEL COMMENT PROPERTIES (ID, LENGTH))
    EDGE TABLES (
      KNOWS SOURCE KEY (P1) REFERENCES PERSON (ID) DESTINATION KEY (P2) REFERENCES PERSON (ID) LABEL KNOWS PROPERTIES (SINCE),
      CONTAINER_OF SOURCE KEY (FORUM) REFERENCES FORUM (ID) DESTINATION KEY (POST) REFERENCES POST (ID) LABEL CONTAINER_OF,
      POST_CREATOR SOURCE KEY (POST) REFERENCES POST (ID) DESTINATION KEY (PERSON) REFERENCES PERSON (ID) LABEL HAS_CREATOR PROPERTIES (ID),
      COMMENT_CREATOR SOURCE KEY (COMMENT) REFERENCES COMMENT (ID) DESTINATION KEY (PERSON) REFERENCES PERSON (ID) LABEL HAS_CREATOR PROPERTIES (ID),
      REPLY_OF SOURCE KEY (COMMENT) REFERENCES COMMENT (ID) DESTINATION KEY (POST) REFERENCES POST (ID) LABEL REPLY_OF,
      LIKES SOURCE KEY (PERSON) REFERENCES PERSON (ID) DESTINATION KEY (POST) REFERENCES POST (ID) LABEL LIKES PROPERTIES (CREATED))";

/// First names people are given.
const NAMES: [&str; 8] = ["Ada", "Bo", "Chen", "Dana", "Emil", "Fay", "Gus", "Hana"];

/// A seeded sequence: splitmix64, the same on every run.
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..n`.
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    /// A number in `0..n`, small ones far likelier: a power law's.
    fn skewed(&mut self, n: u64) -> u64 {
        let u = (self.next() >> 11) as f64 / (1_u64 << 53) as f64;
        ((u * u * u) * n as f64) as u64 % n.max(1)
    }
}

/// Inserts `rows`, each a parenthesized list of values, into `table`.
fn insert(s: &Session, table: &str, rows: &[String]) {
    for chunk in rows.chunks(PER_INSERT) {
        s.run(&format!("INSERT INTO {table} VALUES {}", chunk.join(", "))).unwrap();
    }
}

/// The KNOWS edges of `people` people: person p knows a number of people
/// falling as a power of its rank, the people it knows chosen likelier
/// among the first, as the well connected are; no one knows itself, and
/// no one twice.
fn knows(rng: &mut Seeded, people: u64) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    for p in 0..people {
        let rank = scatter(p, people) + 1;
        let degree = ((5.0 * (people as f64 / rank as f64).sqrt()) as u64).clamp(1, MOST_KNOWN);
        for _ in 0..degree {
            let q = scatter(rng.skewed(people), people);
            if q != p && seen.insert((p.min(q), p.max(q))) {
                rows.push(format!("({}, {p}, {q}, {})", rows.len(), 2010 + rng.below(14)));
            }
        }
    }
    rows
}

/// Writes the network of `people` people into `s`'s tables.
fn generate(s: &Session, people: u64) {
    let mut rng = Seeded(SEED);
    let persons: Vec<String> =
        (0..people).map(|p| format!("({p}, '{}', {}, {})", NAMES[(p % 8) as usize], rng.below(CITIES), 1960 + rng.below(45))).collect();
    insert(s, "PERSON", &persons);
    insert(s, "KNOWS", &knows(&mut rng, people));
    let (forums, posts, comments, likes) = ((people / 100).max(3), people * 2, people * 3, people * 5);
    insert(s, "FORUM", &(0..forums).map(|f| format!("({f}, 'forum {f}')")).collect::<Vec<_>>());
    insert(s, "POST", &(0..posts).map(|p| format!("({p}, {})", 10 + rng.below(990))).collect::<Vec<_>>());
    insert(s, "COMMENT", &(0..comments).map(|c| format!("({c}, {})", 5 + rng.below(200))).collect::<Vec<_>>());
    insert(s, "CONTAINER_OF", &(0..posts).map(|p| format!("({p}, {}, {p})", rng.skewed(forums))).collect::<Vec<_>>());
    insert(s, "POST_CREATOR", &(0..posts).map(|p| format!("({p}, {p}, {})", scatter(rng.skewed(people), people))).collect::<Vec<_>>());
    let creators: Vec<String> = (0..comments).map(|c| format!("({c}, {c}, {})", rng.below(people))).collect();
    insert(s, "COMMENT_CREATOR", &creators);
    insert(s, "REPLY_OF", &(0..comments).map(|c| format!("({c}, {c}, {})", rng.skewed(posts))).collect::<Vec<_>>());
    let rows_of_likes: Vec<String> =
        (0..likes).map(|l| format!("({l}, {}, {}, {})", rng.below(people), rng.skewed(posts), 2015 + rng.below(9))).collect();
    insert(s, "LIKES", &rows_of_likes);
}

/// The graph PLAIN, TRUSTED, over a copy of KNOWS of no index.
const PLAIN: [&str; 3] = [
    "CREATE TABLE KNOWS_PLAIN (ID INTEGER PRIMARY KEY, P1 INTEGER, P2 INTEGER, SINCE INTEGER)",
    "INSERT INTO KNOWS_PLAIN SELECT * FROM KNOWS",
    "CREATE PROPERTY GRAPH PLAIN VERTEX TABLES (PERSON LABEL PERSON PROPERTIES (ID)) EDGE TABLES (KNOWS_PLAIN \
     SOURCE KEY (P1) REFERENCES PERSON (ID) DESTINATION KEY (P2) REFERENCES PERSON (ID) LABEL KNOWS) OPTIONS (TRUSTED MODE)",
];

/// The network of `people` people and its graphs, their adjacency indexes
/// built and compacted.
fn database(people: u64) -> Database {
    let db = Database::new();
    db.schema(SCHEMA).unwrap();
    let s = db.session("SNB");
    generate(&s, people);
    s.run(GRAPH).unwrap();
    for sql in PLAIN {
        s.run(sql).unwrap();
    }
    s.commit().unwrap();
    db.maintain().unwrap();
    db
}

/// FNV-1a of `text`.
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xCBF2_9CE4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01B3))
}

/// The rows of `sql`, fetched: how many, and the sum of their hashes.
fn summed(s: &Session, sql: &str) -> (u64, u64) {
    let mut cursor = s.open_cursor(sql).unwrap();
    let (mut rows, mut sum) = (0_u64, 0_u64);
    while let Some(row) = cursor.fetch().unwrap() {
        let text: Vec<String> = row.iter().map(|v| v.to_string().trim().to_owned()).collect();
        rows += 1;
        sum = sum.wrapping_add(fnv(&text.join("|")));
    }
    s.commit().unwrap();
    (rows, sum)
}

/// Prints the checksum of `queries`, then measures them, each run all of
/// them, as `name`.
fn workload(bench: &Bench, s: &Session, name: &str, queries: &[String]) {
    if env::var(WORKLOAD_VAR).is_ok_and(|only| !name.contains(only.as_str())) {
        return;
    }
    let (mut rows, mut sum) = (0_u64, 0_u64);
    for q in queries {
        let (r, h) = summed(s, q);
        rows += r;
        sum = sum.wrapping_add(h);
    }
    note(&format!("checksum {name}: {sum:016x} ({rows} rows)"));
    bench.measure(Case::new(name, "queries"), || {
        for q in queries {
            summed(s, q);
        }
        queries.len() as u64
    });
}

/// `ASKED` people scattered over `people`, offset by `offset`.
fn asked(people: u64, offset: u64) -> Vec<u64> {
    (0..ASKED).map(|n| scatter(n * 37 + offset, people)).collect()
}

pub fn workloads(bench: &Bench) {
    let scale = env::var(SCALE_VAR).ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(DEFAULT_SCALE).max(1);
    let people = scale * PEOPLE_PER_SCALE;
    let db = database(people);
    let s = db.session("SNB");
    let (knows, _) = summed(&s, "SELECT ID FROM KNOWS");
    note(&format!("social network: scale {scale}, {people} people, {knows} KNOWS edges"));
    let of = |starts: &[u64], sql: &dyn Fn(u64) -> String| starts.iter().map(|&p| sql(p)).collect::<Vec<_>>();
    let from = asked(people, 0);
    let one = of(&from, &|p| {
        format!("SELECT B FROM GRAPH_TABLE (SOCIAL MATCH (a:PERSON WHERE a.id = {p})-[:KNOWS]-(b:PERSON) COLUMNS (b.id AS B))")
    });
    workload(bench, &s, "social friends, 1 hop", &one);
    let two = of(&from, &|p| {
        format!(
            "SELECT C FROM GRAPH_TABLE (SOCIAL MATCH (a:PERSON WHERE a.id = {p})-[:KNOWS]-(b:PERSON)-[:KNOWS]-(c:PERSON) COLUMNS (c.id AS C))"
        )
    });
    workload(bench, &s, "social friends, 2 hops", &two);
    let filtered = of(&from, &|p| {
        format!(
            "SELECT DISTINCT C, NAME FROM GRAPH_TABLE (SOCIAL MATCH (a:PERSON WHERE a.id = {p})-[:KNOWS]-(b:PERSON)-[:KNOWS]-(c:PERSON) \
             WHERE c.city = a.city AND c.birth_year >= 1980 AND c.id <> a.id COLUMNS (c.id AS C, c.first_name AS NAME))"
        )
    });
    workload(bench, &s, "social friends of friends, filtered", &filtered);
    let ends = asked(people, 11);
    let pairs: Vec<String> = from
        .iter()
        .zip(&ends)
        .map(|(a, b)| {
            format!(
                "SELECT N FROM GRAPH_TABLE (SOCIAL MATCH ANY SHORTEST p = (a:PERSON WHERE a.id = {a})-[:KNOWS]-+(b:PERSON WHERE b.id = {b}) \
                 COLUMNS (PATH_LENGTH(p) AS N) OPTIONS (DETERMINISTIC_PATHS))"
            )
        })
        .collect();
    workload(bench, &s, "social ANY SHORTEST of two people", &pairs);
    let many = vec![format!(
        "SELECT A, N FROM GRAPH_TABLE (SOCIAL MATCH ANY SHORTEST p = (a:PERSON WHERE a.city = 3)-[:KNOWS]-+(b:PERSON WHERE b.id = {}) \
         COLUMNS (a.id AS A, PATH_LENGTH(p) AS N) OPTIONS (DETERMINISTIC_PATHS))",
        ends[0]
    )];
    workload(bench, &s, "social ANY SHORTEST from a city's people", &many);
    planned(bench, &s, &from);
}

/// A TRAIL of 1 to 3 edges, searched, and the workloads a rigid
/// pattern's join plan answers (RULES.md D147, D148): an edge walked
/// back, a small quantifier unrolled, and a cycle; each from `from` where
/// it starts from a person, in `s`.
fn planned(bench: &Bench, s: &Session, from: &[u64]) {
    let of = |starts: &[u64], sql: &dyn Fn(u64) -> String| starts.iter().map(|&p| sql(p)).collect::<Vec<_>>();
    let trails = of(from, &|p| {
        format!(
            "SELECT B, N FROM GRAPH_TABLE (SOCIAL MATCH TRAIL p = (a:PERSON WHERE a.id = {p})-[:KNOWS]->{{1,3}}(b:PERSON) \
             COLUMNS (b.id AS B, PATH_LENGTH(p) AS N) OPTIONS (MAX_PATH_LENGTH 3))"
        )
    });
    workload(bench, s, "social TRAIL of 1 to 3 KNOWS", &trails);
    let reversed = of(from, &|p| {
        format!("SELECT B FROM GRAPH_TABLE (SOCIAL MATCH (a:PERSON WHERE a.id = {p})<-[:KNOWS]-(b:PERSON) COLUMNS (b.id AS B))")
    });
    workload(bench, s, "social KNOWS walked back, 1 hop", &reversed);
    let unrolled = of(from, &|p| {
        format!("SELECT B FROM GRAPH_TABLE (SOCIAL MATCH (a:PERSON WHERE a.id = {p})-[:KNOWS]->{{1,2}}(b:PERSON) COLUMNS (b.id AS B))")
    });
    workload(bench, s, "social 1 to 2 KNOWS, every walk", &unrolled);
    let triangles = vec![
        "SELECT COUNT(*) FROM GRAPH_TABLE (SOCIAL MATCH (a:PERSON)-[:KNOWS]->(b:PERSON)-[:KNOWS]->(c:PERSON)-[:KNOWS]->(a) \
         COLUMNS (a.id AS A))"
            .to_owned(),
    ];
    workload(bench, s, "social KNOWS triangles", &triangles);
    // [RULES.md D147] Past 4 whole scans of an edge table of no adjacency index, a search.
    for (name, path) in
        [("1 hop", "(a:PERSON WHERE a.id = {p})-[:KNOWS]-(b)"), ("2 hops", "(a:PERSON WHERE a.id = {p})-[:KNOWS]-()-[:KNOWS]-(b)")]
    {
        let queries =
            of(from, &|p| format!("SELECT B FROM GRAPH_TABLE (PLAIN MATCH {} COLUMNS (b.id AS B))", path.replace("{p}", &p.to_string())));
        let plan = s.explain(queries.first().map_or("", String::as_str), &Params::new()).unwrap();
        note(&format!("unindexed friends, {name}: {}", if plan.contains("Graph table") { "searched" } else { "joined" }));
        workload(bench, s, &format!("social unindexed friends, {name}"), &queries);
    }
}
