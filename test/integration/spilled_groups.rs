//! \[DESIGN\] A grouping run in parallel whose partitions outgrow its
//! grant (RULES.md D1226): its partitions are written to the spill by the
//! threads that merged them and merged back at the end, split again where
//! one alone outgrows the grant, and it gives the rows it gives in memory,
//! in their order, rather than giving way to a grouping a row at a time.

use std::sync::{Arc, Mutex};
use std::thread::ThreadId;

use turborel::db::{Database, Session};
use turborel::memory::MemoryBudget;
use turborel::spill::{Area, PagedSpill, Records, Spill, SpillArea, SpillRun, SpillTable, TempFiles};
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{exec, rows};

/// The smallest MEMORY.WORK_BYTES, 64 KiB.
const SMALL: i128 = 64 << 10;

/// What the runs of a provider's areas showed: the thread each run was
/// made on, the bytes written to them, and how many areas were made.
#[derive(Default)]
struct Seen {
    made_on: Vec<ThreadId>,
    written: usize,
    areas: usize,
}

/// The built-in provider, keeping what `Seen` keeps.
#[derive(Clone, Default)]
struct Threads(Arc<Mutex<Seen>>);

impl Spill for Threads {
    fn name(&self) -> &'static str {
        "threads"
    }
    fn area(&self, memory: &Arc<MemoryBudget>, temp: &Arc<TempFiles>, area: Area) -> turborel::error::Result<Box<dyn SpillArea>> {
        self.0.lock().unwrap().areas += 1;
        Ok(Box::new(Kept(Spill::area(&PagedSpill, memory, temp, area)?, Arc::clone(&self.0))))
    }
}

