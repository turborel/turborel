//! Unit tests of a morsel's rows written once by partition (RULES.md
//! D1313): what is handed over is what the rows appended in the order
//! they come and then gathered by partition give, of integer keys with
//! nulls, of text keys, of batches a filter kept some rows of, by whole
//! keys and by a lane; and a table written by other partitions than it
//! is handed by is gathered as any table is.

use super::super::tests::{calls, exact, fed};
use super::*;
use crate::exec::table::{Key, hash_lanes, hash_rows};
use crate::kernels::Portable;
use crate::storage::{Column, Ints, Mask};
use crate::value::Value;

/// The columns of a key in the batches of `held`.
const AT: [usize; 2] = [0, 1];

/// Partitions by whole keys, by the first lane, and each way a test
/// splits its rows: those and by the second lane.
const WHOLE: &[usize] = &[];
const LANE: &[usize] = &[0];
const BY: [&[usize]; 3] = [WHOLE, LANE, &[1]];

/// No set function.
const NO_CALLS: &[AggCall] = &[];

/// A morsel's batches, three of 400 rows: integers, each key held four
/// times and a null every eleventh row, and texts with and without a
/// trailing space; the second batch kept every row but each fifth. With
/// `nulls`, every integer is null.
fn held(nulls: bool) -> Vec<(Batch, Option<Sel>)> {
    let batch = |b: i64| {
        let n = |r: i64| b.saturating_mul(400).saturating_add(r);
        let units: Vec<i64> = (0..400).map(|r| n(r).rem_euclid(290)).collect();
        let null: Vec<bool> = (0..400).map(|r| nulls || n(r).rem_euclid(11) == 3).collect();
        let texts = (0..400).map(|r| Value::Char(format!("t{}{}", n(r).rem_euclid(7), if r % 2 == 0 { " " } else { "" }).into()));
        let columns = vec![Column::Ints(Ints::new(units, 0, Some(Mask::from_bools(&null)))), Column::Values(texts.collect())];
        let kept = (b == 1).then(|| (0..400_u32).filter(|r| r % 5 != 0).collect::<Sel>());
        (Batch { columns, rows: 400 }, kept)
    };
    (0..3).map(batch).collect()
}

/// The hashes of the rows kept of `held`, as they come: of their keys,
/// and of their keys' `lanes` alone, by `seed`.
fn hashed(held: &[(Batch, Option<Sel>)], lanes: &[usize], seed: &Seed) -> (Vec<u64>, Vec<u64>) {
    let (mut hashes, mut by, mut one) = (Vec::new(), Vec::new(), Vec::new());
    for (batch, rows) in held {
        let rows = rows.as_ref().map(Sel::as_slice);
        hash_rows(batch, (AT.as_slice(), rows), (seed, &Portable), &mut one);
        hashes.extend_from_slice(&one);
        hash_lanes(batch, (AT.as_slice(), lanes, rows), (seed, &Portable), &mut one);
        by.extend_from_slice(&one);
    }
    (hashes, by)
}

/// The rows kept of `held`, each a group appended in the order they
/// come and fed its row: the table a morsel made before its rows were
/// written by partition.
fn appended(held: &[(Batch, Option<Sel>)], hashes: &[u64], seed: &Seed) -> GroupTable {
    let mut table = GroupTable::unplaced(AT.len(), &calls(), seed, 0);
    let mut hash = hashes.iter();
    for (batch, rows) in held {
        let rows = rows.as_ref().map(Sel::as_slice);
        for n in 0..rows.map_or(batch.rows, <[u32]>::len) {
            let number = u64::try_from(table.len()).unwrap().saturating_add(1000);
            let own = table.appended(*hash.next().unwrap(), Key::Row(batch, &AT, crate::storage::sel::nth(rows, n)), number);
            fed(&mut table, own, (number, &vec![exact(i128::from(number % 97))]));
        }
    }
    table
}

/// The rows kept of `held` written by partition among `parts` by
/// `lanes`, each fed its row at its place.
fn written(held: &[(Batch, Option<Sel>)], (parts, lanes): (usize, &[usize]), seed: &Seed) -> GroupTable {
    let (hashes, by) = hashed(held, lanes, seed);
    let by = (!lanes.is_empty()).then_some(by.as_slice());
    let mut table = GroupTable::placed((held, AT.as_slice()), (calls().as_slice(), seed), (hashes.as_slice(), by, 1000), (parts, lanes));
    let places = table.placed.as_ref().unwrap().places.clone();
    for (number, &place) in (1000_u64..).zip(&places) {
        fed(&mut table, place, (number, &vec![exact(i128::from(number % 97))]));
    }
    table
}

/// What `made` hands over, told whole: its hashes, first rows, places and
/// starts, whether it may repeat keys, each group's key, and each
/// partition's groups merged into a table, finished, in their order.
fn told(made: &Made, parts: usize, seed: &Seed) -> Vec<String> {
    let mut all = vec![format!("{:?} {:?} {:?} {:?} {}", made.hashes, made.firsts, made.places, made.starts, made.repeats())];
    all.extend((0..made.hashes.len()).map(|g| format!("{:?}", made.keys.values(g))));
    for p in 0..parts {
        let mut table = GroupTable::new(AT.len(), &calls(), seed, usize::MAX);
        table.reserve(made, made.groups_of(p));
        table.absorb(made, p).unwrap();
        all.extend(table.finished().unwrap().iter().map(|(first, row)| format!("{p} {first} {row:?}")));
    }
    all
}

