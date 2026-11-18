//! A range search (the vector design document §9.4, R9.1): a WHERE
//! condition bounding the distance ORDER BY measures, by `<` or `<=`,
//! planned onto the approximate index, of a dense column and of a sparse
//! one, giving the exact search's rows.

use super::fixture::{documents, plan, read};

/// The documents nearest `[1,0,0,0]` by COSINE, of `fetch`, of those whose
/// distance is `op` `bound`.
fn within(fetch: &str, op: &str, bound: &str) -> String {
    let distance = "VECTOR_DISTANCE(EMB, TO_VECTOR('[1,0,0,0]', 4, 'FLOAT32'), 'COSINE')";
    format!("SELECT ID FROM DOCS WHERE {distance} {op} {bound} ORDER BY {distance} FETCH {fetch} FIRST 2 ROWS ONLY")
}

/// Verifies: [EXTENSION] vector_standard §9.4, R9.1, R13.1: a distance
/// bounded by `<` or `<=` in WHERE, the bound on either side, is a range
/// search the approximate index of the metric serves, here the vector
/// part of a hybrid index; its rows are the exact search's, and none past
/// the bound.
#[test]
fn a_radius_in_where_reads_the_approximate_index_9_4() {
    let (_db, s) = documents(Some("CREATE HYBRID VECTOR INDEX DOCS_H ON DOCS (BODY, EMB) PARAMETERS (VECTOR_IDX 'DISTANCE COSINE')"));
    for (op, bound) in [("<", "0.1"), ("<=", "0.3")] {
        let approx = within("APPROX", op, bound);
        let shown = plan(&s, &approx);
        assert!(shown.contains("NearestScan S.DOCS using DOCS_H [hybrid]") && shown.contains("approximate COSINE"), "{shown}");
        assert_eq!(read(&s, &approx), read(&s, &within("EXACT", op, bound)), "{op}");
    }
    assert_eq!(read(&s, &within("APPROX", "<", "0.1")), ["1", "2"]);
    let flipped = "SELECT ID FROM DOCS WHERE 0.3 >= VECTOR_DISTANCE(EMB, TO_VECTOR('[1,0,0,0]', 4, 'FLOAT32'), 'COSINE') \
                   ORDER BY VECTOR_DISTANCE(EMB, TO_VECTOR('[1,0,0,0]', 4, 'FLOAT32'), 'COSINE') FETCH APPROX FIRST 3 ROWS ONLY";
    assert!(plan(&s, flipped).contains("using DOCS_H [hybrid]"));
    assert_eq!(read(&s, flipped), ["1", "2", "5"]);
}
