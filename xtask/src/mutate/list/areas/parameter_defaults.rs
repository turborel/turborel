//! The mutations of a procedure's parameter defaults (RULES.md R-TEST-9,
//! D979): a default read and kept, refused with a parameter in it,
//! checked and its names kept as the procedure is defined, a CALL's
//! arguments placed with defaults past them, the procedure of fewest
//! parameters invoked, DEFAULT taking the default of an IN or INOUT
//! parameter, and the default encoded and shown.

use super::super::{Mutation, Package};

/// The test of an IN parameter's default.
const IN_DEFAULT: &str = "parameter_defaults::an_in_parameter_takes_its_default_9_18";
/// The test of an INOUT parameter's default.
const INOUT_DEFAULT: &str = "parameter_defaults::an_inout_parameter_takes_its_default_9_18";
/// The test of the procedure of fewest parameters.
const FEWEST: &str = "parameter_defaults::a_call_invokes_the_procedure_of_fewest_parameters_9_18";
/// The test of a default checked as it is defined.
const CHECKED: &str = "parameter_defaults::a_default_is_checked_as_the_procedure_is_defined_11_60";
/// The test of what a default names.
const NAMES: &str = "parameter_defaults::what_a_default_names_is_kept_with_the_procedure_11_31";
/// The test of defaults shown and kept.
const KEPT: &str = "parameter_defaults::defaults_are_shown_and_kept_across_reopening_11_60";

/// Mechanisms whose removal must fail a procedure's parameter defaults.
pub const PARAMETER_DEFAULTS: [Mutation; 12] = [
    Mutation {
        name: "default-kept",
        package: Package::Core,
        breaks: "the parser reads a parameter's default and keeps none",
        file: "src/parser/routines.rs",
        original: "        let default = if self.is_kw(kw::DEFAULT) { self.parameter_default(kind, mode, &data_type)? } else { None };",
        replacement: "        let default = if self.is_kw(kw::DEFAULT) { self.parameter_default(kind, mode, &data_type).map(|_| None)? } else { None };",
        integration: &[IN_DEFAULT],
        unit: &["parser::routines::tests::a_procedure_parameter_has_a_default_11_60"],
    },
    Mutation {
        name: "default-no-parameter",
        package: Package::Core,
        breaks: "a `?` or host parameter in a default is refused by another rule than SR19",
        file: "src/parser/routines.rs",
        original: "        let value = value.map_err(|e| self.body_parameter_refusal(e))?;",
        replacement: "        let value = value?;",
        integration: &[],
        unit: &["parser::routines::tests::default_holds_no_parameter_11_60"],
    },
    Mutation {
        name: "default-trailing",
        package: Package::Core,
        breaks: "an argument left out past those written is taken as written, not as DEFAULT",
        file: "src/binder/named.rs",
        original: "(k < names.len()).then_some(k)",
        replacement: "Some(k)",
        integration: &[IN_DEFAULT],
        unit: &["binder::named::tests::named_arguments_take_their_parameters_places"],
    },
    Mutation {
        name: "default-required",
        package: Package::Core,
        breaks: "a parameter of no default may be left out",
        file: "src/binder/named.rs",
        original: "    places.iter().zip(parameters).all(|(place, (_, defaulted))| place.is_some() || *defaulted).then_some(places)",
        replacement: "    places.iter().zip(parameters).all(|(place, _)| place.is_some() || place.is_none()).then_some(places)",
        integration: &[IN_DEFAULT],
        unit: &["binder::named::tests::named_arguments_take_their_parameters_places"],
    },
    Mutation {
        name: "default-fewest",
        package: Package::Core,
        breaks: "of the procedures that take the arguments, the one of most parameters is invoked",
        file: "src/binder/invocation/resolve.rs",
        original: "    let fewest = |r: &&SqlRoutineDesc| (rank(r), r.parameters.len());",
        replacement: "    let fewest = |r: &&SqlRoutineDesc| (rank(r), usize::MAX - r.parameters.len());",
        integration: &[FEWEST],
        unit: &[],
    },
    Mutation {
        name: "default-value",
        package: Package::Core,
        breaks: "a parameter that takes DEFAULT is null, its default unread",
        file: "src/db/routines/defaults.rs",
        original: "    let Some(text) = &p.default else { return Ok(Expr::Const(Value::Null)) };",
        replacement: "    let Some(text) = None::<&String> else { return Ok(Expr::Const(Value::Null)) };",
        integration: &[IN_DEFAULT, INOUT_DEFAULT],
        unit: &[],
    },
    Mutation {
        name: "default-inout",
        package: Package::Core,
        breaks: "an INOUT parameter given DEFAULT is null, not its default",
        file: "src/db/routines/call.rs",
        original: "        (ParameterMode::In | ParameterMode::InOut, SqlArgument::Default(_), _) => default_input(cx, desc, p),",
        replacement: "        (ParameterMode::In | ParameterMode::InOut, SqlArgument::Default(_), _) => Ok(Expr::Const(Value::Null)),",
        integration: &[IN_DEFAULT, INOUT_DEFAULT],
        unit: &[],
    },
    Mutation {
        name: "default-checked",
        package: Package::Core,
        breaks: "a default no store assignment takes is defined",
        file: "src/db/routines/defaults.rs",
        original: "                bind_default(cx, p, &d.value)?;",
        replacement: "                drop(bind_default(cx, p, &d.value));",
        integration: &[CHECKED],
        unit: &[],
    },
    Mutation {
        name: "default-reads",
        package: Package::Core,
        breaks: "a table a default reads is dropped without CASCADE",
        file: "src/db/routines/body.rs",
        original: "            reads.push(t);",
        replacement: "            drop(t);",
        integration: &[NAMES],
        unit: &[],
    },
    Mutation {
        name: "default-invokes",
        package: Package::Core,
        breaks: "a function a default invokes is dropped without CASCADE",
        file: "src/db/routines/body.rs",
        original: "            invocations.push(s);",
        replacement: "            drop(s);",
        integration: &[NAMES],
        unit: &[],
    },
    Mutation {
        name: "default-encoded",
        package: Package::Core,
        breaks: "a parameter's default is not encoded, so lost on reopening",
        file: "src/catalog/codec/routines.rs",
        original: "    super::triggers::put_optional(out, p.default.as_deref());",
        replacement: "    super::triggers::put_optional(out, None);",
        integration: &[KEPT],
        unit: &["catalog::codec::routines::tests::each_kind_of_routine_reads_back_as_written"],
    },
    Mutation {
        name: "default-shown",
        package: Package::Core,
        breaks: "PARAMETERS shows no parameter's default",
        file: "src/system/definition/routines.rs",
        original: "            let default = p.default.as_deref().map_or(Value::Null, text);",
        replacement: "            let default = Value::Null;",
        integration: &[KEPT],
        unit: &[],
    },
];
