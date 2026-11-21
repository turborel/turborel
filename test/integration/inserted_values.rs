//! \[DESIGN\] The rows of an INSERT's VALUES, and a CALL's arguments,
//! evaluated in turn where no value reads a table or invokes a routine,
//! not planned as a query (RULES.md D419).

use turborel::db::Database;
use turborel::error::Code;

use crate::common::{exec, sorted};

/// Verifies: [X3.135-1986 §8.7 GR1]; [ISO/IEC 9075:1992 §13.8, §6.12
/// GR4]; [DESIGN] RULES.md D419: rows of values computed of constants,
/// of a procedure's variables and of its parameters are inserted as
/// written, cast to their columns' types; a value that raises an error
/// refuses the statement, no row of it inserted; a row whose value reads
/// a table is inserted as before.
#[test]
fn rows_of_values_are_inserted_as_computed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION V CREATE TABLE T (K INTEGER, D DECIMAL(6,2), S VARCHAR(8)) CREATE TABLE U (N INTEGER)").unwrap();
    let s = db.session("V");
    s.run("INSERT INTO U VALUES (7)").unwrap();
    s.run("INSERT INTO T VALUES (1, 2.5 * 3, 'a' || 'b'), (2, NULL, CAST(12 AS VARCHAR(8)))").unwrap();
    assert_eq!(exec(&s, "INSERT INTO T VALUES (3, 1, 'x'), (4, 1 / 0, 'y')"), Err(Code::Data), "a division by zero");
    s.run("INSERT INTO T VALUES (5, (SELECT N FROM U), 'q')").unwrap();
    s.run(
        "CREATE PROCEDURE PUT (IN K INTEGER, IN F DECIMAL(4,1)) MODIFIES SQL DATA
         BEGIN
           DECLARE TWICE DECIMAL(6,2);
           SET TWICE = F * 2;
           INSERT INTO T VALUES (K, TWICE + K, 'p' || CAST(K AS VARCHAR(3)));
         END",
    )
    .unwrap();
    for (k, f) in [(10, "1.5"), (11, "2.0"), (12, "0.5")] {
        s.run(&format!("CALL PUT({k} + 0, {f})")).unwrap();
    }
    assert_eq!(exec(&s, "CALL PUT(13, 1 / 0)"), Err(Code::Data), "an argument raising");
    let all = vec!["1,7.50,ab", "10,13.00,p10", "11,15.00,p11", "12,13.00,p12", "2,NULL,12", "5,7.00,q"];
    assert_eq!(sorted(&s, "SELECT K, D, S FROM T"), all);
}
