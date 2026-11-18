//! Distance functions of SQL (CREATE VECTOR DISTANCE FUNCTION) and
//! metrics a program registers.

use turborel::vector::metric::{MetricProperties, VectorMetric};
use turborel::vector::{Element, VectorRef};

use super::{error, got, near, number, table};

/// The definition of A.WEIGHTED, a weighted Manhattan distance.
const WEIGHTED: &str = "CREATE VECTOR DISTANCE FUNCTION A.WEIGHTED (X VECTOR, Y VECTOR) RETURNS FLOAT64 DETERMINISTIC \
                        PROPERTIES (SYMMETRIC, NON_NEGATIVE) VALID FORMATS (FLOAT32) LANGUAGE SQL \
                        AS 'VECTOR_NORM(VECTOR_SUBTRACT(X, Y), ''NORM1'') * 2'";

/// Verifies: [EXTENSION] vector_standard §8, §6.1 R6.2: a distance
/// function of SQL is named by VECTOR_DISTANCE, qualified or not, as a key
/// word or a string, in both argument orders; is listed by
/// VECTOR_DISTANCE_FUNCTIONS; and is dropped by DROP VECTOR DISTANCE
/// FUNCTION, after which naming it is SQLCODE -301.
#[test]
fn sql_distance_functions_are_defined_used_and_dropped() {
    let (_db, a) = table("V VECTOR(2), W VECTOR(2)");
    a.run("INSERT INTO T VALUES ('[1, 2]', '[4, 0]')").unwrap();
    a.run(WEIGHTED).unwrap();
    for sql in [
        "SELECT VECTOR_DISTANCE(V, W, A.WEIGHTED) FROM T",
        "SELECT VECTOR_DISTANCE(V, W, WEIGHTED) FROM T",
        "SELECT VECTOR_DISTANCE('A.WEIGHTED', W, V) FROM T",
    ] {
        assert!(near(number(&a, sql), 10.0), "{sql}");
    }
    assert_eq!(
        got(
            &a,
            "SELECT FUNCTION_SCHEMA, FUNCTION_NAME, SYMMETRIC, NON_NEGATIVE, TRIANGLE_INEQUALITY, FORMATS FROM INFORMATION_SCHEMA.VECTOR_DISTANCE_FUNCTIONS"
        ),
        ["A,WEIGHTED,YES,YES,NO,FLOAT32"]
    );
    assert_eq!(error(&a, "SELECT VECTOR_DISTANCE(CAST(V AS VECTOR(2, FLOAT64)), CAST(W AS VECTOR(2, FLOAT64)), WEIGHTED) FROM T").0, -301);
    a.run("DROP VECTOR DISTANCE FUNCTION A.WEIGHTED RESTRICT").unwrap();
    assert_eq!(error(&a, "SELECT VECTOR_DISTANCE(V, W, WEIGHTED) FROM T").0, -301);
}

/// Verifies: [EXTENSION] vector_standard §8: a distance function of other
/// than two VECTOR parameters, a property or format written twice or
/// unknown is SQLCODE -103; of LANGUAGE PYTHON -900; a body of a query,
/// or a name a built-in metric's, -103; and one of no EXECUTE privilege on it is -104 (R8.5).
#[test]
fn distance_function_definitions_are_checked() {
    let (db, a) = table("V VECTOR(2)");
    let code = |sql: &str| a.run(sql).map_err(|e| e.sqlcode()).err();
    let head = "CREATE VECTOR DISTANCE FUNCTION A.F";
    assert_eq!(code(&format!("{head} (X VECTOR) RETURNS FLOAT64 LANGUAGE SQL AS '1'")), Some(-103));
    assert_eq!(code(&format!("{head} (X VECTOR, Y INTEGER) RETURNS FLOAT64 LANGUAGE SQL AS '1'")), Some(-103));
    assert_eq!(
        code(&format!("{head} (X VECTOR, Y VECTOR) RETURNS FLOAT64 PROPERTIES (SYMMETRIC, SYMMETRIC) LANGUAGE SQL AS '1'")),
        Some(-103)
    );
    assert_eq!(code(&format!("{head} (X VECTOR, Y VECTOR) RETURNS FLOAT64 PROPERTIES (CONVEX) LANGUAGE SQL AS '1'")), Some(-103));
    assert_eq!(code(&format!("{head} (X VECTOR, Y VECTOR) RETURNS FLOAT64 LANGUAGE PYTHON AS '1'")), Some(-900));
    assert_eq!(code(&format!("{head} (X VECTOR, Y VECTOR) RETURNS FLOAT64 LANGUAGE SQL AS '(SELECT 1 FROM T)'")), Some(-103));
    assert_eq!(code("CREATE VECTOR DISTANCE FUNCTION A.COSINE (X VECTOR, Y VECTOR) RETURNS FLOAT64 LANGUAGE SQL AS '1'"), Some(-103));
    a.run(WEIGHTED).unwrap();
    a.commit().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE U (V VECTOR(2))").unwrap();
    let b = db.session("B");
    b.run("INSERT INTO U VALUES ('[1, 1]')").unwrap();
    assert_eq!(error(&b, "SELECT VECTOR_DISTANCE(V, V, A.WEIGHTED) FROM U").0, -104);
    a.run("GRANT EXECUTE ON FUNCTION A.WEIGHTED TO B").unwrap();
    a.commit().unwrap();
    assert!(near(number(&b, "SELECT VECTOR_DISTANCE(V, V, A.WEIGHTED) FROM U"), 0.0));
}

