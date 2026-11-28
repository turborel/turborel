//! The mutations of expressions over vectors (RULES.md R-TEST-9, D362,
//! D363, D1320): the flag of a batch's integer arithmetic, its rows null,
//! the runs a result keeps, three-valued OR, the bits of a null truth
//! value, the rows a CASE's WHEN leaves to the next, and the fields and
//! casts of a vector of datetimes.

use super::super::{Mutation, Package};

/// The expressions given by batch and by rows, of either engine.
const EXPRESSIONS: &str = "batch_exprs::expressions_give_by_batch_what_rows_give";

/// The bodies over vectors of the functions of datetimes.
const DATETIMES: &str = "src/functions/over_vectors/datetimes.rs";
/// Every field of a vector against the field of each value.
const FIELDS: &str = "functions::over_vectors::datetimes::tests::every_field_of_a_vector_is_the_field_of_each_value";
/// Every cast of a vector against the cast of each value.
const CASTS: &str = "functions::over_vectors::datetimes::tests::a_cast_of_a_vector_is_the_cast_of_each_value";
/// Every rule's loop over a vector against the cast of a row.
const RULES: &str = "functions::over_vectors::datetimes::tests::every_rule_s_loop_casts_a_row_as_the_row_s_cast_does";
/// A slot past the years, under a null and not.
const SLOTS: &str = "functions::over_vectors::datetimes::tests::a_slot_past_the_years_under_a_null_is_not_judged";
/// Fields and casts of datetimes by batch and by rows, of either engine.
const MOMENTS: &str = "batch_exprs::fields_and_casts_of_datetimes_give_by_batch_what_rows_give";

