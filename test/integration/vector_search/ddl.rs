//! CREATE VECTOR INDEX in the vector design document's Oracle and SQL
//! Server forms, CREATE INDEX ... USING of a vector method, and ALTER
//! INDEX REBUILD, MERGE DELTA, LOAD and UNLOAD (§10.2, §10.3, §10.6),
//! each desugared to an index of a method and its options, read back
//! from INFORMATION_SCHEMA.INDEX_OPTIONS. Fake methods stand in for the
//! engines under their names.

use turborel::db::Database;
use turborel::error::Code;

use super::fixture::{Log, Method, database, session};
use crate::common::rows;

/// `database`, with fakes named as the vector methods are.
fn methods() -> (Database, Log) {
    let (db, log) = database(10);
    for name in ["hnsw", "ivf", "diskann", "inverted"] {
        db.register_index_method(Box::new(Method { name, log: log.clone() })).unwrap();
    }
    (db, log)
}

/// The method of index `name`, then its options set, as `METHOD;OPTION=VALUE...`.
fn described(db: &Database, name: &str) -> String {
    let s = session(db);
    let method = rows(&s, &format!("SELECT METHOD_NAME FROM INFORMATION_SCHEMA.INDEXES WHERE INDEX_NAME = '{name}'")).unwrap();
    let options = rows(
        &s,
        &format!(
            "SELECT OPTION_NAME || '=' || OPTION_VALUE FROM INFORMATION_SCHEMA.INDEX_OPTIONS \
             WHERE INDEX_NAME = '{name}' AND VALUE_SOURCE <> 'DEFAULT' ORDER BY OPTION_NAME"
        ),
    )
    .unwrap();
    format!("{};{}", method.join(""), options.join(";"))
}

/// Verifies: [EXTENSION] vector_standard §10.2: each ORGANIZATION names
/// its method, INMEMORY NEIGHBOR GRAPH hnsw, NEIGHBOR PARTITIONS ivf,
/// DISK NEIGHBOR GRAPH diskann, INVERTED inverted, diskann of none; each
/// clause an option of the method's group: DISTANCE its METRIC, TARGET
/// ACCURACY its TARGET_ACCURACY, PARAMETERS each its own, Oracle's HNSW
/// and IVF names as the methods name them, QUANTIZATION and PQ(m) its
/// QUANTIZER and PQ_BYTES, RERANK its RERANK; LOCAL, GLOBAL, ONLINE and
/// PARALLEL nothing.
#[test]
fn oracle_forms_desugar_to_a_method_and_options_10_2() {
    let (db, _) = methods();
    let s = session(&db);
    for (sql, expected) in [
        ("CREATE VECTOR INDEX I1 ON T (X)", "diskann;"),
        (
            "CREATE VECTOR INDEX I2 ON T (X) ORGANIZATION INMEMORY NEIGHBOR GRAPH DISTANCE COSINE WITH TARGET ACCURACY 95 \
             PARAMETERS (TYPE HNSW, NEIGHBORS 32, EFCONSTRUCTION 300) ONLINE PARALLEL 4",
            "hnsw;HNSW.EF_CONSTRUCTION=300;HNSW.M=32;HNSW.METRIC=COSINE;HNSW.TARGET_ACCURACY=95",
        ),
        (
            "CREATE VECTOR INDEX I3 ON T (X) ORGANIZATION NEIGHBOR PARTITIONS DISTANCE EUCLIDEAN \
             PARAMETERS (TYPE IVF, NEIGHBOR PARTITIONS 16, SAMPLE_PER_PARTITION 64) LOCAL",
            "ivf;IVF.LISTS=16;IVF.METRIC=EUCLIDEAN;IVF.SAMPLE_PER_LIST=64",
        ),
        (
            "CREATE VECTOR INDEX I4 ON T (X) ORGANIZATION DISK NEIGHBOR GRAPH DISTANCE DOT \
             PARAMETERS (TYPE DISKANN, MAX_DEGREE 48, BUILD_LIST 80, ALPHA 1.3) QUANTIZATION PQ(16) RERANK OFF GLOBAL",
            "diskann;DISKANN.ALPHA=1.3E0;DISKANN.BUILD_LIST=80;DISKANN.MAX_DEGREE=48;DISKANN.METRIC=DOT;DISKANN.PQ_BYTES=16;DISKANN.QUANTIZER=PQ;DISKANN.RERANK=OFF",
        ),
        (
            "CREATE VECTOR INDEX I5 ON T (X) ORGANIZATION INVERTED PARAMETERS (TYPE INVERTED, BLOCK_SIZE 64) QUANTIZATION SQ8",
            "inverted;INVERTED.BLOCK_SIZE=64;INVERTED.QUANTIZATION=SQ8",
        ),
        ("CREATE VECTOR INDEX I6 ON T (X) QUANTIZATION BQ", "diskann;DISKANN.QUANTIZER=BQ"),
    ] {
        s.run(sql).unwrap();
        let name = sql.split_whitespace().nth(3).unwrap();
        assert_eq!(described(&db, name), expected, "{sql}");
    }
}

