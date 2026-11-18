//! \[EXTENSION\] Vector pipelines, auto-embedded columns and the queue of
//! embeddings after commit (the vector design's §12.3 to §12.5, R12.5,
//! R12.6): CREATE and DROP VECTOR PIPELINE, MODE SYNC and ASYNC, the
//! version column, GENERATED ALWAYS AS (EMBED(...)) STORED ASYNC, the hint
//! VECTOR_WAIT_FRESH, FROM CHUNK_TEXT, bracketed comments, the views
//! VECTOR_PIPELINES and EMBEDDING_QUEUE, and the queue through a crash.

use std::sync::atomic::Ordering;

use turborel::db::{Database, OpenOptions, Session};
use turborel::error::Code;

use crate::common::{Scratch, after_crash, exec, rows};
use crate::embedding_models::counted::{database, prepared, registered};

/// The chunks of U.CHUNKS, each its document, number, text, whether its
/// embedding is null, and its version.
const CHUNKS: &str = "SELECT DOC, N, T, E IS NULL, V FROM CHUNKS ORDER BY DOC, N";

/// Tables U.DOCS (ID, BODY) of two texts and U.CHUNKS of chunks, and the
/// model M of COUNTED.
fn documents(s: &Session) {
    s.run("CREATE TABLE DOCS (ID INTEGER PRIMARY KEY, BODY VARCHAR(200), OTHER INTEGER)").unwrap();
    s.run("CREATE TABLE CHUNKS (DOC INTEGER, N INTEGER, T VARCHAR(200), E VECTOR(4), V VARCHAR(300))").unwrap();
    s.run("INSERT INTO DOCS VALUES (1, 'one two three', 0), (2, NULL, 0)").unwrap();
    s.run("CREATE EMBEDDING MODEL M WITH (TYPE = 'COUNTED')").unwrap();
}

/// Verifies: [EXTENSION] vector design §12.5, R12.5: a pipeline of MODE
/// SYNC fills its target from its source as it is made, and keeps it in
/// step as rows are inserted, updated and deleted, each chunk embedded
/// with its model's version; DROP VECTOR PIPELINE keeps the target's rows
/// and keeps it no more; VECTOR_PIPELINES shows it.
#[test]
fn a_pipeline_keeps_its_target_in_step_r12_5() {
    let (db, _) = database();
    let s = db.session("U");
    documents(&s);
    s.run("CREATE VECTOR PIPELINE P SOURCE DOCS (ID, BODY) CHUNK BY WORDS MAX 2 EMBED USING M TARGET CHUNKS (DOC, N, T, E, V)").unwrap();
    assert_eq!(rows(&s, CHUNKS).unwrap(), ["1,1,one two,FALSE,U.M:1", "1,2,three,FALSE,U.M:1"]);
    s.run("INSERT INTO DOCS VALUES (3, 'four', 0)").unwrap();
    s.run("UPDATE DOCS SET BODY = 'five six seven' WHERE ID = 1").unwrap();
    s.run("UPDATE DOCS SET OTHER = 9 WHERE ID = 3").unwrap();
    s.run("DELETE FROM DOCS WHERE ID = 2").unwrap();
    let kept = ["1,1,five six,FALSE,U.M:1", "1,2,seven,FALSE,U.M:1", "3,1,four,FALSE,U.M:1"];
    assert_eq!(rows(&s, CHUNKS).unwrap(), kept);
    assert_eq!(rows(&s, "SELECT E FROM CHUNKS WHERE T = 'four'").unwrap(), ["[4,1,0,0]"]);
    let shown =
        "SELECT PIPELINE_NAME, SOURCE_NAME, TEXT_COLUMN, CHUNKING, MODEL_NAME, TARGET_NAME, MODE FROM INFORMATION_SCHEMA.VECTOR_PIPELINES";
    assert_eq!(rows(&s, shown).unwrap(), ["P,DOCS,BODY,BY WORDS MAX 2 OVERLAP 0,M,CHUNKS,SYNC"]);
    assert_eq!(rows(&db.session("V"), shown).unwrap(), Vec::<String>::new(), "of a schema V does not own");
    s.run("DROP VECTOR PIPELINE P").unwrap();
    s.run("DELETE FROM DOCS WHERE ID = 3").unwrap();
    assert_eq!(rows(&s, CHUNKS).unwrap(), kept, "its rows stay, kept no more");
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.TRIGGERS WHERE TRIGGER_SCHEMA = 'U'").unwrap(), ["0"]);
    assert_eq!(exec(&s, "DROP VECTOR PIPELINE P"), Err(Code::SyntaxRule));
}

