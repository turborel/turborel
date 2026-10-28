//! Schema definition, privileges, INSERT and searched UPDATE/DELETE
//! [X3.135-1986 §6, §8.5, §8.7, §8.12].

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use turborel::catalog::{Catalog, ColumnDesc, ConstraintDesc, ConstraintKind, IndexDesc, MemoryCatalog, QualifiedName, TableDesc};
use turborel::db::{Completion, Database, Session};

use turborel::error::{self, Code};
use turborel::index::{BTreeMethod, Index, IndexMethod, Key, Probe};
use turborel::storage::RowId;
use turborel::value::{CompOp, DOUBLE_PRECISION, DataType, MAX_CHARACTER_LENGTH, MAX_EXACT_PRECISION};

use crate::common::{code, count, exec, rows, setup};
use turborel::value::Spelled;

/// The error code of `sql`, an INSERT that must fail.
fn insert_error(s: &mut Session, sql: &str) -> Code {
    s.run(sql).expect_err(sql).code
}

/// Verifies: [X3.135-1986 §4.5] INSERT enforces UNIQUE, including a
/// multi-column one, and NOT NULL.
#[test]
fn insert_enforces_unique_and_not_null_4_5() {
    let db = setup();
    let mut s = db.session("APP");
    assert_eq!(insert_error(&mut s, "INSERT INTO T VALUES (1, 'X', 1, 1)"), Code::Constraint); // UNIQUE
    assert_eq!(insert_error(&mut s, "INSERT INTO T (NAME) VALUES ('X')"), Code::Constraint); // NOT NULL
    assert_eq!(insert_error(&mut s, "INSERT INTO U VALUES (1, 'A  ')"), Code::Constraint); // 'A' = 'A  '
    s.run("INSERT INTO U VALUES (1, 'C')").unwrap(); // multi-column UNIQUE
}

/// The built-in ordered index method, counting the UNIQUE checks made
/// through its indexes (`Index::rows`).
struct Counting {
    inner: BTreeMethod,
    checks: Arc<AtomicUsize>,
}

/// An index of `Counting`.
struct CountingIndex {
    inner: Box<dyn Index>,
    checks: Arc<AtomicUsize>,
}

impl IndexMethod for Counting {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn supports(&self, op: CompOp) -> bool {
        self.inner.supports(op)
    }
    fn ordered(&self) -> bool {
        self.inner.ordered()
    }
    fn create(&self, columns: &[DataType]) -> error::Result<Box<dyn Index>> {
        Ok(Box::new(CountingIndex { inner: self.inner.create(columns)?, checks: Arc::clone(&self.checks) }))
    }
}

impl Index for CountingIndex {
    fn insert(&self, key: Key, row: RowId) -> error::Result<()> {
        self.inner.insert(key, row)
    }
    fn remove(&self, key: &Key, row: RowId) -> error::Result<()> {
        self.inner.remove(key, row)
    }
    fn rows(&self, key: &Key) -> error::Result<Vec<RowId>> {
        self.checks.fetch_add(1, Ordering::Relaxed);
        self.inner.rows(key)
    }
    fn estimate(&self, probe: &Probe) -> Option<usize> {
        self.inner.estimate(probe)
    }
    fn search(&self, probe: &Probe) -> error::Result<Vec<(Key, RowId)>> {
        self.inner.search(probe)
    }
}

