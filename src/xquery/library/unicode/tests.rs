//! Tests of the Unicode table of the functions on strings.

use super::*;

/// Verifies: [DESIGN] RULES.md D3 the table is the one generated, by its
/// digest, and its sections are what its header says.
#[test]
fn the_table_is_the_one_generated() {
    assert_eq!(digest(), DIGEST, "regenerate with cargo xtask unicode and update DIGEST");
    assert!(Table::read().is_ok());
}

/// Verifies: [DESIGN] \[UAX #15 §1.3\] [XQueryFO §7.4.7, §7.4.8] the table
/// gives classes, full decompositions of either kind, full case mappings
/// and primary composites, of Unicode 13.0; nothing of a character that
/// maps to itself.
#[test]
fn the_table_reads_back() {
    let table = Table::read().unwrap();
    assert_eq!((table.class('\u{301}'), table.class('\u{316}'), table.class('a')), (230, 220, 0));
    assert_eq!(table.mapped('\u{1E0A}', Map::Canonical), Some(vec!['D', '\u{307}']));
    assert_eq!(table.mapped('\u{1E9B}', Map::Canonical), Some(vec!['\u{17F}', '\u{307}']));
    assert_eq!(table.mapped('\u{1E9B}', Map::Compatible), Some(vec!['s', '\u{307}']));
    assert_eq!(table.mapped('\u{FB01}', Map::Canonical), None);
    assert_eq!(table.mapped('a', Map::Canonical), None);
    assert_eq!(table.mapped('ß', Map::Upper), Some(vec!['S', 'S']));
    assert_eq!(table.mapped('A', Map::Lower), Some(vec!['a']));
    assert_eq!(table.mapped('1', Map::Upper), None);
    assert_eq!(table.composite('A', '\u{30A}'), Some('\u{C5}'));
    assert_eq!(table.composite('\u{915}', '\u{93C}'), None);
    assert_eq!(table.composite('a', 'b'), None);
}