/// Verifies: [DESIGN] RULES.md D1313, D1260 a morsel's rows written once
/// by partition hand over what the rows appended in the order they come
/// and then gathered by partition hand over: the same hashes, first rows'
/// numbers, places, starts and keys, a partition's rows in the order they
/// came, and each partition's table merges them into the same groups of
/// the same states. Of integer keys with nulls and of text keys, of a
/// batch a filter kept some rows of, of keys all null, by whole keys and
/// by the hash of one lane or the other, a text's compared unpadded too
/// (RULES.md D1113, D1183).
#[test]
fn rows_written_by_partition_are_the_rows_appended_and_gathered() {
    for (nulls, unpadded) in [(false, false), (true, false), (false, true)] {
        let (held, seed) = (held(nulls), Seed::fixed(0x5EED).unpadded(&[false, unpadded]));
        for lanes in BY {
            let (hashes, _) = hashed(&held, lanes, &seed);
            assert_eq!(hashes.len(), 1120, "400 rows, 320 kept of 400, and 400");
            let (apart, placed) = (appended(&held, &hashes, &seed), written(&held, (8, lanes), &seed));
            assert!(placed.placed.is_some(), "written by partition");
            let (gathered, lying) = (apart.made(8, lanes).unwrap(), placed.made(8, lanes).unwrap());
            assert!((0..8).filter(|&p| lying.groups_of(p) > 0).count() > 1 || nulls && lanes == LANE, "more partitions than one hold rows");
            assert_eq!(told(&lying, 8, &seed), told(&gathered, 8, &seed), "nulls {nulls}, unpadded {unpadded}, lanes {lanes:?}");
        }
    }
}

/// Each partition's groups of `made`, merged into a table and finished,
/// in the order of their first rows, and where its partitions begin.
fn groups(made: &Made, parts: usize, seed: &Seed) -> (Vec<String>, Vec<u32>) {
    let mut all = Vec::new();
    for p in 0..parts {
        let mut table = GroupTable::new(AT.len(), &calls(), seed, usize::MAX);
        table.reserve(made, made.groups_of(p));
        table.absorb(made, p).unwrap();
        let mut finished = table.finished().unwrap();
        finished.sort_by_key(|(first, _)| *first);
        all.extend(finished.iter().map(|(first, row)| format!("{p} {first} {row:?}")));
    }
    (all, made.starts.clone())
}

/// Verifies: [DESIGN] RULES.md D1313 a table written by partition is
/// handed over as it lies only by the partitions it was written by. Asked
/// for other partitions it is gathered as any table is, and each of those
/// partitions' tables merges what it hands over into the groups the rows
/// appended and gathered by them give: every key's rows still in the
/// order they came, so each state is the same, and a key lies in one
/// partition.
///
/// Decision: as many partitions as it was written by, or not; the lanes
/// it was written by, or not. Written by 8 and no lane: asked 8 and no
/// lane it lies so; asked 16 and no lane it does not (the count alone
/// differs); asked 8 by lane 0 it does not (the lanes alone differ).
#[test]
fn a_table_written_by_other_partitions_is_gathered_again() {
    let (held, seed) = (held(false), Seed::fixed(0x5EED));
    let lying = |parts: usize, lanes: &[usize]| written(&held, (8, WHOLE), &seed).starts_placed(parts, lanes).is_some();
    assert!(lying(8, WHOLE) && !lying(16, WHOLE) && !lying(8, LANE));
    assert!(written(&held, (8, LANE), &seed).starts_placed(8, LANE).is_some(), "by a lane, asked by it");
    let (hashes, _) = hashed(&held, WHOLE, &seed);
    for (parts, lanes) in [(16_usize, WHOLE), (8, LANE)] {
        let (gathered, again) = (appended(&held, &hashes, &seed).made(parts, lanes).unwrap(), written(&held, (8, WHOLE), &seed));
        assert_eq!(groups(&again.made(parts, lanes).unwrap(), parts, &seed), groups(&gathered, parts, &seed), "{parts} by {lanes:?}");
    }
}

/// Verifies: [DESIGN] RULES.md D1313 the rows of a batch taken into a
/// table written by partition are to be fed to the groups at their
/// places: the rows from the one asked, each as given and as read of its
/// batch, a filter's rows where it kept some; and none is noted of a
/// table that computes no set function, which counts them all the same.
#[test]
fn rows_taken_of_a_batch_are_fed_at_their_places() {
    let (held, seed) = (held(false), Seed::fixed(0x5EED));
    let mut table = written(&held, (8, WHOLE), &seed);
    let places = table.placed.as_ref().unwrap().places.clone();
    let kept = held.get(1).and_then(|(_, rows)| rows.as_ref()).map(Sel::as_slice).unwrap();
    table.take_placed(400, (Some(kept), 320));
    assert_eq!(table.pending.groups, &places[400..720]);
    assert_eq!(table.pending.read, kept);
    assert_eq!(table.pending.given, (0..320).collect::<Vec<u32>>());
    table.pending = crate::exec::table::Pending::default();
    table.take_placed(720, (None, 400));
    assert_eq!((table.pending.groups.as_slice(), table.taken), (&places[720..], 720));
    assert_eq!(table.pending.read, (0..400).collect::<Vec<u32>>());
    let (hashes, _) = hashed(&held, WHOLE, &seed);
    let mut keys = GroupTable::placed((held.as_slice(), AT.as_slice()), (NO_CALLS, &seed), (hashes.as_slice(), None, 0), (8, WHOLE));
    keys.take_placed(0, (None, 400));
    assert!(keys.pending.groups.is_empty() && keys.taken == 400, "no set function, nothing to feed");
}
