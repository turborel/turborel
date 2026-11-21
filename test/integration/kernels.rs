//! \[DESIGN\] The kernels (RULES.md D354): the portable ones keep the
//! contract every implementation keeps (`support/src/kernels.rs`), and a
//! database runs its batches, its vector distances and its checksums on
//! the kernels it is given.

use std::sync::Arc;

use turborel::db::{Database, OpenOptions};
use turborel::kernels::{DistanceTerm, Kernels, PORTABLE, Packed, Portable};

use crate::common::{Scratch, both_ways, exec, os, rows};
use crate::kernel_contract::contract;

/// Verifies: [DESIGN] RULES.md D354 each portable kernel gives what a plain
/// loop gives, at every length to past two mask words, of every operator,
/// nulls and selections, and the extremes of each type.
#[test]
fn portable_kernels_keep_the_contract() {
    assert_eq!(Portable.name(), PORTABLE);
    contract(&Portable);
}

/// Kernels that count their selections, each the portable one's, their
/// decimals decoded, sort keys made, and keys hashed.
struct Counting(Arc<std::sync::atomic::AtomicUsize>);

impl Kernels for Counting {
    fn name(&self) -> &'static str {
        "COUNTING"
    }

    fn select_i64(&self, values: &[i64], pred: (turborel::value::CompOp, i64), given: turborel::kernels::Given<'_>, out: &mut Vec<u32>) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Portable.select_i64(values, pred, given, out);
    }

    fn alp_decode(&self, words: &mut [i64], scale: turborel::kernels::AlpScale) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Portable.alp_decode(words, scale);
    }

    fn normalize_i64(&self, given: (&[i64], &[u32]), shape: (i64, usize, bool), place: (usize, usize), keys: &mut [u8]) -> Option<()> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Portable.normalize_i64(given, shape, place, keys)
    }

    fn hash_i64(&self, values: &[i64], seed: u64, sel: Option<&[u32]>, out: &mut Vec<u64>) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Portable.hash_i64(values, seed, sel, out);
    }

    fn combine_i64(&self, hashes: &mut [u64], next: &[u64], seed: u64) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Portable.combine_i64(hashes, next, seed);
    }

    fn hash_texts(&self, given: (&[u8], &[usize]), seeded: &turborel::hash::xxhash::Seeded, sel: Option<&[u32]>, out: &mut Vec<u64>) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Portable.hash_texts(given, seeded, sel, out);
    }
}

/// Verifies: [DESIGN] RULES.md D354, D460 a database whose kernels are
/// registered (`Database::register_kernels`) judges its batches' filters by
/// them, with the rows the portable ones keep; a second set is refused
/// (SQLCODE -910); EXECUTOR.KERNELS = 'PORTABLE' runs the portable ones.
#[test]
fn a_database_runs_the_kernels_registered() {
    let db = Database::new();
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    db.register_kernels(Box::new(Counting(Arc::clone(&count)))).unwrap();
    let again = db.register_kernels(Box::new(Portable)).unwrap_err();
    assert_eq!(again.sqlcode(), -910, "{again}");
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N INTEGER)").unwrap();
    let a = db.session("A");
    let values: Vec<String> = (0..3000).map(|k| format!("({k}, {})", k % 17)).collect();
    a.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    exec(&a, "SET EXECUTOR.DEGREE = 1").unwrap();
    let (batched, by_rows) = both_ways(&a, "SELECT COUNT(*), SUM(K) FROM T WHERE N < 5");
    assert_eq!(batched, by_rows);
    let counted = count.load(std::sync::atomic::Ordering::Relaxed);
    assert!(counted > 0, "no selection by the kernels registered");
    exec(&a, "SET EXECUTOR.KERNELS = 'PORTABLE'").unwrap();
    let (forced, _) = both_ways(&a, "SELECT COUNT(*), SUM(K) FROM T WHERE N < 5");
    assert_eq!(forced, batched);
    assert_eq!(count.load(std::sync::atomic::Ordering::Relaxed), counted, "PORTABLE ran the registered kernels");
}