/// Verifies: [EXTENSION] vector design §12.5: a pipeline is refused of a
/// target of the wrong count of columns, of a source, target, column or
/// model not there, of a text not of a character type, of a name taken,
/// or of a chunking CHUNK_TEXT refuses; DROP TABLE of its source or target
/// is refused, RESTRICT, and with CASCADE drops it.
#[test]
fn a_pipeline_is_refused_of_what_it_cannot_keep() {
    let (db, _) = database();
    let s = db.session("U");
    documents(&s);
    let made = |source: &str, chunk: &str, model: &str, target: &str| {
        format!("CREATE VECTOR PIPELINE P SOURCE {source} {chunk} EMBED USING {model} TARGET {target}")
    };
    for (refused, code) in [
        (made("DOCS (ID, BODY)", "", "M", "CHUNKS (DOC, N, T)"), Code::SyntaxRule),
        (made("DOCS (BODY)", "", "M", "CHUNKS (N, T, E)"), Code::SyntaxRule),
        (made("NONE (ID, BODY)", "", "M", "CHUNKS (DOC, N, T, E)"), Code::SyntaxRule),
        (made("DOCS (ID, BODY)", "", "M", "NONE (DOC, N, T, E)"), Code::SyntaxRule),
        (made("DOCS (ID, NONE)", "", "M", "CHUNKS (DOC, N, T, E)"), Code::SyntaxRule),
        (made("DOCS (ID, BODY)", "", "M", "CHUNKS (DOC, N, T, NONE)"), Code::SyntaxRule),
        (made("DOCS (ID, OTHER)", "", "M", "CHUNKS (DOC, N, T, E)"), Code::SyntaxRule),
        (made("DOCS (ID, BODY)", "", "NONE", "CHUNKS (DOC, N, T, E)"), Code::SyntaxRule),
        (made("DOCS (ID, BODY)", "CHUNK BY LINES MAX 2", "M", "CHUNKS (DOC, N, T, E)"), Code::Data),
        (made("DOCS (ID, BODY)", "", "M", "CHUNKS (DOC, N, T, E) MODE LATER"), Code::Syntax),
    ] {
        assert_eq!(exec(&s, &refused), Err(code), "{refused}");
    }
    s.run(&made("DOCS (ID, BODY)", "CHUNK BY CHARS MAX 5 OVERLAP 1", "M", "CHUNKS (DOC, N, T, E) MODE SYNC")).unwrap();
    assert_eq!(exec(&s, &made("DOCS (ID, BODY)", "", "M", "CHUNKS (DOC, N, T, E)")), Err(Code::SyntaxRule), "its name is taken");
    assert_eq!(exec(&s, "DROP EMBEDDING MODEL M"), Err(Code::SyntaxRule), "the pipeline embeds by it");
    assert_eq!(exec(&s, "DROP TABLE CHUNKS RESTRICT"), Err(Code::SyntaxRule));
    assert_eq!(exec(&s, "DROP TABLE DOCS RESTRICT"), Err(Code::SyntaxRule));
    s.run("DROP TABLE DOCS CASCADE").unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.VECTOR_PIPELINES").unwrap(), ["0"]);
    s.run("DROP EMBEDDING MODEL M").unwrap();
}