impl Threads {
    /// What the runs showed since the last call.
    fn taken(&self) -> Seen {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

/// An area of the built-in provider, keeping what `Threads` keeps.
struct Kept(Box<dyn SpillArea>, Arc<Mutex<Seen>>);

impl SpillArea for Kept {
    fn table(&self) -> turborel::error::Result<Box<dyn SpillTable>> {
        self.0.table()
    }
    fn run(&self) -> turborel::error::Result<Box<dyn SpillRun>> {
        self.1.lock().unwrap().made_on.push(std::thread::current().id());
        Ok(Box::new(Written(self.0.run()?, Arc::clone(&self.1))))
    }
}

/// A run of the built-in provider, counting the bytes written to it.
struct Written(Box<dyn SpillRun>, Arc<Mutex<Seen>>);

impl SpillRun for Written {
    fn push(&mut self, record: &[u8]) -> turborel::error::Result<()> {
        self.1.lock().unwrap().written += record.len();
        self.0.push(record)
    }
    fn records(&mut self) -> turborel::error::Result<Records> {
        self.0.records()
    }
}

/// Table C of the column store in groups of 2048: K, a key; G, one of
/// 24000, each of four rows, null for every 37th; T, a text of G with trailing spaces; N, a
/// number.
fn table() -> (Database, Session, Threads) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    let threads = Threads::default();
    db.set_spill(Box::new(threads.clone())).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE C (K INTEGER PRIMARY KEY, G BIGINT, T VARCHAR(20), N INTEGER)
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    let values: Vec<String> = (0_i64..96_000)
        .map(|k| {
            let g = k * 7919 % 24_000;
            let shown = if k % 37 == 0 { "NULL".to_owned() } else { g.to_string() };
            format!("({k}, {shown}, 'g{g}{}', {})", " ".repeat(usize::try_from(k % 3).unwrap()), k % 101)
        })
        .collect();
    a.run(&format!("INSERT INTO C VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    threads.taken();
    (db, a, threads)
}

/// Runs `sql` alone, a row at a time.
fn alone(s: &Session, sql: &str) -> Vec<String> {
    exec(s, "SET EXECUTOR.DEGREE = 1").unwrap();
    exec(s, "SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    rows(s, sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"))
}

/// Runs `sql` in parallel on 4 threads in morsels of 256 rows.
fn parallel(s: &Session, sql: &str) -> Vec<String> {
    for set in
        ["SET EXECUTOR.BATCHES = 'ON'", "SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 256", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]
    {
        exec(s, set).unwrap();
    }
    rows(s, sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"))
}

/// Groupings of many groups run in parallel: of a count, an exact sum and
/// mean, a text's greatest, written by its accumulator; of a text key; under
/// HAVING.
const QUERIES: [&str; 3] = [
    "SELECT G, COUNT(*), SUM(N), AVG(N), MAX(T) FROM C GROUP BY G",
    "SELECT T, COUNT(*), MIN(N) FROM C GROUP BY T",
    "SELECT G, SUM(N) FROM C GROUP BY G HAVING COUNT(*) > 2",
];

/// \[DESIGN\] The partitions a grouping run in parallel merges, as the
/// executor splits its groups (`exec/parallel/probe.rs`).
const PARTITIONS: usize = 64;

/// Verifies: [DESIGN] (RULES.md D1226) groupings run in parallel at the
/// smallest MEMORY.WORK_BYTES, whose partitions outgrow it many times,
/// write their partitions to the spill as they merge them, a run a
/// partition a wave, so each partition is written many times over, not
/// once after giving way as a grouping given way to is; and give the
/// rows they give run alone a row at a time, in their order, with their
/// spilled partitions split again where one outgrows the grant. That
/// such a grouping keeps to its own area is shown where its groups are
/// not all given back (`a_grouping_spilled_holds_its_partitions_in_one_area`).
///
/// The thread a run is made on is not asserted. A partition's unit of a
/// wave is taken by a worker or by the statement's thread, whichever
/// comes first, and at this grant a unit merges a few rows, so on a
/// machine of few processors the statement's thread may take every one
/// of a statement's. That is the scheduler's choice and D1226 claims
/// nothing of it. No unit of the grouping is one the statement's thread
/// cannot take, so no thread is forced here.
#[test]
fn partitions_past_the_grant_are_spilled_by_their_threads() {
    let (db, a, threads) = table();
    let in_memory: Vec<Vec<String>> = QUERIES.iter().map(|q| alone(&a, q)).collect();
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(SMALL, 0))).unwrap();
    threads.taken();
    for (q, expected) in QUERIES.iter().zip(&in_memory) {
        assert_eq!(&parallel(&a, q), expected, "{q}");
        let Seen { made_on, written, .. } = threads.taken();
        let shown = a.physical_plan(q, &turborel::params::Params::new()).unwrap();
        assert!(
            made_on.len() >= PARTITIONS.saturating_mul(2),
            "{q}: a run a partition a wave, of waves many times the grant: {} runs, {written} bytes\n{shown}",
            made_on.len()
        );
        assert!(written > usize::try_from(SMALL).unwrap() * 4, "{q}: its partitions many times the grant, spilled");
    }
}

/// The groupings of `QUERIES`, their first five groups asked by a set
/// function's value, then by key: rows few enough to be held in no area
/// of their own, of a grouping run in parallel, as a sort by its grouping
/// columns would group the rows sorted.
const ASKED: [&str; 3] = [
    "SELECT G, COUNT(*), SUM(N) AS S, AVG(N), MAX(T) FROM C GROUP BY G ORDER BY S DESC, G FETCH FIRST 5 ROWS ONLY",
    "SELECT T, COUNT(*), MIN(N) AS M FROM C GROUP BY T ORDER BY M DESC, T FETCH FIRST 5 ROWS ONLY",
    "SELECT G, SUM(N) AS S FROM C GROUP BY G HAVING COUNT(*) > 2 ORDER BY S DESC, G FETCH FIRST 5 ROWS ONLY",
];

/// The plan of `sql` as `s` runs it, which must run a part in parallel.
fn in_parallel(s: &Session, sql: &str) -> String {
    let shown = s.physical_plan(sql, &turborel::params::Params::new()).unwrap();
    assert!(shown.contains("Parallel"), "{sql}: run in parallel\n{shown}");
    shown
}

/// Verifies: [DESIGN] (RULES.md D1226, D1124) the groupings of
/// `partitions_past_the_grant_are_spilled_by_their_threads`, their first
/// five groups asked, at the smallest MEMORY.WORK_BYTES are held in the
/// one area of their grant, and a grouping given way to would spill in
/// one more; and give the rows they give in memory.
#[test]
fn a_grouping_spilled_holds_its_partitions_in_one_area() {
    let (db, a, threads) = table();
    let in_memory: Vec<Vec<String>> = ASKED.iter().map(|q| alone(&a, q)).collect();
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(SMALL, 0))).unwrap();
    threads.taken();
    for (q, expected) in ASKED.iter().zip(&in_memory) {
        assert_eq!(&parallel(&a, q), expected, "{q}");
        let Seen { written, areas, .. } = threads.taken();
        let shown = in_parallel(&a, q);
        assert!(written > usize::try_from(SMALL).unwrap() * 4, "{q}: its partitions many times the grant, spilled\n{shown}");
        assert_eq!(areas, 1, "{q}: the grouping's own area alone, none of a grouping given way to\n{shown}");
    }
}

