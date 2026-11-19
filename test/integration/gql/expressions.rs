//! GQL's expressions written again as SQL over a binding table (RULES.md
//! D144): label expressions, group variables read alone and across their
//! elements, path functions, the properties of an open graph, named
//! procedure calls, and the refusals of each.

use super::{bank, direct, failed, gql};

/// Verifies: [EXTENSION] graph standard §5, RULES.md D144: a label
/// expression of a variable outside its pattern tests the label set, with
/// "|", "&", "!", "%", parentheses and IS NOT LABELED, a label named as a
/// variable of the scope read as the label. A variable bound across
/// element tables of different labels holds each element's own label
/// set, unpadded. It is a regression test, since the set of a CITY read
/// `|        CITY|PLACE|`, the empty arm of the ACCOUNT test padded to
/// its length, and `place:CITY` was false of every city.
#[test]
fn label_expressions_outside_the_pattern_5() {
    let (_db, s) = bank();
    let labels = gql(&s, "MATCH (place) RETURN DISTINCT LABELS(place) AS l ORDER BY l");
    assert_eq!(labels, ["|ACCOUNT|", "|CITY|PLACE|"]);
    let count = |cond: &str| gql(&s, &format!("MATCH (place) FILTER {cond} RETURN COUNT(*) AS n")).join("");
    let conds = [
        "place:CITY&PLACE",
        "place:ACCOUNT|CITY",
        "place:!PLACE",
        "place:(ACCOUNT|PLACE)&!CITY",
        "place:%",
        "place IS NOT LABELED ACCOUNT",
        "place IS LABELED CITY|ACCOUNT",
    ];
    let got: Vec<String> = conds.iter().map(|c| count(c)).collect();
    assert_eq!(got, ["2", "6", "4", "4", "6", "2", "6"]);
}

/// Verifies: [EXTENSION] graph standard §5, RULES.md D144: a label
/// expression missing its label is refused (SQLCODE -102).
#[test]
fn a_label_expression_needs_a_label_5() {
    let (_db, s) = bank();
    let (code, msg) = failed(&s, "EXECUTE GQL ON GRAPH BANK 'MATCH (a) FILTER a:1 RETURN a'");
    assert_eq!(code, -102, "{msg}");
    assert!(msg.contains("expected a label"), "{msg}");
}

/// Verifies: [EXTENSION] graph standard §6, RULES.md D144: an aggregate
/// of a group variable takes DISTINCT, a group variable's property is
/// refused as a single value (GRF-0022, SQLCODE -103), an aggregate of
/// a group variable's property whose argument goes on past it is refused
/// (SQLCODE -102), and so are PREV
/// and a path function of an element variable outside a pattern (SQLCODE
/// -103).
#[test]
fn group_variables_and_path_functions_6() {
    let (_db, s) = bank();
    let got = gql(
        &s,
        "MATCH (a:ACCOUNT {owner: 'Ann'})-[t:TRANSFER]->{2}(b) RETURN COUNT(DISTINCT t.amount) AS d, MIN(ALL t.amount) AS m, COUNT(ALL t) AS c",
    );
    assert_eq!(got, ["2,50.00,2"]);
    let refused = [
        ("MATCH (a:ACCOUNT {owner: ''Ann''})-[t:TRANSFER]->{2}(b) RETURN t.amount AS x", -103, "GRF-0022"),
        ("MATCH (a:ACCOUNT {owner: ''Ann''})-[t:TRANSFER]->{2}(b) RETURN SUM(t.amount + 1) AS x", -102, "closing SUM"),
        ("MATCH (a:ACCOUNT) RETURN PREV(a) AS x", -103, "PREV"),
        ("MATCH (a:ACCOUNT) RETURN PATH_LENGTH(a) AS x", -103, "path variable"),
    ];
    for (text, sqlcode, word) in refused {
        let (code, msg) = failed(&s, &format!("EXECUTE GQL ON GRAPH BANK '{text}'"));
        assert_eq!(code, sqlcode, "{text}: {msg}");
        assert!(msg.contains(word), "{text}: {msg}");
    }
}

