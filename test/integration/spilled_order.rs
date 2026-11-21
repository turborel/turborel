//! \[DESIGN\] A spilled operator's order (RULES.md D357, D38): its
//! partitioners draw their seeds of the database's seed, each its own,
//! so two runs of a statement alike in one database give their rows in
//! the same order, partitioned once or again.

use turborel::db::Database;

use crate::common::rows;
use crate::spilling::{SMALL, bytes, filled};

/// Statements whose operators spill and give their rows in no order the
/// statement asks: a grouping over a hash join, the join itself, a
/// grouping, DISTINCT, an outer join and a set operation.
const QUERIES: [&str; 6] = [
    "SELECT T.G, COUNT(*), MIN(U.K) FROM T, U WHERE T.G = U.G GROUP BY T.G",
    "SELECT T.K, U.K FROM T, U WHERE T.G = U.G",
    "SELECT S, K, COUNT(*) FROM U GROUP BY S, K",
    "SELECT DISTINCT G, S FROM T",
    "SELECT T.K, U.K FROM T LEFT OUTER JOIN U ON T.G = U.G",
    "SELECT G FROM T INTERSECT SELECT G FROM U",
];

/// Verifies: [DESIGN] RULES.md D357: with MEMORY.WORK_BYTES at its
/// least, each of `QUERIES` gives the same rows in the same order on two
/// runs, of SPILL.PARTITIONS at its default and of 2 partitioned again.
#[test]
fn a_spilled_operator_gives_its_rows_in_one_order() {
    let db = Database::new();
    filled(&db);
    let s = db.session("S");
    db.set_setting("MEMORY.WORK_BYTES", &bytes(SMALL)).unwrap();
    for partitions in [None, Some(2)] {
        if let Some(p) = partitions {
            db.set_setting("SPILL.PARTITIONS", &bytes(p)).unwrap();
        }
        for q in QUERIES {
            let first = rows(&s, q).unwrap();
            assert_eq!(rows(&s, q).unwrap(), first, "{q}, partitions {partitions:?}");
        }
    }
}