/// Verifies: [DESIGN] (RULES.md D1276, D1247, D1226) a grouping of few
/// groups a morsel, its morsels merged on the statement's thread, where
/// the groups of one morsel alone pass the smallest MEMORY.WORK_BYTES by
/// the states of a hundred and eighty set functions, writes that thread's
/// table to the spill each time and gives every group, with the rows the
/// statement gives alone at degree 1. A regression test: with no wave
/// merged the spill was forgotten at the end, and the groups of the
/// statement thread's last table alone were given.
#[test]
fn a_table_of_few_groups_past_the_grant_gives_every_group() {
    let (db, a, threads) = table();
    let made: Vec<String> = (0..60).map(|n| format!("N + {n} AS N{n}")).collect();
    let asked: Vec<String> = (0..60).map(|n| format!("SUM(N{n}), MIN(N{n}), MAX(N{n})")).collect();
    let sql =
        format!("SELECT B, COUNT(*), {} FROM (SELECT K - MOD(K, 8) AS B, {} FROM C) AS Q GROUP BY B", asked.join(", "), made.join(", "));
    let in_memory = alone(&a, &sql);
    assert_eq!(in_memory.len(), 12_000);
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(SMALL, 0))).unwrap();
    threads.taken();
    for set in
        ["SET EXECUTOR.BATCHES = 'ON'", "SET EXECUTOR.DEGREE = 4", "SET EXECUTOR.MORSEL_ROWS = 1024", "SET EXECUTOR.PARALLEL_MIN_ROWS = 0"]
    {
        exec(&a, set).unwrap();
    }
    let spilled = rows(&a, &sql).unwrap();
    let shown = in_parallel(&a, &sql);
    assert!(threads.taken().written > 0, "its table went to the spill\n{shown}");
    let differs = spilled.iter().zip(&in_memory).position(|(row, same)| row != same);
    assert_eq!((spilled.len(), differs), (12_000, None), "{shown}");
}

/// Verifies: [DESIGN] (RULES.md D1226, D1113) the first grouping of a
/// pair, as a DISTINCT set function is run, and SELECT DISTINCT, at the
/// smallest MEMORY.WORK_BYTES, give the rows they give run alone, in
/// their order.
#[test]
fn a_pair_and_select_distinct_past_the_grant_give_their_rows() {
    let (db, a, _threads) = table();
    let queries = ["SELECT T, COUNT(DISTINCT N) FROM C GROUP BY T", "SELECT DISTINCT G, N FROM C"];
    let in_memory: Vec<Vec<String>> = queries.iter().map(|q| alone(&a, q)).collect();
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(SMALL, 0))).unwrap();
    for (q, expected) in queries.iter().zip(&in_memory) {
        assert_eq!(&parallel(&a, q), expected, "{q}");
    }
}

