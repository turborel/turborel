//! The Information Schema [ISO/IEC 9075:1992 §21.2]: its views, each the
//! standard's own definition over the Definition Schema [§21.3], showing
//! each user what it may see of the catalog.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::value::Value;

use crate::common::{code, exec};

/// The rows of `sql` in `s`, each value with its trailing spaces cut, in
/// order.
fn seen(s: &Session, sql: &str) -> Vec<String> {
    let mut c = s.open_cursor(sql).unwrap_or_else(|e| panic!("{sql}: {}", e.msg));
    let mut out = Vec::new();
    while let Some(r) = c.fetch().unwrap_or_else(|e| panic!("{sql}: {}", e.msg)) {
        out.push(r.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join("|"));
    }
    out
}

/// Two schemas: S's tables, one with a referential constraint on the
/// other, two views, and privileges granted to R and PUBLIC; and R's table.
fn catalog() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S
           CREATE TABLE P (K INTEGER NOT NULL, J INTEGER NOT NULL, NAME CHAR(8) DEFAULT 'NONE', PRIMARY KEY (K, J))
           CREATE TABLE C (I INTEGER PRIMARY KEY, A INTEGER, B INTEGER, X REAL, Y FLOAT(40), D DECIMAL(6,2),
             CONSTRAINT C_P FOREIGN KEY (B, A) REFERENCES P (J, K) ON DELETE CASCADE,
             CONSTRAINT POSITIVE CHECK (A > 0 AND B IN (SELECT J FROM P)) DEFERRABLE INITIALLY DEFERRED)
           CREATE VIEW BIG (I, A) AS SELECT I, A FROM C WHERE A > 9 WITH CHECK OPTION
           CREATE VIEW ALL_P AS SELECT * FROM P
           GRANT SELECT ON BIG TO R
           GRANT UPDATE (A) ON C TO R WITH GRANT OPTION
           GRANT INSERT ON P TO PUBLIC",
    )
    .unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION R CREATE TABLE MINE (M CHAR(3))").unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.2.4, §21.2.7, §21.2.9]:
