//! A vector's text given as a CHARACTER LARGE OBJECT.

use super::{got, table};

/// Verifies: [EXTENSION] vector_standard §5.2 R5.1: a CHARACTER LARGE
/// OBJECT holding a vector's text is read as a VECTOR by CAST and by
/// assignment to a VECTOR column, as a character string is.
#[test]
fn a_clob_of_text_is_read_as_a_vector() {
    let (_db, a) = table("V VECTOR(2), C CLOB(100)");
    a.run("INSERT INTO T (C) VALUES ('[1.5, -2]')").unwrap();
    a.run("UPDATE T SET V = C").unwrap();
    assert_eq!(got(&a, "SELECT FROM_VECTOR(V), FROM_VECTOR(CAST(C AS VECTOR(2))) FROM T"), ["[1.5,-2],[1.5,-2]"]);
}