/// Verifies: [DESIGN] (RULES.md D1226) a grouping whose spilled
/// partitions outgrow the grant past SPILL.PARTITION_DEPTH, here 0, gives
/// way, the grouping given way to spilling in an area of its own beside
/// the one the partitions were spilled in, and then gives the rows it
/// gives run alone, in their order.
#[test]
fn partitions_past_the_depth_give_way() {
    let (db, a, threads) = table();
    let q = QUERIES.first().unwrap();
    let in_memory = alone(&a, q);
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(SMALL, 0))).unwrap();
    db.set_setting("SPILL.PARTITION_DEPTH", &Value::Exact(Decimal::new(0, 0))).unwrap();
    assert_eq!(parallel(&a, q), in_memory);
    let asked = ASKED.first().unwrap();
    let expected = alone(&a, asked);
    threads.taken();
    assert_eq!(&parallel(&a, asked), &expected);
    let areas = threads.taken().areas;
    let shown = in_parallel(&a, asked);
    assert!(areas > 1, "the grouping's area and one of the grouping given way to: {areas} areas\n{shown}");
}

/// Verifies: [DESIGN] (RULES.md D1228, D1133) a second grouping that
/// crosses the first's partitions, by a set function's result of the
/// first or a projection of it, of many groups, at the smallest
/// MEMORY.WORK_BYTES, its groups held in the spill and merged run by run,
/// gives the rows it gives run alone, in their order, under HAVING too.
#[test]
fn a_crossing_second_grouping_past_the_grant_gives_its_rows() {
    let (db, a, _threads) = table();
    let queries = [
        "SELECT X.M, COUNT(*) FROM (SELECT G, SUM(N) AS M FROM C GROUP BY G) AS X GROUP BY X.M",
        "SELECT X.M, COUNT(*) FROM (SELECT G, SUM(N) AS M FROM C GROUP BY G) AS X GROUP BY X.M HAVING COUNT(*) > 1",
        "SELECT Y.P, COUNT(Y.G) FROM (SELECT X.G, X.M + 1 AS P FROM (SELECT G, SUM(N) AS M FROM C GROUP BY G) AS X) AS Y GROUP BY Y.P",
    ];
    let in_memory: Vec<Vec<String>> = queries.iter().map(|q| alone(&a, q)).collect();
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(SMALL, 0))).unwrap();
    for (q, expected) in queries.iter().zip(&in_memory) {
        assert!(expected.len() > 100, "{q}: many groups of the second");
        assert_eq!(&parallel(&a, q), expected, "{q}");
        in_parallel(&a, q);
    }
}

/// Verifies: [DESIGN] (RULES.md D1281, D1226) the first rows of a sort
/// over a pair of groupings, at the smallest MEMORY.WORK_BYTES, whose
/// partitions are written to the spill and merged back, are the rows the
/// plan run alone gives in memory, in its order: by a count nearly every
/// group ties on, where the first rows' numbers order them, and past an
/// offset. Each part read back gives its first rows alone, so an analyzed
/// plan counts its projection fewer rows than there are groups, which a
/// part given way to would not. The WHERE clause keeps a third of the
/// groups, that of null G not among them. Of every group this pair gives
/// way at this grant, with a sort over it or none.
#[test]
fn a_pairs_first_rows_past_the_grant_are_the_plans() {
    let (db, a, threads) = table();
    let queries = [
        "SELECT 1, G, COUNT(DISTINCT N / 2) FROM C WHERE G < 8000 GROUP BY G ORDER BY 3 DESC FETCH FIRST 2 ROWS ONLY",
        "SELECT 1, G, COUNT(DISTINCT N / 40) FROM C WHERE G < 8000 GROUP BY G ORDER BY 3 DESC OFFSET 1 ROWS FETCH FIRST 2 ROWS ONLY",
    ];
    let in_memory: Vec<Vec<String>> = queries.iter().map(|q| alone(&a, q)).collect();
    let groups: u64 = alone(&a, "SELECT COUNT(*) FROM (SELECT G FROM C WHERE G < 8000 GROUP BY G) AS X").concat().parse().unwrap();
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(SMALL, 0))).unwrap();
    threads.taken();
    for (q, expected) in queries.iter().zip(&in_memory) {
        assert_eq!(&parallel(&a, q), expected, "{q}");
        let written = threads.taken().written;
        let shown = in_parallel(&a, q);
        assert!(written > usize::try_from(SMALL).unwrap() * 4, "{q}: its partitions many times the grant, spilled\n{shown}");
        let analyzed = a.analyzed(q, &turborel::params::Params::new()).unwrap();
        let projected = analyzed.operators.iter().find(|o| o.label.starts_with("Project")).map(|o| o.rows);
        assert!(projected.is_some_and(|kept| kept < groups), "{q}: the rows the parts kept, {projected:?} of {groups}\n{shown}");
    }
}

