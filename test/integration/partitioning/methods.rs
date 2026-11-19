//! Each index method as an index of a partitioned table: made on each
//! partition, read through it, its rows those of the same table not
//! partitioned.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;

use crate::common::{exec, rows, sorted};

/// Schema S's table `name` of `columns`, partitioned by HASH of ID in two
/// partitions, `name`0 and `name`1, and the same table not partitioned,
/// `name`_FLAT, each holding `values`, and of each an index of
/// `index`, which names the table `{T}`; the session of S.
fn twins(columns: &str, values: &[String], index: &str) -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION S").unwrap();
    let s = db.session("S");
    s.run("SET SQL.FUNCTION_CALLS = 'ON'").unwrap();
    for sql in [
        format!("CREATE TABLE T ({columns}) PARTITION BY HASH (ID)"),
        "CREATE TABLE T0 PARTITION OF T FOR VALUES WITH (MODULUS 2, REMAINDER 0)".to_owned(),
        "CREATE TABLE T1 PARTITION OF T FOR VALUES WITH (MODULUS 2, REMAINDER 1)".to_owned(),
        format!("CREATE TABLE FLAT ({columns})"),
    ] {
        exec(&s, &sql).unwrap();
    }
    for table in ["T", "FLAT"] {
        for chunk in values.chunks(200) {
            exec(&s, &format!("INSERT INTO {table} VALUES {}", chunk.join(", "))).unwrap();
        }
    }
    exec(&s, "COMMIT").unwrap();
    for table in ["T", "FLAT"] {
        exec(&s, &index.replace("{T}", table).replace("{N}", &format!("{table}_X"))).unwrap();
    }
    (db, s)
}

/// That `sql`, of `{T}`, gives the partitioned table the rows it gives the
/// table not partitioned, and that its plan of the partitioned table holds
/// each of `shown`.
fn agree(s: &Session, sql: &str, shown: &[&str]) {
    let plan = s.physical_plan(&sql.replace("{T}", "T"), &Params::new()).unwrap();
    for part in shown {
        assert!(plan.contains(part), "{part} of {sql}\n{plan}");
    }
    let partitioned = rows(s, &sql.replace("{T}", "T"));
    assert!(partitioned.as_ref().is_ok_and(|r| !r.is_empty()), "{sql}: {partitioned:?}");
    assert_eq!(partitioned, rows(s, &sql.replace("{T}", "FLAT")), "{sql}");
}

/// That the template T_X has made an index of `method` on each partition.
fn made_on_each(s: &Session, method: &str) {
    let shown =
        sorted(s, "SELECT PARTITION_NAME || ' ' || METHOD_NAME FROM INFORMATION_SCHEMA.PARTITION_INDEXES WHERE TEMPLATE_NAME = 'T_X'");
    assert_eq!(shown, [format!("T0 {method}"), format!("T1 {method}")]);
}

/// Verifies: [EXTENSION] RULES.md D158: a hash index and a btree index of
/// an expression of a partitioned table are made on each partition, each
/// read for the partition's rows.
#[test]
fn hash_and_expression_indexes_are_made_on_each_partition() {
    let values: Vec<String> = (0..2000).map(|k| format!("({k}, 'name {}')", k % 500)).collect();
    let (_db, s) = twins("ID INTEGER, NAME VARCHAR(20)", &values, "CREATE INDEX {N} ON {T} (NAME) USING hash");
    made_on_each(&s, "hash");
    agree(&s, "SELECT ID FROM {T} WHERE NAME = 'name 7' ORDER BY ID", &["S.T0 using T_X", "S.T1 using T_X"]);
    exec(&s, "COMMIT").unwrap();
    for table in ["T", "FLAT"] {
        exec(&s, &format!("CREATE INDEX {table}_U ON {table} ((UPPER(NAME)))")).unwrap();
    }
    agree(&s, "SELECT ID FROM {T} WHERE UPPER(NAME) = 'NAME 9' ORDER BY ID", &["S.T0 using T_U", "S.T1 using T_U"]);
}

