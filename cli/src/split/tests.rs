//! Tests of splitting text into statements and commands.

use super::*;

fn split(text: &str) -> Vec<String> {
    let mut splitter = Splitter::default();
    let mut all = splitter.feed(text);
    all.extend(splitter.finish());
    all
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.2, §5.3]; [DESIGN] RULES.md D46: text
/// splits at each `;` that ends a statement, not one in a string, its
/// quote doubled, nor in a delimited identifier, nor in a comment; a
/// statement of separators alone is none; the last needs no `;`.
#[test]
fn statements_split_at_their_ends() {
    assert_eq!(split("SELECT 1; SELECT 2"), ["SELECT 1", "SELECT 2"]);
    assert_eq!(split("SELECT 'a;b''c;'; SELECT \"x;\"\"y\" FROM T;"), ["SELECT 'a;b''c;'", "SELECT \"x;\"\"y\" FROM T"]);
    assert_eq!(split("SELECT 1 -- a; comment\n; ;;"), ["SELECT 1 -- a; comment\n"]);
    assert_eq!(split("-- only a note;\n  ;\n"), Vec::<String>::new());
    assert_eq!(split("SELECT 1 - 2;"), ["SELECT 1 - 2"]);
    assert_eq!(split("SELECT 1 --- three\n"), ["SELECT 1 --- three\n"]);
    assert_eq!(split("SELECT 1 -- open"), ["SELECT 1 -- open\n"], "the end of the text ends a comment");
    assert_eq!(split("SELECT 1 \n  ;"), ["SELECT 1"], "no newline kept but a comment's");
    assert_eq!(split("-- one\n-- two\n;"), Vec::<String>::new());
}

/// Verifies: [DESIGN] RULES.md D46: text given in parts splits as if
/// whole; a statement begun, or a string or a comment open, is pending.
#[test]
fn text_in_parts_splits_as_whole() {
    let mut splitter = Splitter::default();
    assert_eq!(splitter.feed("SELECT 'a"), Vec::<String>::new());
    assert!(splitter.pending(), "in a string");
    assert_eq!(splitter.feed(";b' FROM T -"), Vec::<String>::new());
    assert_eq!(splitter.feed("- x;\n;"), ["SELECT 'a;b' FROM T -- x;\n"]);
    assert!(!splitter.pending());
    assert_eq!(splitter.feed("  "), Vec::<String>::new());
    assert!(!splitter.pending(), "separators alone");
    assert_eq!(splitter.finish(), None);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49] a `;` in BEGIN