/// Verifies: [DESIGN] (RULES.md D1290, D1226) the strings a grouping's
/// states hold count against its grant. MIN and MAX of texts of some 500
/// characters, of 240 groups, hold several times the smallest
/// MEMORY.WORK_BYTES in their strings alone, though their groups are few:
/// run in parallel the grouping writes its partitions to the spill, and
/// gives the rows it gives run alone a row at a time, by octets under a
/// collation of NO PAD and padded with none. Under a collation that makes
/// sort keys, kept by an accumulator a group, it gives those rows too.
#[test]
fn the_strings_of_best_states_count_against_the_grant() {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.set_setting("EXECUTOR.WORKERS", &Value::Exact(Decimal::new(4, 0))).unwrap();
    let threads = Threads::default();
    db.set_spill(Box::new(threads.clone())).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE COLLATION BYTES FOR SQL_TEXT FROM UCS_BASIC NO PAD
           CREATE TABLE L (K INTEGER PRIMARY KEY, G INTEGER, T VARCHAR(600), U VARCHAR(600) COLLATE A.BYTES)
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    let values: Vec<String> = (0_i64..4800).map(|k| format!("({k}, {}, '{t:0>505}', '{t:0>505}')", k % 240, t = k * 7919 % 4800)).collect();
    a.run(&format!("INSERT INTO L VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    let queries = ["SELECT G, MIN(T), MAX(T) FROM L GROUP BY G", "SELECT G, MIN(U), MAX(U), COUNT(*) FROM L GROUP BY G"];
    let in_memory: Vec<Vec<String>> = queries.iter().map(|q| alone(&a, q)).collect();
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(SMALL, 0))).unwrap();
    threads.taken();
    for (q, expected) in queries.iter().zip(&in_memory) {
        assert_eq!(expected.len(), 240, "{q}");
        assert_eq!(&parallel(&a, q), expected, "{q}");
        let written = threads.taken().written;
        let shown = in_parallel(&a, q);
        assert!(written > usize::try_from(SMALL).unwrap(), "{q}: more text than the grant, {written} bytes spilled\n{shown}");
    }
    let keyed = "SELECT G, MIN(T COLLATE UCS_BASIC), MAX(T COLLATE UCS_BASIC) FROM L GROUP BY G";
    let expected = alone(&a, keyed);
    assert_eq!(parallel(&a, keyed), expected);
}

/// A value of W's B, a BIGINT of 18 digits, eleven of which leave 64 bits.
const BIG: i64 = 900_000_000_000_000_000;