/// Verifies: [X3.135-1986 §4.5, §6.4 GR2, §8.12 GR6] UPDATE enforces
/// UNIQUE after the statement: keys may pass through duplicates (SET K =
/// K + 1) but not end with them, a constraint of two columns checked when
/// either is assigned; [DESIGN] an UPDATE that assigns no column of a
/// constraint does not check it, as it leaves every key as it was.
#[test]
fn update_checks_the_unique_constraints_it_could_break_8_12() {
    let checks = Arc::new(AtomicUsize::new(0));
    let db = Database::new();
    db.register_index_method(Box::new(Counting { inner: BTreeMethod::in_memory(), checks: Arc::clone(&checks) })).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION APP
           CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V INTEGER)
           CREATE TABLE U (A INTEGER NOT NULL, B INTEGER NOT NULL, V INTEGER, UNIQUE (A, B))",
    )
    .unwrap();
    let s = db.session("APP");
    for k in 1..=3 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 0)")).unwrap();
        s.run(&format!("INSERT INTO U VALUES ({k}, {k}, 0)")).unwrap();
    }
    let made = |sql: &str| {
        let before = checks.load(Ordering::Relaxed);
        let outcome = exec(&s, sql).map(|_| ());
        (outcome, checks.load(Ordering::Relaxed) - before)
    };
    assert_eq!(made("UPDATE T SET V = V + 1"), (Ok(()), 0), "no key assigned");
    assert_eq!(made("UPDATE U SET V = 7 WHERE A = 2"), (Ok(()), 0));
    assert_eq!(made("UPDATE T SET K = K + 1").0, Ok(()), "through duplicates, ending with none");
    assert!(made("UPDATE T SET K = K + 1").1 >= 3, "each row's key checked");
    assert_eq!(made("UPDATE T SET K = 9").0, Err(Code::Constraint));
    let (outcome, made_checks) = made("UPDATE U SET A = 1 WHERE B = 2");
    assert!(outcome.is_ok() && made_checks >= 1, "(2, 2) to (1, 2): one column assigned, checked, no duplicate");
    assert_eq!(made("UPDATE U SET B = 1 WHERE A = 1 AND B = 2").0, Err(Code::Constraint), "(1, 2) to (1, 1), a duplicate");
    assert_eq!(rows(&s, "SELECT K FROM T ORDER BY K"), Ok(vec!["3".into(), "4".into(), "5".into()]));
}

/// Verifies: [X3.135-1986 §8.7 SR6, GR7a, GR7c] an inserted value must fit its
/// column's type, length and precision; values exactly at the limits are
/// stored. [ISO/IEC 9075:1992 §9.2 GR3b, GR3k] A string too long and an
/// approximate number into an exact column are checked as they are stored.
#[test]
fn insert_values_must_fit_their_columns_8_7() {
    let db = setup();
    let mut s = db.session("APP");
    assert_eq!(insert_error(&mut s, "INSERT INTO T VALUES (9, 'TOOLONG', 1, 1)"), Code::Data); // §9.2 GR3b ii
    assert_eq!(insert_error(&mut s, "INSERT INTO T VALUES (9, 1, 1, 1)"), Code::SyntaxRule); // §9.2 SR2
    assert_eq!(insert_error(&mut s, "INSERT INTO T VALUES (9, 'X', 10000.00, 1)"), Code::Data); // GR7c
    // The accepting side of the limits above.
    s.run("INSERT INTO T VALUES (90, 'SIXSIX', 1, 1)").unwrap(); // exactly the column's length [GR7a]
    s.run("INSERT INTO T VALUES (91, 'X', 9999.99, 1)").unwrap(); // the largest DECIMAL(6,2) [GR7c]
    assert_eq!(
        rows(&s, "SELECT ID, NAME, AMT FROM T WHERE ID > 89 ORDER BY 1"),
        Ok(vec!["90,SIXSIX,1.00".into(), "91,X     ,9999.99".into()])
    );
}

/// Verifies: [X3.135-1986 §8.7 SR3, SR5a] the column list names each column once,
/// and the values match it in number.
#[test]
fn insert_values_must_match_the_column_list_8_7() {
    let db = setup();
    let mut s = db.session("APP");
    assert_eq!(insert_error(&mut s, "INSERT INTO T VALUES (9, 'X', 1)"), Code::SyntaxRule); // SR5a
    assert_eq!(insert_error(&mut s, "INSERT INTO T (ID, ID) VALUES (9, 9)"), Code::SyntaxRule); // SR3
}

