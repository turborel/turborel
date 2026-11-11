//! The collations `UNICODE_<LOCALE>` [ISO/IEC 9075-2:2023 §4.3.6, §10.7;
//! RULES.md D60]: a column, an ORDER BY and an index ordering a language's
//! text as its dictionaries do.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::rows;

/// Schema A: S, whose WORD orders by UNICODE_SV and is unique; G, of a
/// plain NAME. S holds Swedish words, G German names.
fn database() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE S (WORD VARCHAR(20) COLLATE UNICODE_SV UNIQUE)
           CREATE TABLE G (NAME VARCHAR(20))",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO S VALUES ('ödla'), ('zebra'), ('ärta'), ('åka'), ('apa')").unwrap();
    a.run("INSERT INTO G VALUES ('Mueller'), ('Müller'), ('Muff'), ('Mull')").unwrap();
    a.commit().unwrap();
    db
}

fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3.6, §13.1] RULES.md D60: a
/// column of a language's collation orders as the language does, Swedish
/// å, ä and ö after z; COLLATE names another for an ORDER BY, German
/// phonebook ü as ue.
#[test]
fn a_language_orders_its_own_letters_4_3_6() {
    let db = database();
    let a = db.session("A");
    assert_eq!(query(&a, "SELECT WORD FROM S ORDER BY WORD"), ["apa", "zebra", "åka", "ärta", "ödla"]);
    assert_eq!(query(&a, "SELECT WORD FROM S ORDER BY WORD COLLATE UNICODE"), ["åka", "apa", "ärta", "ödla", "zebra"]);
    assert_eq!(query(&a, "SELECT NAME FROM G ORDER BY NAME COLLATE UNICODE_DE_PHONEBOOK"), ["Mueller", "Müller", "Muff", "Mull"]);
    assert_eq!(query(&a, "SELECT NAME FROM G ORDER BY NAME COLLATE UNICODE"), ["Mueller", "Muff", "Mull", "Müller"]);
}

/// Verifies: [EXTENSION] RULES.md D60: a comparison and UNIQUE are by the
/// column's collation, its index holding its sort keys: a range finds the
/// letters after z, and a word equal to one held is refused.
#[test]
fn comparisons_and_unique_follow_the_language_4_3_6() {
    let db = database();
    let a = db.session("A");
    assert_eq!(query(&a, "SELECT WORD FROM S WHERE WORD > 'zz' ORDER BY WORD"), ["åka", "ärta", "ödla"]);
    assert_eq!(query(&a, "SELECT WORD FROM S WHERE WORD = 'a\u{30A}ka'"), ["åka"]);
    assert_eq!(a.run("INSERT INTO S VALUES ('a\u{30A}ka')").err().map(|e| e.code), Some(Code::Constraint));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.5, §11.43] RULES.md D60:
/// LIKE under a language's collation is refused, as under UNICODE; CREATE
/// COLLATION gives one another name; the Information Schema shows each,
/// NO PAD, its USAGE PUBLIC's.
#[test]
fn a_language_collation_is_a_built_in_4_3_6() {
    let db = database();
    let a = db.session("A");
    assert_eq!(a.open_cursor("SELECT 1 FROM S WHERE WORD LIKE 'a%'").err().map(|e| e.code), Some(Code::Unsupported));
    a.run("CREATE COLLATION SWEDISH FOR SQL_TEXT FROM UNICODE_SV PAD SPACE").unwrap();
    assert_eq!(query(&a, "SELECT WORD FROM S WHERE WORD COLLATE A.SWEDISH = 'åka   '"), ["åka"]);
    let seen = query(&a, "SELECT PAD_ATTRIBUTE FROM INFORMATION_SCHEMA.COLLATIONS WHERE COLLATION_NAME IN ('UNICODE_SV', 'UNICODE_FR_CA')");
    assert_eq!(seen, ["NO PAD", "NO PAD"]);
    let usage = "SELECT GRANTEE, IS_GRANTABLE FROM INFORMATION_SCHEMA.USAGE_PRIVILEGES WHERE OBJECT_NAME = 'UNICODE_SV'";
    assert_eq!(query(&a, usage), ["PUBLIC,NO"]);
    assert_eq!(a.run("CREATE TABLE X (W VARCHAR(9) COLLATE UNICODE_XX)").err().map(|e| e.code), Some(Code::SyntaxRule));
}
