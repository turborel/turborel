//! \[EXTENSION\] Embedding models (the vector design's §12.1, §12.2, §16):
//! CREATE, ALTER and DROP EMBEDDING MODEL, EXECUTE on one granted and
//! revoked, EMBED and EMBED_VERSION of one, its calls batched and their
//! failures, the views INFORMATION_SCHEMA.EMBEDDING_MODELS and
//! EMBEDDING_MODEL_PRIVILEGES, and models through reopen and crash
//! recovery. Pipelines and the queue after commit are `vector_pipelines.rs`.

pub mod counted;

use std::sync::atomic::Ordering;

use counted::{database, prepared, registered};
use turborel::db::{Database, OpenOptions, Session};
use turborel::error::Code;

use crate::common::{Scratch, after_crash, exec, rows};

/// The table U.T of `n` texts, `text` and its number.
fn texts(s: &Session, n: usize) {
    s.run("CREATE TABLE T (K INTEGER PRIMARY KEY, X VARCHAR(40))").unwrap();
    for k in 1..=n {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'text {k:02}')")).unwrap();
    }
}

/// Verifies: [EXTENSION] vector design §12.1, R12.1, R12.5: a model is
/// made of a plug-in registered and its options; EMBED of it, named by a
/// literal or a bare name, is of VECTOR of its shape, and EMBED_VERSION
/// is its name and version; EMBEDDING_MODELS shows it.
#[test]
fn a_model_is_made_of_a_plug_in_r12_1() {
    let (db, _) = database();
    let s = db.session("U");
    s.run("CREATE EMBEDDING MODEL M WITH (TYPE = 'HASH_DENSE', DIMENSIONS = 16, VERSION = 2, NGRAMS = 2)").unwrap();
    s.run("CREATE TABLE E (V VECTOR(16))").unwrap();
    s.run("INSERT INTO E VALUES (EMBED('M', 'some words')), (EMBED(U.M, 'other words', 'query')), (EMBED('\"U\".\"M\"', CAST(NULL AS VARCHAR(5))))").unwrap();
    assert_eq!(rows(&s, "SELECT V IS NULL FROM E").unwrap(), ["FALSE", "FALSE", "TRUE"]);
    assert_eq!(rows(&s, "VALUES (EMBED_VERSION(M), EMBED_VERSION('M', CAST(NULL AS VARCHAR(5))))").unwrap(), ["U.M:2,NULL"]);
    let shown = rows(
        &s,
        "SELECT MODEL_SCHEMA, MODEL_NAME, MODEL_TYPE, DIMENSIONS, ELEMENT_FORMAT, LAYOUT, MODEL_VERSION, MODEL_FILE, IS_REGISTERED, MODEL_OWNER FROM INFORMATION_SCHEMA.EMBEDDING_MODELS",
    );
    assert_eq!(shown.unwrap(), ["U,M,HASH_DENSE,16,FLOAT32,DENSE,2,NULL,YES,U"]);
    // EMBED of a plug-in by its name, as before.
    assert_eq!(rows(&s, "VALUES (EMBED('HASH_SPARSE', 'w') IS NULL)").unwrap(), ["FALSE"]);
}

/// Verifies: [EXTENSION] vector design §12.1 VEC-0030, VEC-0031: a model
/// is refused of a plug-in not registered, without TYPE, of a shape its
/// plug-in does not give, of an option the plug-in or the database does
/// not take, of a name its schema holds, or in a schema the user does not
/// own; a call naming no model is refused.
#[test]
fn a_model_is_refused_of_what_it_cannot_be_vec_0030() {
    let (db, _) = database();
    let s = db.session("U");
    for refused in [
        "CREATE EMBEDDING MODEL M WITH (TYPE = 'LOCAL_ONNX', DIMENSIONS = 384)",
        "CREATE EMBEDDING MODEL M WITH (DIMENSIONS = 16)",
        "CREATE EMBEDDING MODEL M WITH (TYPE = 'HASH_DENSE', FORMAT = FLOAT16)",
        "CREATE EMBEDDING MODEL M WITH (TYPE = 'HASH_DENSE', TOKENIZER = 'bert')",
        "CREATE EMBEDDING MODEL M WITH (TYPE = 'HASH_DENSE', ON_ERROR = 'MAYBE')",
        "CREATE EMBEDDING MODEL M WITH (TYPE = 'HASH_DENSE', BATCH_SIZE = 0)",
        "CREATE EMBEDDING MODEL M WITH (TYPE = 'HASH_DENSE', VERSION = 'two')",
        "CREATE EMBEDDING MODEL M WITH (TYPE = 'HASH_DENSE', NGRAMS = 9)",
        "CREATE EMBEDDING MODEL NOWHERE.M WITH (TYPE = 'HASH_DENSE')",
    ] {
        assert_eq!(exec(&s, refused), Err(Code::SyntaxRule), "{refused}");
    }
    assert_eq!(exec(&s, "CREATE EMBEDDING MODEL M WITH (TYPE = HASH_DENSE"), Err(Code::Syntax));
    s.run("CREATE EMBEDDING MODEL M WITH (TYPE = HASH_DENSE, FORMAT = FLOAT32, LAYOUT = DENSE)").unwrap();
    assert_eq!(exec(&s, "CREATE EMBEDDING MODEL M WITH (TYPE = 'HASH_SPARSE')"), Err(Code::SyntaxRule), "its name is taken");
    assert_eq!(exec(&db.session("V"), "CREATE EMBEDDING MODEL U.N WITH (TYPE = 'HASH_DENSE')"), Err(Code::Privilege));
    assert_eq!(rows(&s, "VALUES (EMBED('NONE', 'x'))"), Err(Code::SyntaxRule));
    assert_eq!(rows(&s, "VALUES (EMBED('M', 1))"), Err(Code::SyntaxRule));
}