/// Verifies: [EXTENSION] vector design R12.6, R12.3: a pipeline of MODE
/// ASYNC leaves its chunks' embeddings null as they are written; the
/// queue after commit, a job asked for by each commit that writes, fills
/// them, its texts given the model in batches; a statement hinted
/// VECTOR_WAIT_FRESH runs a round first; EMBEDDING_QUEUE shows each
/// round, and a failing one's error, its rows left for the next.
#[test]
fn an_async_pipeline_is_filled_after_commit_r12_6() {
    let (db, asked) = database();
    let s = db.session("U");
    documents(&s);
    s.run("CREATE VECTOR PIPELINE P SOURCE DOCS (ID, BODY) CHUNK BY WORDS MAX 1 EMBED USING M TARGET CHUNKS (DOC, N, T, E, V) MODE ASYNC")
        .unwrap();
    assert_eq!(rows(&s, CHUNKS).unwrap(), ["1,1,one,TRUE,NULL", "1,2,two,TRUE,NULL", "1,3,three,TRUE,NULL"]);
    s.commit().unwrap();
    asked.taken();
    db.run_due().unwrap();
    assert_eq!(asked.taken(), [3], "one batch");
    assert_eq!(rows(&s, CHUNKS).unwrap(), ["1,1,one,FALSE,U.M:1", "1,2,two,FALSE,U.M:1", "1,3,three,FALSE,U.M:1"]);
    s.run("INSERT INTO DOCS VALUES (3, 'four', 0)").unwrap();
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT E IS NULL FROM CHUNKS WHERE DOC = 3").unwrap(), ["TRUE"]);
    s.commit().unwrap();
    assert_eq!(rows(&s, "/*+ VECTOR_WAIT_FRESH */ SELECT E FROM CHUNKS WHERE DOC = 3").unwrap(), ["[4,1,0,0]"]);
    let queue = "SELECT TABLE_NAME, COLUMN_NAME, MODEL_VERSION, PENDING, EMBEDDED, FAILURES, LAST_ERROR IS NULL FROM INFORMATION_SCHEMA.EMBEDDING_QUEUE";
    assert_eq!(rows(&s, queue).unwrap(), ["CHUNKS,E,U.M:1,1,4,0,TRUE"]);
    s.commit().unwrap();
    s.run("INSERT INTO DOCS VALUES (4, 'bad words', 0)").unwrap();
    s.commit().unwrap();
    assert_eq!(db.wait_for_embeddings().unwrap_err().code, Code::Data);
    assert_eq!(rows(&s, queue).unwrap(), ["CHUNKS,E,U.M:1,2,4,1,FALSE"]);
    assert_eq!(rows(&s, "SELECT T FROM CHUNKS WHERE E IS NULL ORDER BY T").unwrap(), ["bad", "words"]);
    s.commit().unwrap();
    s.run("UPDATE DOCS SET BODY = 'good words' WHERE ID = 4").unwrap();
    s.commit().unwrap();
    db.wait_for_embeddings().unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM CHUNKS WHERE E IS NULL").unwrap(), ["0"]);
    assert_eq!(rows(&db.session("V"), queue).unwrap(), Vec::<String>::new(), "of a schema V does not own");
}

/// Verifies: [EXTENSION] vector design R12.5: a model's new version has
/// the chunks of a pipeline with a version column embedded again by the
/// queue, SYNC as ASYNC, the version column naming the new version.
#[test]
fn a_new_version_is_embedded_again_r12_5() {
    let (db, asked) = database();
    let s = db.session("U");
    documents(&s);
    s.run("CREATE VECTOR PIPELINE P SOURCE DOCS (ID, BODY) EMBED USING M TARGET CHUNKS (DOC, N, T, E, V)").unwrap();
    s.commit().unwrap();
    db.run_due().unwrap();
    assert_eq!(rows(&s, CHUNKS).unwrap(), ["1,1,one two three,FALSE,U.M:1"]);
    s.commit().unwrap();
    asked.version.store(2, Ordering::SeqCst);
    s.run("ALTER EMBEDDING MODEL M WITH (POOLING = 'mean')").unwrap();
    s.commit().unwrap();
    db.run_due().unwrap();
    assert_eq!(rows(&s, CHUNKS).unwrap(), ["1,1,one two three,FALSE,U.M:2"]);
    s.commit().unwrap();
    s.run("ALTER EMBEDDING MODEL M WITH (VERSION = 9)").unwrap();
    s.commit().unwrap();
    db.run_due().unwrap();
    assert_eq!(rows(&s, CHUNKS).unwrap(), ["1,1,one two three,FALSE,U.M:9"]);
}