/// Verifies: [EXTENSION] graph standard §4.3, RULES.md D144: an open
/// graph's property compared with a Boolean literal is read as BOOLEAN,
/// and an aggregate of a group variable's property across its elements
/// reads each from its map, as a number for SUM.
#[test]
fn open_graph_properties_and_groups_4_3() {
    let db = turborel::db::Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let s = db.session("A");
    s.run("CREATE GRAPH FREE ANY").unwrap();
    let on = |text: &str| direct(&s, &format!("EXECUTE GQL ON GRAPH FREE '{}'", text.replace('\'', "''")));
    on("INSERT (:A {x: 1})-[:R {w: 2}]->(:B {y: true})-[:R {w: 3}]->(:C {y: false})");
    assert_eq!(on("MATCH (b) FILTER b.y = TRUE RETURN LABELS(b) AS l"), ["|B|"]);
    assert_eq!(on("MATCH (b) FILTER b.y = -1 RETURN COUNT(*) AS n"), ["0"]);
    assert_eq!(on("MATCH (a:A)-[r]->{2}(c) RETURN SUM(r.w) AS s, MAX(r.w) AS m"), ["5E0,3"]);
    let (code, msg) = failed(&s, "EXECUTE GQL ON GRAPH FREE 'MATCH (a:A)-[r]->{2}(c) RETURN r.w AS w'");
    assert_eq!(code, -103, "{msg}");
    assert!(msg.contains("GRF-0022"), "{msg}");
}

/// Verifies: [EXTENSION] graph standard §9 [P2 §5.1]: a named procedure's
/// call joins each row with the rows of its table function, YIELD naming
/// its columns, OPTIONAL keeping the rows it gives none; FOR without a
/// place unnests a list; a variable bound twice is refused (SQLCODE -103).
#[test]
fn named_calls_and_bindings_9() {
    let (_db, s) = bank();
    s.run(
        "CREATE PROCEDURE UPTO_F (IN F INTEGER, IN RR VARCHAR(128), INOUT ST CHAR(5)) CONTAINS SQL \
         BEGIN DECLARE I INTEGER DEFAULT 0; L: WHILE I < F DO SET I = I + 1; \
         SET DESCRIPTOR PTF RR VALUE 1 DATA = I; PIPE ROW PTF RR; END WHILE L; END",
    )
    .unwrap();
    s.run(
        "CREATE PROCEDURE ASK (IN F INTEGER, IN RQ VARCHAR(128), INOUT ST CHAR(5)) CONTAINS SQL DETERMINISTIC \
         BEGIN SET DESCRIPTOR PTF RQ COUNT = 1; SET DESCRIPTOR PTF RQ VALUE 1 NAME = 'N', TYPE = 4; END",
    )
    .unwrap();
    s.run("CREATE FUNCTION SERIES (F INTEGER) RETURNS TABLE CONTAINS SQL DESCRIBE WITH PROCEDURE ASK FULFILL WITH PROCEDURE UPTO_F")
        .unwrap();
    let got = gql(&s, "MATCH (c:CITY) CALL SERIES(2) YIELD N AS k RETURN c.city_name AS c, k ORDER BY c, k");
    assert_eq!(got, ["Oslo,1", "Oslo,2", "Rome,1", "Rome,2"]);
    let optional = gql(&s, "MATCH (c:CITY) OPTIONAL CALL SERIES(0) RETURN COUNT(*) AS n");
    assert_eq!(optional, ["2"]);
    assert_eq!(gql(&s, "FOR x IN [3, 4] RETURN SUM(x) AS s"), ["7"]);
    let (code, msg) = failed(&s, "EXECUTE GQL ON GRAPH BANK 'MATCH (a:ACCOUNT) LET a = 1 RETURN a'");
    assert_eq!(code, -103, "{msg}");
    assert!(msg.contains("already bound"), "{msg}");
}

/// Verifies: [EXTENSION] graph standard §9, RULES.md D144: GQL_TABLE
/// refuses a data-modifying statement (SQLCODE -900), which runs through
/// EXECUTE GQL alone; a group variable bound again as one element is
/// refused (SQLCODE -103).
#[test]
fn gql_table_reads_only_9() {
    let (_db, s) = bank();
    let e = s.direct("SELECT * FROM GQL_TABLE(BANK, 'MATCH (a:ACCOUNT) SET a.owner = ''X'' RETURN a.owner AS o') AS G").err().unwrap();
    assert_eq!(e.sqlcode(), -900, "{}", e.msg);
    let again = "EXECUTE GQL ON GRAPH BANK 'MATCH (a:ACCOUNT {owner: ''Ann''})-[t:TRANSFER]->{2}(b) MATCH ()-[t]->() RETURN COUNT(*) AS n'";
    let (code, msg) = failed(&s, again);
    assert_eq!(code, -103, "{msg}");
}
