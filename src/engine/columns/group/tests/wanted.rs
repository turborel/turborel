//! Unit tests of headers read for some columns (`group/wanted.rs`).

use super::*;

/// The chunks of each column `group` holds, as text, `None` for one its
/// header was read without.
fn chunks(group: &Group) -> Vec<Option<String>> {
    (0..TYPES.len()).map(|c| group.zones(c).ok().flatten().map(|zones| format!("{zones:?}"))).collect()
}

/// Verifies: [DESIGN] RULES.md D1173: a header read for some columns holds
/// each of their chunks as a header read for every column does, and of the
/// others none, asked of which it fails rather than say it has no chunks;
/// of a column past its own, as one added after it was written, it has
/// none; what it takes in memory is less; it is no group whose pages or
/// summary can be told, as those need every column; a column added to
/// those it was read for, as a read again reads them, is read so too; and
/// a header read for no column holds no chunks at all.
#[test]
fn headers_read_for_some_columns_hold_theirs_alone() {
    let (pool, file) = file();
    let pages = Pages {
        pool: &pool,
        file,
        capacity: 1024,
        sealing: &Plain,
        compress: true,
        pax: false,
        imprints: true,
        kernels: &crate::kernels::Portable,
        base: 0,
        sink: None,
    };
    let group = written(&pages, (100, 1), 4000);
    let all = read(&pages, group.header[0], &TYPES, None).unwrap();
    let whole = chunks(&all);
    assert!(whole.iter().all(Option::is_some) && all.holds(None));
    for wanted in [vec![1_usize], vec![0, 2], vec![2]] {
        let some = read(&pages, group.header[0], &TYPES, Some(&wanted)).unwrap();
        let expected: Vec<Option<String>> = whole.iter().enumerate().map(|(c, z)| z.clone().filter(|_| wanted.contains(&c))).collect();
        assert_eq!(chunks(&some), expected, "{wanted:?}");
        assert_eq!((some.first, some.last, some.rows, &some.header), (all.first, all.last, all.rows, &all.header));
        assert!(some.holds(Some(&wanted)) && !some.holds(None), "{wanted:?}");
        let other = (0..TYPES.len()).find(|c| !wanted.contains(c)).unwrap();
        assert!(some.zones(other).is_err() && !some.holds(Some(&[other])), "{wanted:?}: {other} unread");
        assert!(some.zones(TYPES.len()).unwrap().is_none(), "a column added after it was written");
        assert!(some.bytes() < all.bytes(), "{wanted:?}");
        assert!(some.pages().is_err() && some.every_zone().is_err() && Summary::of(&some, some.header[0]).is_err());
        let more = some.with_columns(&[other]);
        let again = read(&pages, group.header[0], &TYPES, Some(&more)).unwrap();
        assert!(again.holds(Some(&wanted)) && again.holds(Some(&[other])), "{wanted:?} with {other}");
        assert_eq!(chunks(&again).get(other), whole.get(other));
    }
    let none = read(&pages, group.header[0], &TYPES, Some(&[])).unwrap();
    assert_eq!(chunks(&none), vec![None; TYPES.len()]);
    assert_eq!(none.row_ids(&pages).unwrap(), all.row_ids(&pages).unwrap(), "its row ids read all the same");
    assert_eq!(all.pages().unwrap(), group.pages().unwrap());
}
