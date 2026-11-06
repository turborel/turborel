//! The mutations of collations (RULES.md R-TEST-9, D60): what keeps an
//! index of sort keys honest, the order of a key, and what a DROP COLLATION
//! leaves when it fails.

use super::{Mutation, Package};

/// The mutations of collations, in the order they run.
pub const COLLATIONS: [Mutation; 8] = [
    Mutation {
        name: "collated-read-tracked",
        package: Package::Core,
        breaks: "MVCC reads: a probe of sort keys tells the engine it read what no row's value matches",
        file: "src/exec.rs",
        original: "    let probe = if ix.is_collated() { Probe::All } else { probe };",
        replacement: "    let probe = if ix.is_collated() && false { Probe::All } else { probe };",
        integration: &["collated_indexes::a_read_through_sort_keys_is_tracked_for_its_commit"],
        unit: &[],
    },
    Mutation {
        name: "collated-index-gate",
        package: Package::Core,
        breaks: "index choice: a comparison by one collation probes an index of another",
        file: "src/physical/probe.rs",
        original: "            (Some(a), Some(b)) => a.behaves_as(b),",
        replacement: "            (Some(_), Some(_)) => true,",
        integration: &["collated_indexes::an_index_of_sort_keys_is_probed_by_its_own_collation"],
        unit: &[],
    },
    Mutation {
        name: "collated-key",
        package: Package::Core,
        breaks: "an index's key: a row's key is its values, not their sort keys",
        file: "src/catalog/keys.rs",
        original: "    if collations.is_empty() {\n        return Key(values);\n    }",
        replacement: "    if true {\n        return Key(values);\n    }",
        integration: &["collations::unique_follows_the_collation_11_7", "collated_indexes::a_collation_survives_reopening"],
        unit: &[],
    },
    Mutation {
        name: "pad-low-tag",
        package: Package::Core,
        breaks: "a PAD SPACE key: spaces before a character below a space sort as before one above",
        file: "src/value/collation/pad.rs",
        original: "        out.push(BEFORE_LOW);",
        replacement: "        out.push(BEFORE_HIGH);",
        integration: &[],
        unit: &["value::collation::tests::a_pad_space_key_orders_as_comparison_does"],
    },
    Mutation {
        name: "collated-foreign-key",
        package: Package::Core,
        breaks: "a foreign key: a row is matched by its values, not by the column's collation",
        file: "src/db/integrity/references.rs",
        original: "        let wanted = self.normal(table, columns, values)?;",
        replacement: "        let wanted = values.to_vec();",
        integration: &["collations::foreign_keys_follow_the_collation_9_15"],
        unit: &[],
    },
    Mutation {
        name: "drop-collation-refused-first",
        package: Package::Core,
        breaks: "DROP COLLATION: a refusal comes after its columns are reverted",
        file: "src/db/drop.rs",
        original: "            State::clone(st).drop_object(&target, cascade).or_at(object.at())?;\n",
        replacement: "",
        integration: &["collations::a_drop_collation_refused_leaves_no_effect"],
        unit: &[],
    },
    Mutation {
        name: "drop-collation-doomed",
        package: Package::Core,
        breaks: "DROP COLLATION: a failure past its first table leaves the transaction able to commit",
        file: "src/db/drop.rs",
        original: "                    tx.doom(\"a DROP COLLATION failed with a table done\");\n",
        replacement: "",
        integration: &["collations::a_drop_collation_failing_on_a_later_table_only_rolls_back"],
        unit: &[],
    },
    Mutation {
        name: "subquery-run-exact",
        package: Package::Core,
        breaks: "a subquery's kept rows: strings equal when padded share a run",
        file: "src/exec/subquery.rs",
        original: "        Value::Char(s) => NormKey::Char(s.clone()),",
        replacement: "        Value::Char(_) => v.norm(),",
        integration: &["collated_indexes::a_subquery_tells_apart_strings_equal_when_padded"],
        unit: &[],
    },
];