/// Verifies: [EXTENSION] RULES.md D156, D158: an rtree index of a
/// partitioned table serves its windows and its nearest reads, each
/// partition's read by its own index, the first rows of all by distance
/// those of the table not partitioned.
#[test]
fn rtree_indexes_serve_windows_and_distances() {
    let values: Vec<String> = (0..2000).map(|k| format!("({k}, ST_POINT({k}, {k}, 4326))")).collect();
    let (_db, s) = twins("ID INTEGER, G GEOMETRY(4326)", &values, "CREATE INDEX {N} ON {T} (G) USING rtree");
    made_on_each(&s, "rtree");
    agree(&s, "SELECT ID FROM {T} WHERE ST_INTERSECTS(G, ST_POINT(5, 5, 4326)) = 1", &["T_X"]);
    let nearest = "SELECT ID FROM {T} ORDER BY ST_DISTANCE(G, ST_POINT(500.2, 500.2, 4326)), ID FETCH FIRST 5 ROWS ONLY";
    agree(&s, nearest, &["NearestScan S.T0 using T_X", "NearestScan S.T1 using T_X"]);
}

/// Verifies: [EXTENSION] RULES.md D158: a fulltext index of a partitioned
/// table finds the rows of each partition, and Rank scores them by its
/// partitions' statistics together, as the table not partitioned scores
/// them, whichever partitions a read prunes.
#[test]
fn fulltext_ranks_with_the_statistics_of_every_partition() {
    let words = ["red fox", "lazy red dog", "quick brown fox", "red red red", "blue whale"];
    let values: Vec<String> = (0..1000).map(|k| format!("({k}, '{} filler {k}')", words[k % words.len()])).collect();
    let (_db, s) = twins("ID INTEGER, BODY VARCHAR(100)", &values, "CREATE INDEX {N} ON {T} (BODY) USING fulltext");
    made_on_each(&s, "fulltext");
    agree(&s, "SELECT ID FROM {T} WHERE TEXT_MATCHES(BODY, 'brown fox') ORDER BY ID", &["using T_X"]);
    let ranked = "SELECT ID, CAST(RANK(BODY, '\"red\"') * 1000 AS INTEGER) FROM {T} WHERE ID IN (1, 3, 8) ORDER BY ID";
    agree(&s, ranked, &[]);
    let pruned = "SELECT CAST(RANK(BODY, '\"red\"') * 1000 AS INTEGER) FROM {T} WHERE ID = 3";
    agree(&s, pruned, &[]);
    skewed_ranks_agree();
}

/// That Rank scores the rows of a partitioned table as those of the table
/// not partitioned where one partition holds every row of the word and the
/// other none, so neither partition's statistics alone are the table's.
fn skewed_ranks_agree() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S").unwrap();
    let s = db.session("S");
    let words = ["red fox", "lazy red dog", "quick brown fox", "red red red"];
    let values: Vec<String> =
        (0..400).map(|k| format!("({k}, '{}')", if k < 100 { words[k % words.len()] } else { "blue whale" })).collect();
    for sql in [
        "CREATE TABLE T (ID INTEGER, BODY VARCHAR(100)) PARTITION BY RANGE (ID)",
        "CREATE TABLE T0 PARTITION OF T FOR VALUES FROM (0) TO (100)",
        "CREATE TABLE T1 PARTITION OF T FOR VALUES FROM (100) TO (MAXVALUE)",
        "CREATE TABLE FLAT (ID INTEGER, BODY VARCHAR(100))",
        &format!("INSERT INTO T VALUES {}", values.join(", ")),
        &format!("INSERT INTO FLAT VALUES {}", values.join(", ")),
        "COMMIT",
        "CREATE INDEX T_X ON T (BODY) USING fulltext",
        "CREATE INDEX FLAT_X ON FLAT (BODY) USING fulltext",
    ] {
        exec(&s, sql).unwrap();
    }
    agree(&s, "SELECT ID, CAST(RANK(BODY, '\"red\"') * 1000 AS INTEGER) FROM {T} WHERE ID IN (0, 1, 3) ORDER BY ID", &["'T_X'"]);
}

/// Verifies: [EXTENSION] RULES.md D158: a json index of a partitioned
/// table serves JSON_EXISTS on each partition.
#[test]
fn json_indexes_are_made_on_each_partition() {
    let values: Vec<String> = (0..2000).map(|k| format!("({k}, '{{\"a\": {{\"b\": {}}}}}')", k % 7)).collect();
    let (_db, s) = twins("ID INTEGER, DOC VARCHAR(100)", &values, "CREATE INDEX {N} ON {T} (DOC) USING json");
    made_on_each(&s, "json");
    agree(&s, "SELECT COUNT(*) FROM {T} WHERE JSON_EXISTS(DOC, 'lax $.a.b')", &[]);
}

