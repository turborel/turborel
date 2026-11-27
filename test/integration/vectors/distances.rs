//! VECTOR_DISTANCE, its shorthands and operators, against values worked
//! by hand.

use turborel::db::Session;

use super::{error, got, near, number, table};

/// Table T of A = [1, 2, 3] and B = [4, 6, 8], whose differences are
/// [3, 4, 5], dot product 40, norms √14 and √116.
fn pair() -> (turborel::db::Database, Session) {
    let (db, a) = table("A VECTOR(3), B VECTOR(3), Z VECTOR(3), N VECTOR(3)");
    a.run("INSERT INTO T VALUES ('[1, 2, 3]', '[4, 6, 8]', '[0, 0, 0]', NULL)").unwrap();
    (db, a)
}

/// Verifies: [EXTENSION] vector_standard §6.1 R6.1, R6.2, R6.5, R6.6,
/// §6.2: each metric of the table, written as a key word, a string in
/// either case, and in both argument orders, gives its value by hand:
/// EUCLIDEAN √50, EUCLIDEAN_SQUARED and L2_SQUARED 50, MANHATTAN 12, DOT
/// -40, COSINE 1 - 40/√1624, HAMMING 3.
#[test]
fn each_metric_matches_hand_values_in_both_orders() {
    let (_db, a) = pair();
    let cosine = 1.0 - 40.0 / 1624.0_f64.sqrt();
    for (metric, want) in [
        ("EUCLIDEAN", 50.0_f64.sqrt()),
        ("EUCLIDEAN_SQUARED", 50.0),
        ("L2_SQUARED", 50.0),
        ("MANHATTAN", 12.0),
        ("DOT", -40.0),
        ("COSINE", cosine),
        ("HAMMING", 3.0),
    ] {
        let last = number(&a, &format!("SELECT VECTOR_DISTANCE(A, B, {metric}) FROM T"));
        let first = number(&a, &format!("SELECT VECTOR_DISTANCE('{}', A, B) FROM T", metric.to_lowercase()));
        let named = number(&a, &format!("SELECT VECTOR_DISTANCE(B, A, '{metric}') FROM T"));
        assert!(near(last, want) && near(first, want) && near(named, want), "{metric}: {last} {first} {named}");
    }
    assert!(near(number(&a, "SELECT VECTOR_DISTANCE(A, B) FROM T"), cosine));
}

/// Verifies: [EXTENSION] vector_standard §6.3: the shorthands and the
/// operators give VECTOR_DISTANCE's values; INNER_PRODUCT is DOT's
/// negation; a distance operator binds less tightly than + and more than
/// a comparison; JACCARD of BINARY [11110000] and [11111111] is 1/2.
#[test]
fn shorthands_and_operators_match_vector_distance() {
    let (_db, a) = pair();
    let same = |left: &str, right: &str| {
        let (x, y) = (number(&a, &format!("SELECT {left} FROM T")), number(&a, &format!("SELECT {right} FROM T")));
        assert!(near(x, y), "{left} {x} {right} {y}");
    };
    same("L1_DISTANCE(A, B)", "VECTOR_DISTANCE(A, B, MANHATTAN)");
    same("L2_DISTANCE(A, B)", "VECTOR_DISTANCE(A, B, EUCLIDEAN)");
    same("COSINE_DISTANCE(A, B)", "VECTOR_DISTANCE(A, B, COSINE)");
    same("HAMMING_DISTANCE(A, B)", "VECTOR_DISTANCE(A, B, HAMMING)");
    same("A <-> B", "VECTOR_DISTANCE(A, B, EUCLIDEAN)");
    same("A <=> B", "VECTOR_DISTANCE(A, B, COSINE)");
    same("A <#> B", "VECTOR_DISTANCE(A, B, DOT)");
    same("A <+> B", "VECTOR_DISTANCE(A, B, MANHATTAN)");
    same("A <~> B", "VECTOR_DISTANCE(A, B, HAMMING)");
    same("INNER_PRODUCT(A, B)", "-VECTOR_DISTANCE(A, B, DOT)");
    assert!(near(number(&a, "SELECT INNER_PRODUCT(A, B) FROM T"), 40.0));
    // Binding less tightly than + and more than <: A <-> (B + B), whose
    // differences are [7, 10, 13].
    assert!(near(number(&a, "SELECT A <-> B + B FROM T"), 318.0_f64.sqrt()));
    assert_eq!(got(&a, "SELECT COUNT(*) FROM T WHERE A <-> B < 8"), ["1"]);
    let bits = "SELECT JACCARD_DISTANCE(CAST('[240]' AS VECTOR(8, BINARY)), CAST('[255]' AS VECTOR(8, BINARY))) FROM T";
    assert!(near(number(&a, bits), 0.5));
    let bits = "SELECT CAST('[240]' AS VECTOR(8, BINARY)) <%> CAST('[255]' AS VECTOR(8, BINARY)) FROM T";
    assert!(near(number(&a, bits), 0.5));
}

