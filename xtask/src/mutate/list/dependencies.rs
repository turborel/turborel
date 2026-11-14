//! The mutations of functional dependencies (RULES.md R-TEST-9): a
//! deferrable or nullable key taken, an outer join's extended operand or a
//! UNION kept whole, a WHERE equality dropped, a collation or a COALESCE
//! misjudged, grouping sets checked of one set, a dependent column's
//! nullability or its constraint's refusal lost, and groups of no rows
//! taken as rows.

use super::{Mutation, Package};

/// The file of a base table's dependencies.
const TABLES: &str = "src/binder/dependencies/tables.rs";
/// The file of conditions' and outer joins' dependencies.
const CONDITIONS: &str = "src/binder/dependencies/conditions.rs";
/// The file of the rules of deduction and each plan's dependencies.
const ENGINE: &str = "src/binder/dependencies.rs";
/// The file of group-invariant columns.
const INVARIANT: &str = "src/binder/invariant.rs";
/// The unit test of a base table's keys.
const KEYS: &str = "binder::dependencies::tests::a_base_tables_keys_determine_its_rows";
/// The unit test of equalities.
const EQUALITIES: &str = "binder::dependencies::tests::equalities_of_a_condition_determine_columns";
/// The unit test of outer joins.
const OUTER: &str = "binder::dependencies::tests::an_outer_join_keeps_what_nulls_cannot_break";
/// The unit test of projections and COALESCE.
const PROJECTION: &str = "binder::dependencies::tests::a_projection_keeps_and_computes";
/// The unit test of groups, set operations and collections.
const GROUPS: &str = "binder::dependencies::tests::groups_set_operations_and_collections";
/// The test of the refusals.
const REFUSED: &str = "dependencies::columns_no_grouping_column_determines_are_refused_6_7";
/// The test of WHERE's equalities.
const WHERE: &str = "dependencies::equalities_of_where_determine_columns_4_26_12";
/// The test of joins.
const JOINS: &str = "dependencies::joins_keep_and_add_dependencies_4_26_7";
/// The test of views, derived tables and set operations.
const QUERIES: &str = "dependencies::queries_read_keep_their_dependencies_4_26_16";
/// The test of constraints.
const CONSTRAINT: &str = "dependencies::a_constraint_names_a_dependent_number_alone_6_7";

