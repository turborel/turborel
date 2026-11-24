//! The SQL dynamic statements of a polymorphic table function's component
//! procedures [ISO/IEC 9075-2:2023 §20.6, §20.13, §20.28, §5.4 SR11]
//! (Features B207, B209): FETCH from a PTF cursor into a PTF descriptor
//! area, COPY DESCRIPTOR of a whole area and of an item's DATA or NAME,
//! and each refused as its rules say.

use turborel::db::{Database, Session};
use turborel::error::SqlError;

/// Schema A: T (K, N), rows (1, 10), (2, 20), (3, 30).
fn defined() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10), (2, 20), (3, 30)").unwrap();
    a.commit().unwrap();
    db
}

/// The rows of `name` (T TABLE WITH ROW SEMANTICS) RETURNS TABLE (R
/// INTEGER), of T, its fulfill component procedure's body `body`, which
/// may use the integers K, N and V, the character string X and the flag
/// DONE, set as a fetch finds no row; or the SQLSTATE its definition or
/// its rows fail with.
fn fulfilled(a: &Session, name: &str, body: &str) -> Result<Vec<String>, String> {
    a.run(&format!(
        "CREATE PROCEDURE {name}_F (IN CR VARCHAR(128), IN CU VARCHAR(128), IN RR VARCHAR(128), INOUT ST CHAR(5)) CONTAINS SQL \
         BEGIN DECLARE K, N, V INTEGER; DECLARE X VARCHAR(128); DECLARE DONE BOOLEAN DEFAULT FALSE; \
         DECLARE CONTINUE HANDLER FOR NOT FOUND SET DONE = TRUE; {body} END"
    ))
    .map_err(|e| e.sqlstate().as_str().to_owned())?;
    a.run(&format!(
        "CREATE FUNCTION {name} (T TABLE WITH ROW SEMANTICS) RETURNS TABLE (R INTEGER) CONTAINS SQL FULFILL WITH PROCEDURE {name}_F"
    ))
    .unwrap();
    let run = || -> Result<Vec<String>, SqlError> {
        let mut c = a.open_cursor(&format!("SELECT P.R FROM TABLE ({name} (TABLE (T))) AS P ORDER BY 1"))?;
        let mut out = Vec::new();
        while let Some(row) = c.fetch()? {
            out.push(row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
        }
        Ok(out)
    };
    run().map_err(|e| e.sqlstate().as_str().to_owned())
}

/// Verifies: [X3.135-1986 §5.20] [ISO/IEC 9075-2:2023 §20.13 GR3, §20.6
/// GR6a, GR7] a fulfill component procedure fetches each row of its PTF
/// cursor into the cursor row type descriptor area and reads it there;
/// copies an item's DATA to the intermediate result row type descriptor
/// area and pipes it; copies an item's NAME, the data staying null; and
/// refuses a whole area copied to one of fewer items than it holds
/// (07009).
#[test]
fn fetched_into_and_copied_between_descriptor_areas_20_13() {
    let db = defined();
    let a = db.session("A");
    let read = "L: LOOP FETCH NEXT FROM PTF CU INTO SQL DESCRIPTOR PTF CR; IF DONE THEN LEAVE L; END IF; \
                GET DESCRIPTOR PTF CR VALUE 2 V = DATA; SET V = V + 1; SET DESCRIPTOR PTF RR VALUE 1 DATA = V; PIPE ROW PTF RR; END LOOP L;";
    assert_eq!(fulfilled(&a, "READING", read), Ok(vec!["11".to_owned(), "21".to_owned(), "31".to_owned()]));
    let data = "FETCH NEXT FROM PTF CU INTO SQL DESCRIPTOR PTF CR; COPY PTF CR VALUE 1 (DATA) TO PTF RR VALUE 1; PIPE ROW PTF RR;";
    assert_eq!(fulfilled(&a, "DATA_ONE", data), Ok(vec!["1".to_owned()]));
    let named = "COPY PTF CR VALUE 2 (NAME) TO PTF RR VALUE 1; GET DESCRIPTOR PTF RR VALUE 1 X = NAME; \
                 IF X = 'N' THEN SET V = 7; END IF; SET DESCRIPTOR PTF RR VALUE 1 DATA = V; PIPE ROW PTF RR;";
    assert_eq!(fulfilled(&a, "NAMED", named), Ok(vec!["7".to_owned()]));
    let whole = "COPY PTF CR TO PTF RR;";
    assert_eq!(fulfilled(&a, "WHOLE", whole), Err("07009".to_owned()), "two items to an area of one");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §20.13 SR2, GR3, GR4, §5.4
/// SR11, §20.6 GR7b, GR7e] in a component procedure, each refused: FETCH
/// other than NEXT, as a PTF cursor is NO SCROLL (SQLCODE -103, 42000);
/// a name of no PTF cursor (34000) or descriptor area (33000), a null name
/// (34000) and a name not a character string (SQLCODE -103); targets fewer
/// than the cursor's columns (07002) and a descriptor area not describing them
/// (07008); a copied item of no item of the source, or to no item of the
/// target (07009).
#[test]
fn component_statements_refused_20_13() {
    let db = defined();
    let a = db.session("A");
    let refused = |name: &str, body: &str| fulfilled(&a, name, body).unwrap_err();
    assert_eq!(refused("BACKWARD", "FETCH PRIOR FROM PTF CU INTO K, N;"), "42000");
    assert_eq!(refused("NO_CURSOR", "FETCH NEXT FROM PTF 'NOPE' INTO K, N;"), "34000");
    assert_eq!(refused("NO_AREA", "COPY PTF 'NOPE' TO PTF RR;"), "33000");
    assert_eq!(refused("NULL_NAME", "FETCH NEXT FROM PTF X INTO K, N;"), "34000");
    assert_eq!(refused("NUMBER_NAME", "SET V = 1; FETCH NEXT FROM PTF V INTO K, N;"), "42000");
    assert_eq!(refused("FEWER", "FETCH NEXT FROM PTF CU INTO K;"), "07002");
    assert_eq!(refused("UNDESCRIBED", "FETCH NEXT FROM PTF CU INTO SQL DESCRIPTOR PTF RR;"), "07008");
    assert_eq!(refused("NO_ITEM", "COPY PTF CR VALUE 9 (DATA) TO PTF RR VALUE 1;"), "07009");
    assert_eq!(refused("NO_TARGET", "COPY PTF CR VALUE 1 (DATA) TO PTF RR VALUE 0;"), "07009");
}

/// Verifies: [ISO/IEC 9075-2:2023 §20.6 GR7h iii 2 A, §20.1 Table 29]
/// (RULES.md D1042) COPY DESCRIPTOR with TYPE copies the SCOPE components
/// of an item that describes a reference type with its other type
/// components, and each is read as the character string it is. The
/// scope's name of a column REF(PERSON) SCOPE PEOPLE, copied to an item
/// of another area, is PEOPLE there, with its referenced type's name.
#[test]
fn a_reference_types_scope_is_copied_with_its_type_20_6() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    for sql in [
        "CREATE TYPE PERSON AS (NAME CHARACTER VARYING(20)) NOT FINAL REF IS SYSTEM GENERATED",
        "CREATE TABLE PEOPLE OF PERSON (REF IS ID SYSTEM GENERATED)",
        "CREATE TABLE T (K REF(PERSON) SCOPE PEOPLE, N INTEGER)",
        "INSERT INTO PEOPLE (NAME) VALUES ('ANN')",
        "INSERT INTO T SELECT ID, 1 FROM PEOPLE",
    ] {
        a.run(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    a.commit().unwrap();
    let said = |name: &str, field: &str, value: &str| {
        let body = format!(
            "COPY PTF CR VALUE 1 (TYPE) TO PTF RR VALUE 1; GET DESCRIPTOR PTF RR VALUE 1 X = {field}; \
             IF X = '{value}' THEN SIGNAL SQLSTATE '75001'; END IF; SIGNAL SQLSTATE '75002';"
        );
        fulfilled(&a, name, &body).unwrap_err()
    };
    assert_eq!(said("TYPE_NAME", "USER_DEFINED_TYPE_NAME", "PERSON"), "75001");
    assert_eq!(said("SCOPED", "SCOPE_NAME", "PEOPLE"), "75001");
    assert_eq!(said("SCOPED_IN", "SCOPE_SCHEMA", "A"), "75001");
}
