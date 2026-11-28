//! Unit tests of a chunk's runs spread to their rows (RULES.md D1315,
//! D1171): of any range of positions, the values and the runs' ends are
//! those the values read one at a time give, of runs shorter than a line,
//! of a line and longer, by vectors kept from one reading to the next and
//! by vectors of a reading's own; and runs that are not those of the
//! positions give nothing and leave the rows as they were.

use super::super::fields::Cells;
use super::super::tests::records_of;
use super::*;

/// The lengths of the runs a test's chunk holds, in turn: of one row, of
/// fewer rows than a line, of a line, of a row more, of two lines and
/// about, and of several.
const LENGTHS: [usize; 12] = [1, 2, 3, 7, 8, 9, 1, 15, 16, 17, 40, 1];

/// The integers of a chunk of runs of `LENGTHS`, taken `turns` times, no
/// two runs in a row of one value, some below zero.
fn held(turns: usize) -> Vec<i64> {
    let lengths = LENGTHS.iter().cycle().take(LENGTHS.len() * turns);
    (0_i64..).zip(lengths).flat_map(|(r, &len)| std::iter::repeat_n(r * 1_000_003 % 4099 - 2000, len)).collect()
}

/// `values` as a chunk's runs.
fn runs_of(values: &[i64]) -> Vec<u8> {
    let fields: Vec<Option<Vec<u8>>> = values.iter().map(|v| Some(v.to_le_bytes().to_vec())).collect();
    let records = records_of(&fields);
    let (least, most) = (values.iter().copied().min().unwrap(), values.iter().copied().max().unwrap());
    let mut out = Vec::new();
    encode(Cells::whole(&records), (least, bits_for(most.abs_diff(least))), &mut out).unwrap();
    out
}

/// Where each run of `values` among positions `from` to `to` ends, past
/// `base`, the last at `to`.
fn ends_of(values: &[i64], (from, to): (usize, usize), base: usize) -> Vec<usize> {
    let mut ends: Vec<usize> = (from + 1..to).filter(|&n| values[n] != values[n - 1]).map(|n| base + n - from).collect();
    if to > from {
        ends.push(base + to - from);
    }
    ends
}

/// Verifies: [DESIGN] RULES.md D1315, D1171 a chunk's runs written a line
/// a run give, of any range of positions, the value each position holds
/// and where each run ends among them, pushed after what the vectors
/// held: of ranges that begin and end within a run, at a run's edge and
/// at the chunk's, of a last run of one row and of several lines, by
/// vectors kept from one reading to the next, which hold nothing of the
/// reading before, and the same by vectors of the reading's own. Past
/// the chunk's end nothing is read.
#[test]
fn runs_written_a_line_at_a_time_are_their_rows_values() {
    let values = held(3);
    let (bytes, count) = (runs_of(&values), values.len());
    let runs = Runs::parse(&bytes, count).unwrap();
    assert_eq!(runs.many, LENGTHS.len() * 3, "a run a length");
    let edges = [0, 1, 2, 5, 6, 13, 21, 22, 30, 31, 45, 46, 62, 79, 119, 120, 121, count - 41, count - 40, count - 1, count];
    let mut unpacked = Unpacked::default();
    for (from, to) in edges.iter().flat_map(|&from| edges.iter().filter(move |&&to| to >= from).map(move |&to| (from, to))) {
        let (mut out, mut ends) = (vec![5_i64], vec![1_usize]);
        assert_eq!(runs.ints_ends((from, to), (&mut out, Some((&mut ends, 10))), &mut unpacked), Some(()), "{from}..{to}");
        assert_eq!((out[0], &out[1..]), (5, &values[from..to]), "{from}..{to}: each position's value, after what was there");
        assert_eq!((ends[0], &ends[1..]), (1, ends_of(&values, (from, to), 10).as_slice()), "{from}..{to}: the runs' ends");
        let mut alone = Vec::new();
        assert_eq!(runs.ints((from, to), &mut alone), Some(()), "{from}..{to}");
        assert_eq!(alone, values[from..to], "{from}..{to}: by vectors of its own");
    }
    let mut out = vec![5_i64];
    assert_eq!(runs.ints_ends((0, count + 1), (&mut out, None), &mut unpacked), None, "past the end");
    assert_eq!(out, [5]);
}

/// Verifies: [DESIGN] RULES.md D1315 runs that are not those of the
/// positions asked give nothing and leave the rows as they were, with no
/// room kept past them. Such are runs one of whose ends lies before the
/// one before it, as a chunk's bytes gone wrong hold, and runs that end
/// short of the last position. The same chunk read where its runs are
/// whole gives their values, and a run that passes the last position is
/// cut there.
#[test]
fn runs_that_are_not_the_positions_leave_the_rows_as_they_were() {
    // Four runs of 12 positions, the second's end before the first's.
    let mut bytes = word(4).unwrap().to_vec();
    bytes.extend_from_slice(&0_i64.to_le_bytes());
    bytes.push(16);
    pack([1_u64, 2, 3, 4].into_iter(), 16, &mut bytes);
    pack([5_u64, 3, 9, 12].into_iter(), bits_for(12), &mut bytes);
    let runs = Runs::parse(&bytes, 12).unwrap();
    let (mut out, mut unpacked) = (vec![5_i64, 6], Unpacked::default());
    assert_eq!(runs.ints_ends((0, 12), (&mut out, None), &mut unpacked), None, "an end before the one before it");
    assert_eq!(out, [5, 6], "nothing pushed, and no room kept past the rows");
    assert_eq!(runs.ints_ends((0, 3), (&mut out, None), &mut unpacked), Some(()), "of the first run alone");
    assert_eq!(out, [5, 6, 1, 1, 1]);
    let spread = |ends: &[i64]| {
        let mut slots = vec![7_i64; 12 + LINE];
        (Runs::spread((&[1, 2], ends), (0, 12), &mut slots, None), slots)
    };
    assert_eq!(spread(&[5, 9]).0, None, "runs that end short of the last position");
    let (read, slots) = spread(&[5, 30]);
    assert_eq!((read, &slots[..12]), (Some(()), [1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2].as_slice()), "a run past the last position");
}