/// Verifies: [EXTENSION] vector design §12.4, R12.6: a generated column of
/// EMBED, STORED and SYNC, is computed as a row is written; ASYNC, it is
/// null until the queue fills it; an update of a column it reads makes it
/// null again, of another keeps it, and SET of it to DEFAULT computes it
/// now; a row whose text is null waits for nothing.
#[test]
fn an_async_generated_column_is_filled_after_commit_r12_6() {
    let (db, _) = database();
    let s = db.session("U");
    s.run("CREATE EMBEDDING MODEL M WITH (TYPE = 'COUNTED')").unwrap();
    s.run(
        "CREATE TABLE A (ID INTEGER, BODY VARCHAR(40), OTHER INTEGER,
           E VECTOR(4) GENERATED ALWAYS AS (EMBED('M', BODY)) STORED ASYNC,
           S VECTOR(4) GENERATED ALWAYS AS (EMBED(M, BODY)) STORED SYNC)",
    )
    .unwrap();
    s.run("INSERT INTO A (ID, BODY, OTHER) VALUES (1, 'one', 0), (2, NULL, 0)").unwrap();
    let shown = "SELECT ID, E, S FROM A ORDER BY ID";
    assert_eq!(rows(&s, shown).unwrap(), ["1,NULL,[3,1,0,0]", "2,NULL,NULL"]);
    s.commit().unwrap();
    db.run_due().unwrap();
    assert_eq!(rows(&s, shown).unwrap(), ["1,[3,1,0,0],[3,1,0,0]", "2,NULL,NULL"]);
    s.run("UPDATE A SET OTHER = 5").unwrap();
    assert_eq!(rows(&s, "SELECT E FROM A WHERE ID = 1").unwrap(), ["[3,1,0,0]"], "kept");
    s.run("UPDATE A SET BODY = 'three' WHERE ID = 1").unwrap();
    assert_eq!(rows(&s, "SELECT E, S FROM A WHERE ID = 1").unwrap(), ["NULL,[5,1,0,0]"]);
    s.run("UPDATE A SET E = DEFAULT WHERE ID = 1").unwrap();
    assert_eq!(rows(&s, "SELECT E FROM A WHERE ID = 1").unwrap(), ["[5,1,0,0]"]);
    assert_eq!(exec(&s, "UPDATE A SET E = NULL"), Err(Code::SyntaxRule));
    s.commit().unwrap();
    let queue = "SELECT TABLE_NAME, COLUMN_NAME, MODEL_VERSION FROM INFORMATION_SCHEMA.EMBEDDING_QUEUE";
    assert_eq!(rows(&s, queue).unwrap(), ["A,E,U.M:1"]);
}