/// Verifies: [DESIGN] RULES.md D570 the kernels registered reach the
/// column store, its scans' filters judged and its decimals decoded by
/// them, a table opened after they are registered; and a sort's keys are
/// made by them unless EXECUTOR.KERNELS = 'PORTABLE', which no sort then
/// passes by.
#[test]
fn the_kernels_registered_reach_storage_and_sorts() {
    use std::sync::atomic::Ordering::Relaxed;
    let db = Database::new();
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    db.register_kernels(Box::new(Counting(Arc::clone(&count)))).unwrap();
    db.set_job_policy(turborel::threads::JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE C (K INTEGER, F DOUBLE PRECISION) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    let values: Vec<String> = (0..5000).map(|k| format!("({k}, {}.25)", k % 300)).collect();
    a.run(&format!("INSERT INTO C VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    exec(&a, "SET EXECUTOR.DEGREE = 1").unwrap();
    let (filtered, by_rows) = both_ways(&a, "SELECT COUNT(*), SUM(F) FROM C WHERE K < 1000");
    assert_eq!(filtered, by_rows);
    assert_eq!(filtered, vec!["1000,1.3975E5"], "the rows the portable kernels keep");
    assert!(count.load(Relaxed) > 0, "no kernel registered ran in the column store");
    let sorted = |a: &turborel::db::Session| {
        let before = count.load(Relaxed);
        let rows = crate::common::sorted(a, "SELECT K FROM C ORDER BY K DESC");
        (rows, count.load(Relaxed) - before)
    };
    let (by_registered, made) = sorted(&a);
    exec(&a, "SET EXECUTOR.KERNELS = 'PORTABLE'").unwrap();
    let (by_portable, made_portable) = sorted(&a);
    assert_eq!(by_registered, by_portable);
    assert!(made > 0, "no sort key made by the kernels registered");
    assert_eq!(made_portable, 0, "PORTABLE ran the registered kernels in a sort");
}

/// Kernels that tally each sum, extreme and gather they make, by name,
/// each the portable one's.
struct Tally(Arc<std::sync::Mutex<std::collections::BTreeMap<&'static str, usize>>>);

impl Tally {
    fn count(&self, name: &'static str) {
        if let Ok(mut tally) = self.0.lock() {
            *tally.entry(name).or_default() += 1;
        }
    }
}

impl Kernels for Tally {
    fn name(&self) -> &'static str {
        "TALLY"
    }

    fn sum_i64(&self, values: &[i64], sel: Option<&[u32]>) -> i128 {
        self.count("sum_i64");
        Portable.sum_i64(values, sel)
    }

    fn min_max_i64(&self, values: &[i64], given: turborel::kernels::Given<'_>) -> Option<(i64, i64)> {
        self.count("min_max_i64");
        Portable.min_max_i64(values, given)
    }

    fn gather_i64(&self, values: &[i64], sel: &[u32], out: &mut Vec<i64>) {
        self.count("gather_i64");
        Portable.gather_i64(values, sel, out);
    }
}

/// Verifies: [DESIGN] RULES.md D570 [X3.135-1986 §5.8 GR1]: set functions
/// over a batch's integers sum and compare them on the kernels
/// registered, and a join's, a projection's and a top-N's rows gather
/// them so, of the row store and the column store alike; each query
/// gives what it gives on the portable kernels (EXECUTOR.KERNELS =
/// 'PORTABLE') and read row by row, nulls left out alike.
#[test]
fn set_functions_and_gathers_run_on_the_kernels_registered() {
    let db = Database::new();
    let tally = Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new()));
    db.register_kernels(Box::new(Tally(Arc::clone(&tally)))).unwrap();
    db.set_job_policy(turborel::threads::JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE R (K INTEGER, G INTEGER, V BIGINT, N INTEGER)
           CREATE TABLE C (K INTEGER, G INTEGER, V BIGINT, N INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)
           CREATE TABLE H (K INTEGER, W INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    let rows: Vec<String> = (0..6000_i64)
        .map(|k| {
            let n = if k % 7 == 0 { "NULL".to_owned() } else { (k * 31 % 1000 - 500).to_string() };
            format!("({k}, {}, {}, {n})", k % 13, (k * 7_919) % 100_003 - 50_000)
        })
        .collect();
    for table in ["R", "C"] {
        a.run(&format!("INSERT INTO {table} VALUES {}", rows.join(", "))).unwrap();
    }
    let held: Vec<String> = (0..500_i64).map(|k| format!("({}, {k})", k * 11)).collect();
    a.run(&format!("INSERT INTO H VALUES {}", held.join(", "))).unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    exec(&a, "SET EXECUTOR.DEGREE = 1").unwrap();
    for table in ["R", "C"] {
        for sql in [
            format!("SELECT SUM(V), MIN(V), MAX(V), SUM(N), MIN(N), MAX(N), AVG(N), COUNT(N) FROM {table}"),
            format!("SELECT G, SUM(V), MIN(N), MAX(N) FROM {table} GROUP BY G"),
            format!("SELECT SUM(V), MAX(N) FROM {table} WHERE K < 3000"),
            format!("SELECT {table}.K, {table}.V, H.W FROM {table} JOIN H ON {table}.K = H.K"),
            format!("SELECT K, N FROM {table} WHERE G = 3"),
            format!("SELECT K, V FROM {table} ORDER BY V DESC FETCH FIRST 20 ROWS ONLY"),
        ] {
            exec(&a, "SET EXECUTOR.KERNELS = ''").unwrap();
            let (registered, by_rows) = both_ways(&a, &sql);
            exec(&a, "SET EXECUTOR.KERNELS = 'PORTABLE'").unwrap();
            let (portable, _) = both_ways(&a, &sql);
            assert_eq!(registered, by_rows, "{sql}");
            assert_eq!(registered, portable, "{sql}");
        }
    }
    let tally = tally.lock().unwrap().clone();
    for name in ["sum_i64", "min_max_i64", "gather_i64"] {
        assert!(tally.get(name).is_some_and(|&n| n > 0), "{name} never ran: {tally:?}");
    }
}

/// Verifies: [DESIGN] RULES.md D573 the kernels registered hash a
/// grouping's keys, of integers and of texts, and join a key's columns,
/// with the groups the portable ones find, by one thread and by several,
/// a set operation's rows too; EXECUTOR.KERNELS = 'PORTABLE' runs none.
#[test]
fn the_kernels_registered_hash_groupings() {
    use std::sync::atomic::Ordering::Relaxed;
    let db = Database::new();
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    db.register_kernels(Box::new(Counting(Arc::clone(&count)))).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE G (K INTEGER, S VARCHAR(20))").unwrap();
    let a = db.session("A");
    let values: Vec<String> = (0..6000).map(|k| format!("({}, 'name {}  ')", k % 37, k % 11)).collect();
    a.run(&format!("INSERT INTO G VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    let query = "SELECT K, S, COUNT(*) FROM G GROUP BY K, S";
    let distinct = "SELECT S, K FROM G INTERSECT SELECT S, K FROM G WHERE K < 20";
    let run = |a: &turborel::db::Session| {
        let before = count.load(Relaxed);
        let rows = (crate::common::sorted(a, query), crate::common::sorted(a, distinct));
        (rows, count.load(Relaxed) - before)
    };
    exec(&a, "SET EXECUTOR.DEGREE = 1").unwrap();
    let (one, hashed) = run(&a);
    assert_eq!(one.0.len(), 37 * 11);
    assert!(hashed > 0, "no key hashed by the kernels registered");
    exec(&a, "SET EXECUTOR.DEGREE = 4").unwrap();
    assert_eq!(run(&a).0, one);
    exec(&a, "SET EXECUTOR.KERNELS = 'PORTABLE'").unwrap();
    let (portable, none) = run(&a);
    assert_eq!(portable, one);
    assert_eq!(none, 0, "PORTABLE ran the registered kernels in a grouping");
}

/// Kernels that count their distance sums and their CRCs, each the
/// portable one's.
struct Measuring {
    sums: Arc<std::sync::atomic::AtomicUsize>,
    crcs: Arc<std::sync::atomic::AtomicUsize>,
}

impl Measuring {
    fn summed(&self) {
        self.sums.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Kernels for Measuring {
    fn name(&self) -> &'static str {
        "MEASURING"
    }

    fn distance_f32(&self, a: &[f32], b: &[f32], term: DistanceTerm) -> f32 {
        self.summed();
        Portable.distance_f32(a, b, term)
    }

    fn cosine_f32(&self, a: &[f32], b: &[f32]) -> (f32, f32, f32) {
        self.summed();
        Portable.cosine_f32(a, b)
    }

    fn distance_packed(&self, a: &[u8], b: &[u8], shape: (Packed, DistanceTerm)) -> f32 {
        self.summed();
        Portable.distance_packed(a, b, shape)
    }

    fn distance_i8(&self, a: &[u8], b: &[u8], term: DistanceTerm) -> i64 {
        self.summed();
        Portable.distance_i8(a, b, term)
    }

    fn crc32c(&self, register: u32, data: &[u8]) -> u32 {
        self.crcs.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Portable.crc32c(register, data)
    }
}

/// A vector of 40 elements of `k`, whole numbers an INT8 holds, no two
/// rows' alike.
fn vector_of(k: i64) -> String {
    let elements: Vec<String> = (0..40).map(|d| ((k * 7 + d * 13 + k * d * d) % 201 - 100).to_string()).collect();
    format!("[{}]", elements.join(", "))
}

/// Verifies: [DESIGN] RULES.md D571, D572 the kernels registered sum the
/// built-in metrics' distances of FLOAT32, FLOAT16 and INT8 vectors, and
/// an HNSW index's, each the distance the portable kernels give; and they
/// checksum the files by CRC-32/ISCSI where STORAGE.CHECKSUM names it,
/// whose pages a database without them reads.
#[test]
fn the_kernels_registered_measure_vectors_and_checksum_files() {
    use std::sync::atomic::Ordering::Relaxed;
    let scratch = Scratch::new("kernels-measure");
    let (sums, crcs) = (Arc::new(std::sync::atomic::AtomicUsize::new(0)), Arc::new(std::sync::atomic::AtomicUsize::new(0)));
    let db = Database::new();
    db.register_kernels(Box::new(Measuring { sums: Arc::clone(&sums), crcs: Arc::clone(&crcs) })).unwrap();
    db.set_setting("STORAGE.CHECKSUM", &turborel::value::Value::Char("CRC-32/ISCSI".to_owned())).unwrap();
    db.open_files(&os(), &scratch.dir, &OpenOptions::default()).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE V (K INTEGER PRIMARY KEY, E VECTOR(40), H VECTOR(40, FLOAT16), I VECTOR(40, INT8))",
    )
    .unwrap();
    let a = db.session("A");
    for k in 0..300 {
        let v = vector_of(k);
        a.run(&format!("INSERT INTO V VALUES ({k}, '{v}', '{v}', '{v}')")).unwrap();
    }
    a.commit().unwrap();
    let q = vector_of(7);
    let measure = format!(
        "SELECT K, VECTOR_DISTANCE(E, '{q}', COSINE), VECTOR_DISTANCE(H, CAST('{q}' AS VECTOR(40, FLOAT16)), EUCLIDEAN), \
         VECTOR_DISTANCE(I, CAST('{q}' AS VECTOR(40, INT8)), DOT) FROM V ORDER BY K"
    );
    let measured = rows(&a, &measure).unwrap();
    let nearest = format!("SELECT K FROM V ORDER BY VECTOR_DISTANCE(E, '{q}', COSINE), K FETCH FIRST 1 ROWS ONLY");
    let exact = rows(&a, &nearest).unwrap();
    a.commit().unwrap();
    assert!(sums.load(Relaxed) >= 900, "the metrics summed on the kernels registered {} times", sums.load(Relaxed));
    a.run("CREATE INDEX V_E ON V (E) USING hnsw WITH (HNSW.METRIC = COSINE)").unwrap();
    a.commit().unwrap();
    let before = sums.load(Relaxed);
    let near = format!("SELECT K FROM V ORDER BY VECTOR_DISTANCE(E, '{q}', COSINE) FETCH APPROX FIRST 3 ROWS ONLY");
    let found = rows(&a, &near).unwrap();
    assert!(found.contains(&exact[0]), "the index finds the nearest row {exact:?}: {found:?}");
    assert!(sums.load(Relaxed) > before, "the index measured on the kernels registered");
    a.commit().unwrap();
    drop(a);
    db.checkpoint().unwrap();
    assert!(crcs.load(Relaxed) > 0, "no page checksummed by the kernels registered");
    db.close().unwrap();
    drop(db);
    let plain = Database::new();
    plain.open_files(&os(), &scratch.dir, &OpenOptions::default()).unwrap();
    assert_eq!(rows(&plain.session("A"), &measure).unwrap(), measured, "the portable kernels' distances, of the pages read back");
}