/// Verifies: [EXTENSION] vector design §16: EXECUTE on a model is its
/// schema's owner's, grantably; a call of it needs it; it is granted and
/// revoked, with a grant made of it abandoned only by CASCADE, and
/// EMBEDDING_MODEL_PRIVILEGES shows the grants.
#[test]
fn execute_on_a_model_is_granted_and_revoked_r16() {
    let (db, _) = database();
    db.schema("CREATE SCHEMA AUTHORIZATION W").unwrap();
    let (u, v, w) = (db.session("U"), db.session("V"), db.session("W"));
    u.run("CREATE EMBEDDING MODEL M WITH (TYPE = 'HASH_DENSE')").unwrap();
    u.commit().unwrap();
    let call = "VALUES (EMBED('U.M', 'x') IS NULL)";
    assert_eq!(rows(&v, call), Err(Code::Privilege));
    assert_eq!(exec(&u, "GRANT SELECT ON EMBEDDING MODEL M TO V"), Err(Code::SyntaxRule));
    u.run("GRANT EXECUTE ON EMBEDDING MODEL M TO V WITH GRANT OPTION").unwrap();
    u.commit().unwrap();
    assert_eq!(rows(&v, call).unwrap(), ["FALSE"]);
    v.run("GRANT ALL PRIVILEGES ON EMBEDDING MODEL U.M TO W").unwrap();
    v.commit().unwrap();
    assert_eq!(rows(&w, call).unwrap(), ["FALSE"]);
    let shown = "SELECT GRANTOR, GRANTEE, MODEL_NAME, PRIVILEGE_TYPE, IS_GRANTABLE FROM INFORMATION_SCHEMA.EMBEDDING_MODEL_PRIVILEGES ORDER BY GRANTOR, GRANTEE";
    assert_eq!(rows(&u, shown).unwrap(), ["U,V,M,EXECUTE,YES", "V,W,M,EXECUTE,NO", "_SYSTEM,U,M,EXECUTE,YES"]);
    assert_eq!(rows(&w, shown).unwrap(), ["V,W,M,EXECUTE,NO"]);
    assert_eq!(exec(&u, "REVOKE EXECUTE ON EMBEDDING MODEL M FROM V RESTRICT"), Err(Code::SyntaxRule), "W's grant is abandoned");
    u.run("REVOKE GRANT OPTION FOR EXECUTE ON EMBEDDING MODEL M FROM V CASCADE").unwrap();
    u.commit().unwrap();
    assert_eq!(rows(&v, call).unwrap(), ["FALSE"]);
    assert_eq!(rows(&w, call), Err(Code::Privilege));
    u.run("REVOKE EXECUTE ON EMBEDDING MODEL M FROM V RESTRICT").unwrap();
    u.commit().unwrap();
    assert_eq!(rows(&v, call), Err(Code::Privilege));
    assert_eq!(rows(&v, "SELECT * FROM INFORMATION_SCHEMA.EMBEDDING_MODELS").unwrap(), Vec::<String>::new());
}

