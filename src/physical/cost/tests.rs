//! Tests of the costs of spills, of the degree a part runs at, and of the
//! rows a runtime filter leaves (RULES.md D651 to D653).

use super::parallel::Degrees;
use super::runtime::{FILTER_ROW, kept, probed};
use super::spill::{ENTRY_BYTES, PARTITIONED_ROW, SPILLED_BYTE, SPILLED_ROW, hash_spilled, row_bytes, sort_spilled};
use super::{CostModel, HASH_ROW};

/// Whether `a` and `b` are the same cost, to rounding.
fn same(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(1.0_f64)
}

/// Verifies: [DESIGN] (RULES.md D651) an operator whose rows fit its
/// grant costs nothing more; one byte past it, its rows are written and
/// read once more, a hash join's of both operands at a partitioned row's
/// weight, a sort's its own at a spilled row's, each with its bytes'. The boundary pair of `held * bytes
/// <= grant`: at the grant, none; past it, all.
#[test]
fn rows_past_the_grant_are_spilled_once_more() {
    let bytes = row_bytes(2, None);
    assert!(same(bytes, 16.0_f64 + ENTRY_BYTES));
    let grant = 1_000.0_f64 * bytes;
    assert!(same(hash_spilled((1_000.0_f64, bytes), 5_000.0_f64, grant), 0.0_f64));
    let partitioned = PARTITIONED_ROW + SPILLED_BYTE * bytes;
    assert!(same(hash_spilled((1_001.0_f64, bytes), 5_000.0_f64, grant), 6_001.0_f64 * partitioned));
    let each = SPILLED_ROW + SPILLED_BYTE * bytes;
    assert!(same(sort_spilled((1_000.0_f64, bytes), grant), 0.0_f64));
    assert!(same(sort_spilled((1_001.0_f64, bytes), grant), 1_001.0_f64 * each));
    let model = CostModel { grant_bytes: grant, ..CostModel::default() };
    assert!(same(model.sort_held(1_001.0_f64, bytes), model.sort(1_001.0_f64) + 1_001.0_f64 * each));
    assert!(same(row_bytes(2, Some(100.0_f64)), 100.0_f64 + ENTRY_BYTES));
}

/// Verifies: [DESIGN] (RULES.md D652, D291) the degree a part is costed
/// at is the marking pass's: one below the least rows, else a thread a
/// morsel up to the most, as a cost too.
/// The pairs of `rows <
/// self.least`: just below and at the least.
#[test]
fn a_part_shares_its_work_by_the_marking_passs_degree() {
    let degrees = Degrees { most: 8, morsel: 16_384, least: 65_536 };
    assert_eq!(degrees.degree(65_535), 1);
    assert_eq!(degrees.degree(65_536), 4);
    assert_eq!(degrees.degree(10_000_000), 8);
    assert!(same(degrees.degree_of(1_000.0_f64), 1.0_f64));
    assert!(same(degrees.degree_of(1e7_f64), 8.0_f64));
    assert_eq!(Degrees::default().degree(10_000_000), 1);
}

/// Verifies: [DESIGN] (RULES.md D653) a runtime filter keeps of the
/// probing rows the built operand's distinct keys over the probing
/// operand's, the built distinct at most its rows, the least of the
/// join's classes; and the probing rows cost the filter's test each and
/// the hashing of those kept, or only the hashing where it keeps more
/// than nine in ten, as the executor stops such a filter.
#[test]
fn a_runtime_filter_leaves_the_built_keys_share_of_the_probe() {
    assert!(same(kept(&[(1_000.0_f64, 100_000.0_f64)], 1_000_000.0_f64), 0.01_f64));
    assert!(same(kept(&[(1_000.0_f64, 100_000.0_f64)], 10.0_f64), 0.0001_f64));
    assert!(same(kept(&[(100_000.0_f64, 1_000.0_f64)], 1_000_000.0_f64), 1.0_f64));
    assert!(same(kept(&[(1_000.0_f64, 10_000.0_f64), (50.0_f64, 100.0_f64)], 1e9_f64), 0.1_f64));
    assert!(same(kept(&[], 10.0_f64), 1.0_f64));
    assert!(same(probed(1_000.0_f64, HASH_ROW, 0.01_f64), 1_000.0_f64 * (FILTER_ROW + HASH_ROW * 0.01_f64)));
    assert!(same(probed(1_000.0_f64, HASH_ROW, 0.95_f64), 1_000.0_f64 * HASH_ROW));
}
