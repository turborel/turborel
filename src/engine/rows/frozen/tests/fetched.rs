//! Unit tests of the planner's cost of a frozen row fetched by its id
//! (`frozen.rs`, `frozen_fetch_cost`, RULES.md D1097).

use super::*;
use crate::engine::rows::frozen::{FROZEN_FETCH, FROZEN_FETCH_COLUMN};

/// What a frozen row of `columns` read costs against one of a column.
fn against_one(columns: f64) -> f64 {
    (FROZEN_FETCH + FROZEN_FETCH_COLUMN * columns) / (FROZEN_FETCH + FROZEN_FETCH_COLUMN)
}

/// Verifies: [DESIGN] RULES.md D1097, a fetch priced by its columns: of
/// a table whose rows are all frozen, a fetch costs `FROZEN_FETCH` and
/// `FROZEN_FETCH_COLUMN` for each column read, so one of every column
/// costs more than one of four, and that more than one of a column, in
/// the measured proportions; of a table whose rows are all in its tree,
/// a fetch costs the same whatever it reads, its row being read whole.
#[test]
fn a_frozen_fetch_is_priced_by_its_columns() {
    let (_, files) = files();
    let (tree, _) = table(&files, 0, 100);
    assert_eq!(tree.fetch_cost(1).to_bits(), tree.fetch_cost(17).to_bits(), "no row frozen");
    let (frozen, _) = table(&files, 0, 4096);
    ended(&frozen, 1);
    assert_eq!(entries(&frozen), 0, "every row frozen");
    let costs: Vec<f64> = [1_usize, 4, 17].iter().map(|&n| frozen.fetch_cost(n)).collect();
    assert!(costs.windows(2).all(|w| w[0] < w[1]), "{costs:?}");
    for (cost, columns) in costs.iter().zip([1.0_f64, 4.0_f64, 17.0_f64]) {
        assert!((cost / costs[0] - against_one(columns)).abs() < 1e-9_f64, "{columns} columns: {costs:?}");
    }
}