/// Mechanisms whose removal must fail the requirements of expressions
/// over vectors.
pub const EXPR: [Mutation; 16] = [
    Mutation {
        name: "expr-flag-ignored",
        package: Package::Core,
        breaks: "a batch whose integer arithmetic overflows gives its wrapped integers",
        file: "src/exec/batch/expr/ints.rs",
        original: "    if flag {\n        return None;\n    }\n    let made",
        replacement: "    if flag && units.is_empty() {\n        return None;\n    }\n    let made",
        integration: &["batch_exprs::raising_expressions_raise_as_rows_do"],
        unit: &["exec::batch::expr::ints::tests::the_flag_counts_rows_not_null"],
    },
    Mutation {
        name: "expr-flag-of-nulls",
        package: Package::Core,
        breaks: "an overflow of a null row's garbage slot sends the batch to rows",
        file: "src/exec/batch/expr/ints.rs",
        original: "flag = (0..count).any(|n| !nulls.get(n) && apply(left.unit(n), right.unit(n)).1);",
        replacement: "flag = (0..count).any(|n| apply(left.unit(n), right.unit(n)).1);",
        integration: &[],
        unit: &["exec::batch::expr::ints::tests::the_flag_counts_rows_not_null"],
    },
    Mutation {
        name: "expr-runs-of-one",
        package: Package::Core,
        breaks: "a sum of two vectors in runs keeps the runs of the first alone",
        file: "src/exec/batch/expr/ints.rs",
        original: "            first.extend(second);",
        replacement: "            drop(second);",
        integration: &["batches::columns_of_runs_answer_as_row_by_row"],
        unit: &["exec::batch::expr::ints::tests::runs_of_both_are_kept"],
    },
    Mutation {
        name: "expr-or-of-unknown",
        package: Package::Core,
        breaks: "OR of unknown and true is unknown",
        file: "src/exec/batch/expr/logic.rs",
        original: "    let unknown = either(a, b).map(|u| u.and_not(&true_of));",
        replacement: "    let unknown = either(a, b);",
        integration: &[EXPRESSIONS],
        unit: &["exec::batch::expr::tests::nan_compares_unknown_and_logic_is_three_valued"],
    },
    Mutation {
        name: "expr-null-truth-bits",
        package: Package::Core,
        breaks: "a null truth value whose bit is set reads true",
        file: "src/exec/batch/expr/logic.rs",
        original: "    let bits = unknown.as_ref().map_or_else(|| bits.clone(), |u| bits.and_not(u));",
        replacement: "    let bits = bits.clone();",
        integration: &[],
        unit: &["exec::batch::expr::tests::null_truth_values_read_unknown_whatever_their_bit"],
    },
    Mutation {
        name: "expr-case-rows-not-left",
        package: Package::Core,
        breaks: "a CASE's later WHENs and its ELSE take the rows an earlier WHEN took",
        file: "src/exec/batch/expr/case.rs",
        original: "        left = rest;",
        replacement: "        drop(rest);",
        integration: &[EXPRESSIONS],
        unit: &["exec::batch::expr::tests::overflows_give_none_and_untaken_branches_none"],
    },
    Mutation {
        name: "datetime-field-read-in-utc",
        package: Package::Core,
        breaks: "a field of a vector of datetimes is read of its instant in UTC, not as its displacement shows it",
        file: DATETIMES,
        original: "    let local = |utc: i64, zone: i16| utc.saturating_add(shift(zone));",
        replacement: "    let local = |utc: i64, _zone: i16| utc;",
        integration: &[MOMENTS],
        unit: &[FIELDS],
    },
    Mutation {
        name: "datetime-minute-not-of-its-hour",
        package: Package::Core,
        breaks: "MINUTE of a vector of datetimes is every minute since the first, not those of its hour",
        file: DATETIMES,
        original: "floor_rem(floor_div(local(utc, zone), MICROS_PER_MINUTE), 60)",
        replacement: "floor_div(local(utc, zone), MICROS_PER_MINUTE)",
        integration: &[MOMENTS],
        unit: &[FIELDS],
    },
    Mutation {
        name: "datetime-zones-of-fewer-rows",
        package: Package::Core,
        breaks: "a vector of datetimes with fewer displacements than rows gives a vector of fewer rows",
        file: DATETIMES,
        original: "(zones.len() == datetimes.utc.len()).then(",
        replacement: "(zones.len() <= datetimes.utc.len()).then(",
        integration: &[],
        unit: &[FIELDS],
    },
    Mutation {
        name: "datetime-cast-shown-as-kept",
        package: Package::Core,
        breaks: "a cast of a vector of timestamps without time zone reads their day in UTC, not as the session shows it",
        file: DATETIMES,
        original: "            let shown = if self.keeps { zone } else { self.session.0 };",
        replacement: "            let shown = zone;",
        integration: &[MOMENTS],
        unit: &[CASTS],
    },
    Mutation {
        name: "datetime-cast-fraction-kept",
        package: Package::Core,
        breaks: "a cast of a vector of datetimes to fewer digits keeps the fraction past them",
        file: DATETIMES,
        original: "        let cut = made.saturating_sub(self.fraction(made));",
        replacement: "        let cut = made;",
        integration: &[MOMENTS],
        unit: &[CASTS],
    },
    Mutation {
        name: "datetime-cast-shown-year-unjudged",
        package: Package::Core,
        breaks: "a timestamp the session shows in the year before 0001 is cast by batch, and no error is raised",
        file: DATETIMES,
        original: "self.times.1 || (within(cut) && within(cut.saturating_add(shift(carried))))",
        replacement: "self.times.1 || within(cut)",
        integration: &[MOMENTS],
        unit: &[SLOTS, CASTS],
    },
    Mutation {
        name: "datetime-cast-nulls-judged",
        package: Package::Core,
        breaks: "a null's slot past the year 9999 sends its batch's cast to the rows",
        file: DATETIMES,
        original: ".all(|(n, &utc)| nulls.get(n) || cast.row(utc, zone(n)).2)",
        replacement: ".all(|(n, &utc)| cast.row(utc, zone(n)).2)",
        integration: &[],
        unit: &[SLOTS],
    },
    Mutation {
        name: "datetime-cast-called-by-rows",
        package: Package::Core,
        breaks: "a cast of a vector of datetimes has no body, so EXTRACT of a timestamp as shown is called a row at a time",
        file: "src/binder/datetime.rs",
        original: "datetimes::cast(given, fields(&self.to)?, self.now).map(",
        replacement: "datetimes::cast(given, fields(&self.to)?, self.now).filter(|_| false).map(",
        integration: &[],
        unit: &["binder::datetime::tests::extract_of_timestamps_as_shown_is_computed_over_vectors"],
    },
    Mutation {
        name: "datetime-cast-rule-of-no-day",
        package: Package::Core,
        breaks: "a vector of timestamps cast to dates keeps each one's time of day, where a row's cast gives its midnight (RULES.md D1341)",
        file: DATETIMES,
        original: "Rule::Day => self.each(datetimes, |local, _, _| (midnight_of(local), 0)),",
        replacement: "Rule::Day => self.each(datetimes, |local, _, _| (local, 0)),",
        integration: &[MOMENTS],
        unit: &[RULES, CASTS],
    },
    Mutation {
        name: "datetime-cast-fraction-of-another-digit",
        package: Package::Core,
        breaks: "a cast to three digits of the second's fraction keeps a fourth (RULES.md D1341)",
        file: DATETIMES,
        original: "            3 => floor_rem(made, 1_000),",
        replacement: "            3 => floor_rem(made, 100),",
        integration: &[],
        unit: &[RULES, CASTS],
    },
];