/// The mutations of functional dependencies, in the order they run.
pub const DEPENDENCIES: [Mutation; 13] = [
    Mutation {
        name: "fd-deferrable-key-taken",
        package: Package::Core,
        breaks: "a deferrable PRIMARY KEY determines its table's columns, though it may not hold until commit [§4.26.3]",
        file: TABLES,
        original: "    for c in desc.constraints.iter().filter(|c| !c.attributes.deferrable) {\n        let ConstraintKind::Unique",
        replacement: "    for c in desc.constraints.iter() {\n        let ConstraintKind::Unique",
        integration: &[REFUSED],
        unit: &[KEYS],
    },
    Mutation {
        name: "fd-nullable-unique-taken",
        package: Package::Core,
        breaks: "a UNIQUE of a nullable column determines its table's columns [§4.26.3]",
        file: TABLES,
        original: "if !(*primary || !*nulls_distinct || columns.iter().all(|k| not_null.contains(k))) {",
        replacement: "if !(*primary || !*nulls_distinct || true) {",
        integration: &[REFUSED],
        unit: &[KEYS],
    },
    Mutation {
        name: "fd-right-join-extended-kept",
        package: Package::Core,
        breaks: "a RIGHT join keeps every dependency of the operand it extends with nulls [§4.26.7]",
        file: CONDITIONS,
        original: "OuterJoin::Right => left.null_extended().beside(right),",
        replacement: "OuterJoin::Right => left.beside(right),",
        integration: &[JOINS],
        unit: &[OUTER],
    },
    Mutation {
        name: "fd-extended-not-null-ignored",
        package: Package::Core,
        breaks: "of an operand extended with nulls, a dependency of a determinant of no column known not null is kept [§4.26.7]",
        file: CONDITIONS,
        original: "a.is_subset(&visible) && b.is_subset(&visible) && a.iter().any(|x| not_null.contains(x))",
        replacement: "a.is_subset(&visible) && b.is_subset(&visible)",
        integration: &[JOINS],
        unit: &[OUTER],
    },
    Mutation {
        name: "fd-where-equality-dropped",
        package: Package::Core,
        breaks: "an equality AND-component of WHERE determines nothing [§4.26.12]",
        file: CONDITIONS,
        original: "                    if *op == CompOp::Eq {",
        replacement: "                    if *op == CompOp::Eq && false {",
        integration: &[WHERE],
        unit: &[EQUALITIES],
    },
    Mutation {
        name: "fd-collation-ignored",
        package: Package::Core,
        breaks: "a column compared by a collation not its own is taken as of one value [§4.26.12]",
        file: CONDITIONS,
        original: "self.collations.iter().any(|(at, own)| *at == attribute && own == by).then_some(*c)",
        replacement: "Some(*c)",
        integration: &[WHERE],
        unit: &[],
    },
    Mutation {
        name: "fd-coalesce-of-nullable-alike",
        package: Package::Core,
        breaks: "a COALESCE of a nullable column is taken as alike it, though it may be the other's value [§4.26.7, §4.26.15]",
        file: ENGINE,
        original: "let alike = tested == x && (self.not_null.contains(&ax) || self.nulled_with.contains(&(ax, ay)));",
        replacement: "let alike = tested == x && (ay == ay || ax == ay);",
        integration: &[JOINS],
        unit: &[PROJECTION],
    },
    Mutation {
        name: "fd-union-keeps-first",
        package: Package::Core,
        breaks: "a UNION keeps its first operand's dependencies, though its rows are of either [§4.26.16]",
        file: ENGINE,
        original: "SetOp::Union => w.fresh(width),",
        replacement: "SetOp::Union => known(w, left).widened(w, width),",
        integration: &[QUERIES],
        unit: &[GROUPS],
    },
    Mutation {
        name: "fd-empty-groups-take-rows",
        package: Package::Core,
        breaks: "what is known of the rows holds of groups of no rows made before any row [§4.26.13, §7.13 GR2a]",
        file: ENGINE,
        original: "let mut known = if rowed {",
        replacement: "let mut known = if true {",
        integration: &[],
        unit: &[GROUPS],
    },
    Mutation {
        name: "fd-one-grouping-set",
        package: Package::Core,
        breaks: "a column dependent on one grouping set's columns is taken, though another set's groups may hold rows of other values [§6.7 SR8]",
        file: INVARIANT,
        original: "sets.iter().all(|s| known.determines(s, pos))",
        replacement: "sets.iter().any(|s| known.determines(s, pos))",
        integration: &[REFUSED],
        unit: &[],
    },
    Mutation {
        name: "fd-collated-grouping-determines",
        package: Package::Core,
        breaks: "a grouping column written with COLLATE determines columns, though its group holds values it does not tell apart [§6.7 SR8]",
        file: "src/binder/grouping_sets.rs",
        original: "set.iter().filter(|g| g.collate.is_none()).map(",
        replacement: "set.iter().map(",
        integration: &[REFUSED],
        unit: &[],
    },
    Mutation {
        name: "fd-empty-group-not-null",
        package: Package::Core,
        breaks: "a dependent column of a group that may have no rows is described as not null, though it is null there [§7.13 GR2a]",
        file: "src/binder/query.rs",
        original: " || (!g.groups_by(pos) && g.may_be_empty()),",
        replacement: ",",
        integration: &[WHERE],
        unit: &[],
    },
    Mutation {
        name: "fd-constraint-chosen-value-taken",
        package: Package::Core,
        breaks: "a constraint names a dependent character string column, whose value of its group is chosen (UV070) [§6.7 GR1e]",
        file: INVARIANT,
        original: "if self.purpose.constrains() && chosen_of_group(t) {",
        replacement: "if self.purpose.constrains() && chosen_of_group(t) && false {",
        integration: &[CONSTRAINT],
        unit: &[],
    },
];
