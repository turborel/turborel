//! Unit tests of statements cut short: each statement below is read
//! whole, and each text that is a proper prefix of it, ending before one
//! of its tokens, is refused with SQLCODE -102 unless it is a statement
//! itself. The prefixes that are statements are listed, so a Format
//! that reads too much or too little shows.

use super::*;
use crate::error::Code;
use crate::lexer::lex;

/// The statement `text` reads as, its end included.
fn statement(text: &str) -> Result<SqlStatement> {
    Parser::new(text)?.sql_statement()
}

/// The proper prefixes of `text` ending before a token of it that are
/// statements, read whole; `text` itself must be one. Every other prefix
/// must be refused with SQLCODE -102.
fn complete_prefixes(text: &str) -> Vec<String> {
    prefixes_read_by(text, &|t| statement(t).map(|_| ()))
}

/// `complete_prefixes`, each text read by `read`.
fn prefixes_read_by(text: &str, read: &dyn Fn(&str) -> Result<()>) -> Vec<String> {
    read(text).unwrap_or_else(|e| panic!("{text}: {e:?}"));
    let chars: Vec<char> = text.chars().collect();
    let mut complete = Vec::new();
    for t in lex(text).unwrap().iter().filter(|t| t.tok != Tok::End).skip(1) {
        let prefix: String = chars[..t.start].iter().collect();
        match read(&prefix) {
            Ok(()) => complete.push(prefix.trim_end().to_owned()),
            Err(e) => assert_eq!(e.code, Code::Syntax, "{prefix}: {e:?}"),
        }
    }
    complete
}

/// Verifies: [EXTENSION] RULES.md D140 a `<create property graph
/// statement>` of every part of its Format cut short is refused (SQLCODE
/// -102) but where what is read is a statement: its vertex tables alone,
/// and before its options and its rights.
#[test]
fn create_property_graph_cut_short() {
    let text = "CREATE OR REPLACE PROPERTY GRAPH S.G VERTEX TABLES (A.ACCOUNT AS ACC KEY (ID) LABEL ACCOUNT \
                PROPERTIES (ID, UPPER(OWNER) AS LOUD) DEFAULT LABEL NO PROPERTIES, CITY PROPERTIES ARE ALL COLUMNS EXCEPT (CODE, NAME)) \
                EDGE TABLES (TRANSFER KEY (TXN_ID) SOURCE KEY (FROM_ACCT) REFERENCES ACC (ID) DESTINATION KEY (TO_ACCT) REFERENCES ACC \
                LABEL T PROPERTIES ARE ALL COLUMNS, LIVES UNDIRECTED SOURCE ACC DESTINATION CITY NO PROPERTIES) \
                OPTIONS (ENFORCED MODE) WITH INVOKER RIGHTS";
    let complete = complete_prefixes(text);
    assert_eq!(complete.len(), 3, "{complete:#?}");
}

/// Verifies: [EXTENSION] RULES.md D140 [GS §11.3] a `<create property
/// graph statement>` of queries' rows, IF NOT EXISTS, cut short is refused
/// (SQLCODE -102) but where its vertices are read whole.
#[test]
fn create_property_graph_of_queries_cut_short() {
    let text = "CREATE PROPERTY GRAPH IF NOT EXISTS LARGE AS VERTICES FROM (SELECT ID FROM ACCOUNT) KEY (ID) LABEL HOLDER \
                PROPERTIES (ID), (SELECT ID FROM CITY) AS TOWN KEY (ID) EDGES FROM (SELECT ID, SRC, DST FROM TRANSFER) KEY (ID) \
                SOURCE KEY (SRC) REFERENCES HOLDER (ID) DESTINATION KEY (DST) REFERENCES HOLDER (ID) LABEL BIG";
    let complete = complete_prefixes(text);
    assert_eq!(complete.len(), 4, "{complete:#?}");
}

/// Verifies: [EXTENSION] RULES.md D140 each action of an `<alter property
/// graph statement>` and a `<drop property graph statement>` cut short is
/// refused (SQLCODE -102) but where what is read is a statement.
#[test]
fn alter_and_drop_property_graph_cut_short() {
    let texts = [
        ("ALTER PROPERTY GRAPH G ADD VERTEX TABLES (PHONE KEY (DIGITS) LABEL PLACE, CITY)", 0),
        ("ALTER PROPERTY GRAPH G ADD EDGE TABLES (E SOURCE A DESTINATION B DEFAULT LABEL)", 0),
        ("ALTER PROPERTY GRAPH G DROP EDGE TABLES (T, U) CASCADE", 1),
        ("ALTER PROPERTY GRAPH G ALTER VERTEX TABLE ACCOUNT ALTER LABEL ACCOUNT ADD PROPERTIES (ACCT_ID AS ID, UPPER(OWNER) AS LOUD)", 0),
        ("ALTER PROPERTY GRAPH G ALTER EDGE TABLE ACCOUNT ALTER LABEL PERSON DROP PROPERTIES (OWNER, X) CASCADE", 1),
        ("ALTER PROPERTY GRAPH G ALTER VERTEX TABLE CITY DROP LABEL PLACE RESTRICT", 1),
        ("ALTER PROPERTY GRAPH G ALTER VERTEX TABLE ACCOUNT ADD LABEL PERSON PROPERTIES (OWNER)", 1),
        ("ALTER PROPERTY GRAPH S.G COMPILE", 0),
        ("DROP PROPERTY GRAPH IF EXISTS S.G CASCADE", 3),
    ];
    let got: Vec<(&str, usize)> = texts.iter().map(|(text, _)| (*text, complete_prefixes(text).len())).collect();
    assert_eq!(got, texts);
}