/// Verifies: [EXTENSION] vector design §12.3: CHUNK_TEXT in FROM is the
/// table of the chunks of its text, its chunking a string, written as
/// words or CHUNK_TEXT's default, its correlation name the function's if none is written, its
/// columns CHUNK_ID, CHUNK_OFFSET, CHUNK_LENGTH and CHUNK_TEXT unless a
/// derived column list names them; it may read a table before it.
#[test]
fn chunk_text_is_a_table_in_from_r12_3() {
    let (db, _) = database();
    let s = db.session("U");
    s.run("CREATE TABLE DOCS (ID INTEGER, BODY VARCHAR(100))").unwrap();
    s.run("INSERT INTO DOCS VALUES (1, 'a b c'), (2, 'd e')").unwrap();
    let chunks = "SELECT D.ID, C.CHUNK_ID, C.CHUNK_TEXT FROM DOCS AS D, CHUNK_TEXT(D.BODY, BY WORDS, MAX 2, OVERLAP 1) AS C ORDER BY 1, 2";
    assert_eq!(rows(&s, chunks).unwrap(), ["1,1,a b", "1,2,b c", "2,1,d e"]);
    let named = "SELECT CHUNK_TEXT.CHUNK_OFFSET, CHUNK_TEXT.CHUNK_LENGTH FROM CHUNK_TEXT('xy zw', 'BY CHARS MAX 3')";
    assert_eq!(rows(&s, named).unwrap(), ["1,3", "4,2"]);
    let listed = "SELECT P.K, P.W FROM VECTOR_CHUNKS('one. two.', BY SENTENCES MAX 1) P (K, O, L, W)";
    assert_eq!(rows(&s, listed).unwrap(), ["1,one.", "2,two."]);
    assert_eq!(rows(&s, "SELECT * FROM CHUNK_TEXT('x', BY LINES MAX 1) AS C"), Err(Code::Data));
    assert_eq!(rows(&s, "SELECT CHUNK_ID, CHUNK_TEXT FROM CHUNK_TEXT('x y')").unwrap(), ["1,x y"], "BY WORDS MAX 256");
    assert_eq!(rows(&s, "SELECT * FROM CHUNK_TEXT('x', 'BY WORDS MAX 1'"), Err(Code::Syntax));
}

/// Verifies: [ISO/IEC 9075-2:2023 §5.2 SR11, SR12, Feature T351] A
/// bracketed comment is a separator, one begun within one nested in it;
/// one that does not end is refused.
#[test]
fn bracketed_comments_are_separators_t351() {
    let (db, _) = database();
    let s = db.session("U");
    assert_eq!(rows(&s, "VALUES (1/* one */+/* two /* nested */ still */2)").unwrap(), ["3"]);
    assert_eq!(rows(&s, "/*+ NO_SUCH_HINT */ VALUES (4 /**/)").unwrap(), ["4"]);
    assert_eq!(rows(&s, "VALUES (5) /* open /* */"), Err(Code::Lexical));
    assert_eq!(rows(&s, "VALUES (5) /* open"), Err(Code::Lexical));
    assert_eq!(rows(&s, "VALUES (6 /* -- */)").unwrap(), ["6"]);
}

/// Verifies: [EXTENSION] vector design R12.6, §12.5: a pipeline's rows
/// waiting for the queue, committed but not yet filled, survive a crash;
/// the queue fills them as the database opens again, once its plug-in is
/// registered; the pipeline keeps its target after recovery.
#[test]
fn the_queue_survives_a_crash_r12_6() {
    let scratch = Scratch::new("embedding-queue");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        prepared(&db);
        registered(&db);
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION U").unwrap();
    let s = db.session("U");
    documents(&s);
    s.run("CREATE VECTOR PIPELINE P SOURCE DOCS (ID, BODY) EMBED USING M TARGET CHUNKS (DOC, N, T, E, V) MODE ASYNC").unwrap();
    s.commit().unwrap();
    db.checkpoint().unwrap();
    s.run("INSERT INTO DOCS VALUES (3, 'after the checkpoint', 0)").unwrap();
    s.commit().unwrap();
    let crashed = s;
    drop(db);
    let db = open();
    let s = db.session("U");
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM CHUNKS WHERE E IS NULL").unwrap(), ["2"]);
    s.commit().unwrap();
    db.run_due().unwrap();
    assert_eq!(rows(&s, CHUNKS).unwrap(), ["1,1,one two three,FALSE,U.M:1", "3,1,after the checkpoint,FALSE,U.M:1"]);
    s.run("INSERT INTO DOCS VALUES (4, 'new', 0)").unwrap();
    assert_eq!(rows(&s, "SELECT T FROM CHUNKS WHERE DOC = 4").unwrap(), ["new"]);
    drop((s, crashed));
}
