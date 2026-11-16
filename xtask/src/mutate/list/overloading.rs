//! The mutations of overloading (RULES.md R-TEST-9, D73, D74). Of the
//! choice: an approximate parameter as near an exact argument as an exact
//! one, a `?` ending the choice, two alike taken, a `?` argument not typed
//! by its parameter, of a function or of CALL. Of definitions: a procedure
//! of a number of parameters another has, and a function or a distinct
//! type's function not told apart, taken. Of designators: a distinct
//! type's function overlooked. Of subject routines kept: a definition
//! choosing afresh, its routines not kept in the catalog, and what invokes
//! a routine not found from them. Of registered functions: one not told
//! apart taken, an argument not stored in its declared type, and the
//! refusal of a function of the schema winning over one registered, and a
//! registered aggregate hidden by a function of the schema of another
//! number of parameters; and the rule a refusal cites.

use super::{Mutation, Package};

/// The file of type precedence lists.
const PRECEDENCE: &str = "src/value/precedence.rs";
/// The file of the functions of a name, and the one an invocation invokes.
const CANDIDATES: &str = "src/binder/invocation/candidates.rs";
/// The file of CREATE and DROP of a routine.
const DEFINE: &str = "src/db/routines/define.rs";
/// The file of the contract of registered functions.
const FUNCTIONS: &str = "src/functions.rs";
/// The test of the choice by argument types.
const CHOSEN: &str = "routines_overloading::an_argument_type_chooses_the_function_9_18";
/// The test of a registered aggregate beside functions of its name.
const AGGREGATE: &str = "functions_overloading::a_registered_aggregate_is_hidden_only_by_a_function_of_its_arity";
/// The test of a view's subject routine kept.
const VIEW_KEPT: &str = "routines_fixed::a_view_keeps_its_subject_routine_4_35_2";