/// TABLES and COLUMNS show a user the tables and views it holds a privilege
/// on, the Information Schema's among them, and SCHEMATA the schemas it
/// owns; each column at its position, with its default, nullability and
/// data type: a character string with its length, length in octets and
/// character set, an exact numeric of decimal precision, an approximate one
/// of binary precision, each by its type's name as written.
#[test]
fn tables_and_columns_are_described_21_2_7() {
    let db = catalog();
    let (owner, other) = (db.session("S"), db.session("R"));
    let tables = "SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE FROM INFORMATION_SCHEMA.TABLES \
                  WHERE TABLE_SCHEMA <> 'INFORMATION_SCHEMA' ORDER BY TABLE_SCHEMA DESC, TABLE_NAME";
    assert_eq!(seen(&owner, tables), ["S|ALL_P|VIEW", "S|BIG|VIEW", "S|C|BASE TABLE", "S|P|BASE TABLE"]);
    assert_eq!(seen(&other, tables), ["S|BIG|VIEW", "S|C|BASE TABLE", "S|P|BASE TABLE", "R|MINE|BASE TABLE"], "by its privileges");
    assert_eq!(seen(&db.session("NOBODY"), tables), ["S|P|BASE TABLE"], "PUBLIC's");
    let own = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'INFORMATION_SCHEMA'";
    assert_eq!(
        seen(&db.session("NOBODY"), own),
        ["31"],
        "23 views, TABLE_OPTIONS, INDEXES, INDEX_OPTIONS, INFORMATION_SCHEMA_CATALOG_NAME, SETTINGS, USERS, MEMORY_RESERVATIONS and JOBS"
    );
    assert_eq!(
        seen(&owner, "SELECT CATALOG_NAME, SCHEMA_NAME, DEFAULT_CHARACTER_SET_NAME FROM INFORMATION_SCHEMA.SCHEMATA"),
        ["TURBOREL|S|SQL_TEXT"]
    );
    let columns = "SELECT COLUMN_NAME, ORDINAL_POSITION, COLUMN_DEFAULT, IS_NULLABLE, DATA_TYPE, CHARACTER_MAXIMUM_LENGTH, \
                   CHARACTER_OCTET_LENGTH, NUMERIC_PRECISION, NUMERIC_PRECISION_RADIX, NUMERIC_SCALE, CHARACTER_SET_NAME \
                   FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = ";
    assert_eq!(
        seen(&owner, &format!("{columns}'P' ORDER BY ORDINAL_POSITION")),
        [
            "K|1|NULL|NO|INTEGER|NULL|NULL|10|10|0|NULL",
            "J|2|NULL|NO|INTEGER|NULL|NULL|10|10|0|NULL",
            "NAME|3|'NONE    '|YES|CHARACTER|8|32|NULL|NULL|NULL|SQL_TEXT",
        ]
    );
    assert_eq!(
        seen(&owner, &format!("{columns}'C' AND ORDINAL_POSITION > 3 ORDER BY 2")),
        [
            "X|4|NULL|YES|REAL|NULL|NULL|24|2|NULL|NULL",
            "Y|5|NULL|YES|FLOAT|NULL|NULL|53|2|NULL|NULL",
            "D|6|NULL|YES|DECIMAL|NULL|NULL|6|10|2|NULL",
        ]
    );
    assert_eq!(seen(&other, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'C'"), ["1"], "the column it may update");
    let described = "SELECT ORDINAL_POSITION, COLUMN_NAME, DATA_TYPE, CHARACTER_MAXIMUM_LENGTH FROM INFORMATION_SCHEMA.COLUMNS \
                     WHERE TABLE_NAME = 'TABLES' AND TABLE_SCHEMA = 'INFORMATION_SCHEMA' ORDER BY ORDINAL_POSITION";
    assert_eq!(
        seen(&other, described),
        [
            "1|TABLE_CATALOG|CHARACTER VARYING|128",
            "2|TABLE_SCHEMA|CHARACTER VARYING|128",
            "3|TABLE_NAME|CHARACTER VARYING|128",
            "4|TABLE_TYPE|CHARACTER VARYING|32767",
        ]
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.2.8, §21.2.21, §21.2.22,
/// §11.19 SR13]: VIEWS shows each view, its definition to its owner alone,
/// WITH CHECK OPTION as CASCADED, and whether it is updatable; and
/// VIEW_TABLE_USAGE and VIEW_COLUMN_USAGE show its owner the tables and
/// columns each references, `*` naming each column.
#[test]
fn views_and_what_they_use_are_described_21_2_8() {
    let db = catalog();
    let (owner, other) = (db.session("S"), db.session("R"));
    let views = "SELECT TABLE_NAME, VIEW_DEFINITION, CHECK_OPTION, IS_UPDATABLE FROM INFORMATION_SCHEMA.VIEWS \
                 WHERE TABLE_SCHEMA = 'S' ORDER BY TABLE_NAME";
    assert_eq!(seen(&owner, views), ["ALL_P|SELECT * FROM P|NONE|YES", "BIG|SELECT I, A FROM C WHERE A > 9|CASCADED|YES"]);
    assert_eq!(seen(&other, views), ["BIG|NULL|CASCADED|YES"], "its definition is its owner's to see");
    let information = "SELECT CHECK_OPTION, IS_UPDATABLE FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_NAME = 'VIEWS'";
    assert_eq!(seen(&other, information), ["NONE|NO"]);
    let tables = "SELECT VIEW_NAME, TABLE_NAME FROM INFORMATION_SCHEMA.VIEW_TABLE_USAGE ORDER BY VIEW_NAME";
    assert_eq!(seen(&owner, tables), ["ALL_P|P", "BIG|C"]);
    assert_eq!(seen(&other, tables), Vec::<String>::new(), "its owner's");
    let columns = "SELECT VIEW_NAME, TABLE_NAME, COLUMN_NAME FROM INFORMATION_SCHEMA.VIEW_COLUMN_USAGE \
                   ORDER BY VIEW_NAME, COLUMN_NAME";
    assert_eq!(seen(&owner, columns), ["ALL_P|P|J", "ALL_P|P|K", "ALL_P|P|NAME", "BIG|C|A", "BIG|C|I"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.2.13-§21.2.16, §21.2.23,
/// §21.2.24, §11.4 SR9a]: the constraints of a user's schema, each of its
/// type and deferral, NOT NULL among them as the CHECK it is; a key's
/// columns in order, a foreign key's at the positions of the columns they
/// reference; a referential constraint's unique constraint, match type and
/// rules; a CHECK's condition, and the tables and columns each constraint
/// uses.
#[test]
fn constraints_are_described_21_2_13() {
    let db = catalog();
    let s = db.session("S");
    let constraints = "SELECT CONSTRAINT_NAME, CONSTRAINT_TYPE, TABLE_NAME, IS_DEFERRABLE, INITIALLY_DEFERRED \
                       FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS WHERE TABLE_SCHEMA = 'S' ORDER BY 1";
    assert_eq!(
        seen(&s, constraints),
        [
            "C_P|FOREIGN KEY|C|NO|NO",
            "C_PRIMARY1|PRIMARY KEY|C|NO|NO",
            "POSITIVE|CHECK|C|YES|YES",
            "P_NOTNULL1|CHECK|P|NO|NO",
            "P_NOTNULL2|CHECK|P|NO|NO",
            "P_PRIMARY1|PRIMARY KEY|P|NO|NO",
        ]
    );
    let keys = "SELECT CONSTRAINT_NAME, COLUMN_NAME, ORDINAL_POSITION FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE ORDER BY 1, 3";
    assert_eq!(seen(&s, keys), ["C_P|A|1", "C_P|B|2", "C_PRIMARY1|I|1", "P_PRIMARY1|K|1", "P_PRIMARY1|J|2"]);
    let referential = "SELECT CONSTRAINT_NAME, UNIQUE_CONSTRAINT_NAME, MATCH_OPTION, UPDATE_RULE, DELETE_RULE \
                       FROM INFORMATION_SCHEMA.REFERENTIAL_CONSTRAINTS";
    assert_eq!(seen(&s, referential), ["C_P|P_PRIMARY1|NONE|NO ACTION|CASCADE"]);
    let checks = "SELECT CONSTRAINT_NAME, CHECK_CLAUSE FROM INFORMATION_SCHEMA.CHECK_CONSTRAINTS ORDER BY 1";
    assert_eq!(seen(&s, checks), ["POSITIVE|A > 0 AND B IN (SELECT J FROM P)", "P_NOTNULL1|K IS NOT NULL", "P_NOTNULL2|J IS NOT NULL"]);
    let tables = "SELECT TABLE_NAME, CONSTRAINT_NAME FROM INFORMATION_SCHEMA.CONSTRAINT_TABLE_USAGE WHERE CONSTRAINT_NAME = 'POSITIVE'";
    assert_eq!(seen(&s, tables), ["C|POSITIVE", "P|POSITIVE"]);
    let columns =
        "SELECT TABLE_NAME, COLUMN_NAME FROM INFORMATION_SCHEMA.CONSTRAINT_COLUMN_USAGE WHERE CONSTRAINT_NAME = 'POSITIVE' ORDER BY 1, 2";
    assert_eq!(seen(&s, columns), ["C|A", "C|B", "P|J"]);
    assert_eq!(
        seen(&db.session("R"), "SELECT * FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS WHERE TABLE_SCHEMA = 'S'"),
        Vec::<String>::new(),
        "S's"
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.2.10-§21.2.12, §4.26,
/// §11.3 GR4, §10.4 SR2]: TABLE_PRIVILEGES and COLUMN_PRIVILEGES show the
/// privileges a user granted or holds, PUBLIC's among them: an owner's of
/// the grantor "_SYSTEM", every privilege on its base table and its
/// columns, grantable; one granted on a column alone a column privilege;
/// and USAGE_PRIVILEGES PUBLIC's USAGE on the character set and collation.
#[test]
fn privileges_are_described_21_2_10() {
    let db = catalog();
    let other = db.session("R");
    let table = "SELECT GRANTOR, GRANTEE, TABLE_NAME, PRIVILEGE_TYPE, IS_GRANTABLE FROM INFORMATION_SCHEMA.TABLE_PRIVILEGES \
                 WHERE TABLE_SCHEMA = 'S'";
    assert_eq!(seen(&other, table), ["S|PUBLIC|P|INSERT|NO", "S|R|BIG|SELECT|NO"]);
    let mine = "SELECT GRANTOR, PRIVILEGE_TYPE, IS_GRANTABLE FROM INFORMATION_SCHEMA.TABLE_PRIVILEGES WHERE TABLE_NAME = 'MINE'";
    assert_eq!(
        seen(&other, mine),
        ["_SYSTEM|DELETE|YES", "_SYSTEM|INSERT|YES", "_SYSTEM|REFERENCES|YES", "_SYSTEM|SELECT|YES", "_SYSTEM|UPDATE|YES"]
    );
    let column = "SELECT GRANTOR, GRANTEE, TABLE_NAME, COLUMN_NAME, PRIVILEGE_TYPE, IS_GRANTABLE \
                  FROM INFORMATION_SCHEMA.COLUMN_PRIVILEGES WHERE TABLE_SCHEMA = 'S' AND TABLE_NAME <> 'P'";
    assert_eq!(seen(&other, column), ["S|R|BIG|A|SELECT|NO", "S|R|BIG|I|SELECT|NO", "S|R|C|A|UPDATE|YES"]);
    let usage = "SELECT GRANTOR, GRANTEE, OBJECT_NAME, OBJECT_TYPE, PRIVILEGE_TYPE, IS_GRANTABLE FROM INFORMATION_SCHEMA.USAGE_PRIVILEGES";
    assert_eq!(seen(&other, usage), ["_SYSTEM|PUBLIC|SQL_TEXT|CHARACTER SET|USAGE|YES", "_SYSTEM|PUBLIC|SQL_TEXT|COLLATION|USAGE|NO"]);
    let information = "SELECT GRANTOR, GRANTEE, IS_GRANTABLE FROM INFORMATION_SCHEMA.TABLE_PRIVILEGES WHERE TABLE_NAME = 'COLUMNS'";
    assert_eq!(seen(&other, information), ["INFORMATION_SCHEMA|PUBLIC|YES"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.1, §21.2.2, §21.2.18,
/// §21.2.19, §21.3.2, §21.3.26]: the Definition Schema cannot be accessed in an
/// SQL-statement, nor a schema of either made; the views cannot be
/// updated; the catalog's name is the setting CATALOG_NAME's; the
/// character set and collation are SQL_TEXT, PAD SPACE; and a view that
/// reads one shows what its reader sees, CURRENT_USER being the reader's
/// [§4.26, §6.2 GR9], SELECT on it grantable, SELECT on the Information
/// Schema being PUBLIC's WITH GRANT OPTION \[§21.1\]; SQL_LANGUAGES
/// gives the conformance claimed.
#[test]
fn the_information_schema_is_the_databases_21_1() {
    let db = catalog();
    let s = db.session("S");
    assert_eq!(code(&s, "SELECT * FROM DEFINITION_SCHEMA.TABLES"), Code::SyntaxRule);
    assert_eq!(
        code(&s, "SELECT * FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME IN (SELECT TABLE_NAME FROM DEFINITION_SCHEMA.VIEWS)"),
        Code::SyntaxRule
    );
    for made in
        ["CREATE SCHEMA AUTHORIZATION DEFINITION_SCHEMA CREATE TABLE T (K INTEGER)", "CREATE SCHEMA AUTHORIZATION INFORMATION_SCHEMA"]
    {
        assert_eq!(db.schema(made).err().map(|e| e.code), Some(Code::SyntaxRule), "{made}");
    }
    assert_eq!(exec(&s, "DELETE FROM INFORMATION_SCHEMA.TABLES"), Err(Code::Privilege));
    assert_eq!(exec(&s, "UPDATE INFORMATION_SCHEMA.COLUMNS SET COLUMN_NAME = 'X'"), Err(Code::Privilege));
    db.set_setting("CATALOG_NAME", &Value::Char("SHOP".into())).unwrap();
    assert_eq!(seen(&s, "SELECT * FROM INFORMATION_SCHEMA.INFORMATION_SCHEMA_CATALOG_NAME"), ["SHOP"]);
    assert_eq!(seen(&s, "SELECT TABLE_CATALOG FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = 'P'"), ["SHOP"]);
    assert_eq!(
        seen(
            &s,
            "SELECT CHARACTER_SET_NAME, FORM_OF_USE, NUMBER_OF_CHARACTERS, DEFAULT_COLLATE_NAME FROM INFORMATION_SCHEMA.CHARACTER_SETS"
        ),
        ["SQL_TEXT|UTF-8|1112064|SQL_TEXT"]
    );
    assert_eq!(seen(&s, "SELECT COLLATION_NAME, PAD_ATTRIBUTE FROM INFORMATION_SCHEMA.COLLATIONS"), ["SQL_TEXT|PAD SPACE"]);
    db.schema(
        "CREATE SCHEMA AUTHORIZATION V CREATE VIEW SEEN (T) AS SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'S'",
    )
    .unwrap();
    assert_eq!(seen(&db.session("V"), "SELECT T FROM SEEN"), ["P"], "V's: PUBLIC's");
    db.schema(
        "CREATE SCHEMA AUTHORIZATION X CREATE VIEW SEEN (T) AS SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES \
               WHERE TABLE_SCHEMA = 'S' GRANT SELECT ON SEEN TO R",
    )
    .unwrap();
    assert_eq!(seen(&db.session("R"), "SELECT T FROM X.SEEN ORDER BY T"), ["BIG", "C", "P"], "the reader's, R's");
    for empty in ["DOMAINS", "DOMAIN_CONSTRAINTS", "ASSERTIONS", "TRANSLATIONS", "COLUMN_DOMAIN_USAGE"] {
        assert_eq!(seen(&s, &format!("SELECT * FROM INFORMATION_SCHEMA.{empty}")), Vec::<String>::new(), "{empty}");
    }
    // [§21.3.26] the conformance claimed: SQL-86, ISO 9075 of 1987, Level 2, by direct invocation.
    assert_eq!(seen(&s, "SELECT * FROM INFORMATION_SCHEMA.SQL_LANGUAGES"), ["ISO 9075|1987|2|NULL|NULL|DIRECT|NULL"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.2.9, §21.3.5, §6.1]: a
/// column's DATA_TYPE is its type's name as written: INTEGER, SMALLINT,
/// DECIMAL, NUMERIC, REAL, DOUBLE PRECISION or FLOAT, as a database kept
/// in a directory gives it once opened again; the name is no part of the
/// type, so a foreign key of INTEGER references NUMERIC(10,0), and a
/// message names the column's type as written.
#[test]
fn number_types_are_described_as_written_21_3_5() {
    let scratch = crate::common::Scratch::new("type-names");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&crate::common::os(), &dir, &turborel::db::OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION N
           CREATE TABLE P (K NUMERIC(10) PRIMARY KEY)
           CREATE TABLE T (I INTEGER REFERENCES P, S SMALLINT, D DEC(4,1), N NUMERIC(3), R REAL, P DOUBLE PRECISION, F FLOAT(10))",
    )
    .unwrap();
    let types = "SELECT ORDINAL_POSITION, COLUMN_NAME, DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'T' ORDER BY 1";
    let written = ["1|I|INTEGER", "2|S|SMALLINT", "3|D|DECIMAL", "4|N|NUMERIC", "5|R|REAL", "6|P|DOUBLE PRECISION", "7|F|FLOAT"];
    assert_eq!(seen(&db.session("N"), types), written);
    let refused = db.session("N").run("INSERT INTO T (S) VALUES (100000)").unwrap_err();
    assert!(refused.msg.contains("does not fit SMALLINT"), "{}", refused.msg);
    db.close().unwrap();
    drop(db);
    assert_eq!(seen(&open().session("N"), types), written, "kept with the catalog");
}
