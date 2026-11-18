//! Vector indexes that keep codes (RULES.md D93, D96, D97, D98): each
//! method trains the codebook of the quantizer it names in maintenance,
//! answers from its codes, and reads the codebook back as it reopens;
//! each built-in quantizer serves each method.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use turborel::db::{Database, OpenOptions, Opened, Session};
use turborel::error::Result;
use turborel::vector::quantizer::{BQ, Codebook, PQ, Quantizer, SQ8, built_in_quantizers};

use super::fixture::{K, METHODS, database, insert, nearest_sql, populate, register};
use crate::common::{Scratch, after_crash, rows};

/// The rows of the tables the tests index: enough for IVF to train its
/// lists (16 lists of 39 rows), and past `TRAIN_AT`.
const ROWS: u64 = 700;
/// The rows an index of a graph holds before maintenance trains its
/// codes, so the tables' rows reach it.
const TRAIN_AT: u64 = 200;
/// The queries agreement is measured over.
const QUERIES: u64 = 8;
/// IVF's lists scanned: every list, so its codes alone set what it finds.
const EVERY_LIST: u64 = 64;

/// The quantizer the tests register, counting its calls.
const COUNTED: &str = "COUNTED";

/// How often a codebook was trained and read back.
#[derive(Default)]
struct Calls {
    trained: AtomicUsize,
    read: AtomicUsize,
}

/// The built-in PQ under the name COUNTED, counting what it is asked.
struct Counted(Arc<Calls>, Arc<dyn Quantizer>);

impl Quantizer for Counted {
    fn name(&self) -> &str {
        COUNTED
    }
    fn train(
        &self,
        sample: &mut dyn Iterator<Item = Result<Vec<f32>>>,
        dims: u32,
        code_bytes: usize,
        seed: u64,
    ) -> Result<Box<dyn Codebook>> {
        self.0.trained.fetch_add(1, Ordering::SeqCst);
        self.1.train(sample, dims, code_bytes, seed)
    }
    fn read(&self, bytes: &[u8]) -> Result<Box<dyn Codebook>> {
        self.0.read.fetch_add(1, Ordering::SeqCst);
        self.1.read(bytes)
    }
}

/// The built-in quantizer `name`.
fn built_in(name: &str) -> Arc<dyn Quantizer> {
    built_in_quantizers()().into_iter().find(|q| q.name() == name).unwrap()
}

/// CREATE INDEX of `method` on T(V) by EUCLIDEAN, keeping codes of
/// `quantizer`, a graph's trained at `TRAIN_AT` rows; IVF scanning every
/// list.
fn create(s: &Session, method: &str, quantizer: &str) {
    let group = method.to_ascii_uppercase();
    let more = match method {
        "ivf" => format!("IVF.PROBES = {EVERY_LIST}"),
        _ => format!("{group}.TRAIN_AT = {TRAIN_AT}"),
    };
    s.run(&format!("CREATE INDEX T_V ON T (V) USING {method} WITH ({group}.METRIC = EUCLIDEAN, {group}.QUANTIZER = {quantizer}, {more})"))
        .unwrap();
    s.commit().unwrap();
}

/// The share of the exact `K` nearest rows FETCH APPROX finds, over
/// `QUERIES` queries, each answering the `K` rows asked.
fn agreement(s: &Session) -> f64 {
    let (mut found, mut asked) = (0_u32, 0_u32);
    for n in 0..QUERIES {
        let exact = rows(s, &nearest_sql("L2_DISTANCE", n, ("EXACT", ""))).unwrap();
        let approx = rows(s, &nearest_sql("L2_DISTANCE", n, ("APPROX", ""))).unwrap();
        assert_eq!(approx.len(), K, "FETCH APPROX answers the rows asked");
        asked += u32::try_from(exact.len()).unwrap();
        found += u32::try_from(approx.iter().filter(|r| exact.contains(r)).count()).unwrap();
    }
    f64::from(found) / f64::from(asked.max(1))
}

/// The answers of FETCH APPROX to each query.
fn answers(s: &Session) -> Vec<Vec<String>> {
    (0..QUERIES).map(|n| rows(s, &nearest_sql("L2_DISTANCE", n, ("APPROX", ""))).unwrap()).collect()
}

/// The database in `dir`, COUNTED registered, counting into `calls`.
fn open(dir: &Path, calls: &Arc<Calls>) -> (Database, Opened) {
    let db = Database::new();
    register(&db);
    db.register_quantizer(Box::new(Counted(Arc::clone(calls), built_in(PQ)))).unwrap();
    let opened = db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    (db, opened)
}

/// Verifies: [EXTENSION] RULES.md D93, D96, D97, D98, D25: an index of
/// each method naming a registered quantizer trains its codebook through
/// it, once, in maintenance, not as it is made; answering from its codes,
/// it finds nine in ten of the exact nearest rows; and, the database
/// closed and reopened, it reads the codebook back through the quantizer,
/// training none, and answers as it did. Regression test (R-TEST-7): a
/// DiskANN index's training wrote its last code page short of the codes
/// a page has room for, which opening refuses as malformed (SQLCODE
/// -916), so the database could not be opened.
#[test]
fn each_method_trains_and_reads_back_its_quantizers_codebook() {
    for method in METHODS {
        let scratch = Scratch::new(&format!("vector-codes-{method}"));
        let dir = scratch.path("db");
        let calls = Arc::new(Calls::default());
        let (db, _) = open(&dir, &calls);
        populate(&db, ROWS);
        let s = db.session("S");
        create(&s, method, COUNTED);
        assert_eq!(calls.trained.load(Ordering::SeqCst), 0, "{method}: not trained as it is made");
        db.maintain().unwrap();
        assert_eq!(calls.trained.load(Ordering::SeqCst), 1, "{method}: trained in maintenance");
        let found = agreement(&s);
        assert!(found >= 0.9, "{method}: recall@10 from codes {found}");
        let before = answers(&s);
        s.commit().unwrap();
        drop(s);
        db.close().unwrap();
        drop(db);
        let again = Arc::new(Calls::default());
        let (db, opened) = open(&dir, &again);
        assert_eq!(opened, Opened::Reopened, "{method}");
        let s = db.session("S");
        assert_eq!(answers(&s), before, "{method}: answers as before");
        assert!(again.read.load(Ordering::SeqCst) >= 1, "{method}: its codebook read back");
        assert_eq!(again.trained.load(Ordering::SeqCst), 0, "{method}: none trained again");
    }
}

/// Verifies: [EXTENSION] RULES.md D93, D94, D96, D97, D98: each built-in
/// quantizer, PQ, SQ8 and BQ, serves each method: trained in maintenance,
/// the index searched from its codes answers the rows asked, rows
/// inserted after the training coded as they come; of PQ and SQ8 it finds
/// nine in ten of the exact nearest rows, of BQ, a bit a dimension, half.
#[test]
fn each_built_in_quantizer_serves_each_method() {
    for method in METHODS {
        for (quantizer, least) in [(PQ, 0.9), (SQ8, 0.9), (BQ, 0.5)] {
            let db = database(ROWS);
            let s = db.session("S");
            create(&s, method, quantizer);
            db.maintain().unwrap();
            insert(&s, (ROWS, ROWS + 100));
            let found = agreement(&s);
            assert!(found >= least, "{method}, {quantizer}: recall@10 from codes {found}");
        }
    }
}
