//! The mutations of sorts by normal keys (RULES.md R-TEST-9, D364, D365):
//! a string's padding, a null's byte under DESC, the bytes after a string
//! longer than its key, the place that keeps a sort stable, a digit a radix
//! sort skips, the tree of losers' tie, the number of a run's own layout,
//! and the peers a top-N passes over.

use super::super::{Mutation, Package};

/// Normal keys sort as the comparator does.
const AS_COMPARATOR: &str = "exec::sort::normal::tests::normal_keys_sort_as_the_comparator";

/// Mechanisms whose removal must fail the requirements of sorts by normal
/// keys.
pub const SORTS: [Mutation; 8] = [
    Mutation {
        name: "sort-padding-zero",
        package: Package::Core,
        breaks: "a string's key is padded with zeros, not spaces, so 'a' and 'a ' differ",
        file: "src/exec/sort/normal/encode.rs",
        original: "    head.fill(b' ');",
        replacement: "    head.fill(0);",
        integration: &[],
        unit: &["exec::sort::normal::tests::padding_null_bytes_and_widths", AS_COMPARATOR],
    },
    Mutation {
        name: "sort-null-byte-inverted",
        package: Package::Core,
        breaks: "a DESC key inverts its null's byte, so NULLS LAST puts nulls first",
        file: "src/exec/sort/normal/encode.rs",
        original: "*head = u8::from(null != field.order.nulls_first);",
        replacement: "*head = u8::from(null != field.order.nulls_first) ^ u8::from(field.order.descending);",
        integration: &[],
        unit: &["exec::sort::normal::tests::padding_null_bytes_and_widths", AS_COMPARATOR],
    },
    Mutation {
        name: "sort-cut-tail-kept",
        package: Package::Core,
        breaks: "the keys after a string longer than its key decide rows its whole value orders",
        file: "src/exec/sort/normal/encode.rs",
        original: "        tail.fill(0);\n",
        replacement: "",
        integration: &[],
        unit: &[AS_COMPARATOR, "exec::sort::merge::tests::merged_by_keys_as_compared_whole"],
    },
    Mutation {
        name: "sort-place-dropped",
        package: Package::Core,
        breaks: "a comparison sort of short keys breaks ties by no place, so is not stable",
        file: "src/exec/sort/normal/order.rs",
        original: "    pairs.sort_unstable();",
        replacement: "    pairs.sort_unstable_by_key(|p| std::cmp::Reverse(p.1));\n    pairs.sort_by_key(|p| p.0);",
        integration: &[],
        unit: &[AS_COMPARATOR, "exec::sort::normal::tests::radix_sorts_as_a_stable_sort"],
    },
    Mutation {
        name: "sort-radix-skips-all",
        package: Package::Core,
        breaks: "a radix sort skips every digit, so keys stay in the order they came",
        file: "src/exec/sort/normal/radix.rs",
        original: "if count.contains(&from.len()) {",
        replacement: "if count.iter().any(|&c| c <= from.len()) {",
        integration: &[],
        unit: &["exec::sort::normal::tests::radix_sorts_as_a_stable_sort"],
    },
    Mutation {
        name: "sort-losers-tie-later",
        package: Package::Core,
        breaks: "the merge's tie goes to the later run, so rows of equal keys lose their order",
        file: "src/exec/sort/merge.rs",
        original: "Some((x, Some(y))) => compared(keys, x, y).then(a.cmp(&b)).is_lt(),",
        replacement: "Some((x, Some(y))) => compared(keys, x, y).then(b.cmp(&a)).is_lt(),",
        integration: &["spilled_sorts::"],
        unit: &["exec::sort::merge::tests::merged_by_keys_as_compared_whole"],
    },
    Mutation {
        name: "sort-run-layout-shared",
        package: Package::Core,
        breaks: "a run the shared layout does not admit takes its number, so the merge compares keys of two layouts",
        file: "src/exec/sort/keyed.rs",
        original: "        Some((layout, self.next))",
        replacement: "        Some((layout, SHARED))",
        integration: &[],
        unit: &["exec::sort::normal::tests::vectors_and_open_layouts_sort_as_the_comparator"],
    },
    Mutation {
        name: "sort-top-peers-passed",
        package: Package::Core,
        breaks: "a top-N WITH TIES passes over a row whose key equals the greatest held's",
        file: "src/exec/sort/top/batched.rs",
        original: "if by_key.is_gt() || (by_key.is_eq() && !truncates && !self.most.ties) {",
        replacement: "if by_key.is_gt() || (by_key.is_eq() && !truncates) {",
        integration: &[],
        unit: &["exec::sort::top::tests::the_first_rows_of_batches_are_a_whole_sorts"],
    },
];
