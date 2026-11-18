//! \[EXTENSION\] The queue of embeddings after commit, in detail (the
//! vector design's R12.3, R12.6): MERGE leaving a column of ASYNC to the
//! queue as UPDATE and INSERT do; a round reading only the tables written
//! since the last; and a round's texts embedded in slices of at most
//! `MOST_PRIMED`, each in batches, however many wait.

use turborel::vector::embed::MOST_PRIMED;

use crate::common::rows;
use crate::embedding_models::counted::database;

/// Verifies: [EXTENSION] vector design §12.4, R12.6: MERGE gives a
/// generated column of ASYNC the null value in a row it inserts, and in
/// one it updates by a column the column reads, keeps it in one it updates
/// otherwise, and computes it in one it sets to DEFAULT; the queue fills
/// it after commit.
#[test]
fn merge_leaves_an_async_column_to_the_queue() {
    let (db, _) = database();
    let s = db.session("U");
    s.run("CREATE EMBEDDING MODEL M WITH (TYPE = 'COUNTED')").unwrap();
    s.run("CREATE TABLE A (ID INTEGER, BODY VARCHAR(40), OTHER INTEGER, E VECTOR(4) GENERATED ALWAYS AS (EMBED('M', BODY)) STORED ASYNC)")
        .unwrap();
    s.run("INSERT INTO A (ID, BODY, OTHER) VALUES (1, 'one', 0), (2, 'two', 0), (3, 'three', 0)").unwrap();
    s.commit().unwrap();
    db.run_due().unwrap();
    let shown = "SELECT ID, E FROM A ORDER BY ID";
    assert_eq!(rows(&s, shown).unwrap(), ["1,[3,1,0,0]", "2,[3,1,0,0]", "3,[5,1,0,0]"]);
    s.run(
        "MERGE INTO A USING (VALUES (1, 'eleven'), (2, 'x'), (4, 'four')) AS S (K, B) ON A.ID = S.K
           WHEN MATCHED AND S.K = 1 THEN UPDATE SET BODY = S.B
           WHEN MATCHED THEN UPDATE SET OTHER = 9
           WHEN NOT MATCHED THEN INSERT (ID, BODY, OTHER) VALUES (S.K, S.B, 0)",
    )
    .unwrap();
    assert_eq!(rows(&s, shown).unwrap(), ["1,NULL", "2,[3,1,0,0]", "3,[5,1,0,0]", "4,NULL"]);
    s.run("MERGE INTO A USING (VALUES (3)) AS S (K) ON A.ID = S.K WHEN MATCHED THEN UPDATE SET BODY = 'longer text', E = DEFAULT").unwrap();
    assert_eq!(rows(&s, "SELECT E FROM A WHERE ID = 3").unwrap(), ["[11,1,0,0]"]);
    s.commit().unwrap();
    db.run_due().unwrap();
    assert_eq!(
        rows(&s, shown).unwrap(),
        ["1,[6,1,0,0]", "2,[3,1,0,0]", "3,[11,1,0,0]", "4,[6,1,0,0]"],
        "'four' padded as the VALUES column is"
    );
}

/// Verifies: [EXTENSION] vector design R12.6: a round reads only the
/// tables commits wrote since the last round: a row the model gives no
/// vector, ON_ERROR = 'NULL', waits, unasked, while other tables are
/// written, and is asked for again once its own table is; the round's own
/// writes ask for no other.
#[test]
fn a_round_reads_only_the_tables_written() {
    let (db, asked) = database();
    let s = db.session("U");
    s.run("CREATE EMBEDDING MODEL N WITH (TYPE = 'COUNTED', ON_ERROR = 'NULL')").unwrap();
    s.run("CREATE TABLE A (ID INTEGER, BODY VARCHAR(40), E VECTOR(4) GENERATED ALWAYS AS (EMBED('N', BODY)) STORED ASYNC)").unwrap();
    s.run("CREATE TABLE B (ID INTEGER)").unwrap();
    s.run("INSERT INTO A (ID, BODY) VALUES (1, 'bad text')").unwrap();
    s.commit().unwrap();
    asked.taken();
    db.run_due().unwrap();
    assert_eq!(asked.taken(), [1], "asked once, and the round ends");
    db.run_due().unwrap();
    assert_eq!(asked.taken(), Vec::<usize>::new(), "its own commit marks nothing");
    s.run("INSERT INTO B VALUES (1)").unwrap();
    s.commit().unwrap();
    db.run_due().unwrap();
    assert_eq!(asked.taken(), Vec::<usize>::new(), "A was not written");
    s.run("INSERT INTO A (ID, BODY) VALUES (2, 'fine')").unwrap();
    s.commit().unwrap();
    db.run_due().unwrap();
    assert_eq!(asked.taken(), [2, 1, 1], "both texts, then each alone");
    assert_eq!(rows(&s, "SELECT ID, E IS NULL FROM A ORDER BY ID").unwrap(), ["1,TRUE", "2,FALSE"]);
}

/// Verifies: [EXTENSION] vector design R12.3, R12.6: more distinct texts
/// waiting than a slice holds are embedded slice by slice, every one in a
/// batch of at most BATCH_SIZE, none alone by the UPDATE; EMBEDDING_QUEUE
/// counts the texts found and the rows embedded.
#[test]
fn many_texts_are_embedded_in_slices_of_batches() {
    let (db, asked) = database();
    let s = db.session("U");
    s.run("CREATE EMBEDDING MODEL M WITH (TYPE = 'COUNTED', BATCH_SIZE = 500)").unwrap();
    s.run("CREATE TABLE DOCS (ID INTEGER, BODY CLOB(1M))").unwrap();
    s.run("CREATE TABLE CHUNKS (DOC INTEGER, N INTEGER, T VARCHAR(20), E VECTOR(4))").unwrap();
    s.run("CREATE VECTOR PIPELINE P SOURCE DOCS (ID, BODY) CHUNK BY WORDS MAX 1 EMBED USING M TARGET CHUNKS (DOC, N, T, E) MODE ASYNC")
        .unwrap();
    let n = MOST_PRIMED + 300;
    let words: Vec<String> = (0..n).map(|k| format!("w{k:05}")).collect();
    // Each word twice, so the texts are fewer than the rows.
    s.run(&format!("INSERT INTO DOCS VALUES (1, '{}'), (2, '{}')", words.join(" "), words[..10].join(" "))).unwrap();
    s.commit().unwrap();
    asked.taken();
    db.run_due().unwrap();
    let batches = asked.taken();
    assert_eq!(batches.iter().sum::<usize>(), n, "each text once");
    assert!(batches.iter().all(|&b| b <= 500), "{batches:?}");
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM CHUNKS WHERE E IS NULL").unwrap(), ["0"]);
    let queue = "SELECT PENDING, EMBEDDED, FAILURES FROM INFORMATION_SCHEMA.EMBEDDING_QUEUE";
    assert_eq!(rows(&s, queue).unwrap(), [format!("{n},{},0", n + 10)]);
}