/// ATOMIC ... END ends no statement, a CASE ... END in the block
/// closing no block, words matched in any case; the `;` after the
/// block's END ends the statement; ATOMIC alone, or a word BEGIN only
/// begins, opens none.
#[test]
fn semicolons_in_begin_atomic_end_no_statement() {
    let trigger = "CREATE TRIGGER TR AFTER INSERT ON T begin  atomic \
                   INSERT INTO U VALUES (CASE WHEN 1 = 1 THEN 1 ELSE 2 END); DELETE FROM U; End";
    assert_eq!(split(&format!("{trigger}; SELECT 1;")), [trigger, "SELECT 1"]);
    assert_eq!(split("SELECT ATOMIC FROM T; SELECT 2"), ["SELECT ATOMIC FROM T", "SELECT 2"]);
    assert_eq!(split("SELECT CASE WHEN A THEN 1 END FROM T; SELECT 2"), ["SELECT CASE WHEN A THEN 1 END FROM T", "SELECT 2"]);
    assert_eq!(split("SELECT BEGIN_ATOMIC; SELECT 2"), ["SELECT BEGIN_ATOMIC", "SELECT 2"]);
    assert_eq!(split("X BEGIN -- note\n ATOMIC A; B END; C"), ["X BEGIN -- note\n ATOMIC A; B END", "C"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49]; [ISO/IEC
/// 9075:1992 §5.2, §5.3] BEGIN ATOMIC in a string, a delimited
/// identifier or a comment opens no block.
#[test]
fn begin_atomic_quoted_or_commented_opens_no_block() {
    assert_eq!(split("SELECT 'BEGIN ATOMIC'; SELECT 2"), ["SELECT 'BEGIN ATOMIC'", "SELECT 2"]);
    assert_eq!(split("SELECT \"BEGIN ATOMIC\"; SELECT 2"), ["SELECT \"BEGIN ATOMIC\"", "SELECT 2"]);
    assert_eq!(split("SELECT 1 -- BEGIN ATOMIC\n; SELECT 2"), ["SELECT 1 -- BEGIN ATOMIC\n", "SELECT 2"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §14.1 to §14.13] a `;` in a
/// compound statement, plain or ATOMIC, ends no statement, nor one in
/// IF, CASE, LOOP, REPEAT, or WHILE or FOR, whose DO opens a level;
/// END IF and the like close one level, the word after END opening
/// none; out of a block IF, LOOP, REPEAT and DO open none. MC/DC of
/// `opens`: BEGIN out of a block opens, an opener in one opens, an
/// opener after END does not, an opener out of a block does not.
#[test]
fn semicolons_in_control_statements_end_no_statement() {
    let body = "CREATE PROCEDURE P () BEGIN DECLARE I INTEGER DEFAULT 0; \
                L: LOOP SET I = I + 1; IF I > 3 THEN LEAVE L; END IF; END LOOP L; \
                WHILE I > 0 DO SET I = I - 1; END WHILE; REPEAT SET I = I + 1; UNTIL I > 2 END REPEAT; \
                FOR R AS SELECT K FROM T DO CASE WHEN R.K = 1 THEN SET I = 0; ELSE SET I = 1; END CASE; END FOR; \
                B: BEGIN ATOMIC SET I = 2; END B; END";
    assert_eq!(split(&format!("{body}; SELECT 1;")), [body, "SELECT 1"]);
    assert_eq!(split("begin insert into t values (1); end; select 2"), ["begin insert into t values (1); end", "select 2"]);
    assert_eq!(
        split("DROP TABLE IF; SELECT LOOP, REPEAT, DO FROM T; SELECT 2"),
        ["DROP TABLE IF", "SELECT LOOP, REPEAT, DO FROM T", "SELECT 2"]
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §9.21] a `;` in a
/// CREATE MODULE, after a routine of it or in a body, ends no
/// statement; END MODULE closes it, a module in a schema definition
/// too; MODULE alone, as in MODULE.T, opens nothing.
#[test]
fn semicolons_in_a_module_end_no_statement() {
    let module = "create module M PROCEDURE P () BEGIN SET X = 1; END; \
                  FUNCTION F () RETURNS INTEGER RETURN CASE WHEN 1 = 1 THEN 1 END; END MODULE";
    assert_eq!(split(&format!("{module}; SELECT 2")), [module, "SELECT 2"]);
    let schema = "CREATE SCHEMA S CREATE TABLE T (A INT) CREATE MODULE M PROCEDURE P () DELETE FROM T; END MODULE";
    assert_eq!(split(&format!("{schema}; SELECT 2")), [schema, "SELECT 2"]);
    assert_eq!(split("SELECT * FROM MODULE.T; SELECT 2"), ["SELECT * FROM MODULE.T", "SELECT 2"]);
    assert_eq!(
        split("SELECT CREATE_ FROM T; DROP MODULE M CASCADE; SELECT 2"),
        ["SELECT CREATE_ FROM T", "DROP MODULE M CASCADE", "SELECT 2"]
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §4.10.7, §14.6 to §14.13] a
/// control statement run alone, labelled or not, keeps its `;`s: IF,
/// CASE, LOOP, REPEAT, and WHILE and FOR by their DO. A query whose
/// first word is none of them, or whose label-like first word is not
/// one, splits as before.
#[test]
fn control_statements_run_alone_keep_their_semicolons() {
    for alone in [
        "IF X > 1 THEN SIGNAL SQLSTATE '75001'; ELSE SIGNAL SQLSTATE '75002'; END IF",
        "case x when 1 then signal sqlstate '75001'; end case",
        "L: LOOP LEAVE L; END LOOP L",
        "REPEAT SIGNAL SQLSTATE '01001'; UNTIL TRUE END REPEAT",
        "W: WHILE FALSE DO SIGNAL SQLSTATE '75001'; END WHILE W",
        "FOR R AS SELECT K FROM T DO IF R.K = 1 THEN SIGNAL SQLSTATE '75001'; END IF; END FOR",
    ] {
        assert_eq!(split(&format!("{alone}; SELECT 1")), [alone, "SELECT 1"], "{alone}");
    }
    assert_eq!(split("SELECT IF FROM T; SELECT 2"), ["SELECT IF FROM T", "SELECT 2"]);
    assert_eq!(split("X: SELECT 1; SELECT 2"), ["X: SELECT 1", "SELECT 2"]);
}

fn pieces(lines: &[&str], vars: &Variables) -> Vec<Piece> {
    let mut splitter = Splitter::default();
    let mut all: Vec<Piece> = lines.iter().flat_map(|l| splitter.pieces(l, Some(vars))).collect();
    all.extend(splitter.finish().map(Piece::Statement));
    all
}

fn statement(sql: &str) -> Piece {
    Piece::Statement(sql.to_owned())
}

/// Verifies: [DESIGN] RULES.md D210: a `\` in code begins a command that
/// runs to the end of its line, the statement begun before it left
/// pending; a `\` in a literal, an identifier or a comment begins none.
#[test]
fn a_backslash_in_code_begins_a_command() {
    let vars = Variables::default();
    assert_eq!(
        pieces(&["SELECT 1; \\echo a; b\n", "SELECT '\\x', \"\\y\" -- \\z\n", ";"], &vars),
        [statement("SELECT 1"), Piece::Command("echo a; b".into()), statement("SELECT '\\x', \"\\y\" -- \\z\n")]
    );
    let mut splitter = Splitter::default();
    assert_eq!(splitter.pieces("SELECT 2 \\g\n", None), [Piece::Command("g".into())]);
    assert_eq!(splitter.pending_text(), "SELECT 2 ");
    assert_eq!(splitter.take_pending().as_deref(), Some("SELECT 2"));
    assert_eq!(splitter.take_pending(), None);
    splitter.pieces("SELECT 3", None);
    splitter.reset();
    assert!(!splitter.pending());
}

/// Verifies: [DESIGN] RULES.md D212; [ISO/IEC 9075:1992 §5.2, §5.3] a
/// variable is read into code as text, as a literal and as an identifier,
/// its text split as if typed, so a `;` in it ends a statement; in a
/// literal, an identifier or a comment it is not read; a variable not set,
/// `::`, and a label's colon are kept as written.
#[test]
fn variables_are_read_into_code_alone() {
    let mut vars = Variables::default();
    vars.set("T", "STAFF");
    vars.set("V", "it's");
    vars.set("TWO", "1; SELECT 2");
    assert_eq!(
        pieces(&["SELECT :'V', :\"T\" FROM :T WHERE ':T' = \":T\" -- :T\n", ";"], &vars),
        [statement("SELECT 'it''s', \"STAFF\" FROM STAFF WHERE ':T' = \":T\" -- :T\n")]
    );
    assert_eq!(pieces(&["SELECT :TWO;"], &vars), [statement("SELECT 1"), statement("SELECT 2")]);
    assert_eq!(pieces(&["SELECT :NONE, a::T, :{?T}, :{?NONE}"], &vars), [statement("SELECT :NONE, a::T, true, false")]);
    assert_eq!(pieces(&["L: LOOP LEAVE L; END LOOP L"], &vars), [statement("L: LOOP LEAVE L; END LOOP L")]);
    assert_eq!(pieces(&["SELECT :'T"], &vars), [statement("SELECT :'T")], "a quote not closed reads nothing");
}

/// Verifies: [DESIGN] RULES.md D219: a delimiter named ends a statement
/// wherever it stands in code, in a block too, and is cut; `;` ends none
/// then; in a literal it ends none; `;` named again is the default; a
/// delimiter of spaces, quotes, minus signs or backslashes is refused.
#[test]
fn a_delimiter_named_ends_statements() {
    let mut splitter = Splitter::default();
    splitter.set_delimiter(Some("//".into())).unwrap();
    let body = "CREATE PROCEDURE P () BEGIN SELECT 1; END";
    assert_eq!(splitter.pieces(&format!("{body}//\n"), None), [statement(body)]);
    assert_eq!(splitter.pieces("SELECT '//'; SELECT 2 //", None), [statement("SELECT '//'; SELECT 2")]);
    splitter.set_delimiter(Some(";".into())).unwrap();
    assert_eq!(splitter.pieces("SELECT 3;", None), [statement("SELECT 3")]);
    for bad in ["", "a b", "'", "--", "\\"] {
        assert!(splitter.set_delimiter(Some(bad.into())).is_err(), "{bad:?}");
    }
    splitter.set_delimiter(None).unwrap();
    assert_eq!(splitter.pieces("SELECT 4;", None), [statement("SELECT 4")]);
}

/// Verifies: [DESIGN] RULES.md D192, D210: SQL alone, with no command and
/// no variable, splits here as the protocol's splitter splits it, which
/// the server runs a Query of several statements by; so text typed and
/// text sent whole end their statements alike, whole or in parts.
#[test]
fn sql_splits_as_the_servers_splitter_splits_it() {
    let texts = [
        "SELECT 1; SELECT 2",
        "SELECT 'a;b''c;'; SELECT \"x;\"\"y\" FROM T;",
        "SELECT 1 -- a; comment\n; ;;",
        "SELECT 1 -- open",
        "CREATE PROCEDURE P() BEGIN IF 1 = 1 THEN SET X = 1; END IF; END; CALL P()",
        "L: WHILE X < 3 DO SET X = X + 1; END WHILE L; VALUES 1",
        "CREATE MODULE M CREATE PROCEDURE P() BEGIN END; END MODULE; VALUES 2",
        "CASE WHEN 1 = 1 THEN SET X = 1; ELSE SET X = 2; END CASE; FOR R AS SELECT 1 DO SET Y = 1; END FOR",
    ];
    for text in texts {
        let mut server = turborel_client::Splitter::default();
        let mut sent = server.feed(text);
        sent.extend(server.finish());
        assert_eq!(split(text), sent, "{text}");
        let (mut here, mut there) = (Splitter::default(), turborel_client::Splitter::default());
        for c in text.chars() {
            let mut one = [0_u8; 4];
            assert_eq!(here.feed(c.encode_utf8(&mut one)), there.feed(c.encode_utf8(&mut one)), "{text}");
            assert_eq!(here.pending(), there.pending(), "{text}");
        }
    }
}
