//! Unit tests of the pages a B-link tree takes, counted (`pages.rs`).

use super::{BLinkTree, fill, tree};

/// The pages `tree` frees as it is freed: every page it takes.
fn freed(tree: &BLinkTree) -> u64 {
    let before = tree.pool.pages_in_use(tree.file).unwrap();
    tree.free().unwrap();
    before.saturating_sub(tree.pool.pages_in_use(tree.file).unwrap())
}

/// Verifies: [DESIGN] RULES.md D1165, the pages a tree takes, its leaves
/// counted as the level above names them, none read: those its freeing
/// frees, of a tree of one leaf, of two levels and of three.
#[test]
fn a_trees_pages_are_those_it_frees() {
    for (n, levels) in [(10_u64, 1_u8), (300, 2), (20_000, 3)] {
        let (tree, _scratch) = tree(&format!("pages-{n}"));
        fill(&tree, n);
        assert_eq!(tree.height().unwrap(), levels, "{n} keys");
        let pages = tree.pages().unwrap();
        assert_eq!(pages, freed(&tree), "{n} keys");
    }
}
