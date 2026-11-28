//! Unit tests of a column of an exact numeric type of more digits than 64
//! bits always hold, read as integers of the chunks whose values 64 bits
//! hold (RULES.md D1287).

use super::super::*;
use super::{Plain, file, of_values};
use crate::engine::columns::encoded::{CONSTANT, DICTIONARY, FIXED};
use crate::engine::columns::group::Group;
use crate::engine::columns::judged::Judgment;
use crate::engine::columns::scan::{column_at, column_of_run, ints, ints_at, rows};
use crate::index::Probe;
use crate::storage::{Column, Condition, Conjunct, Conjunction, Row};
use crate::value::{CompOp, Decimal, Spelled, Value};

const IDENT: DataType = DataType::Exact { precision: 19, scale: 0, spelled: Spelled::Numeric };
const MONEY: DataType = DataType::Exact { precision: 38, scale: 2, spelled: Spelled::Numeric };
/// An identifier of values far apart and one of sixty values, a hundred
/// rows of each past 64 bits; one value throughout; an amount some of
/// whose rows pass 64 bits; and nulls only.
const TYPES: [DataType; 5] = [IDENT, IDENT, IDENT, MONEY, IDENT];
const ROWS: u32 = 5000;
/// The rows whose identifiers and amount are past the greatest 64 bits
/// hold, and those whose amount is below the least.
const ABOVE: (u32, u32) = (2000, 2100);
const BELOW: (u32, u32) = (3000, 3050);
/// A number of 19 digits past the greatest 64 bits hold.
const PAST: i128 = 9_900_000_000_000_000_000;

/// The amount of row `n`, in units of its scale.
fn amount(n: u32) -> i128 {
    let (near, huge) = (i128::from(n).wrapping_mul(1_000_003), 10_i128.pow(30));
    if (ABOVE.0..ABOVE.1).contains(&n) {
        huge.wrapping_add(near)
    } else if (BELOW.0..BELOW.1).contains(&n) {
        near.wrapping_sub(huge)
    } else {
        near.wrapping_sub(1_000_000_000)
    }
}

/// Row `n`, every eleventh value of each column but the first null.
fn row(n: u32) -> Row {
    let i = |v: i128, scale: u32| Value::Exact(Decimal::new(v, scale));
    let null = |v: Value| if n % 11 == 4 { Value::Null } else { v };
    let above = (ABOVE.0..ABOVE.1).contains(&n);
    // The greatest and the least 64 bits hold are rows 0 and 1.
    let far = match n % 3 {
        0 => i128::from(i64::MAX).wrapping_sub(i128::from(n)),
        1 => i128::from(i64::MIN).wrapping_add(i128::from(n.wrapping_sub(1))),
        _ => i128::from(n).wrapping_mul(1_000_000_007),
    };
    let entry = i128::from(n.wrapping_mul(7) % 60).wrapping_mul(1_000_000_000_000_000);
    let few = if n.is_multiple_of(2) { i128::from(i64::MAX).wrapping_sub(entry) } else { i128::from(i64::MIN).wrapping_add(entry) };
    vec![
        i(if above { PAST.wrapping_add(i128::from(n)) } else { far }, 0),
        null(i(if above && n.is_multiple_of(3) { PAST } else { few }, 0)),
        null(i(9_000_000_000_000_000_001, 0)),
        null(i(amount(n), 2)),
        Value::Null,
    ]
}

/// The encodings of the chunks of column `c` of `group`.
fn kinds(pages: &Pages<'_, Plain>, group: &Group, c: usize) -> Vec<u8> {
    let mut found: Vec<u8> =
        group.zones(c).unwrap().unwrap().iter().map(|z| pages.with_run(&z.run, |bytes| Ok(bytes[0])).unwrap()).collect();
    found.sort_unstable();
    found.dedup();
    found
}

/// A group of `ROWS` rows, its pages of 2,048 bytes, and its rows.
fn group(pool: &crate::disk::buffer::BufferPool, file: crate::disk::buffer::FileId) -> (Pages<'_, Plain>, Group, Vec<Row>) {
    let pages = Pages {
        pool,
        file,
        capacity: 2048,
        sealing: &Plain,
        compress: true,
        pax: false,
        imprints: true,
        kernels: &crate::kernels::Portable,
        base: 0,
        sink: None,
    };
    let ids: Vec<u64> = (1..=u64::from(ROWS)).collect();
    let all: Vec<Row> = (0..ROWS).map(row).collect();
    let group = Group::write(&pages, &TYPES, (&ids, &all), (&[], &[], &[])).unwrap();
    (pages, group, all)
}