/// Table W of the column store, of 12000 rows: K, a key; G, one of three
/// groups; J, a group of sixteen rows that follow one another; H, one of
/// 1000 groups of twelve rows 1000 apart; and B, a BIGINT, null for
/// twelve rows. Every row of H's group 7 holds `BIG` and every row of its
/// group 8 the negative, each row in a morsel of its own. Every row of
/// J's group 400 holds `BIG` and every row of its group 800 the negative,
/// each group in one morsel. Twenty-one more rows of G's group 1 hold
/// `BIG` in the first morsel, which has no null. Every other value is
/// within a thousand of nothing.
fn wide_sums(db: &Database, s: &Session) {
    let columns = "K INTEGER PRIMARY KEY, G INTEGER, J INTEGER, H INTEGER, B BIGINT";
    s.run(&format!("CREATE TABLE W ({columns}) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)")).unwrap();
    let b = |k: i64| {
        let most = k % 1000 == 7 || (400..=415).contains(&k) || ((100..=160).contains(&k) && k % 3 == 1);
        let least = k % 1000 == 8 || (800..=815).contains(&k);
        let far = if most { BIG } else { -BIG };
        let value = if most || least { far } else { k * 7919 % 2001 - 1000 };
        if k % 997 == 500 { "NULL".to_owned() } else { value.to_string() }
    };
    for first in (0_i64..12_000).step_by(500) {
        let row = |k: i64| format!("({k}, {}, {}, {}, {})", k % 3, k - k % 16, k % 1000, b(k));
        let values: Vec<String> = (first..first + 500).map(row).collect();
        s.run(&format!("INSERT INTO W VALUES {}", values.join(", "))).unwrap();
    }
    s.commit().unwrap();
    db.maintain().unwrap();
}

/// Groupings of W whose exact totals leave 64 bits in some groups and not
/// in others: of few groups, of many whose rows lie in one morsel and in
/// twelve, with no grouping, as a pair of groupings, and their first rows
/// by a sum.
const WIDE_SUMS: [&str; 7] = [
    "SELECT G, COUNT(*), SUM(B), AVG(B), MIN(B), MAX(B) FROM W GROUP BY G",
    "SELECT J, COUNT(B), SUM(B), AVG(B) FROM W GROUP BY J",
    "SELECT H, SUM(B), AVG(B), MIN(B), MAX(B) FROM W GROUP BY H",
    "SELECT COUNT(*), SUM(B), AVG(B) FROM W",
    "SELECT H, COUNT(DISTINCT G), SUM(B), AVG(B) FROM W GROUP BY H",
    "SELECT J, SUM(B) AS S FROM W GROUP BY J ORDER BY S DESC, J FETCH FIRST 5 ROWS ONLY",
    "SELECT H, SUM(B) AS S, AVG(B) FROM W GROUP BY H ORDER BY S, H FETCH FIRST 5 ROWS ONLY",
];

/// Verifies: [DESIGN] (RULES.md D1308, D1226) groupings whose sums and
/// means of BIGINT values leave 64 bits in some groups, above and below,
/// and stay within them in the others, run in parallel in memory and
/// again at the smallest MEMORY.WORK_BYTES, their partitions written to
/// the spill and read back, give the rows they give run alone a row at a
/// time, in their order: the totals past 64 bits among them, of a group
/// whose rows one morsel holds and of one whose morsels' totals leave 64
/// bits only as they are merged.
#[test]
fn sums_that_leave_64_bits_come_as_alone_in_memory_and_spilled() {
    let (db, a, threads) = table();
    wide_sums(&db, &a);
    let alone_rows: Vec<Vec<String>> = WIDE_SUMS.iter().map(|q| alone(&a, q)).collect();
    for (n, rows, held) in [(1, 16, "a morsel"), (2, 12, "a merge")] {
        let (shown, most) = (alone_rows[n].join("\n"), (rows * i128::from(BIG)).to_string());
        assert!(shown.contains(&most) && shown.contains(&format!("-{most}")), "{}: totals past 64 bits in {held}", WIDE_SUMS[n]);
    }
    for (q, expected) in WIDE_SUMS.iter().zip(&alone_rows) {
        assert_eq!(&parallel(&a, q), expected, "{q}, in memory");
        in_parallel(&a, q);
    }
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(SMALL, 0))).unwrap();
    threads.taken();
    for (q, expected) in WIDE_SUMS.iter().zip(&alone_rows) {
        assert_eq!(&parallel(&a, q), expected, "{q}, at the smallest grant");
    }
    assert!(threads.taken().written > 0, "partitions of exact totals went to the spill");
}
