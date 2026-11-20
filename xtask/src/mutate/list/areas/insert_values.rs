//! The mutations of an INSERT's rows (RULES.md R-TEST-9, D390, D391): a
//! `?` typed by another row's literal, not its object column, and a `?`
//! cut by its cast without the warning, bound as a constant, as a CALL's
//! argument is, or filled in a kept plan's slot.

use super::super::{Mutation, Package};

/// Mechanisms whose removal must fail the requirements of an INSERT's rows.
pub const INSERT_VALUES: [Mutation; 3] = [
    Mutation {
        name: "insert-parameter-literal-typed",
        package: Package::Core,
        breaks: "a `?` in a row of an INSERT's VALUES takes another row's literal's type, so its value is cut [§20.7 GR5a xix 1 A]",
        file: "src/binder/set_operations.rs",
        original: "            if let Some(object) = objects.and_then(|o| o.get(k)) {",
        replacement: "            if let Some(object) = objects.and_then(|o| o.get(k)).filter(|_| deciding(rows, k).is_none()) {",
        integration: &[
            "insert_values::a_parameter_takes_its_object_columns_type_20_7",
            "prepared::prepared_writes_take_their_columns_types",
        ],
        unit: &[],
    },
    Mutation {
        name: "insert-parameter-cut-unwarned",
        package: Package::Core,
        breaks: "a `?` bound as a constant and cut by its cast raises no 01004 [§20.11 GR6c ii 2 B]",
        file: "src/binder/expression.rs",
        original: "Expr::Dynamic(Slot { index, data_type: t.clone(), at }) } else { warned(value, warning) };",
        replacement: "Expr::Dynamic(Slot { index, data_type: t.clone(), at }) } else { warned(value, warning.filter(|_| false)) };",
        integration: &["insert_values::a_parameter_cut_by_its_cast_is_warned_of_20_11"],
        unit: &[],
    },
    Mutation {
        name: "insert-slot-cut-unwarned",
        package: Package::Core,
        breaks: "a kept plan's slot filled with a value its cast cut raises no 01004 [§20.11 GR6c ii 2 B]",
        file: "src/logical/dynamic.rs",
        original: "            Ok((v, warning)) => warned(v, warning),",
        replacement: "            Ok((v, warning)) => warned(v, warning.filter(|_| false)),",
        integration: &["insert_values::a_parameter_cut_by_its_cast_is_warned_of_20_11"],
        unit: &[],
    },
];