/// Whether a chunk of column `c` with a row among `from` to `to` holds a
/// row past 64 bits.
fn touches_wide(group: &Group, c: usize, (from, to): (u32, u32)) -> bool {
    let past: &[(u32, u32)] = match c {
        0 | 1 => &[ABOVE],
        3 => &[ABOVE, BELOW],
        _ => &[],
    };
    let wide = |first: u32, end: u32| past.iter().any(|&(a, b)| first < b && end > a);
    let zones = group.zones(c).unwrap().unwrap();
    zones
        .iter()
        .map(|z| (z.run.first, z.run.first.saturating_add(z.run.rows)))
        .any(|(first, end)| first < to && end > from && wide(first, end))
}

/// Asserts column `c` of `run`, whose rows are `these`, reads as integers
/// unless the run takes of a chunk with a row past 64 bits, and then as
/// values, equal to the rows either way: whether as integers.
fn column_read(pages: &Pages<'_, Plain>, group: &Group, prepared: &mut Prepared, (c, run, these): (usize, (u32, u32), &[Row])) -> bool {
    let made = ints(group, (pages, &mut *prepared), (c, &TYPES[c]), run).unwrap();
    assert_eq!(made.is_none(), touches_wide(group, c, run), "column {c}, {run:?}");
    let column = column_of_run(group, (pages, prepared), (c, &TYPES[c]), run).unwrap();
    let Some(units) = made else {
        let Column::Values(values) = column else { panic!("not values: column {c}, {run:?}") };
        assert_eq!(of_values(&values), shown(c, these), "column {c}, {run:?}");
        return false;
    };
    assert!(matches!(column, Column::Ints(_)), "column {c}, {run:?}");
    assert_eq!((of_values(units.values()), units.scale), (shown(c, these), if c == 3 { 2 } else { 0 }), "column {c}, {run:?}");
    true
}

fn shown(c: usize, rows: &[Row]) -> Vec<String> {
    rows.iter().map(|r| r[c].to_string()).collect()
}

/// Verifies: [DESIGN] RULES.md D1287, a wide exact column read as
/// integers: of a column some of whose rows pass 64 bits, so its fields
/// keep 16 bytes, fixed with the least and greatest of 64 bits among them
/// or a dictionary's entries, a run reads as integers at the type's scale
/// unless it takes of a chunk with such a row, and then as values, equal
/// to its rows either way, no chunk opened to refuse it; and (RULES.md
/// D1285) a column every value of which 64 bits hold, one value
/// throughout or nulls only, reads as integers in every run. MC/DC of a
/// chunk's bounds: both of 64 bits (integers), the greatest past them, the
/// least below them (values each), and no bounds, every value null
/// (integers).
#[test]
fn a_wide_column_reads_as_integers_where_64_bits_hold_its_values() {
    let (pool, file) = file();
    let (pages, group, all) = group(&pool, file);
    let read = rows(&group, &pages, &TYPES, &[0, 1, 2, 3, 4], (0, ROWS)).unwrap();
    for c in 0..TYPES.len() {
        assert_eq!(shown(c, &read), shown(c, &all), "column {c} reads back");
    }
    let chunks: Vec<usize> = (0..TYPES.len()).map(|c| group.zones(c).unwrap().unwrap().len()).collect();
    assert!(chunks[0] > 35 && chunks[3] > 35 && chunks[1] > 2, "fields of 16 bytes fill many chunks: {chunks:?}");
    let found: Vec<Vec<u8>> = (0..TYPES.len()).map(|c| kinds(&pages, &group, c)).collect();
    assert_eq!(found, [vec![FIXED], vec![DICTIONARY], vec![CONSTANT], vec![FIXED], vec![CONSTANT]]);
    let mut prepared = Prepared::default();
    assert!(ints(&group, (&pages, &mut prepared), (3, &MONEY), ABOVE).unwrap().is_none() && prepared.opened == 0, "refused by its bounds");
    assert!(ints_at(&group, (&pages, &mut prepared), (3, &MONEY), &[BELOW.0]).unwrap().is_none() && prepared.opened == 0, "at a position");
    let mut narrow = [0_u32; 5];
    for from in (0..ROWS).step_by(100) {
        let (run, these) = ((from, from + 100), &all[usize::try_from(from).unwrap()..usize::try_from(from + 100).unwrap()]);
        for (c, count) in narrow.iter_mut().enumerate() {
            *count += u32::from(column_read(&pages, &group, &mut prepared, (c, run, these)));
        }
    }
    let runs = ROWS.checked_div(100).unwrap();
    assert!((narrow[2], narrow[4]) == (runs, runs), "{narrow:?}");
    assert!([0, 1, 3].iter().all(|&c| narrow[c] > 20 && narrow[c] < runs), "integers and values both: {narrow:?}");
}

