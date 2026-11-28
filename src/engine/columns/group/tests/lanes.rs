//! Unit tests of lengths lanes (`group/lanes.rs`, RULES.md D1319).

use super::*;
use crate::engine::columns::prepared::Prepared;
use crate::engine::columns::scan::octets_at;

/// Pages of 1,024 bytes of `pool`'s `file`, in the hybrid layout if `pax`.
fn pages(pool: &BufferPool, file: FileId, pax: bool) -> Pages<'_, Plain> {
    Pages {
        pool,
        file,
        capacity: 1024,
        sealing: &Plain,
        compress: true,
        pax,
        imprints: true,
        kernels: &crate::kernels::Portable,
        base: 0,
        sink: None,
    }
}

/// The octet length of row `n`'s text, none of a null.
fn length(n: i128) -> Option<i64> {
    if let Value::Char(text) = &row(n)[1] { i64::try_from(text.len()).ok() } else { None }
}

/// The lengths at `positions` of column 1 of `group`, none of a null.
fn lengths(group: &Group, pages: &Pages<'_, Plain>, positions: &[u32]) -> Vec<Option<i64>> {
    let mut prepared = Prepared::default();
    let read = octets_at(group, (pages, &mut prepared), (1, &TEXT), positions).unwrap().unwrap();
    (0..positions.len()).map(|n| (!read.is_null(n)).then(|| read.units[n])).collect()
}

/// Verifies: [DESIGN] RULES.md D1319: a group writes a lengths lane of
/// its VARCHAR column and of no other; the lane's chunks hold each row's
/// octet length, a null of a null, and their zones the least and the
/// greatest length and the nulls of each part; a header read back holds
/// the lane, read for the column or for every column, and none if read
/// for another column; the lengths read of the lane are those read of
/// the text's chunks, of every row and of rows apart; the group's pages
/// hold the lane's. MC/DC of a lane written: VARCHAR against an integer.
#[test]
fn a_varchar_column_has_a_lane_of_its_lengths() {
    let (pool, file) = file();
    let pages = pages(&pool, file, false);
    let group = written(&pages, (100, 1), 4000);
    assert!(group.lane(0).is_none() && group.lane(2).is_none(), "no lane of integers");
    let lane = group.lane(1).expect("a lane of the text");
    assert_eq!(lane.iter().map(|z| z.run.rows).sum::<u32>(), 4000);
    let zone = &lane[0];
    let shown = zone.bounds.as_ref().map(|(least, greatest)| (least.to_string(), greatest.to_string()));
    assert_eq!(shown, Some(("0".to_owned(), "12".to_owned())), "lengths of 0 to 12");
    assert_eq!(usize::try_from(zone.nulls).unwrap(), (0..i128::from(zone.run.rows)).filter(|&n| length(n).is_none()).count());
    let all: Vec<u32> = (0..4000).collect();
    let wanted: Vec<Option<i64>> = (0..4000).map(length).collect();
    assert_eq!(lengths(&group, &pages, &all), wanted);
    assert_eq!(lengths(&group, &pages, &[3, 7, 2500, 3999]), [length(3), length(7), length(2500), length(3999)]);
    let mut bare = group.clone();
    bare.lanes = vec![None; TYPES.len()];
    assert_eq!(lengths(&bare, &pages, &all), wanted, "as read of the text's chunks");
    for column in lane.iter().map(|z| z.run.page) {
        assert!(group.pages().unwrap().contains(&column), "the group's pages hold the lane's");
    }
    let first = *group.header.first().unwrap();
    let whole = read(&pages, first, &TYPES, None).unwrap();
    assert_eq!(format!("{:?}", whole.lanes), format!("{:?}", group.lanes));
    let of_text = read(&pages, first, &TYPES, Some(&[1])).unwrap();
    assert_eq!(format!("{:?}", of_text.lane(1)), format!("{:?}", group.lane(1)));
    assert!(read(&pages, first, &TYPES, Some(&[0, 2])).unwrap().lane(1).is_none(), "passed over with its column");
}

/// Verifies: [DESIGN] RULES.md D1319, groups with no lane: one written in
/// the hybrid layout has none, nor has a CHARACTER column, and a header
/// written with none, as before
/// lanes were, ends where it did and is read as a group that has none;
/// lengths are then read of the text's chunks. A header whose lane names
/// no column of the table is refused, SQLCODE -916. Robustness.
#[test]
fn a_group_with_no_lane_is_read_as_before() {
    let (pool, file) = file();
    let hybrid = written(&pages(&pool, file, true), (100, 1), 900);
    assert!((0..TYPES.len()).all(|c| hybrid.lane(c).is_none()), "none in the hybrid layout");
    let plain = pages(&pool, file, false);
    let padded: Vec<Row> = (0..50).map(|n| vec![int(n), Value::Char("ab".into())]).collect();
    let ids: Vec<RowId> = (1..=50).collect();
    let fixed = Group::write(&plain, &[INTEGER, DataType::character(4)], (&ids, &padded), (&[], &[], &[])).unwrap();
    assert!(fixed.lane(1).is_none(), "none of a CHARACTER column, padded as it is read");
    let mut group = written(&plain, (100, 1), 900);
    let with = header::encode(&group, &TYPES).unwrap();
    group.lanes = vec![None; TYPES.len()];
    let without = header::encode(&group, &TYPES).unwrap();
    assert!(without.len() < with.len() && with.starts_with(&without), "a header with no lane ends before them");
    let mut written_pages = Vec::new();
    let old = put_chain(&plain, &without, &mut written_pages).unwrap();
    let read_back = read(&plain, old[0], &TYPES, None).unwrap();
    assert!((0..TYPES.len()).all(|c| read_back.lane(c).is_none()));
    let all: Vec<u32> = (0..900).collect();
    assert_eq!(lengths(&read_back, &plain, &all), (0..900).map(length).collect::<Vec<_>>());
    // The lane's column, the four octets after its count, set past the table's.
    let mut wrong = with.clone();
    let at = without.len() + 4;
    wrong[at..at + 4].copy_from_slice(&9_u32.to_le_bytes());
    let bad = put_chain(&plain, &wrong, &mut written_pages).unwrap();
    assert_eq!(read(&plain, bad[0], &TYPES, None).unwrap_err().code, Code::Corrupt);
}
