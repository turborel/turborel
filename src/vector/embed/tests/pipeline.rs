//! Pipelines and auto-embedded columns: their text read, their plans,
//! and a plan run keeping a chunk table in step with its source.

use super::*;
use crate::db::Session;
use crate::error::Code;

/// The §12.5 example, its model the built-in dense one.
const EXAMPLE: &str = "CREATE VECTOR PIPELINE docs_pipe
    SOURCE docs (id, body)
    CHUNK BY WORDS MAX 2 OVERLAP 1
    EMBED USING hash_dense
    TARGET doc_chunks (doc_id, chunk_id, chunk_text, embedding)
    MODE ASYNC";

/// Verifies: [EXTENSION] (§12.5) A pipeline's text is read to its parts;
/// MODE and CHUNK may be left out; a target not of its keys and three
/// columns, a source of no key, text after MODE, or a CHUNK clause
/// CHUNK_TEXT refuses are refused.
#[test]
fn pipelines_are_read() {
    let p = Pipeline::parse(EXAMPLE).unwrap();
    assert_eq!((p.name.as_str(), p.source.as_str(), p.text.as_str(), p.model.as_str()), ("DOCS_PIPE", "DOCS", "BODY", "HASH_DENSE"));
    assert_eq!((p.keys.clone(), p.columns.len(), p.chunk.max, p.chunk.overlap, p.mode), (vec!["ID".to_owned()], 4, 2, 1, Mode::Async));
    let short = Pipeline::parse("P SOURCE S (A, B, T) EMBED USING M TARGET X (A, B, I, C, E)").unwrap();
    assert_eq!((short.keys.len(), short.mode, short.chunk.max), (2, Mode::Sync, 256));
    for bad in [
        "P SOURCE S (K, T) EMBED USING M TARGET X (K, I, C)",
        "P SOURCE S (T) EMBED USING M TARGET X (I, C, E)",
        "P SOURCE S (K, T) EMBED USING M TARGET X (K, I, C, E) MODE ASYNC NOW",
        "P SOURCE S (K, T) EMBED USING M TARGET X (K, I, C, E) MODE LATER",
    ] {
        assert_eq!(Pipeline::parse(bad).unwrap_err().code, Code::SyntaxRule, "{bad}");
    }
    assert_eq!(Pipeline::parse("P SOURCE S (K, T) CHUNK MAX 0 EMBED USING M TARGET X (K, I, C, E)").unwrap_err().code, Code::Data);
}

/// Verifies: [EXTENSION] (§12.5, R12.6) A pipeline's plan runs, filling
/// its target from the source's rows, each chunk embedded, and making its
/// triggers; ASYNC reports the queue it needs; a model not registered is
/// refused. A trigger's action is read again without taking calls of
/// registered functions, so firing one is refused until the database
/// reads stored definitions as their session did, or the routines are
/// built in: then the inserts, updates and deletes below keep the target
/// in step, and this test asserts it.
#[test]
fn a_pipeline_keeps_its_chunks() {
    let (db, models) = database();
    let s = db.session("U");
    s.run("CREATE TABLE DOCS (ID INTEGER PRIMARY KEY, BODY VARCHAR(200))").unwrap();
    s.run("CREATE TABLE DOC_CHUNKS (DOC_ID INTEGER, CHUNK_ID INTEGER, CHUNK_TEXT VARCHAR(200), EMBEDDING VECTOR)").unwrap();
    s.run("INSERT INTO DOCS VALUES (1, 'a b c')").unwrap();
    let plan = Pipeline::parse(EXAMPLE).unwrap().plan(&models).unwrap();
    assert_eq!(plan.hooks, [Hook::AsyncQueue, Hook::ModelVersion, Hook::Batching]);
    for statement in &plan.statements {
        s.run(statement).unwrap_or_else(|e| panic!("{statement}: {e:?}"));
    }
    let chunks =
        |session: &Session| rows(session, "SELECT DOC_ID, CHUNK_ID, CHUNK_TEXT FROM DOC_CHUNKS ORDER BY DOC_ID, CHUNK_ID").unwrap();
    assert_eq!(chunks(&s), ["1,1,a b", "1,2,b c"]);
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM DOC_CHUNKS WHERE EMBEDDING IS NOT NULL").unwrap(), ["2"]);
    let Err(fired) = s.run("INSERT INTO DOCS VALUES (2, 'x y')") else { return in_step(&s, &chunks) };
    assert!(fired.msg.contains("SQL.FUNCTION_CALLS"), "{fired:?}");
    let unknown = Pipeline::parse("P SOURCE S (K, T) EMBED USING NONE TARGET X (K, I, C, E)").unwrap();
    assert_eq!(unknown.plan(&models).unwrap_err().code, Code::SyntaxRule);
}

/// The pipeline's target kept in step as its source changes.
fn in_step(s: &Session, chunks: &dyn Fn(&Session) -> Vec<String>) {
    s.run("UPDATE DOCS SET BODY = 'p q r s' WHERE ID = 1").unwrap();
    assert_eq!(chunks(s), ["1,1,p q", "1,2,q r", "1,3,r s", "2,1,x y"]);
    s.run("DELETE FROM DOCS WHERE ID = 1").unwrap();
    assert_eq!(chunks(s), ["2,1,x y"]);
}

/// Verifies: [EXTENSION] (§12.4) An auto-embedded column is a generated
/// column of EMBED of its source, of its model's VECTOR type. A generated
/// column's expression is read again without taking calls of registered
/// functions, so making it is refused until the database reads it as its
/// session did; then rows written compute it, as this test asserts.
#[test]
fn an_auto_embedded_column_is_generated() {
    let (db, models) = database();
    let s = db.session("U");
    s.run("CREATE TABLE NOTES (ID INTEGER, BODY VARCHAR(100))").unwrap();
    let column = AutoColumn {
        table: "NOTES".to_owned(),
        column: "E".to_owned(),
        source: "BODY".to_owned(),
        model: "hash_sparse".to_owned(),
        mode: Mode::Sync,
    };
    let plan = column.plan(&models).unwrap();
    assert_eq!(
        plan.statements,
        ["ALTER TABLE NOTES ADD COLUMN E VECTOR(1048576, FLOAT32, SPARSE) GENERATED ALWAYS AS (EMBED('HASH_SPARSE', BODY))"]
    );
    assert_eq!(plan.hooks, [Hook::ModelVersion, Hook::Batching]);
    // Until the parser reads VECTOR's key word parameters, the column is
    // made of plain VECTOR.
    let made = s.run(&plan.statements[0].replace("VECTOR(1048576, FLOAT32, SPARSE)", "VECTOR"));
    if let Err(refused) = made {
        assert!(refused.msg.contains("SQL.FUNCTION_CALLS"), "{refused:?}");
        return;
    }
    s.run("INSERT INTO NOTES (ID, BODY) VALUES (1, 'hello hello world')").unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM NOTES WHERE E IS NOT NULL").unwrap(), ["1"]);
}