/// Verifies: [EXTENSION] RULES.md D144 [graph standard §4.3, §9] CREATE
/// GRAPH TYPE, CREATE GRAPH, DROP GRAPH and EXECUTE GQL cut short are
/// refused (SQLCODE -102) but where what is read is a statement.
#[test]
fn gql_statements_cut_short() {
    let texts = [
        (
            "CREATE GRAPH TYPE IF NOT EXISTS SOCIAL AS { NODE TYPE PERSON LABEL P & Q {NAME STRING NOT NULL, AGE INT32}, \
             NODE CITY {NAME STRING}, DIRECTED EDGE TYPE KNOWS LABELS K {SINCE INT32} CONNECTING (PERSON -> PERSON), \
             UNDIRECTED EDGE NEAR : N CONNECTING (CITY ~ CITY) }",
            0,
        ),
        ("CREATE GRAPH IF NOT EXISTS S.NET TYPED SOCIAL", 3),
        ("CREATE GRAPH FREE ANY PROPERTY GRAPH", 2),
        ("DROP GRAPH TYPE IF EXISTS SOCIAL CASCADE", 2),
        ("EXECUTE GQL ON GRAPH S.NET 'MATCH (a) RETURN a'", 0),
        ("INSERT INTO T SELECT * FROM GQL_TABLE(S.BANK, 'MATCH (a) RETURN a') AS G (A, B)", 3),
    ];
    let got: Vec<(&str, usize)> = texts.iter().map(|(text, _)| (*text, complete_prefixes(text).len())).collect();
    assert_eq!(got, texts);
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.6, §10.4, §20.16] a `<PTF derived
/// table>` whose arguments are table arguments of every option,
/// descriptor arguments and values, named or not, with a `<co-partition
/// clause>`, cut short is refused (SQLCODE -102) but where what is read is
/// a statement.
#[test]
fn ptf_arguments_cut_short() {
    let text = "INSERT INTO T SELECT * FROM TABLE (S.F (TABLE (S.T) AS X PARTITION BY (K, N) PRUNE WHEN EMPTY ORDER BY (N DESC), \
                TABLE (U) AS Y PARTITION BY K KEEP WHEN EMPTY, DESCRIPTOR (A, B INTEGER), CAST (NULL AS DESCRIPTOR), 1 + 2, \
                W => 3 COPARTITION (X, Y))) AS R (A, B)";
    let complete = complete_prefixes(text);
    assert_eq!(complete.len(), 2, "{complete:#?}");
}

/// Verifies: [EXTENSION] RULES.md D149, D150 [GS §13.2] CREATE ADJACENCY,
/// LABEL and REACHABILITY INDEX cut short are refused (SQLCODE -102) but
/// where what is read is a statement.
#[test]
fn graph_indexes_cut_short() {
    let texts = [
        ("CREATE ADJACENCY INDEX FWD ON GRAPH BANK EDGE TABLE TRANSFER DIRECTION FORWARD INCLUDE (AMOUNT, X)", 2),
        ("CREATE LABEL INDEX L ON GRAPH S.BANK", 1),
        ("CREATE REACHABILITY INDEX R ON GRAPH S.BANK EDGE TABLE TRANSFER", 0),
    ];
    let got: Vec<(&str, usize)> = texts.iter().map(|(text, _)| (*text, complete_prefixes(text).len())).collect();
    assert_eq!(got, texts);
}

/// Verifies: [X3.135-1986 §6.1] [ISO/IEC 9075:1992 §11.1, §11.3] a `<schema
/// definition>` of many kinds of element, a temporary table and a module's
/// declared local temporary table cut short are refused (SQLCODE -102) but
/// where what is read is a statement.
#[test]
fn schema_definitions_cut_short() {
    let texts = [(
        "CREATE SCHEMA S AUTHORIZATION A DEFAULT CHARACTER SET SQL_TEXT \
             CREATE TABLE T (K INTEGER PRIMARY KEY, A INTEGER ARRAY[3], R ROW (X INTEGER, Y VARCHAR(5)) MULTISET) \
             CREATE VIEW V (K) AS SELECT K FROM T WITH CASCADED CHECK OPTION CREATE SEQUENCE Q \
             CREATE DOMAIN D AS INTEGER CHECK (VALUE > 0) GRANT SELECT ON T TO PUBLIC",
        9,
    )];
    let schema = |t: &str| Parser::new(t)?.schema().map(|_| ());
    let got: Vec<(&str, usize)> = texts.iter().map(|(text, _)| (*text, prefixes_read_by(text, &schema).len())).collect();
    assert_eq!(got, texts);
    let tables = [
        ("CREATE GLOBAL TEMPORARY TABLE TT (K INTEGER) ON COMMIT PRESERVE ROWS", 1),
        ("DECLARE LOCAL TEMPORARY TABLE MODULE.TT (K INTEGER) ON COMMIT DELETE ROWS", 1),
    ];
    let read: Vec<(&str, usize)> = tables.iter().map(|(text, _)| (*text, complete_prefixes(text).len())).collect();
    assert_eq!(read, tables);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.16, §6.25, §6.31, §6.47, §7.6] arrays,
/// rows, their functions and a collection derived table cut short are
/// refused (SQLCODE -102) but where what is read is a statement.
#[test]
fn arrays_and_rows_cut_short() {
    let text = "INSERT INTO T SELECT ARRAY[1, 2][1], ARRAY(SELECT K FROM T ORDER BY K), CARDINALITY(A), TRIM_ARRAY(A, 1), \
                ARRAY_MAX_CARDINALITY(A), ROW(1, 2), (R).X, T.R.* AS (P, Q) FROM T, UNNEST(A, B) WITH ORDINALITY AS U (X, Y, N)";
    let complete = complete_prefixes(text);
    assert_eq!(complete.len(), 3, "{complete:#?}");
}