/// The vector of row `k`, of 4 dimensions, near one of 8 centres.
fn vector(k: u64) -> String {
    let small = |n: u64| f64::from(u32::try_from(n).unwrap());
    let at = |d: u64| small((k % 8) * 10 + d) + small(k.wrapping_mul(2_654_435_761).wrapping_add(d * 40_503) % 1000) / 250.0;
    format!("TO_VECTOR('[{},{},{},{}]', 4, 'FLOAT32')", at(0), at(1), at(2), at(3))
}

/// Verifies: [EXTENSION] RULES.md D156, D158: an index of each vector
/// method of a partitioned table is made on each partition; FETCH APPROX
/// reads each partition's, its first rows by the exact distance of every
/// candidate, nine in ten of FETCH EXACT's; FETCH EXACT gives the rows of
/// the table not partitioned.
#[test]
fn vector_indexes_merge_their_first_rows() {
    let values: Vec<String> = (0..800_u64).map(|k| format!("({k}, {})", vector(k))).collect();
    for method in ["diskann", "hnsw", "ivf"] {
        let group = method.to_ascii_uppercase();
        let index = format!("CREATE INDEX {{N}} ON {{T}} (V) USING {method} WITH ({group}.METRIC = EUCLIDEAN)");
        let (_db, s) = twins("ID INTEGER, V VECTOR(4, FLOAT32)", &values, &index);
        made_on_each(&s, method);
        let (mut found, mut asked) = (0, 0);
        for q in 0..5 {
            let at = vector(q * 977 + 3);
            let sql = |fetch: &str| {
                format!("SELECT ID FROM {{T}} ORDER BY VECTOR_DISTANCE(V, {at}, 'EUCLIDEAN'), ID FETCH {fetch} FIRST 5 ROWS ONLY")
            };
            agree(&s, &sql("EXACT"), &[]);
            let approx = sql("APPROX").replace("{T}", "T");
            let plan = s.physical_plan(&approx, &Params::new()).unwrap();
            assert!(plan.contains("NearestScan S.T0 using T_X") && plan.contains("NearestScan S.T1 using T_X"), "{method}\n{plan}");
            let exact = rows(&s, &sql("EXACT").replace("{T}", "T")).unwrap();
            let got = rows(&s, &approx).unwrap();
            asked += exact.len();
            found += got.iter().filter(|r| exact.contains(r)).count();
        }
        assert!(found * 10 >= asked * 9, "{method}: {found} of {asked}");
    }
}

/// Verifies: [EXTENSION] RULES.md D158: an inverted index of a SPARSE
/// column and a hybrid index of a text and a vector column of a
/// partitioned table are made on each partition.
#[test]
fn sparse_and_hybrid_indexes_are_made_on_each_partition() {
    let sparse = |d: u64| format!("TO_VECTOR('[100,[{d},99],[1,0.5]]', 100, 'FLOAT32', 'SPARSE')");
    let values: Vec<String> = (0..400_u64).map(|k| format!("({k}, {}, 'text {k}', {})", sparse(k % 98), vector(k))).collect();
    let columns = "ID INTEGER, SV VECTOR(100, 257), BODY VARCHAR(40), EMB VECTOR(4, FLOAT32)";
    let (_db, s) = twins(columns, &values, "CREATE INDEX {N} ON {T} (SV) USING sparse");
    made_on_each(&s, "inverted");
    let nearest = format!("SELECT ID FROM {{T}} ORDER BY VECTOR_DISTANCE(SV, {}, 'DOT'), ID FETCH APPROX FIRST 3 ROWS ONLY", sparse(7));
    agree(&s, &nearest, &["NearestScan S.T0 using T_X", "NearestScan S.T1 using T_X"]);
    exec(&s, "COMMIT").unwrap();
    exec(&s, "CREATE HYBRID VECTOR INDEX T_H ON T (BODY, EMB)").unwrap();
    let made =
        sorted(&s, "SELECT PARTITION_NAME || ' ' || METHOD_NAME FROM INFORMATION_SCHEMA.PARTITION_INDEXES WHERE TEMPLATE_NAME = 'T_H'");
    assert_eq!(made, ["T0 hybrid", "T1 hybrid"]);
}
