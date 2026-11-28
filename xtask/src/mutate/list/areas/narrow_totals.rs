//! The mutations of exact totals held in 64 bits and widened (RULES.md
//! R-TEST-9, D1308): an add that leaves 64 bits wrapped, units and an
//! extreme 64 bits do not hold cut to them, the row that leaves them told
//! as taken and the rows taken before it taken again, a batch of few
//! groups that leaves them kept wrapped or not fed again, a narrow column
//! passed over for its errors at any bound, a wide column charged as a
//! narrow one, a column read back wide whatever it holds, and one
//! gathered into the other form.

use super::super::{Mutation, Package};

/// The file of the totals in their two forms.
const TOTALS: &str = "src/exec/table/states/typed/totals.rs";

/// The file of the states of few groups fed by lanes.
const FEW: &str = "src/exec/table/states/typed/few.rs";

/// Totals in either form against a model in 128 bits.
const SAME: &str = "exec::table::states::typed::totals::tests::narrow_and_wide_totals_are_the_same_integers";

/// The charge of a column of totals, and its totals gathered.
const CHARGED: &str = "exec::table::states::typed::totals::tests::a_column_of_totals_charges_its_form_and_gathers_in_it";

/// The rows a narrow column takes in 64 bits.
const TAKEN: &str = "exec::table::states::typed::totals::tests::a_narrow_column_takes_the_rows_before_the_first_that_leaves_64_bits";

/// Narrow totals of few groups fed a batch that leaves 64 bits.
const LANES: &str = "exec::table::states::typed::few::tests::narrow_totals_a_batch_leaves_are_widened_and_fed_it_once";

/// Columns fed past 64 bits against their accumulators.
const FED: &str = "exec::table::states::tests::widened::totals_fed_past_64_bits_are_their_accumulators";

/// States sent in either form merged in turn.
const SENT: &str = "exec::table::states::tests::widened::states_sent_in_either_form_merge_in_turn_as_their_rows";

/// Parts of either form merged whole.
const PARTS: &str = "exec::table::states::tests::widened::parts_of_either_form_merge_whole_as_their_rows";

/// Results that raise, of totals in either form.
const RAISED: &str = "exec::table::states::tests::widened::results_that_raise_are_raised_of_totals_in_either_form";

/// A column of exact totals written to a spill and read back.
const SPILLED: &str =
    "exec::table::states::typed::written::tests::exact_totals_are_written_alike_in_either_form_and_read_back_narrow_where_they_fit";

/// Groupings whose sums leave 64 bits, in parallel against alone.
const ALONE: &str = "spilled_groups::sums_that_leave_64_bits_come_as_alone_in_memory_and_spilled";