/// Verifies: [X3.135-1986 §8.7 GR4, GR5] INSERT with a query inserts one
/// row for each row of its result, the i-th value into the i-th object
/// column, the others null; an empty result inserts nothing and is SQLCODE
/// 100; its degree matches the column list [SR5b] and its types the columns
/// [SR6]; the query may read the target, SR2 being Intermediate SQL's
/// [ISO/IEC 9075:1992 §13.8, Leveling Rules]; a key duplicated among, or with, the rows is a
/// constraint violation and inserts none of them [§7.3 GR3b].
#[test]
fn insert_from_a_query_8_7() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(exec(&s, "INSERT INTO U (CODE, ID) SELECT 'Q', ID + 10 FROM T WHERE NAME IS NOT NULL"), Ok(Completion::Success));
    assert_eq!(rows(&s, "SELECT ID, CODE FROM U WHERE CODE = 'Q' ORDER BY ID"), Ok(vec!["11,Q  ".into(), "12,Q  ".into()]));
    assert_eq!(exec(&s, "INSERT INTO U SELECT ID, 'E' FROM T WHERE ID > 99"), Ok(Completion::NoData), "GR4");
    exec(&s, "INSERT INTO T (ID, AMT) SELECT ID + 100, COUNT(*) FROM U GROUP BY ID").unwrap();
    assert_eq!(
        rows(&s, "SELECT ID, NAME, AMT FROM T WHERE ID > 100 ORDER BY ID"),
        Ok(vec!["101,NULL,2.00".into(), "111,NULL,1.00".into(), "112,NULL,1.00".into(),])
    );
    assert_eq!(exec(&s, "INSERT INTO U SELECT ID FROM T"), Err(Code::SyntaxRule), "SR5b");
    assert_eq!(exec(&s, "INSERT INTO U (ID) SELECT ID, NAME FROM T"), Err(Code::SyntaxRule), "SR5b");
    assert_eq!(exec(&s, "INSERT INTO U SELECT NAME, NAME FROM T"), Err(Code::SyntaxRule), "SR6b");
    assert_eq!(exec(&s, "INSERT INTO U SELECT ID, NAME FROM T"), Err(Code::Constraint), "'ANN   ' fits CHAR(3) [§9.2 GR3b i], NULL not");
    // SR2, that the query not read U, is Intermediate SQL's [ISO/IEC
    // 9075:1992 §13.8, Leveling Rules]: it is evaluated first [GR3].
    let nested = "INSERT INTO U SELECT ID, 'X' FROM T WHERE EXISTS (SELECT * FROM U WHERE U.ID = T.ID)";
    assert_eq!(exec(&s, nested), Ok(Completion::Success), "SR2");
    assert_eq!(rows(&s, "SELECT ID FROM U WHERE CODE = 'X' ORDER BY ID"), Ok(vec!["1".into()]));
    let before = rows(&s, "SELECT ID FROM U").unwrap().len();
    assert_eq!(exec(&s, "INSERT INTO U SELECT 7, 'D' FROM T"), Err(Code::Constraint), "three rows (7, 'D')");
    assert_eq!(exec(&s, "INSERT INTO U SELECT 1, 'A' FROM T WHERE ID = 1"), Err(Code::Constraint), "(1, 'A') is there");
    assert_eq!(rows(&s, "SELECT ID FROM U").unwrap().len(), before, "no row inserted");
    assert_eq!(exec(&s, "INSERT INTO T (NAME) SELECT CODE FROM U"), Err(Code::Constraint));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §13.8, §7.1] an inserted value
/// is a `<value expression>`: SQL-86's `<insert value>`, a `<literal>`, so
/// that "- 9" is none [X3.135-1986 §8.7], is Entry SQL's alone [§13.8,
/// Leveling Rules].
#[test]
fn insert_values_are_value_expressions_13_8() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(exec(&s, "INSERT INTO T VALUES (- 9, 'X' || 'Y', 1 + 2, (SELECT COUNT(*) FROM U))"), Ok(Completion::Success));
    assert_eq!(rows(&s, "SELECT NAME, AMT, F FROM T WHERE ID = -9"), Ok(vec!["XY    ,3.00,2E0".into()]));
}

/// Verifies: [X3.135-1986 §8.7 GR7c] an inserted exact value keeps its leading
/// digits and loses extra fraction digits; USER may be inserted.
#[test]
fn insert_keeps_leading_digits_8_7() {
    let db = setup();
    let s = db.session("APP");
    s.run("INSERT INTO T VALUES (-9, USER, 1.239, 7)").unwrap(); // [X3.135-1986 §8.7 GR7c] keeps leading digits
    assert_eq!(rows(&s, "SELECT NAME, AMT FROM T WHERE ID = -9"), Ok(vec!["APP   ,1.23".into()]));
}

