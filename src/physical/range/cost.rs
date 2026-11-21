//! \[DESIGN\] The cost of a range join (RULES.md D580), in rows of a scan
//! as the cost model's (`physical/cost.rs`): each row of either operand
//! read and its values computed, as a hash join's are hashed; the sorted
//! operand sorted; two binary searches for each row of the other, each a
//! comparison for each halving of the sorted rows; of a pruning comparison,
//! as many again for each pair found, the tree descended to it; and each
//! pair the search finds judged, then each kept given, as Cout counts a
//! join's rows (Neumann and Radke, 2018, §2). A nested loop instead judges
//! every pair.

use crate::physical::CostModel;
use crate::physical::cost::HASH_ROW;

/// The binary searches each searching row makes: one for each bound.
const SEARCHES: f64 = 2.0;

/// The cost of a range join whose operands give `probe` searching rows and
/// `build` sorted ones, the search finding `candidates` pairs, through a
/// tree if `pruned`, of which it keeps `est`.
pub(in crate::physical) fn range_cost(cost: &CostModel, (probe, build): (f64, f64), (candidates, est): (f64, f64), pruned: bool) -> f64 {
    let (probe, build) = (probe.max(0.0), build.max(0.0));
    let height = (build + 1.0).log2() * cost.sort_compare;
    let found = candidates.max(est);
    let descended = if pruned { (found + probe) * height } else { 0.0_f64 };
    HASH_ROW * (probe + build) + cost.sort(build) + probe * SEARCHES * height + descended + found + est
}
