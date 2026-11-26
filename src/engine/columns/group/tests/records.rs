//! A group written of a load's columns given as records (`written.rs`,
//! `column.rs`) is the group written of the same rows, byte for byte.

use super::*;
use crate::engine::columns::tests::records;
use crate::storage::lanes::Records;
use crate::value::Datetime;
use crate::value::datetime::Moment;

const DOUBLE: DataType = DataType::Approximate { precision: 53, spelled: Spelled::DoublePrecision };
const STAMP: DataType = DataType::Timestamp { precision: 6, zoned: true };
const WIDE: DataType = DataType::Exact { precision: 30, scale: 2, spelled: Spelled::Numeric };
const DATE: DataType = DataType::Date;
const ALL: [DataType; 8] = [INTEGER, TEXT, INTEGER, DOUBLE, STAMP, WIDE, DATE, TEXT];

/// Row `n` of a column of each of `ALL`: `row`'s, then a number, a
/// timestamp with time zone, a wide number, a date and a text of many
/// alike in part, so its chunks are coded by symbols, nulls among them;
/// the timestamps of a row of ten alike in their instants, of other zones.
fn wide_row(n: i128) -> Row {
    let small = i64::try_from(n).unwrap();
    let null_or = |every: i128, v: Value| if n.checked_rem(every) == Some(5) { Value::Null } else { v };
    let mut out = row(n);
    out.push(null_or(11, Value::Approx(f64::from(u32::try_from(n % 1000).unwrap()) * 0.5)));
    let utc = small.checked_div(10).unwrap().wrapping_mul(1_000_000);
    let zone = i16::try_from(n % 10).unwrap().wrapping_mul(60);
    out.push(null_or(13, Value::Datetime(Datetime { moment: Moment::Timestamp, utc, zone, zoned: true, precision: 6 })));
    out.push(null_or(17, Value::Exact(Decimal::new(n.wrapping_sub(2000).wrapping_mul(1_000_000_007), 2))));
    let day = Datetime { moment: Moment::Date, utc: (small % 300).wrapping_mul(86_400_000_000), zone: 0, zoned: false, precision: 0 };
    out.push(null_or(19, Value::Datetime(day)));
    let (site, page) = (n.wrapping_mul(7919) % 300, n.wrapping_mul(104_729) % 50_000);
    out.push(null_or(23, Value::Char(format!("http://site{site}.example/page{page}").into())));
    out
}

/// The records of each column of `rows`, as a load gives them.
fn columns_of(rows: &[Row]) -> Vec<Records> {
    ALL.iter().enumerate().map(|(c, t)| records(t, &rows.iter().map(|r| r[c].clone()).collect::<Vec<_>>())).collect()
}

/// Of a group written to its own file, its header's bytes and the content
/// of each of its pages, in order.
fn image(group: &Group, pages: &Pages<'_, Plain>) -> (Vec<u8>, Vec<(u64, Vec<u8>)>) {
    let header = read_chain(pages, group.header[0]).unwrap().0;
    (header, group.pages().unwrap().into_iter().map(|p| (p.0, pages.get(p).unwrap())).collect())
}

/// A group's header bytes, each page's number and content, and its zones
/// as shown.
type Image = (Vec<u8>, Vec<(u64, Vec<u8>)>, String);

/// Writes `rows` as a group to a file of its own, of rows if `by_rows`
/// else of their records, in the layout `pax` says, with Bloom filters
/// and sketches as `places` says: its image and its zones as shown.
fn write_one(rows: &[Row], (by_rows, pax): (bool, bool), places: (&[usize], &[usize], &[usize])) -> Image {
    let (pool, file) = file();
    let pages = Pages {
        pool: &pool,
        file,
        capacity: 2048,
        sealing: &Plain,
        compress: true,
        pax,
        imprints: true,
        kernels: &crate::kernels::Portable,
        base: 0,
        sink: None,
    };
    let ids: Vec<RowId> = (1..=u64::try_from(rows.len()).unwrap()).collect();
    let group = if by_rows {
        Group::write(&pages, &ALL, (&ids, rows), places).unwrap()
    } else {
        Group::write_records(&pages, &ALL, (&ids, &columns_of(rows)), places).unwrap()
    };
    assert!(!places.2.contains(&7) || !group.sketches[7].is_empty(), "a sketch");
    assert!(group.imprinted(), "imprints");
    let zones = format!("{:?} {:?} {:?}", group.columns, group.edges, group.sketches);
    let (header, content) = image(&group, &pages);
    (header, content, zones)
}

/// Verifies: [DESIGN] RULES.md D1148, D31: a group written of the records
/// of each column is the group written of the same rows, its header and
/// every page byte for byte, its zones, imprints and sketch alike: of the
/// column layout and the hybrid one, with Bloom filters and sketches and
/// without, its rows in order of a column or not; of integers narrow and
/// wide, texts, approximate numbers, timestamps with time zone and dates,
/// nulls among them, the least and greatest found of ties as of rows.
#[test]
fn a_group_of_records_is_the_group_of_its_rows() {
    let rows: Vec<Row> = (0..3000).map(wide_row).collect();
    let mut sorted = rows.clone();
    sorted.sort_by(|a, b| crate::value::sort_cmp(&a[2], &b[2]));
    for pax in [false, true] {
        for (these, places) in
            [(&rows, (&[][..], &[][..], &[][..])), (&rows, (&[], &[0, 1, 4], &[1, 7])), (&sorted, (&[2], &[3, 5, 7], &[7]))]
        {
            let of_rows = write_one(these, (true, pax), places);
            assert!(of_rows.1.len() > 10, "many pages");
            assert_eq!(write_one(these, (false, pax), places), of_rows, "pax {pax}, {places:?}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D1148, R-ERR-2: columns given as records
/// that are not one of each type, or not a value of each row, are refused
/// with SQLCODE -910, nothing written.
#[test]
fn records_not_of_the_group_are_refused() {
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
    let rows: Vec<Row> = (0..10).map(wide_row).collect();
    let mut columns = columns_of(&rows);
    let ids: Vec<RowId> = (1..=10).collect();
    let refused = |given: &[Records]| Group::write_records(&pages, &ALL, (&ids, given), (&[], &[], &[])).err().map(|e| e.code);
    assert_eq!(refused(&columns[..7]), Some(Code::Api), "a column short");
    columns[3].ends.pop();
    assert_eq!(refused(&columns), Some(Code::Api), "a value short");
}