/// The mutations of overloading.
pub const OVERLOADING: [Mutation; 18] = [
    Mutation {
        name: "overloading-approximate-first",
        package: Package::Core,
        breaks: "FLOAT stands in NUMERIC's place of an exact type's list, so F(DOUBLE PRECISION) is not told apart from F(INTEGER)",
        file: PRECEDENCE,
        original: "exact.then_some(SECOND)",
        replacement: "exact.then_some(FIRST)",
        integration: &[CHOSEN],
        unit: &["value::precedence::tests::designators_take_the_places_of_their_list"],
    },
    Mutation {
        name: "overloading-dynamic-ends-choice",
        package: Package::Core,
        breaks: "a `?` argument ends the choice, so a later argument decides nothing",
        file: PRECEDENCE,
        original: "let Some(t) = arg else { continue };",
        replacement: "let Some(t) = arg else { break };",
        integration: &[],
        unit: &["value::precedence::tests::subject_routines_are_chosen_left_to_right_9_6"],
    },
    Mutation {
        name: "overloading-alike-taken",
        package: Package::Core,
        breaks: "of two functions that take the arguments alike, the first is invoked, not refused",
        file: CANDIDATES,
        original: "match subject_routines(&lists, args).as_slice() {",
        replacement: "match subject_routines(&lists, args).get(..1).unwrap_or_default() {",
        integration: &["routines_overloading::invocations_without_one_subject_are_refused_9_18"],
        unit: &[],
    },
    Mutation {
        name: "overloading-dynamic-untyped",
        package: Package::Core,
        breaks: "a `?` argument of a function is CHARACTER VARYING, not of its parameter's type",
        file: "src/binder/invocation.rs",
        original: "None => self.typed(arg, ParameterType::Of(p.data_type.clone()), scope)?,",
        replacement: "None => self.typed(arg, ParameterType::Varying, scope)?,",
        integration: &["routines_overloading::invocations_without_one_subject_are_refused_9_18"],
        unit: &[],
    },
    Mutation {
        name: "overloading-call-dynamic-untyped",
        package: Package::Core,
        breaks: "a `?` argument of CALL takes no type of its parameter",
        file: "src/binder/parameters.rs",
        original: "binder.typed(v, ParameterType::Of(parameter.clone()), scope)",
        replacement: "{ let _ = parameter; binder.value_expression(v, scope) }",
        integration: &["routines_overloading::procedures_are_told_apart_by_arity_11_60"],
        unit: &[],
    },
    Mutation {
        name: "overloading-procedure-arity-unchecked",
        package: Package::Core,
        breaks: "a procedure of a name and number of parameters another has is defined",
        file: DEFINE,
        original: "d.kind == RoutineKind::Procedure && d.parameters.len() == parameters.len();",
        replacement: "d.kind == RoutineKind::Procedure && d.parameters.len() == parameters.len() && false;",
        integration: &["routines_overloading::procedures_are_told_apart_by_arity_11_60"],
        unit: &[],
    },
    Mutation {
        name: "overloading-function-alike-defined",
        package: Package::Core,
        breaks: "a function not told apart from another of its name, F(BIGINT) beside F(INTEGER), is defined",
        file: DEFINE,
        original: "match alike(self, name, &types) {",
        replacement: "match alike(self, name, &types).filter(|_| false) {",
        integration: &["routines_overloading::functions_alike_are_refused_11_60"],
        unit: &[],
    },
    Mutation {
        name: "overloading-type-function-alike-defined",
        package: Package::Core,
        breaks: "CREATE TYPE makes a function not told apart from an SQL-invoked function of its name",
        file: "src/db/user_types.rs",
        original: "match alike(self, &named, std::slice::from_ref(parameter)) {",
        replacement: "match alike(self, &named, std::slice::from_ref(parameter)).filter(|_| false) {",
        integration: &["routines_overloading::a_distinct_type_function_is_an_overload_11_60"],
        unit: &[],
    },
    Mutation {
        name: "overloading-designator-overlooks-types",
        package: Package::Core,
        breaks: "a designator of types a distinct type's function has, beside an SQL-invoked function of its name, designates no routine",
        file: "src/db/routines/privileges.rs",
        original: "let casts = self.type_functions(&qn, (specific, Some(wanted)), types.as_deref());",
        replacement: "let casts: Vec<(UserTypeDesc, Routine)> = { let _ = &types; Vec::new() };",
        integration: &["routines_overloading::a_grant_designates_the_type_function_10_6"],
        unit: &[],
    },
    Mutation {
        name: "overloading-subjects-not-kept",
        package: Package::Core,
        breaks: "a definition bound again chooses among every function of the name, so one created later takes over",
        file: CANDIDATES,
        original: "if self.params.horizoned() { self.params.horizon() } else { self.horizon }.subjects()",
        replacement: "{ let _ = self.horizon; None }",
        integration: &[
            VIEW_KEPT,
            "routines_fixed::a_check_keeps_its_subject_routine_4_35_2",
            "routines_fixed::a_routine_body_keeps_its_subject_routine_4_35_2",
        ],
        unit: &[],
    },
    Mutation {
        name: "overloading-subjects-not-logged",
        package: Package::Core,
        breaks: "the subject routines of a definition are lost as the catalog is read back, so reopened it chooses afresh",
        file: "src/catalog/codec.rs",
        original: "let routines = if flag(r)? { Some(get_all(r, qualified)?) } else { None };",
        replacement: "let routines = if flag(r)? { get_all(r, qualified)?; None } else { None };",
        integration: &["routines_fixed::subject_routines_are_kept_across_reopening_4_35_2"],
        unit: &[],
    },
    Mutation {
        name: "overloading-invokers-not-found",
        package: Package::Core,
        breaks: "DROP ROUTINE finds nothing that invokes the routine, so RESTRICT drops one a view invokes",
        file: "src/db/routines/dependents.rs",
        original: "let on = |h: &Horizon| h.subjects().is_some_and(|s| s.iter().any(|r| gone.contains(r)));",
        replacement: "let on = |h: &Horizon| { let _ = (h, gone); false };",
        integration: &["routines_fixed::a_drop_finds_the_subject_routine_11_62"],
        unit: &[],
    },
    Mutation {
        name: "overloading-registered-alike-taken",
        package: Package::Core,
        breaks: "a registered function not told apart from another of its name is registered",
        file: "src/functions/registry.rs",
        original: "if !told_apart(&mine, &theirs.iter().map(Vec::as_slice).collect::<Vec<_>>()) {",
        replacement: "if false && !told_apart(&mine, &theirs.iter().map(Vec::as_slice).collect::<Vec<_>>()) {",
        integration: &["functions_overloading::an_overload_not_told_apart_is_refused"],
        unit: &[],
    },
    Mutation {
        name: "overloading-registered-argument-not-stored",
        package: Package::Core,
        breaks: "an argument of a registered function is given as it is, not stored in its declared parameter type",
        file: FUNCTIONS,
        original: "self.parameter_types().and_then(|types| types.into_iter().nth(position))",
        replacement: "{ let _ = position; None }",
        integration: &["functions_overloading::an_argument_is_stored_in_its_declared_type"],
        unit: &[],
    },
    Mutation {
        name: "overloading-schema-refusal-wins",
        package: Package::Core,
        breaks: "a function of the schema that does not take the arguments refuses a call a registered function takes",
        file: "src/binder/expression.rs",
        original: "Some(refused) if schema.is_some() || registered.is_empty() || !extension => return Err(refused),",
        replacement: "Some(refused) => return Err(refused),",
        integration: &[
            "functions_overloading::a_schema_function_comes_before_a_registered_one",
            "functions_overloading::a_function_not_executable_leaves_the_call_to_a_registered_one",
        ],
        unit: &[],
    },
    Mutation {
        name: "overloading-arity-rule-swapped",
        package: Package::Core,
        breaks: "an invocation no function of its arity takes is refused citing SR10d, and one of the arity citing SR10a",
        file: CANDIDATES,
        original: "[] if !pool.iter().any(|c| c.parameters().len() == args.len()) => {",
        replacement: "[] if pool.iter().any(|c| c.parameters().len() == args.len()) => {",
        integration: &["routines_overloading::invocations_without_one_subject_are_refused_9_18"],
        unit: &[],
    },
    Mutation {
        name: "overloading-aggregate-any-arity",
        package: Package::Core,
        breaks: "a function of the schema of a registered aggregate's name hides it whatever its number of parameters",
        file: "src/binder/routines.rs",
        original: "named.iter().any(|f| f.parameters().len() == arity)",
        replacement: "{ let _ = arity; !named.is_empty() }",
        integration: &[AGGREGATE],
        unit: &[],
    },
    Mutation {
        name: "overloading-aggregate-bound-as-routine",
        package: Package::Core,
        breaks: "a call grouping reads as a registered aggregate's is bound to the schema's function of the name, and refused",
        file: "src/binder/expression.rs",
        original: "let passed = if is_aggregate_call(v, (self.db, self.schema)) {",
        replacement: "let passed = if is_aggregate_call(v, (self.db, self.schema)) && false {",
        integration: &[AGGREGATE],
        unit: &[],
    },
];
