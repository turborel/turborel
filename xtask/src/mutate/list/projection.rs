//! Faults in physical column composition and its evaluation boundaries.

use super::{Mutation, Package};

pub(super) const PROJECTION: [Mutation; 3] = [
    Mutation {
        name: "project-composition",
        package: Package::Core,
        breaks: "a reversed spatial join retains a full intermediate geometry projection",
        file: "src/physical/project.rs",
        original: "columns(&exprs, through)",
        replacement: "columns(&exprs, through).filter(|_| false)",
        integration: &["geometry::index::joins::projected::reversed_spatial_joins_select_the_final_columns_directly"],
        unit: &[],
    },
    Mutation {
        name: "project-computed-boundary",
        package: Package::Core,
        breaks: "column composition removes an unused expression that raises an error",
        file: "src/physical/project.rs",
        original: "through.iter().any(|expr| !matches!(expr, Expr::Column(_)))",
        replacement: "through.iter().any(|expr| !matches!(expr, Expr::Column(_))) && false",
        integration: &[],
        unit: &["physical::project::tests::computed_expressions_remain_evaluated"],
    },
    Mutation {
        name: "project-column-mapping",
        package: Package::Core,
        breaks: "column composition ignores the intermediate column mapping",
        file: "src/physical/project.rs",
        original: "through.get(*column).cloned()",
        replacement: "Some(Expr::Column(*column))",
        integration: &[],
        unit: &["physical::project::tests::column_composition_preserves_values_and_multiplicity"],
    },
];