/// Verifies: [EXTENSION] vector design §12.1, R12.5: ALTER EMBEDDING
/// MODEL merges its options into the model's, a new VERSION giving a new
/// version tag, the plug-in given its options and FROM FILE's path; it
/// refuses a change of its vectors' shape, or a model not there.
#[test]
fn alter_changes_a_models_options_r12_5() {
    let (db, asked) = database();
    let s = db.session("U");
    s.run("CREATE EMBEDDING MODEL M FROM FILE '/models/m.onnx' WITH (TYPE = 'COUNTED', POOLING = 'mean')").unwrap();
    assert_eq!(rows(&s, "VALUES (EMBED_VERSION(M))").unwrap(), ["U.M:1"]);
    asked.version.store(5, Ordering::SeqCst);
    s.run("ALTER EMBEDDING MODEL M WITH (POOLING = 'max')").unwrap();
    assert_eq!(rows(&s, "VALUES (EMBED_VERSION(M))").unwrap(), ["U.M:5"], "the plug-in's own version");
    s.run("ALTER EMBEDDING MODEL M WITH (VERSION = 7)").unwrap();
    assert_eq!(rows(&s, "VALUES (EMBED_VERSION(M))").unwrap(), ["U.M:7"]);
    let options = asked.options.lock().unwrap().clone();
    assert!(options.contains(&"FILE=/models/m.onnx".to_owned()), "{options:?}");
    assert!(options.contains(&"POOLING=max".to_owned()), "{options:?}");
    assert!(!options.iter().any(|o| o.starts_with("VERSION")), "the database reads VERSION itself");
    assert_eq!(exec(&s, "ALTER EMBEDDING MODEL M WITH (DIMENSIONS = 8)"), Err(Code::SyntaxRule));
    assert_eq!(exec(&s, "ALTER EMBEDDING MODEL N WITH (VERSION = 1)"), Err(Code::SyntaxRule));
    assert_eq!(exec(&s, "ALTER EMBEDDING MODEL M WITH (TYPE = 'HASH_DENSE')"), Err(Code::SyntaxRule), "another shape");
}

/// Verifies: [EXTENSION] vector design §12.1: DROP EMBEDDING MODEL is
/// refused, RESTRICT, of a model a generated column embeds by, and with
/// CASCADE drops it first; DROP SCHEMA drops its models with CASCADE
/// alone.
#[test]
fn drop_model_restrict_and_cascade() {
    let (db, _) = database();
    let s = db.session("U");
    s.run("CREATE EMBEDDING MODEL M WITH (TYPE = 'HASH_DENSE', DIMENSIONS = 8)").unwrap();
    s.run("CREATE TABLE A (ID INTEGER, BODY VARCHAR(40), E VECTOR(8) GENERATED ALWAYS AS (EMBED('M', BODY)))").unwrap();
    s.run("INSERT INTO A (ID, BODY) VALUES (1, 'a text')").unwrap();
    assert_eq!(exec(&s, "DROP EMBEDDING MODEL M"), Err(Code::SyntaxRule));
    assert_eq!(exec(&s, "DROP EMBEDDING MODEL M RESTRICT"), Err(Code::SyntaxRule));
    s.run("DROP EMBEDDING MODEL M CASCADE").unwrap();
    assert_eq!(rows(&s, "SELECT * FROM A").unwrap(), ["1,a text"], "its generated column went with it");
    assert_eq!(exec(&s, "DROP EMBEDDING MODEL M"), Err(Code::SyntaxRule));
    s.run("CREATE EMBEDDING MODEL N WITH (TYPE = 'HASH_DENSE')").unwrap();
    s.commit().unwrap();
    assert_eq!(exec(&db.session("V"), "DROP EMBEDDING MODEL U.N"), Err(Code::Privilege));
    s.run("DROP TABLE A CASCADE").unwrap();
    assert_eq!(exec(&s, "DROP SCHEMA U RESTRICT"), Err(Code::SyntaxRule), "it holds a model");
    s.run("DROP SCHEMA U CASCADE").unwrap();
    s.commit().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION U").unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.EMBEDDING_MODELS").unwrap(), ["0"]);
}

/// Verifies: [EXTENSION] vector design R12.3: EMBED of a model of the
/// catalog gives the plug-in the texts of many rows at once, BATCH_SIZE at
/// a time, as a projection read by batches calls it.
#[test]
fn calls_are_batched_across_rows_r12_3() {
    let (db, asked) = database();
    let s = db.session("U");
    texts(&s, 20);
    s.run("CREATE EMBEDDING MODEL M WITH (TYPE = 'COUNTED', BATCH_SIZE = 6)").unwrap();
    asked.taken();
    let embedded = rows(&s, "SELECT EMBED('M', X) FROM T").unwrap();
    assert_eq!(embedded.len(), 20);
    assert!(embedded.iter().all(|v| v == "[7,1,0,0]"), "{embedded:?}");
    let batches = asked.taken();
    assert_eq!(batches.iter().sum::<usize>(), 20, "{batches:?}");
    assert!(batches.iter().all(|&b| b <= 6) && batches.len() < 20, "{batches:?}");
}

