//! The mutations of functional dependencies (RULES.md R-TEST-9): a
//! deferrable or nullable key taken, an outer join's extended operand or a
//! UNION kept whole, a WHERE equality dropped, a collation or a COALESCE
//! misjudged, grouping sets checked of one set, a dependent column's
//! nullability or its constraint's refusal lost, and groups of no rows
//! taken as rows; and of candidate keys, a product's or a condition's keys
//! misjudged, a UNIQUE, grouping columns or an ordinality column not
//! keys, EXCEPT forgetting, the left-most rule or minimality broken, and
//! KEY_MEMBER unset; and of what depends on a constraint dropped, a view,
//! a CHECK, a domain constraint or an assertion that needs it kept, a
//! CHECK refused but not dropped with CASCADE, or one that bound neither
//! before nor after taken as dependent.

use super::{Mutation, Package};

/// The file of what depends on a constraint dropped.
const DEPENDING: &str = "src/db/alter/depending.rs";
/// The test of a CHECK that needs a key.
const CHECK_DEPENDS: &str = "dependencies_drops::a_check_that_needs_a_key_is_dropped_with_it";
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
/// The file of the candidate key search.
const KEYS_SEARCH: &str = "src/binder/dependencies/keys.rs";
/// The unit test of the left-most rule.
const LEFT_MOST: &str = "binder::dependencies::tests::the_left_most_rule_orders_sets";
/// The unit test of strong candidate keys.
const STRONG: &str = "binder::dependencies::tests::a_strong_candidate_key_is_preferred";
/// The unit test of the search.
const SEARCH: &str = "binder::dependencies::tests::the_search_finds_the_left_most_candidate_key";
/// The test of a base table's keys described.
const KEY_BASE: &str = "candidate_keys::a_base_tables_keys_are_described_4_27";
/// The test of the deduction rule.
const KEY_WITHIN: &str = "candidate_keys::a_candidate_key_within_a_key_is_one_4_27";
/// The test of joins' keys.
const KEY_JOINS: &str = "candidate_keys::a_joins_keys_are_described_4_26_7";
/// The test of grouped tables' keys.
const KEY_GROUPS: &str = "candidate_keys::a_grouped_tables_keys_are_described_4_26_13";
/// The test of query expressions' keys.
const KEY_QUERIES: &str = "candidate_keys::queries_keys_are_described_4_26_16";
/// The test of constraints.
const CONSTRAINT: &str = "dependencies::a_constraint_names_a_dependent_number_alone_6_7";

