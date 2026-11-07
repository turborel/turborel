//! \[DESIGN\] What a fetch costs the planner, where an engine does not say.

/// \[DESIGN\] The default cost of one fetch by row id: a random read costs
/// about three sequential ones. Fixed, not a setting (RULES.md D36): an
/// engine states its own costs (`TableStorage::fetch_cost`).
pub const DEFAULT_FETCH_COST: f64 = 3.0;

/// A count of rows as a cost. Exact up to 2^32 rows; costs are estimates, so
/// beyond that it saturates.
pub fn cost_of(rows: usize) -> f64 {
    f64::from(u32::try_from(rows).unwrap_or(u32::MAX))
}
