//! What a GRAPH_TABLE refuses, and a TRUSTED graph whose keys repeat
//! [GS §4.1, §6, §8].

use super::{bank, refused, rows};

/// Verifies: [EXTENSION] [GS §6.8, §8] a prefilter of a quantified
/// element reading a later variable, a subpath variable, and COUNT(*) of
/// no variable are refused; a graph not there is GRF-0001.
#[test]
fn refused_patterns_8() {
    let (_db, s) = bank();
    let cases = [
        ("SELECT 1 FROM GRAPH_TABLE (NONE MATCH (a) COLUMNS (a.id AS A))", "GRF-0001"),
        ("SELECT 1 FROM GRAPH_TABLE (BANK MATCH (a)-[t:Transfer WHERE t.amount > b.id]->{1,2}(b) COLUMNS (a.id AS A))", ""),
        ("SELECT 1 FROM GRAPH_TABLE (BANK MATCH (a) COLUMNS (COUNT(*) AS N))", ""),
        ("SELECT 1 FROM GRAPH_TABLE (BANK MATCH (a) COLUMNS (a.nothing AS N))", ""),
    ];
    for (q, holds) in cases {
        let (code, msg) = refused(&s, q);
        assert!(code < 0 && msg.contains(holds), "{q}: {code} {msg}");
    }
}

/// Verifies: [EXTENSION] [GS §6.6, §6.7, §7.4] the forms CONFORMANCE.md
/// lists as not taken are each SQLCODE -900: a subpath variable, a set
/// function in a bounded group's filter, and AS OF SCN.
#[test]
fn forms_not_taken_are_900() {
    let (_db, s) = bank();
    let cases = [
        "SELECT 1 FROM GRAPH_TABLE (BANK MATCH (a) (q = (b)-[t]->(c)) COLUMNS (a.id AS A))",
        "SELECT 1 FROM GRAPH_TABLE (BANK MATCH (a) ((b)-[t]->(c) WHERE COUNT(t.*) > 0){1,2} COLUMNS (a.id AS A))",
        "SELECT 1 FROM GRAPH_TABLE (BANK MATCH (a) COLUMNS (a.id AS A)) AS OF SCN 5",
    ];
    for q in cases {
        let (code, msg) = refused(&s, q);
        assert_eq!(code, -900, "{q}: {msg}");
    }
}

/// Verifies: [EXTENSION] [GS §4.1 R4.3] a TRUSTED graph's key that
/// repeats binds each of its rows; an edge whose end is no vertex is no
/// edge.
#[test]
fn trusted_keys_are_taken_as_written_4_1() {
    let (_db, s) = bank();
    s.run("CREATE TABLE OWNERS (NAME VARCHAR(20), ACCT VARCHAR(4))").unwrap();
    s.run("INSERT INTO OWNERS VALUES ('X', 'a1'), ('Y', 'a1'), ('Z', 'a9')").unwrap();
    s.run(
        "CREATE PROPERTY GRAPH OWNED VERTEX TABLES (ACCOUNT KEY (ID) PROPERTIES (ID)) EDGE TABLES (TRANSFER KEY (ID) \
         SOURCE KEY (SRC) REFERENCES ACCOUNT (ID) DESTINATION KEY (DST) REFERENCES ACCOUNT (ID) PROPERTIES (ID), \
         OWNERS KEY (ACCT) SOURCE KEY (ACCT) REFERENCES ACCOUNT (ID) DESTINATION KEY (ACCT) REFERENCES ACCOUNT (ID) \
         LABEL OWNS PROPERTIES (NAME)) OPTIONS (TRUSTED MODE)",
    )
    .unwrap();
    let q = "SELECT N FROM GRAPH_TABLE (OWNED MATCH (a)-[o:Owns]->(a) COLUMNS (o.name AS N))";
    assert_eq!(rows(&s, q), ["X", "Y"]);
}