/// The mutations of functional dependencies, in the order they run.
pub const DEPENDENCIES: [Mutation; 30] = [
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
    Mutation {
        name: "key-product-unpruned",
        package: Package::Core,
        breaks: "a product keeps a table reference's key that does not determine every column, so a column of repeated values is described as a key [§4.26.11]",
        file: ENGINE,
        original: ".filter(|(key, _)| (!not_null || key.is_subset(&self.not_null)) && visible.is_subset(&self.closure(key.clone())))",
        replacement: ".filter(|(key, _)| !not_null || key.is_subset(&self.not_null))",
        integration: &[KEY_JOINS],
        unit: &[],
    },
    Mutation {
        name: "key-pending-not-adopted",
        package: Package::Core,
        breaks: "a condition over a product makes none of its table references' keys its own [§4.26.12]",
        file: ENGINE,
        original: "        self.keys.extend(pending);\n        self.keyed(false)",
        replacement: "        drop(pending);\n        self.keyed(false)",
        integration: &[KEY_JOINS],
        unit: &[],
    },
    Mutation {
        name: "key-unique-not-buc",
        package: Package::Core,
        breaks: "a UNIQUE of columns not null is no BUC-set, so a candidate key left of it is preferred [§4.26.3, §4.27]",
        file: TABLES,
        original: "            known.keys.push((key, *primary));",
        replacement: "            known.keys.extend((*primary).then_some((key, true)));",
        integration: &[KEY_BASE],
        unit: &[],
    },
    Mutation {
        name: "key-grouping-subset-exact",
        package: Package::Core,
        breaks: "grouping columns a proper subset of a BPK-set are no BPK-set [§4.26.13]",
        file: ENGINE,
        original: "*primary && g.is_subset(key)",
        replacement: "*primary && g == *key",
        integration: &[KEY_GROUPS],
        unit: &[],
    },
    Mutation {
        name: "key-ordinality-not-primary",
        package: Package::Core,
        breaks: "an ordinality column is no BPK-set of its collection derived table [§4.26.8]",
        file: ENGINE,
        original: ".map(|&o| (Attrs::from([o]), true))",
        replacement: ".map(|&o| (Attrs::from([o]), false))",
        integration: &[KEY_QUERIES],
        unit: &[],
    },
    Mutation {
        name: "key-except-forgets",
        package: Package::Core,
        breaks: "EXCEPT keeps nothing of its first operand [§4.26.16]",
        file: ENGINE,
        original: "SetOp::Except => Known { grouping: None, ..known(w, left) },",
        replacement: "SetOp::Except => w.fresh(width),",
        integration: &[KEY_QUERIES],
        unit: &[],
    },
    Mutation {
        name: "key-every-key-primary",
        package: Package::Core,
        breaks: "a BUC-set is taken as a BPK-set, so a UNIQUE is described as a primary key [§4.27]",
        file: KEYS_SEARCH,
        original: ".filter(|(_, primary)| *primary).map(",
        replacement: ".filter(|(_, primary)| *primary || !*primary).map(",
        integration: &[KEY_BASE],
        unit: &[],
    },
    Mutation {
        name: "key-grouping-not-strong",
        package: Package::Core,
        breaks: "a grouped table's grouping columns are no strong candidate key [§4.27]",
        file: KEYS_SEARCH,
        original: "        strong.extend(&self.grouping);\n",
        replacement: "",
        integration: &[],
        unit: &[STRONG],
    },
    Mutation {
        name: "key-right-most",
        package: Package::Core,
        breaks: "of two candidate keys the right-most is chosen [§4.27]",
        file: KEYS_SEARCH,
        original: "differs.is_some_and(|i| number(a, i) < number(b, i))",
        replacement: "differs.is_some_and(|i| number(a, i) > number(b, i))",
        integration: &[KEY_BASE],
        unit: &[LEFT_MOST],
    },
    Mutation {
        name: "key-not-minimal",
        package: Package::Core,
        breaks: "a set that determines every column is taken as a candidate key though a proper subset does too [§4.27]",
        file: KEYS_SEARCH,
        original: "            !self.superkey(&less)",
        replacement: "            less.len() < positions.len()",
        integration: &[KEY_WITHIN],
        unit: &[SEARCH],
    },
    Mutation {
        name: "key-member-unset",
        package: Package::Core,
        breaks: "no column is described a member of the key KEY_TYPE says [§20.10 GR7c i 3]",
        file: "src/db/dynamic/statements.rs",
        original: "i64::from(members.contains(&at))",
        replacement: "i64::from(members.is_empty() && at > 0)",
        integration: &[KEY_BASE],
        unit: &[],
    },
    Mutation {
        name: "dependent-view-kept",
        package: Package::Core,
        breaks: "a view whose grouped query needs a constraint dropped is kept, not refused or dropped [§11.26 SR13c, GR3]",
        file: DEPENDING,
        original: "is_some_and(|v| binds(before, v) && !binds(self, v))",
        replacement: "is_some_and(|v| binds(before, v) && !binds(self, v) && v.columns.is_empty())",
        integration: &[
            "dependencies_drops::a_view_that_needs_a_key_is_dropped_with_it",
            "dependencies_drops::a_view_that_needs_a_not_null_depends_on_it",
        ],
        unit: &[],
    },
    Mutation {
        name: "dependent-check-kept",
        package: Package::Core,
        breaks: "a CHECK whose query needs a constraint dropped is kept, not refused or dropped [§11.26 SR13a, GR1]",
        file: DEPENDING,
        original: "if c.domain.is_none() && reads.contains(qn) && binds(before) && !binds(self) {",
        replacement: "if c.domain.is_none() && reads.contains(qn) && binds(before) && !binds(self) && names.is_empty() {",
        integration: &[CHECK_DEPENDS],
        unit: &[],
    },
    Mutation {
        name: "dependent-check-not-dropped",
        package: Package::Core,
        breaks: "a CHECK that needs a constraint dropped with CASCADE stays on its table [§11.26 GR1]",
        file: DEPENDING,
        original: "                dropped.absorb(self.drop_constraint((table, &table.auth), &written(c), DropBehavior::Cascade)?);\n",
        replacement: "",
        integration: &[CHECK_DEPENDS],
        unit: &[],
    },
    Mutation {
        name: "dependent-domain-constraint-kept",
        package: Package::Core,
        breaks: "a domain constraint whose query needs a constraint dropped is kept, not refused or dropped [§11.26 SR13e]",
        file: DEPENDING,
        original: "if reads.contains(qn) && binds(before) && !binds(self) {",
        replacement: "if reads.contains(qn) && binds(before) && !binds(self) && names.is_empty() {",
        integration: &["dependencies_drops::a_domain_constraint_that_needs_a_key_is_dropped_with_it"],
        unit: &[],
    },
    Mutation {
        name: "dependent-assertion-kept",
        package: Package::Core,
        breaks: "an assertion whose query needs a constraint dropped is kept, not refused or dropped [§11.26 SR13e, GR4]",
        file: DEPENDING,
        original: "before.assertion_plan(a).is_ok() && self.assertion_plan(a).is_err()",
        replacement: "before.assertion_plan(a).is_ok() && self.assertion_plan(a).is_err() && a.name.ident.is_empty()",
        integration: &["dependencies_drops::an_assertion_that_needs_a_key_is_dropped_with_it"],
        unit: &[],
    },
    Mutation {
        name: "dependent-bound-before-unasked",
        package: Package::Core,
        breaks: "an assertion that a REVOKE abandons as it drops a CHECK is taken as dependent on the CHECK, dropped before the REVOKE drops it",
        file: DEPENDING,
        original: "reading(&a.kind) && before.assertion_plan(a).is_ok() && ",
        replacement: "reading(&a.kind) && ",
        integration: &["domains::revoking_usage_abandons_what_casts_to_the_domain_11_37"],
        unit: &[],
    },
];