/// Verifies: [EXTENSION] vector_standard §6.2 R6.7, §6.1 R6.5, R6.4: COSINE
/// of a zero vector is NULL; a NULL argument gives NULL; vectors of other
/// dimension counts are SQLCODE -301; a metric of no format the vectors are
/// in (JACCARD of FLOAT32), or of no name, too.
#[test]
fn nulls_zero_vectors_and_mismatches() {
    let (_db, a) = pair();
    assert_eq!(got(&a, "SELECT VECTOR_DISTANCE(A, Z, COSINE), A <=> N, L2_DISTANCE(N, B) FROM T"), ["NULL,NULL,NULL"]);
    assert_eq!(error(&a, "SELECT A <-> '[1, 2]' FROM T").0, -301);
    assert_eq!(error(&a, "SELECT VECTOR_DISTANCE(A, B, JACCARD) FROM T").0, -301);
    assert_eq!(error(&a, "SELECT VECTOR_DISTANCE(A, B, 'NO_SUCH') FROM T").0, -301);
}

/// Verifies: [EXTENSION] vector_standard §9: an exact search, ORDER BY a
/// distance FETCH FIRST k ROWS ONLY, gives the first k of the full sort.
#[test]
fn exact_nearest_matches_a_full_sort() {
    let (_db, a) = table("ID INTEGER, V VECTOR(2)");
    for id in 0..40_i32 {
        let (x, y) = ((id * 7) % 13, (id * 5) % 11);
        a.run(&format!("INSERT INTO T VALUES ({id}, '[{x}, {y}]')")).unwrap();
    }
    let all = got(&a, "SELECT ID FROM T ORDER BY V <-> '[6, 5]', ID");
    let first = got(&a, "SELECT ID FROM T ORDER BY V <-> '[6, 5]', ID FETCH FIRST 5 ROWS ONLY");
    assert_eq!(first, all[..5]);
    let cosine = got(&a, "SELECT ID FROM T WHERE ID > 0 ORDER BY VECTOR_DISTANCE(V, '[1, 1]', COSINE), ID FETCH FIRST 3 ROWS ONLY");
    let sorted = got(&a, "SELECT ID FROM T WHERE ID > 0 ORDER BY VECTOR_DISTANCE(V, '[1, 1]', COSINE), ID");
    assert_eq!(cosine, sorted[..3]);
}

/// Verifies: [EXTENSION] vector_standard §6.1 R6.3: VECTOR_DISTANCE
/// written with no metric measures by the metric of an index whose first
/// column is its first argument, EUCLIDEAN here, so [3, 4] is √18 from
/// [0, 1]; of a column no index describes, and of a vector not a column,
/// by COSINE, 1 - 4/5.
#[test]
fn the_default_metric_is_an_index_s() {
    let (_db, a) = table("V VECTOR(2), W VECTOR(2)");
    a.run("INSERT INTO T VALUES ('[3, 4]', '[3, 4]')").unwrap();
    a.commit().unwrap();
    a.run("CREATE VECTOR INDEX X ON T (V) USING hnsw WITH (METRIC = 'EUCLIDEAN')").unwrap();
    assert!(near(number(&a, "SELECT VECTOR_DISTANCE(V, '[0, 1]') FROM T"), 18.0_f64.sqrt()));
    assert!(near(number(&a, "SELECT VECTOR_DISTANCE(W, '[0, 1]') FROM T"), 0.2));
    assert!(near(number(&a, "SELECT VECTOR_DISTANCE(CAST(V AS VECTOR(2)), '[0, 1]') FROM T"), 0.2));
}