/// Verifies: [X3.135-1986 §3.3] a failed INSERT has no effect.
#[test]
fn a_failed_insert_has_no_effect_3_3() {
    let db = setup();
    let mut s = db.session("APP");
    for sql in [
        "INSERT INTO T VALUES (1, 'X', 1, 1)",
        "INSERT INTO T (NAME) VALUES ('X')",
        "INSERT INTO T VALUES (9, 'TOOLONG', 1, 1)",
        "INSERT INTO T VALUES (9, 'X', 1.5E7, 1)",
        "INSERT INTO T VALUES (9, 'X', 1)",
        "INSERT INTO T (ID, ID) VALUES (9, 9)",
        "INSERT INTO T VALUES (9, 'X', 10000.00, 1)",
        "INSERT INTO T VALUES (9, 'X', 1, 1 / 0)",
    ] {
        insert_error(&mut s, sql);
    }
    s.run("INSERT INTO T VALUES (-9, USER, 1.239, 7)").unwrap();
    // A failed statement has no effect [X3.135-1986 §3.3].
    assert_eq!(rows(&s, "SELECT ID FROM T").map(|r| r.len()), Ok(4));
}

/// The error code of `sql`, a `<schema>` that must fail.
fn schema_error(db: &mut Database, sql: &str) -> Code {
    db.schema(sql).expect_err(sql).code
}

/// Verifies: [X3.135-1986 §6.1 SR1] a schema's authorization identifier must not already have one.
#[test]
fn a_schema_authorization_must_be_new_6_1() {
    let mut db = setup();
    assert_eq!(schema_error(&mut db, "CREATE SCHEMA AUTHORIZATION APP"), Code::SyntaxRule); // [X3.135-1986 §6.1 SR1]
}

/// Verifies: [X3.135-1986 §6.2 SR1] a table is defined in its own schema only.
#[test]
fn a_table_belongs_to_its_own_schema_6_2() {
    let mut db = setup();
    assert_eq!(schema_error(&mut db, "CREATE SCHEMA AUTHORIZATION B CREATE TABLE X.T (A INT)"), Code::SyntaxRule); // [X3.135-1986 §6.2 SR1]
}

/// Verifies: [X3.135-1986 §6.2 SR2] the tables of a schema have different names.
#[test]
fn table_names_in_a_schema_must_differ_6_2() {
    let mut db = setup();
    assert_eq!(schema_error(&mut db, "CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (A INT) CREATE TABLE T (C INT)"), Code::SyntaxRule); // [X3.135-1986 §6.2 SR2]
}

/// Verifies: [X3.135-1986 §6.4 SR2] a UNIQUE constraint names distinct columns of its table.
#[test]
fn unique_names_distinct_columns_of_the_table_6_4() {
    let mut db = setup();
    assert_eq!(schema_error(&mut db, "CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (A INT NOT NULL, UNIQUE (NOPE))"), Code::SyntaxRule); // [X3.135-1986 §6.4 SR2]
    assert_eq!(schema_error(&mut db, "CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (A INT NOT NULL, UNIQUE (A, A))"), Code::SyntaxRule); // [X3.135-1986 §6.4 SR2]
}

/// Verifies: [X3.135-1986 §6.3 SR1] the columns of a table have different names.
#[test]
fn column_names_in_a_table_must_differ_6_3() {
    let mut db = setup();
    assert_eq!(schema_error(&mut db, "CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (A INT, A INT)"), Code::SyntaxRule); // [X3.135-1986 §6.3 SR1]
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.7 GR2, §8.9], which takes
/// priority over X3.135-1986 §6.4 SR3: a UNIQUE column may be null, in a
/// table constraint or a column definition, and rows null there do not
/// conflict; two rows of one value do.
#[test]
fn unique_columns_may_be_null_11_7() {
    let db = setup();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (A INT, UNIQUE (A)) CREATE TABLE U (A INT UNIQUE)").unwrap();
    let s = db.session("B");
    for table in ["T", "U"] {
        s.run(&format!("INSERT INTO {table} VALUES (NULL)")).unwrap();
        s.run(&format!("INSERT INTO {table} VALUES (NULL)")).unwrap();
        s.run(&format!("INSERT INTO {table} VALUES (1)")).unwrap();
        assert_eq!(exec(&s, &format!("INSERT INTO {table} VALUES (1)")).err(), Some(Code::Constraint), "{table}");
    }
}

/// Verifies: [X3.135-1986 §5.5 SR2, SR4] a length is at least 1 and a scale is
/// not greater than its precision.
#[test]
fn data_type_lengths_and_scales_must_be_valid_5_5() {
    let mut db = setup();
    assert_eq!(schema_error(&mut db, "CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (A DECIMAL(3,5))"), Code::SyntaxRule); // [X3.135-1986 §5.5 SR4]
    assert_eq!(schema_error(&mut db, "CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (A CHAR(0))"), Code::SyntaxRule); // [X3.135-1986 §5.5 SR2]
}

