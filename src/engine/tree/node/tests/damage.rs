//! Robustness tests of the B-link node format: a node with any byte
//! damaged is refused by whatever reads or changes it, or read as some
//! node, never a panic.

use super::*;

/// That `r` is a value, or the error of a damaged node, SQLCODE -916.
fn corrupt_or_ok<T>(r: Result<T>, at: usize) {
    if let Err(e) = r {
        assert_eq!(e.code, Code::Corrupt, "byte {at}: {e:?}");
    }
}

/// Every reader and writer of a node run on `content`, each error checked
/// to be corruption.
fn exercise(content: &[u8], at: usize) {
    let Ok(node) = Node::parse(content) else { return };
    for i in 0..=node.len() {
        corrupt_or_ok(node.entry(i), at);
        corrupt_or_ok(node.key(i), at);
        corrupt_or_ok(node.child_at(i), at);
    }
    corrupt_or_ok(node.search(b"c"), at);
    corrupt_or_ok(node.child(b"c"), at);
    corrupt_or_ok(node.last_insert(), at);
    corrupt_or_ok(node.entries(), at);
    let _ = (node.live(), node.covers(b"c"), node.first_child(), node.shape());
    corrupt_or_ok(compact(content), at);
    let mut changed = content.to_vec();
    corrupt_or_ok(set_child(&mut changed, 0, PageId(4)), at);
    corrupt_or_ok(overwrite(&mut changed, 0, b"9"), at);
    corrupt_or_ok(insert(&mut changed, 1, b"bb", b"x"), at);
    corrupt_or_ok(remove(&mut changed, 0), at);
    corrupt_or_ok(set_right(&mut changed, PageId(8)), at);
}

/// Verifies: [DESIGN] RULES.md D31, B-link node, R-ERR-2: a leaf and an
/// inner node with any one byte set to 0x00, 0x7F or 0xFF are refused
/// with SQLCODE -916 by every reader and writer, or read as some node;
/// none panics.
#[test]
fn damaged_nodes_are_refused_or_read() {
    let c5 = PageId(5).0.to_le_bytes();
    let c7 = PageId(7).0.to_le_bytes();
    let leaf =
        build(&Shape { level: 0, high: Some(b"m"), right: Some(PageId(9)), first: PageId(0) }, &[(b"b", b"2"), (b"d", b"44")]).unwrap();
    let inner = build(&Shape { level: 1, high: None, right: None, first: PageId(3) }, &[(b"f", &c5), (b"p", &c7)]).unwrap();
    for good in [leaf, inner] {
        exercise(&good, usize::MAX);
        for at in 0..good.len() {
            for byte in [0x00, 0x7F, 0xFF] {
                let mut bad = good.clone();
                bad[at] = byte;
                exercise(&bad, at);
            }
        }
    }
}

/// Verifies: [DESIGN] RULES.md D31, B-link node: a node cut short at any
/// byte is refused or read as some node, never a panic.
#[test]
fn shortened_nodes_are_refused_or_read() {
    let good = build(&Shape { level: 0, high: None, right: None, first: PageId(0) }, &[(b"a", b"1"), (b"b", b"22"), (b"c", b"")]).unwrap();
    for cut in 0..good.len() {
        exercise(&good[..cut], cut);
    }
}
