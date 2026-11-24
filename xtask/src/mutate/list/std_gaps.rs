//! The mutations of the gaps a benchmark's workloads found (RULES.md
//! R-TEST-9, D330 to D336): LOCALTIME and LOCALTIMESTAMP, ABS, MOD, the
//! trigonometric and logarithm functions, a prepared CALL's output
//! dynamic parameters, the database's counts of transactions ended, and
//! a statement cancelled once its client is gone.

use super::{Mutation, Package};

/// The test of MOD.
const MODULUS: &str = "modulus_and_trigonometry::modulus_6_31";
/// The test of LOG and LOG10.
const LOGARITHMS: &str = "modulus_and_trigonometry::logarithms_6_31";
/// The test of a prepared CALL's outputs.
const OUTPUTS: &str = "call_outputs::a_prepared_call_gives_back_its_outputs_20_7";

/// The mutations of the gaps, in the order they run.
pub const STD_GAPS: [Mutation; 13] = [
    Mutation {
        name: "std-gaps-localtime-as-current",
        package: Package::Core,
        breaks: "LOCALTIMESTAMP is CURRENT_TIMESTAMP, with its time zone",
        file: "src/binder/datetime.rs",
        original: "let value = if f.local { current_local(f.moment, precision, now) } else { current(f.moment, precision, now) };",
        replacement: "let value = current(f.moment, precision, now);",
        integration: &["local_datetimes::local_datetimes_6_43"],
        unit: &[],
    },
    Mutation {
        name: "std-gaps-localtime-alone",
        package: Package::Core,
        breaks: "LOCALTIME standing alone is read as a column",
        file: "src/parser/primary.rs",
        original: "if self.at_local_datetime().is_some() {",
        replacement: "if false {",
        integration: &["local_datetimes::local_datetimes_6_43"],
        unit: &[],
    },
    Mutation {
        name: "std-gaps-abs-unsigned",
        package: Package::Core,
        breaks: "ABS of a negative exact number is the number",
        file: "src/functions/numeric.rs",
        original: "Ok(Value::Exact(Decimal::new(units.saturating_neg(), *scale)))",
        replacement: "Ok(v.clone())",
        integration: &["modulus_and_trigonometry::absolute_value_6_31"],
        unit: &[],
    },
    Mutation {
        name: "std-gaps-mod-by-zero",
        package: Package::Core,
        breaks: "MOD by 0 raises no 22012",
        file: "src/functions/numeric/modulus.rs",
        original: "if sign(m)? == Ordering::Equal {",
        replacement: "if false {",
        integration: &[MODULUS],
        unit: &[],
    },
    Mutation {
        name: "std-gaps-mod-sign",
        package: Package::Core,
        breaks: "a DECFLOAT remainder takes no sign of its dividend",
        file: "src/functions/numeric/modulus.rs",
        original: "if n.coefficient() < 0 { r.checked_neg()? } else { r }",
        replacement: "r",
        integration: &[MODULUS],
        unit: &[],
    },
    Mutation {
        name: "std-gaps-mod-type",
        package: Package::Core,
        breaks: "MOD of exact numbers is DECFLOAT, not of the divisor's type",
        file: "src/functions/numeric.rs",
        original: "Ok(if decimal { DECFLOAT } else { divisor })",
        replacement: "Ok(if decimal { divisor } else { DECFLOAT })",
        integration: &[MODULUS],
        unit: &[],
    },
    Mutation {
        name: "std-gaps-log-base-zero",
        package: Package::Core,
        breaks: "LOG of a base of 0 or below raises no 22003",
        file: "src/functions/numeric/trig.rs",
        original: "if sign(base)? != Ordering::Greater || equals(base, 1) {",
        replacement: "if equals(base, 1) {",
        integration: &[LOGARITHMS],
        unit: &[],
    },
    Mutation {
        name: "std-gaps-call-output-lost",
        package: Package::Core,
        breaks: "a `?` of an output parameter keeps no value of it",
        file: "src/db/routines/call.rs",
        original: "params.set_dynamic(*index, out.get(*k).cloned().unwrap_or(Value::Null))?;",
        replacement: "params.set_dynamic(*index, Value::Null)?;",
        integration: &[OUTPUTS],
        unit: &[],
    },
    Mutation {
        name: "std-gaps-call-output-an-input",
        package: Package::Core,
        breaks: "a `?` of an OUT parameter is described as an input",
        file: "src/db/prepared.rs",
        original: ".filter(|p| p.mode != ParameterMode::Out)",
        replacement: ".filter(|_| true)",
        integration: &[OUTPUTS],
        unit: &[],
    },
    Mutation {
        name: "std-gaps-call-output-no-result",
        package: Package::Core,
        breaks: "EXECUTE of a CALL of output dynamic parameters takes no result using clause",
        file: "src/db/dynamic/statements.rs",
        original: "} else if prepared.has_outputs() {",
        replacement: "} else if false {",
        integration: &["call_outputs::execute_puts_the_outputs_where_its_result_clause_says_20_13"],
        unit: &[],
    },
    Mutation {
        name: "std-gaps-call-output-unsent",
        package: Package::Server,
        breaks: "a prepared CALL's outputs are not sent as a row",
        file: "server/src/native/requests.rs",
        original: "            if prepared.has_outputs() {",
        replacement: "            if false {",
        integration: &["prepared::a_prepared_call_sends_its_outputs_as_a_row"],
        unit: &[],
    },
    Mutation {
        name: "std-gaps-transactions-uncounted",
        package: Package::Server,
        breaks: "a database counts no transaction ended",
        file: "src/db/database/ended.rs",
        original: "count.fetch_add(1, Ordering::Relaxed);",
        replacement: "drop(count.load(Ordering::Relaxed));",
        integration: &["shown::a_server_counts_the_transactions_ended"],
        unit: &[],
    },
    Mutation {
        name: "std-gaps-client-gone-unwatched",
        package: Package::Server,
        breaks: "a statement whose client is gone runs on, its locks held",
        file: "server/src/native.rs",
        original: "lent = stopped(slot.take(), present, watch).await;",
        replacement: "lent = stopped(slot.take(), present, None).await;",
        integration: &["ending::a_statement_stops_once_its_client_is_gone"],
        unit: &[],
    },
];