/// Verifies: [X3.135-1986 §3.3] a failed `<schema>` has no effect, so its
/// authorization can be used again; [X3.135-1986 §5.5] every data type can then
/// be declared, stored and read.
#[test]
fn a_failed_schema_has_no_effect_6_1() {
    let mut db = setup();
    for sql in [
        "CREATE SCHEMA AUTHORIZATION B CREATE TABLE X.T (A INT)",
        "CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (A INT) CREATE TABLE T (C INT)",
        "CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (A INT NOT NULL, UNIQUE (NOPE))",
        "CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (A INT NOT NULL, UNIQUE (A, A))",
        "CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (A INT, A INT)",
        "CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (A DECIMAL(3,5))",
        "CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (A CHAR(0))",
        "CREATE SCHEMA AUTHORIZATION B CREATE VIEW V",
    ] {
        schema_error(&mut db, sql);
    }
    // A failed <schema> has no effect, so B can still be created, with each data type working.
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (A DOUBLE PRECISION, B FLOAT(10), C SMALLINT, D DEC(5,1), E INT)").unwrap();
    let b = db.session("B");
    b.run("INSERT INTO T VALUES (1.5E0, 2.5E0, 7, 1234.5, 8)").unwrap();
    assert_eq!(rows(&b, "SELECT A, B, C, D, E FROM T"), Ok(vec!["1.5E0,2.5E0,7,1234.5,8".into()]));
}

/// Verifies: [X3.135-1986 §6.4 GR2] each table's UNIQUE constraint concerns
/// only its own rows; [DESIGN] a constraint index name already taken through
/// `create_index` fails the `<schema>` before anything is created.
///
/// Regression test: `A_B.C` and `A.B_C`
/// were given the same index, so inserting K = 1 into both failed.
#[test]
fn unique_constraints_of_different_tables_never_share_an_index() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A_B CREATE TABLE C (K INTEGER NOT NULL UNIQUE)").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE B_C (K INTEGER NOT NULL UNIQUE)").unwrap();
    assert_eq!(exec(&db.session("A_B"), "INSERT INTO C VALUES (1)"), Ok(Completion::Success));
    assert_eq!(exec(&db.session("A"), "INSERT INTO B_C VALUES (1)"), Ok(Completion::Success));

    db.create_index("Z.T.UNIQUE1", "A.B_C", &["K"], "hash").unwrap();
    let taken = db.schema("CREATE SCHEMA AUTHORIZATION Z CREATE TABLE T (K INTEGER NOT NULL UNIQUE)");
    assert_eq!(taken.map_err(|e| e.code), Err(Code::Api));
    assert_eq!(code(&db.session("Z"), "SELECT K FROM T"), Code::SyntaxRule); // no table was created
}

/// Verifies: [X3.135-1986 §5.5 SR2, §8.7 GR7b]; [DESIGN] `MAX_CHARACTER_LENGTH`:
/// a CHARACTER column of the maximum length stores values padded to it, and a
/// longer one gets SQLCODE -900 before anything is allocated.
///
/// Regression test: CHARACTER(4000000000)
/// was accepted, and storing one value padded it to that length.
#[test]
fn character_length_is_bounded() {
    let db = Database::new();
    let longest = MAX_CHARACTER_LENGTH;
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (C CHARACTER({longest}))")).unwrap();
    let s = db.session("A");
    s.run("INSERT INTO T VALUES ('X')").unwrap();
    let stored = rows(&s, "SELECT C FROM T").unwrap();
    assert_eq!(stored.iter().map(|v| v.chars().count()).collect::<Vec<_>>(), vec![usize::try_from(longest).unwrap()]);
    for length in [u64::from(longest) + 1, 4_000_000_000] {
        let sql = format!("CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (C CHAR({length}))");
        assert_eq!(db.schema(&sql).map_err(|e| e.code), Err(Code::Unsupported), "{length}");
    }
}