/// Verifies: [EXTENSION] vector_standard §10.3: SQL Server's WITH form
/// names the method by TYPE, its METRIC, TARGET_ACCURACY and QUANTIZATION
/// the method's options; MAXDOP and ONLINE nothing; and an option with a
/// period is the method's as written. CREATE INDEX ... USING names a
/// vector method too, SPARSE the method inverted.
#[test]
fn sql_server_form_and_using_desugar_10_3() {
    let (db, _) = methods();
    let s = session(&db);
    s.run("CREATE VECTOR INDEX J1 ON T (X) WITH (METRIC = 'cosine', TYPE = 'HNSW', TARGET_ACCURACY = 90, QUANTIZATION = 'SQ8', MAXDOP = 4, ONLINE = ON)").unwrap();
    assert_eq!(described(&db, "J1"), "hnsw;HNSW.METRIC=COSINE;HNSW.QUANTIZER=SQ8;HNSW.TARGET_ACCURACY=90");
    s.run("CREATE VECTOR INDEX J2 ON T (X) WITH (TYPE = 'DiskANN', DISKANN.SEARCH_LIST = 120)").unwrap();
    assert_eq!(described(&db, "J2"), "diskann;DISKANN.SEARCH_LIST=120");
    s.run("CREATE VECTOR INDEX J3 ON T (X) USING ivf WITH (METRIC = DOT)").unwrap();
    assert_eq!(described(&db, "J3"), "ivf;IVF.METRIC=DOT");
    s.run("CREATE INDEX J4 ON T (X) USING sparse").unwrap();
    s.run("CREATE INDEX J5 ON T (X) USING hnsw WITH (HNSW.M = 8)").unwrap();
    assert_eq!(described(&db, "J4"), "inverted;");
    assert_eq!(described(&db, "J5"), "hnsw;HNSW.M=8");
}

/// Verifies: [EXTENSION] vector_standard §10.2: an index is of one type,
/// so an ORGANIZATION and a TYPE apart are refused (SQLCODE -103), as are
/// an unknown type, PQ of other than 8 bits, and an option the method does
/// not declare.
#[test]
fn conflicting_or_unknown_types_are_refused_10_2() {
    let (db, _) = methods();
    let s = session(&db);
    for sql in [
        "CREATE VECTOR INDEX K ON T (X) ORGANIZATION INMEMORY NEIGHBOR GRAPH PARAMETERS (TYPE IVF)",
        "CREATE VECTOR INDEX K ON T (X) PARAMETERS (TYPE FLAT)",
        "CREATE VECTOR INDEX K ON T (X) WITH (TYPE = 'FLAT')",
        "CREATE VECTOR INDEX K ON T (X) QUANTIZATION PQ(8, 4)",
        "CREATE VECTOR INDEX K ON T (X) PARAMETERS (TYPE HNSW, WIDTH 3)",
    ] {
        assert_eq!(s.run(sql).unwrap_err().code, Code::SyntaxRule, "{sql}");
    }
    assert_eq!(s.run("CREATE VECTOR INDEX K ON T (X) ORGANIZATION FLAT").unwrap_err().code, Code::Syntax);
}

/// Verifies: [EXTENSION] vector_standard §10.6: ALTER INDEX REBUILD builds
/// the index again, of the parameters given, named as CREATE VECTOR INDEX
/// names them; MERGE DELTA runs its maintenance now; LOAD and UNLOAD tell
/// it to take its structure into memory or let it go. Each is the table
/// owner's, outside a transaction, of an index that is there (-103).
#[test]
fn alter_index_rebuilds_merges_loads_and_unloads_10_6() {
    let (db, log) = methods();
    let s = session(&db);
    s.run("CREATE VECTOR INDEX V ON T (X) ORGANIZATION INMEMORY NEIGHBOR GRAPH PARAMETERS (TYPE HNSW, NEIGHBORS 16)").unwrap();
    s.run("ALTER INDEX V REBUILD ONLINE PARAMETERS (NEIGHBORS 24, EFCONSTRUCTION 100)").unwrap();
    assert_eq!(described(&db, "V"), "hnsw;HNSW.EF_CONSTRUCTION=100;HNSW.M=24");
    s.run("ALTER INDEX V REBUILD").unwrap();
    log.calls();
    s.run("ALTER INDEX V MERGE DELTA").unwrap();
    s.run("ALTER INDEX V LOAD").unwrap();
    s.run("ALTER INDEX V UNLOAD").unwrap();
    assert_eq!(log.calls(), ["maintain", "load", "unload"]);
    assert_eq!(s.run("ALTER INDEX NONE_SUCH LOAD").unwrap_err().code, Code::SyntaxRule);
    assert_eq!(s.run("ALTER INDEX V MERGE").unwrap_err().code, Code::Syntax);
}
