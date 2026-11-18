//! The utility, arithmetic, quantization and aggregate functions of
//! VECTOR, and its operators of arithmetic.

use super::{error, got, near, number, table};

/// Verifies: [EXTENSION] vector_standard §6.4: VECTOR_NORM by each norm
/// written as a key word or a string, VECTOR_NORMALIZE, VECTOR_ELEMENT,
/// VECTOR_SLICE, VECTOR_CONCAT and VECTORPROPERTY of [3, -4].
#[test]
fn norms_and_utilities() {
    let (_db, a) = table("V VECTOR(2)");
    a.run("INSERT INTO T VALUES ('[3, -4]')").unwrap();
    assert!(near(number(&a, "SELECT VECTOR_NORM(V) FROM T"), 5.0));
    assert!(near(number(&a, "SELECT VECTOR_NORM(V, NORM1) FROM T"), 7.0));
    assert!(near(number(&a, "SELECT VECTOR_NORM(V, 'norminf') FROM T"), 4.0));
    assert_eq!(
        got(
            &a,
            "SELECT FROM_VECTOR(VECTOR_NORMALIZE(V)), VECTOR_ELEMENT(V, 2), FROM_VECTOR(VECTOR_SLICE(V, 2, 1)), \
             FROM_VECTOR(VECTOR_CONCAT(V, V)), VECTORPROPERTY(V, Dimensions) FROM T"
        ),
        ["[0.6,-0.8],-4E0,[-4],[3,-4,3,-4],2"]
    );
}

/// Verifies: [EXTENSION] vector_standard §6.5 R6.9: + - * of vectors are
/// element-wise, a number times a vector and a vector divided by a number
/// scale it, INT8 operands give FLOAT32; a vector divided by a vector is
/// SQLCODE -103, and by zero 22012.
#[test]
fn arithmetic_operators() {
    let (_db, a) = table("V VECTOR(2), W VECTOR(2), I VECTOR(2, INT8)");
    a.run("INSERT INTO T VALUES ('[1, 2]', '[3, 5]', '[2, 2]')").unwrap();
    assert_eq!(
        got(&a, "SELECT FROM_VECTOR(V + W), FROM_VECTOR(W - V), FROM_VECTOR(V * W), FROM_VECTOR(2 * V), FROM_VECTOR(W / 2) FROM T"),
        ["[4,7],[2,3],[3,10],[2,4],[1.5,2.5]"]
    );
    assert_eq!(got(&a, "SELECT FROM_VECTOR(V + '[1, 1]'), VECTOR_DIMENSION_FORMAT(I + I) FROM T"), ["[2,3],FLOAT32"]);
    assert_eq!(error(&a, "SELECT V / W FROM T").0, -103);
    assert_eq!(error(&a, "SELECT V / 0 FROM T"), (-301, "22012".to_owned()));
}

/// Verifies: [EXTENSION] vector_standard §6.5, R6.10: AVG and SUM of
/// vectors are element-wise, as VECTOR_AVG and VECTOR_SUM; VECTOR_MIN and
/// VECTOR_MAX too; of sparse vectors the sum stays sparse; of no row,
/// NULL.
#[test]
fn aggregates_are_element_wise() {
    let (_db, a) = table("G INTEGER, V VECTOR(2), S VECTOR(4, FLOAT32, SPARSE)");
    a.run("INSERT INTO T VALUES (1, '[1, 4]', '[4, [0], [1]]')").unwrap();
    a.run("INSERT INTO T VALUES (1, '[3, 0]', '[4, [3], [2]]')").unwrap();
    assert_eq!(
        got(
            &a,
            "SELECT FROM_VECTOR(AVG(V)), FROM_VECTOR(SUM(V)), FROM_VECTOR(VECTOR_AVG(V)), FROM_VECTOR(VECTOR_MIN(V)), \
             FROM_VECTOR(VECTOR_MAX(V)), FROM_VECTOR(SUM(S)) FROM T GROUP BY G"
        ),
        ["[2,2],[4,4],[2,2],[1,0],[3,4],[4,[0,3],[1,2]]"]
    );
    assert_eq!(got(&a, "SELECT AVG(V) FROM T WHERE G = 2"), ["NULL"]);
}

/// Verifies: [EXTENSION] vector_standard §7.1: VECTOR_QUANTIZE to INT8,
/// BINARY and FLOAT16, and VECTOR_DEQUANTIZE back; the format named as a
/// key word: the bits of [-1, 0.5, 2, 0, 1, 1, 1, 1] above 0 are
/// 01101111, 111.
#[test]
fn quantize_and_dequantize() {
    let (_db, a) = table("V VECTOR(8)");
    a.run("INSERT INTO T VALUES ('[-1, 0.5, 2, 0, 1, 1, 1, 1]')").unwrap();
    assert_eq!(
        got(
            &a,
            "SELECT FROM_VECTOR(VECTOR_QUANTIZE(V, BINARY)), VECTOR_DIMENSION_FORMAT(VECTOR_QUANTIZE(V, FLOAT16)), \
             FROM_VECTOR(VECTOR_QUANTIZE(V, INT8, 0.5, 0)), FROM_VECTOR(VECTOR_DEQUANTIZE(VECTOR_QUANTIZE(V, INT8, 0.5, 0), 0.5, 0)) FROM T"
        ),
        ["[111],FLOAT16,[-2,1,4,0,2,2,2,2],[-1,0.5,2,0,1,1,1,1]"]
    );
}