/// Verifies: [X3.135-1986 §5.5 SR6, SR7, SR10]; [DESIGN] the standard sets no
/// maximum precision, so a type beyond this implementation's maximum is valid
/// SQL-86 it does not process: SQLCODE -900, while the maximum itself is accepted.
#[test]
fn data_type_limits_are_not_syntax_errors() {
    let db = Database::new();
    let create =
        |n: usize, column: String| db.schema(&format!("CREATE SCHEMA AUTHORIZATION S{n} CREATE TABLE T (C {column})")).map_err(|e| e.code);
    let exact = MAX_EXACT_PRECISION;
    let float = DOUBLE_PRECISION;
    let accepted = [format!("DECIMAL({exact},{exact})"), format!("NUMERIC({exact})"), format!("FLOAT({float})")];
    let beyond = [
        format!("DECIMAL({})", exact + 1),
        format!("NUMERIC({})", exact + 1),
        format!("FLOAT({})", float + 1),
        "CHAR(99999999999)".to_owned(),
    ];
    for (n, column) in accepted.into_iter().enumerate() {
        assert_eq!(create(n, column.clone()), Ok(Completion::Success), "{column}");
    }
    for (n, column) in beyond.into_iter().enumerate() {
        assert_eq!(create(n + 10, column.clone()), Err(Code::Unsupported), "{column}");
    }
}

/// A catalog holding table A.T (K NOT NULL UNIQUE) with its constraint index,
/// kept in the engine `engine`.
fn catalog_with_table(engine: &str) -> MemoryCatalog {
    let name = QualifiedName { auth: "A".into(), ident: "T".into() };
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Numeric };
    let column = ColumnDesc { name: "K".into(), data_type: integer, not_null: true, default: None, domain: None };
    let constraint = ConstraintDesc {
        name: QualifiedName { auth: "A".into(), ident: "T_UNIQUE1".into() },
        kind: ConstraintKind::Unique { columns: vec![0], primary: false },
        attributes: turborel::catalog::ConstraintAttributes::default(),
        domain: None,
    };
    let (columns, constraints, engine) = (vec![column], vec![constraint], engine.into());
    let table = TableDesc { name: name.clone(), columns, constraints, engine, options: Vec::new(), generation: 0, temporary: None };
    let mut catalog = MemoryCatalog::default();
    catalog.create_schema("A", "A", vec![table]);
    let (method, options) = ("btree".into(), Vec::new());
    catalog.add_index(IndexDesc { name: "A.T.UNIQUE1".into(), table: name, columns: vec![0], method, unique: true, options });
    catalog
}

/// Verifies: [DESIGN] a `Database` over a catalog that already holds tables and
/// indexes gives each its storage and index structure; one naming an engine
/// that is not built in is refused with SQLCODE -910.
///
/// Regression test: the first statement
/// on such a table panicked, because the table had no storage.
#[test]
fn a_database_over_an_existing_catalog_can_use_its_tables() {
    let db = Database::with_catalog(Box::new(catalog_with_table("rowstore"))).unwrap();
    let s = db.session("A");
    assert_eq!(rows(&s, "SELECT K FROM T"), Ok(vec![]));
    s.run("INSERT INTO T VALUES (1)").unwrap();
    assert_eq!(s.run("INSERT INTO T VALUES (1)").map_err(|e| e.code), Err(Code::Constraint)); // the UNIQUE index works
    assert_eq!(Database::with_catalog(Box::new(catalog_with_table("no_such_engine"))).err().map(|e| e.code), Some(Code::Api));
}

/// Verifies: [EXTENSION] `create_index` does not wait for the open
/// transactions that wrote its table (RULES.md D27): such a transaction is
/// refused (SQLCODE -911) at its next statement, and runs again; one that
/// wrote another table goes on; the index holds the rows committed.
#[test]
fn create_index_refuses_earlier_writers_of_its_table() {
    let db = setup();
    let (s, other) = (db.session("APP"), db.session("APP"));
    s.run("INSERT INTO T (ID) VALUES (10)").unwrap();
    other.run("INSERT INTO U VALUES (5, 'X')").unwrap();
    db.create_index("T_ID2", "APP.T", &["ID"], "btree").unwrap();
    assert_eq!(exec(&s, "INSERT INTO T (ID) VALUES (11)").map(drop), Err(Code::Conflict));
    s.rollback().unwrap();
    s.run("INSERT INTO T (ID) VALUES (10)").unwrap();
    s.commit().unwrap();
    other.commit().unwrap();
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID = 10"), Ok(vec!["10".into()]), "the index holds the committed row");
}