/// A metric a program registers: the largest element-wise gap, or a
/// negative one if `lies`, though it declares itself never negative.
struct Chebyshev {
    lies: bool,
}

impl VectorMetric for Chebyshev {
    fn name(&self) -> &str {
        if self.lies { "LIAR" } else { "CHEBYSHEV" }
    }
    fn formats(&self) -> &[Element] {
        &[Element::Float32]
    }
    fn properties(&self) -> MetricProperties {
        MetricProperties { symmetric: true, non_negative: true, triangle: true, normalized_as: None }
    }
    fn distance(&self, a: VectorRef<'_>, b: VectorRef<'_>) -> turborel::error::Result<Option<f64>> {
        let gap = (0..a.dims).map(|d| (a.at(d).unwrap() - b.at(d).unwrap()).abs()).fold(0.0, f64::max);
        Ok(Some(if self.lies { -gap } else { gap }))
    }
}

/// Verifies: [EXTENSION] RULES.md R-SAFE-3, vector_standard §6.1 R6.2: a
/// metric registered is named by VECTOR_DISTANCE; one that breaks its
/// contract, a negative distance of a metric declared never negative, is
/// SQLCODE -910; a name registered twice, or one built in, is refused.
#[test]
fn registered_metrics_are_named_and_checked() {
    let (db, a) = table("V VECTOR(2), W VECTOR(2)");
    db.register_metric(Box::new(Chebyshev { lies: false })).unwrap();
    db.register_metric(Box::new(Chebyshev { lies: true })).unwrap();
    a.run("INSERT INTO T VALUES ('[1, 2]', '[4, 0]')").unwrap();
    assert!(near(number(&a, "SELECT VECTOR_DISTANCE(V, W, CHEBYSHEV) FROM T"), 3.0));
    assert_eq!(error(&a, "SELECT VECTOR_DISTANCE(V, W, LIAR) FROM T").0, -910);
    assert_eq!(db.register_metric(Box::new(Chebyshev { lies: false })).map_err(|e| e.sqlcode()).err(), Some(-910));
}

/// Verifies: [EXTENSION] vector_standard §8, [ISO/IEC 9075-2:2023 §4.35.2,
/// §11.62 SR4]: a view whose query names a distance function of SQL
/// depends on it, so DROP VECTOR DISTANCE FUNCTION RESTRICT is refused
/// with SQLCODE -103 while the view is, and CASCADE drops the view too,
/// so a query of it is -103, of no table [X3.135-1986 §5.4 SR5].
#[test]
fn a_view_naming_a_distance_function_depends_on_it() {
    let (_db, a) = table("V VECTOR(2), W VECTOR(2)");
    a.run("INSERT INTO T VALUES ('[1, 2]', '[4, 0]')").unwrap();
    a.run(WEIGHTED).unwrap();
    a.run("CREATE VIEW D (DISTANCE) AS SELECT VECTOR_DISTANCE(V, W, WEIGHTED) FROM T").unwrap();
    a.commit().unwrap();
    assert!(near(number(&a, "SELECT DISTANCE FROM D"), 10.0));
    let restricted = a.run("DROP VECTOR DISTANCE FUNCTION A.WEIGHTED RESTRICT");
    assert_eq!(restricted.map_err(|e| e.sqlcode()).err(), Some(-103));
    a.rollback().unwrap();
    a.run("DROP VECTOR DISTANCE FUNCTION A.WEIGHTED CASCADE").unwrap();
    let gone = crate::common::failed(&a, "SELECT DISTANCE FROM D");
    assert_eq!(gone.sqlcode(), -103, "{gone:?}");
}