/// Mechanisms whose removal must fail the requirements of the two forms.
pub const NARROW_TOTALS: [Mutation; 11] = [
    Mutation {
        name: "narrow-total-add-wraps",
        package: Package::Core,
        breaks: "an add that leaves 64 bits wraps within them, the column not widened (RULES.md D1308)",
        file: TOTALS,
        original: "i64::try_from(units).ok().and_then(|unit| total.checked_add(unit))",
        replacement: "i64::try_from(units).ok().map(|unit| total.wrapping_add(unit))",
        integration: &[ALONE],
        unit: &[SAME, FED, SENT],
    },
    Mutation {
        name: "narrow-total-units-cut",
        package: Package::Core,
        breaks: "units 64 bits do not hold, as a wide column merged into a narrow one gives, are cut to 64 bits and added there (RULES.md D1308)",
        file: TOTALS,
        original: "i64::try_from(units).ok().and_then(|unit| total.checked_add(unit))",
        replacement: "i64::try_from(units.clamp(i128::from(i64::MIN), i128::from(i64::MAX))).ok().and_then(|unit| total.checked_add(unit))",
        integration: &[],
        unit: &[SAME, SENT],
    },
    Mutation {
        name: "narrow-extreme-cut",
        package: Package::Core,
        breaks: "a best value 64 bits do not hold is cut to them, its column not widened (RULES.md D1308)",
        file: TOTALS,
        original: "if let Ok(unit) = i64::try_from(units) {",
        replacement: "if let Ok(unit) = i64::try_from(units.clamp(i128::from(i64::MIN), i128::from(i64::MAX))) {",
        integration: &[],
        unit: &[SAME, FED],
    },
    Mutation {
        name: "narrow-rows-leaving-row-passed",
        package: Package::Core,
        breaks: "the row whose sum leaves 64 bits is told as taken, and is added to no total (RULES.md D1308)",
        file: TOTALS,
        original: "else { return (n, any) };",
        replacement: "else { return (n.saturating_add(1), any) };",
        integration: &[ALONE],
        unit: &[TAKEN, FED],
    },
    Mutation {
        name: "narrow-rows-taken-twice",
        package: Package::Core,
        breaks: "the rows a narrow column took in 64 bits are taken again with the rest (RULES.md D1308)",
        file: "src/exec/table/states/typed.rs",
        original: ".zip(&pending.groups).skip(taken) {",
        replacement: ".zip(&pending.groups).skip(taken.min(0)) {",
        integration: &[],
        unit: &[FED],
    },
    Mutation {
        name: "few-lanes-kept-past-64-bits",
        package: Package::Core,
        breaks: "a batch of few groups that leaves 64 bits is added wrapped, then fed again to the column widened (RULES.md D1308)",
        file: FEW,
        original: "    if !over {\n",
        replacement: "    if true {\n",
        integration: &[],
        unit: &[LANES, FED],
    },
    Mutation {
        name: "few-lanes-not-fed-again",
        package: Package::Core,
        breaks: "a batch of few groups that leaves 64 bits is counted and not fed again, its sums lost (RULES.md D1308)",
        file: FEW,
        original: "{ by_lanes(narrow, rows) } else { true };",
        replacement: "{ by_lanes(narrow, rows) && false } else { true };",
        integration: &[],
        unit: &[LANES, FED],
    },
    Mutation {
        name: "quiet-narrow-at-any-bound",
        package: Package::Core,
        breaks: "a narrow column is passed over whole whatever the bound, so a mean that raises of a total 64 bits hold is lost (RULES.md D1308, D1256)",
        file: "src/exec/table/states/typed/quiet.rs",
        original: "if bound > NARROW_MOST {",
        replacement: "if bound > 0 {",
        integration: &[],
        unit: &[RAISED],
    },
    Mutation {
        name: "wide-totals-charged-narrow",
        package: Package::Core,
        breaks: "a wide column of totals is charged 8 bytes a total, half what it holds (RULES.md D1308)",
        file: TOTALS,
        original: "Totals::Wide(totals) => totals.capacity().saturating_mul(size_of::<i128>()),",
        replacement: "Totals::Wide(totals) => totals.capacity().saturating_mul(size_of::<i64>()),",
        integration: &[],
        unit: &[CHARGED, SENT],
    },
    Mutation {
        name: "spilled-totals-read-wide",
        package: Package::Core,
        breaks: "a column of totals read back from the spill is wide though 64 bits hold every total (RULES.md D1308, D1226)",
        file: TOTALS,
        original: "        let mut totals = Totals::new();",
        replacement: "        let mut totals = Totals::Wide(Vec::new());",
        integration: &[],
        unit: &[SPILLED],
    },
    Mutation {
        name: "gathered-totals-widened",
        package: Package::Core,
        breaks: "narrow totals gathered are given wide, so the column a second grouping merges a narrow part into starts wide (RULES.md D1308, D1280)",
        file: TOTALS,
        original: "Totals::Narrow(totals) => Totals::Narrow(order.iter().map(|&i| totals.get(index(i)).copied().unwrap_or(0)).collect()),",
        replacement: "Totals::Narrow(totals) => Totals::Wide(order.iter().map(|&i| i128::from(totals.get(index(i)).copied().unwrap_or(0))).collect()),",
        integration: &[],
        unit: &[CHARGED, PARTS],
    },
];