/// Verifies: [EXTENSION] an index name already in use is refused with
/// SQLCODE -910.
#[test]
fn index_names_are_unique() {
    let db = setup();
    db.create_index("T_ID2", "APP.T", &["ID"], "btree").unwrap();
    assert_eq!(db.create_index("T_ID2", "APP.T", &["NAME"], "hash").map_err(|e| e.code), Err(Code::Api));
}

/// `setup()` plus schema B with table S, owned by B.
fn with_schema_b() -> Database {
    let db = setup();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE S (A INT)").unwrap();
    db
}

/// Verifies: [X3.135-1986 §4.15, §8.5 SR1, §8.7 SR1, §8.12 SR1] with no
/// `<privilege definition>` (§6.6, not implemented), a user who does not own a
/// table may not select, insert, update or delete.
#[test]
fn a_non_owner_has_no_privileges_4_15() {
    let db = with_schema_b();
    let s = db.session("APP");
    assert_eq!(code(&s, "SELECT A FROM B.S"), Code::Privilege);
    assert_eq!(exec(&s, "INSERT INTO B.S VALUES (1)"), Err(Code::Privilege));
    assert_eq!(exec(&s, "UPDATE B.S SET A = 1"), Err(Code::Privilege));
    assert_eq!(exec(&s, "DELETE FROM B.S"), Err(Code::Privilege));
}

/// Verifies: [X3.135-1986 §4.15] the owner of a schema has every privilege on its tables.
#[test]
fn the_owner_has_every_privilege_4_15() {
    let db = with_schema_b();
    assert_eq!(exec(&db.session("B"), "INSERT INTO S VALUES (1)"), Ok(Completion::Success)); // the owner may
}

/// Verifies: [X3.135-1986 §5.4 SR5] DELETE must name an existing table.
#[test]
fn delete_names_an_existing_table_5_4() {
    let db = with_schema_b();
    let s = db.session("APP");
    assert_eq!(exec(&s, "DELETE FROM NOPE"), Err(Code::SyntaxRule)); // [X3.135-1986 §5.4 SR5] no such table
}

/// Verifies: [X3.135-1986 §8.12 GR2] every SET value is computed from the row before it changes.
#[test]
fn update_computes_set_values_from_the_old_row_8_12() {
    let db = setup();
    let s = db.session("APP");
    // [X3.135-1986 §8.12 GR2] every SET value is computed from the row before it changes: a swap.
    exec(&s, "UPDATE T SET AMT = ID, ID = 0 - ID WHERE ID < 3").unwrap();
    assert_eq!(rows(&s, "SELECT ID, AMT FROM T ORDER BY 1"), Ok(vec!["-2,2.00".into(), "-1,1.00".into(), "3,NULL".into()]));
}

/// Verifies: [X3.135-1986 §7.3 GR3a iv] an UPDATE or DELETE with no object rows gives SQLCODE 100.
#[test]
fn update_and_delete_of_no_rows_give_sqlcode_100_7_3() {
    let db = setup();
    let s = db.session("APP");
    // [X3.135-1986 §7.3 GR3a iv] no object rows: SQLCODE 100.
    assert_eq!(exec(&s, "UPDATE T SET AMT = 1 WHERE ID > 99"), Ok(Completion::NoData));
    assert_eq!(exec(&s, "DELETE FROM T WHERE ID > 99"), Ok(Completion::NoData));
    assert_eq!(Completion::NoData.sqlcode(), 100);
}

/// Verifies: [X3.135-1986 §8.12 SR6] a SET value must suit its column; as
/// SQL-92 has it [ISO/IEC 9075:1992 §13.10 SR5, GR8, §9.2]: a number for a
/// number, a string for a string [SR2]; a null for a NOT NULL column
/// violates its constraint [§11.4 SR9a]; a string too long but for spaces
/// is cut to the length [GR3b i], else a data exception [GR3b ii]; an
/// approximate value for an exact column is rounded [GR3k].
#[test]
fn a_set_value_must_suit_its_column_8_12() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(exec(&s, "UPDATE T SET ID = NULL"), Err(Code::Constraint));
    assert_eq!(exec(&s, "UPDATE T SET NAME = 'TOOLONGNAME'"), Err(Code::Data));
    assert_eq!(exec(&s, "UPDATE T SET NAME = 1"), Err(Code::SyntaxRule));
    assert_eq!(exec(&s, "UPDATE T SET AMT = 'A'"), Err(Code::SyntaxRule));
    assert_eq!(exec(&s, "UPDATE T SET NAME = NAME || '   ', AMT = 1.239E0 WHERE ID = 1"), Ok(Completion::Success));
    assert_eq!(rows(&s, "SELECT NAME, AMT FROM T WHERE ID = 1"), Ok(vec!["ANN   ,1.24".into()]));
    assert_eq!(exec(&s, "UPDATE T SET AMT = 1E10"), Err(Code::Data));
}