/// Verifies: [DESIGN] RULES.md D1287, a wide exact column read at
/// positions: rows picked across the chunks of fixed fields and of a
/// dictionary that hold no row past 64 bits read as integers, equal to
/// their rows, and with such a chunk's among them as none; of the amounts,
/// positions in chunks of values of 64 bits read as integers though a
/// chunk between them holds a row past 64 bits, and positions one of which
/// is in such a chunk read as values, equal to their rows.
#[test]
fn a_wide_column_reads_as_integers_at_positions() {
    let (pool, file) = file();
    let (pages, group, all) = group(&pool, file);
    let picked = |positions: &[u32]| -> Vec<Row> { positions.iter().map(|&p| all[usize::try_from(p).unwrap()].clone()).collect() };
    let mut prepared = Prepared::default();
    let sparse: Vec<u32> = (0..ROWS).filter(|n| n % 13 < 3).collect();
    for c in [2, 4] {
        let made = ints_at(&group, (&pages, &mut prepared), (c, &TYPES[c]), &sparse).unwrap().unwrap();
        assert_eq!(of_values(made.values()), shown(c, &picked(&sparse)), "column {c}");
    }
    for c in [0, 1] {
        let apart: Vec<u32> = sparse.iter().copied().filter(|&n| !touches_wide(&group, c, (n, n + 1))).collect();
        let made = ints_at(&group, (&pages, &mut prepared), (c, &TYPES[c]), &apart).unwrap().unwrap();
        assert_eq!(of_values(made.values()), shown(c, &picked(&apart)), "column {c}");
        assert!(ints_at(&group, (&pages, &mut prepared), (c, &TYPES[c]), &sparse).unwrap().is_none(), "column {c}, a chunk past 64 bits");
    }
    let around: Vec<u32> = (0..ROWS).filter(|&n| n % 13 < 3 && !touches_wide(&group, 3, (n, n + 1))).collect();
    assert!(
        around.iter().any(|&p| p < ABOVE.0) && around.iter().any(|&p| p > ABOVE.1 && p < BELOW.0) && around.iter().any(|&p| p > BELOW.1)
    );
    let made = ints_at(&group, (&pages, &mut prepared), (3, &MONEY), &around).unwrap().unwrap();
    assert_eq!((of_values(made.values()), made.scale), (shown(3, &picked(&around)), 2));
    for one in [ABOVE.0 + 5, BELOW.0 + 5, ABOVE.0 - 1] {
        assert!(touches_wide(&group, 3, (one, one + 1)), "row {one} is in a chunk with a row past 64 bits");
        let mut among = around.clone();
        among.insert(among.partition_point(|&p| p < one), one);
        assert!(ints_at(&group, (&pages, &mut prepared), (3, &MONEY), &among).unwrap().is_none(), "row {one}");
        let Column::Values(values) = column_at(&group, (&pages, &mut prepared), (3, &MONEY), &among).unwrap() else { panic!("not values") };
        assert_eq!(of_values(&values), shown(3, &picked(&among)), "row {one}");
    }
}

/// Verifies: [DESIGN] RULES.md D1287, a conjunct on a wide exact column
/// judged by the scan: an equality and a comparison with a constant past
/// 64 bits keep, of a run read as integers, the rows whose value matches;
/// and of a run that takes of a chunk with a row past 64 bits they keep
/// every row, left to the filter, the column given as values.
#[test]
fn a_scan_judges_a_wide_column_where_it_reads_integers() {
    let (pool, file) = file();
    let (pages, group, all) = group(&pool, file);
    let compare = |column: usize, op: CompOp, value: Value| {
        Conjunction(vec![Conjunct::Condition(Condition { columns: vec![column], probe: Probe::Compare { op, value } })])
    };
    let mut prepared = Prepared::default();
    let wanted = all[1234][3].clone();
    let past = Value::Exact(Decimal::new(10_i128.pow(25), 2));
    for (told, column) in [
        (compare(3, CompOp::Eq, wanted.clone()), 3),
        (compare(3, CompOp::Lt, past), 3),
        (compare(0, CompOp::Gt, Value::Exact(Decimal::new(0, 0))), 0),
    ] {
        let judgment = Judgment::of(Some(&told), &TYPES);
        assert!(!judgment.is_empty(), "a wide exact column's conjunct is the scan's");
        for run in [(1200_u32, 1300_u32), (0, 100), (4900, 5000)] {
            let judged = judgment.judge(&group, (&pages, &mut prepared), (&TYPES, run), None).unwrap();
            let expected: Vec<u32> = (run.0..run.1)
                .filter(|&n| told.0[0].matches_key(&[&all[usize::try_from(n).unwrap()][column]]))
                .map(|n| n - run.0)
                .collect();
            assert_eq!(judged.kept, expected, "{told:?}, {run:?}");
            assert!(matches!(judged.decoded.as_slice(), [(_, Column::Ints(_))]), "{run:?} is judged of integers");
        }
    }
    let judgment = Judgment::of(Some(&compare(3, CompOp::Eq, wanted)), &TYPES);
    let judged = judgment.judge(&group, (&pages, &mut prepared), (&TYPES, (ABOVE.0, ABOVE.1)), None).unwrap();
    assert_eq!(judged.kept, (0..ABOVE.1 - ABOVE.0).collect::<Vec<u32>>(), "every row kept for the filter");
    assert!(matches!(judged.decoded.as_slice(), [(3, Column::Values(_))]));
}