/// Verifies: [EXTENSION] vector design R12.4: ON_ERROR = 'FAIL' fails
/// the statement; 'NULL' gives the null value for the text the model
/// refuses alone; 'RETRY' asks again, up to MAX_RETRIES times.
#[test]
fn a_models_failures_follow_on_error_r12_4() {
    let (db, asked) = database();
    let s = db.session("U");
    s.run("CREATE TABLE T (K INTEGER, X VARCHAR(20))").unwrap();
    s.run("INSERT INTO T VALUES (1, 'good'), (2, 'bad'), (3, 'fine')").unwrap();
    s.run("CREATE EMBEDDING MODEL F WITH (TYPE = 'COUNTED')").unwrap();
    s.run("CREATE EMBEDDING MODEL N WITH (TYPE = 'COUNTED', ON_ERROR = 'NULL')").unwrap();
    s.run("CREATE EMBEDDING MODEL R WITH (TYPE = 'COUNTED', ON_ERROR = 'RETRY', MAX_RETRIES = 2)").unwrap();
    assert_eq!(rows(&s, "SELECT EMBED('F', X) FROM T"), Err(Code::Data));
    assert_eq!(
        rows(&s, "SELECT K, EMBED('N', X) FROM T").unwrap(),
        ["1,[4,1,0,0]", "2,NULL", "3,[4,1,0,0]"],
        "a batch, each text asked alone"
    );
    asked.failing.store(2, Ordering::SeqCst);
    assert_eq!(rows(&s, "VALUES (EMBED('R', 'ok') IS NULL)").unwrap(), ["FALSE"], "a third try");
    asked.failing.store(3, Ordering::SeqCst);
    assert_eq!(rows(&s, "VALUES (EMBED('R', 'ok'))"), Err(Code::Data), "no fourth");
}

/// Verifies: [EXTENSION] vector design §12.1, §16: models, their options
/// and grants survive a checkpoint, a crash and reopen; reopened before
/// its plug-in is registered, a model is shown unregistered and a call of
/// it refused (VEC-0030), though what names it is defined again; once the
/// plug-in is registered, it embeds.
#[test]
fn models_survive_reopen_and_crash_recovery() {
    let scratch = Scratch::new("embedding-models");
    let dir = scratch.path("db");
    let open = |plug_in: bool| {
        let db = Database::new();
        prepared(&db);
        if plug_in {
            registered(&db);
        }
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open(true);
    db.schema("CREATE SCHEMA AUTHORIZATION U").unwrap();
    let s = db.session("U");
    s.run("CREATE EMBEDDING MODEL C WITH (TYPE = 'COUNTED', VERSION = 3)").unwrap();
    s.run("CREATE TABLE A (ID INTEGER, BODY VARCHAR(40), E VECTOR(4) GENERATED ALWAYS AS (EMBED('C', BODY)))").unwrap();
    s.commit().unwrap();
    db.checkpoint().unwrap();
    s.run("ALTER EMBEDDING MODEL C WITH (VERSION = 4)").unwrap();
    s.run("CREATE EMBEDDING MODEL H WITH (TYPE = 'HASH_SPARSE')").unwrap();
    s.run("GRANT EXECUTE ON EMBEDDING MODEL H TO PUBLIC").unwrap();
    s.commit().unwrap();
    s.run("CREATE EMBEDDING MODEL LOST WITH (TYPE = 'HASH_DENSE')").unwrap();
    let crashed = s;
    drop(db);
    let db = open(false);
    let s = db.session("U");
    let shown = "SELECT MODEL_NAME, MODEL_TYPE, DIMENSIONS, LAYOUT, MODEL_VERSION, IS_REGISTERED FROM INFORMATION_SCHEMA.EMBEDDING_MODELS ORDER BY 1";
    assert_eq!(rows(&s, shown).unwrap(), ["C,COUNTED,4,DENSE,4,NO", "H,HASH_SPARSE,1048576,SPARSE,1,YES"]);
    assert_eq!(rows(&db.session("V"), "VALUES (EMBED('U.H', 'x') IS NULL)").unwrap(), ["FALSE"]);
    assert_eq!(exec(&s, "INSERT INTO A (ID, BODY) VALUES (1, 'four')"), Err(Code::SyntaxRule), "VEC-0030");
    s.rollback().unwrap();
    registered(&db);
    s.run("INSERT INTO A (ID, BODY) VALUES (1, 'four')").unwrap();
    assert_eq!(rows(&s, "SELECT E, EMBED_VERSION(C) FROM A").unwrap(), ["[4,1,0,0],U.C:4"]);
    s.commit().unwrap();
    db.close().unwrap();
    drop((s, db, crashed));
    let db = open(true);
    assert_eq!(rows(&db.session("U"), "SELECT COUNT(*) FROM A").unwrap(), ["1"]);
}