/// Verifies: [X3.135-1986 §8.12 SR4] a column may be set only once.
#[test]
fn a_column_is_set_at_most_once_8_12() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(exec(&s, "UPDATE T SET AMT = 1, AMT = 2"), Err(Code::SyntaxRule)); // [X3.135-1986 §8.12 SR4]
}

/// Verifies: [X3.135-1986 §8.12 SR3] a SET value has no set function,
/// the refusal saying so.
#[test]
fn a_set_value_has_no_set_function_8_12() {
    let db = setup();
    let s = db.session("APP");
    let refused = s.execute("UPDATE T SET AMT = SUM(ID)", &mut turborel::params::Params::new()).unwrap_err();
    assert_eq!((refused.code, refused.msg.as_str()), (Code::SyntaxRule, "a value of a SET clause contains no set function"));
}

/// Verifies: [X3.135-1986 §5.8 SR1] a searched DELETE's condition has no set function.
#[test]
fn a_delete_condition_has_no_set_function_5_8() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(exec(&s, "DELETE FROM T WHERE COUNT(*) > 1"), Err(Code::SyntaxRule)); // [X3.135-1986 §5.8 SR1]
}

/// Verifies: [X3.135-1986 §8.12 GR6c] an updated value must keep its leading digits.
#[test]
fn an_updated_value_must_keep_its_leading_digits_8_12() {
    let db = setup();
    let s = db.session("APP");
    // [X3.135-1986 §8.12 GR6c] leading digits must fit DECIMAL(6,2).
    assert_eq!(exec(&s, "UPDATE T SET AMT = 12345.6 WHERE ID = 3"), Err(Code::Data));
}

/// Verifies: [X3.135-1986 §8.5 GR1a] a DELETE with no WHERE deletes every row.
#[test]
fn delete_without_where_deletes_every_row_8_5() {
    let db = setup();
    let s = db.session("APP");
    // [X3.135-1986 §8.5 GR1a] no WHERE: every row.
    assert_eq!(exec(&s, "DELETE FROM T"), Ok(Completion::Success));
    assert_eq!(count(&s, "SELECT ID FROM T"), 0);
}

/// Verifies: [X3.135-1986 §8.7 SR3] the columns of an INSERT exist; [DESIGN]
/// valid SQL-86 that is not implemented gets SQLCODE -900, and text that is
/// not an <SQL statement> a syntax error.
#[test]
fn statements_that_cannot_run() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(exec(&s, "INSERT INTO T (ID, NOPE) VALUES (9, 1)"), Err(Code::SyntaxRule));
    // [X3.135-1986 §8.7 SR2] is Intermediate SQL's [ISO/IEC 9075:1992
    // §13.8, Leveling Rules]: T's own rows duplicate its keys.
    assert_eq!(exec(&s, "INSERT INTO T SELECT ID, NAME, AMT, F FROM T"), Err(Code::Constraint));
    assert_eq!(exec(&s, "DECLARE C CURSOR FOR SELECT ID FROM T"), Err(Code::Syntax));
    assert_eq!(exec(&s, "ID"), Err(Code::Syntax));
}

/// Verifies: [DESIGN] naming a storage engine that is not registered, for a
/// schema or a table, is refused with SQLCODE -910.
#[test]
fn unknown_engines_are_refused() {
    let db = Database::new();
    assert_eq!(db.use_engine("nope").map_err(|e| e.code), Err(Code::Api));
    assert_eq!(db.place_table("A.T", "nope").map_err(|e| e.code), Err(Code::Api));
}

/// Verifies: [X3.135-1986 §7.3 GR3a] a successful statement's SQLCODE is 0,
/// or 100 when no row was found.
#[test]
fn completion_sqlcodes_7_3_gr3a() {
    assert_eq!((Completion::Success.sqlcode(), Completion::NoData.sqlcode()), (0, 100));
}
