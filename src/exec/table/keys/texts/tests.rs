//! Unit tests of a lane's texts in chunks: a text stays where it was
//! written, a chunk is made with room for what the lane holds within its
//! bounds and its room is counted as it is made, groups put in another
//! order keep their texts where they are, and text keys hold their groups
//! made, merged, written to a spill and read back (RULES.md D1255).

use super::*;
use crate::bytes::Reader;
use crate::exec::table::keys::{Data, Keys, Lane};
use crate::exec::table::{GroupTable, Key, Made, Seed, hash_values, narrow};
use crate::functions::Registry;
use crate::logical::AggCall;
use crate::storage::Cell;
use crate::value::Value;

/// Where group `g`'s text lies in memory.
fn place(texts: &Chunks, g: usize) -> *const u8 {
    texts.get(g).unwrap().as_ptr()
}

/// The text group `g` is given.
fn text_of(g: usize) -> String {
    format!("text {g} {}", "x".repeat(g % 97))
}

/// Verifies: [DESIGN] (RULES.md D1255) a text a lane holds stays where it
/// was written however many texts follow it, into chunk after chunk, and
/// every text is read back as it was given.
#[test]
fn a_text_written_never_moves() {
    let mut texts = Chunks::default();
    let mut places = Vec::new();
    for g in 0..40_000 {
        assert!(texts.push(&text_of(g)));
        places.push(place(&texts, g));
    }
    assert!(texts.buffers.len() > 3, "{} chunks", texts.buffers.len());
    for (g, &written) in places.iter().enumerate() {
        assert_eq!(texts.get(g), Some(text_of(g).as_str()));
        assert_eq!(place(&texts, g), written, "group {g} moved");
    }
}

/// Verifies: [DESIGN] (RULES.md D1255) a lane's first chunk has room for a
/// page, each chunk after it for as much text as the lane holds, and none
/// for more than the most a chunk is made with. A text longer than that
/// has a chunk of its own, of its length, and the texts after it go on in
/// the chunk that was open.
#[test]
fn a_chunk_has_room_for_what_the_lane_holds_within_its_bounds() {
    let (mut texts, line) = (Chunks::default(), "y".repeat(64));
    while texts.held < MOST.saturating_mul(4) {
        assert!(texts.push(&line));
    }
    let rooms: Vec<usize> = texts.buffers.iter().map(String::capacity).collect();
    assert_eq!(rooms.first(), Some(&FIRST));
    assert_eq!(texts.room, rooms.iter().sum::<usize>(), "every chunk's room is counted");
    assert_eq!(rooms.last(), Some(&MOST));
    for (at, &room) in rooms.iter().enumerate() {
        let before: usize = rooms.iter().take(at).sum();
        assert_eq!(room, before.clamp(FIRST, MOST), "chunk {at} of {rooms:?}");
    }
    let (mut long, longer) = (Chunks::default(), "z".repeat(MOST.saturating_add(1)));
    assert!(long.push("a") && long.push(&longer) && long.push("b"));
    assert_eq!(long.buffers.iter().map(String::capacity).collect::<Vec<_>>(), [FIRST, MOST.saturating_add(1)]);
    assert_eq!((long.get(0), long.get(1), long.get(2)), (Some("a"), Some(longer.as_str()), Some("b")));
    assert_eq!(place(&long, 2), place(&long, 0).wrapping_add(1), "b follows a in the open chunk");
}

/// Verifies: [DESIGN] (RULES.md D1255) groups put in another order keep
/// their texts where they were written, a null's empty among them, and
/// each is read at its new place.
#[test]
fn groups_gathered_keep_their_texts_where_they_are() {
    let mut texts = Chunks::default();
    for g in 0..3000 {
        if g % 7 == 3 {
            texts.push_null();
        } else {
            assert!(texts.push(&text_of(g)));
        }
    }
    let read = |lane: &Chunks, g: usize| lane.get(g).map(|text| (text.to_owned(), text.as_ptr()));
    let before: Vec<_> = (0..3000).map(|g| read(&texts, g)).collect();
    let places: Vec<u32> = (0..3000_u32).rev().collect();
    texts.gather(&places);
    for (at, &g) in places.iter().enumerate() {
        assert_eq!(read(&texts, at), before[index(g)], "the group at {at} was group {g}");
    }
    assert_eq!(texts.get(2996), Some(""), "a null's text is empty");
}

/// Verifies: [DESIGN] (RULES.md D1255) a lane kept to its first groups
/// lets go of the chunks past the last text kept, holds the rest as they
/// were, and goes on taking texts; kept to none it holds nothing.
#[test]
fn a_lane_shortened_lets_its_last_chunks_go() {
    let mut texts = Chunks::default();
    (0..20_000).for_each(|g| assert!(texts.push(&text_of(g))));
    let (chunks, bytes) = (texts.buffers.len(), texts.bytes());
    texts.shorten(20_000);
    assert_eq!((texts.buffers.len(), texts.bytes()), (chunks, bytes), "nothing to let go");
    texts.shorten(20);
    assert_eq!(texts.buffers.len(), 1);
    assert!(texts.bytes() < bytes);
    assert_eq!(texts.room, texts.buffers.iter().map(String::capacity).sum::<usize>(), "the room let go is no longer counted");
    assert!(texts.push("after"));
    let kept: Vec<String> = (0..20).map(text_of).collect();
    assert_eq!((0..20).map(|g| texts.get(g).unwrap()).collect::<Vec<_>>(), kept);
    assert_eq!((texts.get(20), texts.get(21)), (Some("after"), None));
    assert_eq!(texts.held, kept.iter().map(String::len).sum::<usize>().saturating_add(5));
    texts.shorten(0);
    assert_eq!((texts.buffers.len(), texts.held, texts.get(0)), (0, 0, None));
}

/// Verifies: [DESIGN] (RULES.md D1255) a lane's texts are given as one
/// buffer and each text's end in it: the one chunk as it is where the
/// texts lie in their order in it, nulls among them, and every text
/// copied once in the groups' order where they lie in several chunks or
/// in another order.
#[test]
fn texts_are_joined_in_one_buffer_in_their_groups_order() {
    let mut one = Chunks::default();
    assert!(one.push("ab") && one.push("") && one.push("cde"));
    one.push_null();
    assert!(one.push("f"));
    let written = place(&one, 0);
    let (text, ends) = one.joined();
    assert_eq!((text.as_str(), ends.as_slice(), text.as_ptr()), ("abcdef", [2, 2, 5, 5, 6].as_slice(), written));
    let mut turned = Chunks::default();
    assert!(turned.push("ab") && turned.push("cde"));
    turned.gather(&[1, 0]);
    assert_eq!(turned.joined(), ("cdeab".to_owned(), vec![3, 5]));
    let mut many = Chunks::default();
    (0..20_000).for_each(|g| assert!(many.push(&text_of(g))));
    assert!(many.buffers.len() > 1);
    let (whole, stops) = many.joined();
    assert_eq!(whole, (0..20_000).map(text_of).collect::<String>());
    assert_eq!(stops.len(), 20_000);
    assert_eq!(whole.get(stops[9]..stops[10]), Some(text_of(10).as_str()));
}

/// Verifies: [DESIGN] (RULES.md D1255) texts appended from another lane
/// at places of it are copied in that order into this lane's chunks, and
/// the room no text fills is given back, after which the lane goes on
/// taking texts in a new chunk.
#[test]
fn texts_of_another_lane_are_appended_and_spare_room_is_given_back() {
    let mut from = Chunks::default();
    (0..500).for_each(|g| assert!(from.push(&text_of(g))));
    let mut into = Chunks::default();
    assert!(into.push("first"));
    into.extend(&from, &[499, 0, 250]);
    assert_eq!((1..4).map(|g| into.get(g).unwrap().to_owned()).collect::<Vec<_>>(), [text_of(499), text_of(0), text_of(250)]);
    assert_ne!(place(&into, 2), place(&from, 0), "a copy of its own");
    let held = into.held;
    assert_eq!(into.room, FIRST);
    into.shrink();
    assert_eq!((into.buffers.iter().map(String::capacity).sum::<usize>(), into.room), (held, held));
    assert!(into.push("more"));
    assert_eq!((into.buffers.len(), into.get(4), into.get(1)), (2, Some("more"), Some(text_of(499).as_str())));
}

/// Verifies: [DESIGN] (RULES.md D1255) a chunk's room is counted in what
/// the lane holds as the chunk is made, before a text fills it, and a
/// lane made with room for a known length of text holds one chunk of that
/// length, which its texts fill with nothing spare.
#[test]
fn the_room_of_a_chunk_is_counted_when_it_is_made() {
    let mut texts = Chunks::default();
    assert_eq!(texts.bytes(), 0);
    assert!(texts.push("a"));
    let told = texts.spans.capacity().saturating_mul(size_of::<Span>());
    assert_eq!(texts.bytes(), FIRST.saturating_add(told).saturating_add(texts.buffers.capacity().saturating_mul(size_of::<String>())));
    let mut known = Chunks::with_room(6);
    assert_eq!(known.room, 6);
    assert!(known.push("abc") && known.push("") && known.push("def"));
    assert_eq!((known.buffers.len(), known.room, known.held), (1, 6, 6));
    assert_eq!(known.held_at(&[2, 1, 0, 2]), 9);
    assert!(known.push("g"));
    assert_eq!((known.buffers.len(), known.room), (2, FIRST.saturating_add(6)), "a text past the known length opens a chunk");
}

/// COUNT(*), the one set function of the tables below.
fn count() -> Vec<AggCall> {
    let func = Registry::with_set_functions().aggregate("COUNT").unwrap();
    let result = func.0.result_type(None).unwrap();
    vec![AggCall { func, arg: None, distinct: false, result, filter: None, distribution: None, label: "COUNT".to_owned() }]
}

/// A morsel's table of the keys `keys`, a row each, numbered from `first`.
fn morsel(keys: &[Value], first: u64, seed: &Seed) -> GroupTable {
    let mut table = GroupTable::with_room(1, &count(), seed, 0);
    for (n, key) in (first..).zip(keys) {
        let key = std::slice::from_ref(key);
        let g = table.found_or_made(hash_values(seed, key), Key::Values(key), n);
        table.merge_written(g, (n, &[1_i128.to_le_bytes().to_vec()])).unwrap();
    }
    table
}

/// The groups of `table` finished, with their first rows, sorted.
fn groups(table: &GroupTable) -> Vec<String> {
    let mut rows: Vec<String> = table.finished().unwrap().iter().map(|(first, row)| format!("{first} {row:?}")).collect();
    rows.sort();
    rows
}

/// The text keys of the paths' test: a null, the empty text, a text with
/// and without a trailing space, a text longer than a chunk and the same
/// with a space after it, then texts of many lengths, all of them twice
/// from `from` on.
fn mixed(from: usize) -> Vec<Value> {
    let long = "u".repeat(MOST.saturating_add(7));
    let text = |s: String| Value::Char(s.into());
    let mut keys = vec![Value::Null, text(String::new()), text("a".to_owned()), text("a ".to_owned())];
    keys.extend([text(long.clone()), text(format!("{long} "))]);
    keys.extend((from..from.saturating_add(1500)).map(|g| text(text_of(g))));
    let again = keys.clone();
    keys.extend(again);
    keys
}

/// Verifies: [DESIGN] (RULES.md D1255) text keys, a null, the empty text,
/// texts that differ by a trailing space and a text longer than a chunk
/// among them, hold the same groups whichever way their tables go: two
/// morsels merged whole, split among partitions with their texts left
/// where they are and merged a partition at a time, written to a spill in
/// pieces and read back, and given as a column; padded and unpadded. Of
/// keys compared padded, a text and the same with a trailing space are
/// one group, given as its first row gave it.
#[test]
fn text_keys_hold_their_groups_made_merged_spilled_and_read_back() {
    for unpadded in [false, true] {
        let seed = Seed::fixed(0x5EED).unpadded(&[unpadded]);
        let (early, late) = (mixed(0), mixed(700));
        let table = || GroupTable::new(1, &count(), &seed, usize::MAX);
        let mut whole = table();
        whole.absorb_whole(&morsel(&early, 0, &seed).frozen().unwrap()).unwrap();
        whole.absorb_whole(&morsel(&late, 10_000, &seed).frozen().unwrap()).unwrap();
        let expected = groups(&whole);
        assert_eq!(expected.len(), if unpadded { 2206 } else { 2204 }, "unpadded: {unpadded}");
        let mades = [morsel(&early, 0, &seed).made(8, &[]).unwrap(), morsel(&late, 10_000, &seed).made(8, &[]).unwrap()];
        let (mut parted, mut spilled, mut given) = (Vec::new(), table(), Vec::new());
        for p in 0..8 {
            let mut part = table();
            for made in &mades {
                part.absorb(made, p).unwrap();
            }
            parted.extend(groups(&part));
            let frozen = part.frozen().unwrap();
            let places: Vec<u32> = (0..frozen.len()).map(narrow).collect();
            for piece in places.chunks(97) {
                let mut record = Vec::new();
                assert!(frozen.put_groups(piece, &mut record));
                spilled.absorb_whole(&Made::read(&record, &count()).unwrap()).unwrap();
            }
            let (batch, firsts) = GroupTable::thawed(frozen, &count(), &seed).unwrap().into_batch().unwrap();
            given.extend(
                firsts
                    .iter()
                    .enumerate()
                    .map(|(r, first)| format!("{first} {:?}", [batch.cell(0, r).to_value(), batch.cell(1, r).to_value()])),
            );
        }
        parted.sort();
        given.sort();
        assert_eq!(parted, expected, "split among partitions, unpadded: {unpadded}");
        assert_eq!(groups(&spilled), expected, "read back from the spill, unpadded: {unpadded}");
        assert_eq!(given, expected, "given as columns, unpadded: {unpadded}");
        let long = expected.iter().filter(|row| row.len() > MOST).count();
        assert_eq!(long, if unpadded { 2 } else { 1 }, "the text longer than a chunk, with and without its space");
    }
}

/// Verifies: [DESIGN] (RULES.md D1255, D1226) a lane of texts read from a
/// spill is one chunk of the texts' whole length, and bytes that are no
/// such lane are refused with SQLCODE -916: texts whose lengths pass
/// their whole length, or fall short of it, and octets that are not
/// UTF-8.
#[test]
fn texts_read_back_lie_in_one_chunk_and_bytes_that_are_none_are_refused() {
    let mut keys = Keys::new(1);
    for text in ["ab", "", "cde "] {
        keys.push(Key::Values(&[Value::Char(text.into())]));
    }
    let mut out = Vec::new();
    assert!(keys.put_groups(&[2, 0, 1], &mut out));
    let read = |bytes: &[u8]| Keys::read(&mut Reader::new(bytes, "keys"), 3);
    let back = read(&out).unwrap();
    assert_eq!(
        format!("{:?}", (0..3).map(|g| back.values(g)).collect::<Vec<_>>()),
        format!("{:?}", [keys.values(2), keys.values(0), keys.values(1)])
    );
    let Some(Data::Texts(texts)) = back.lanes.first().map(|lane| &lane.data) else { panic!("a lane of texts is read") };
    assert_eq!((texts.buffers.len(), texts.room, texts.held), (1, 6, 6));
    // After the width, the unpadded flag, three nulls and the tag lie the
    // whole length (8 bytes), three lengths, and the octets.
    let (whole, first, octets) = (9, 17, 29);
    let broken = |at: usize, byte: u8| {
        let mut bytes = out.clone();
        bytes[at] = byte;
        read(&bytes).map(|_| ()).unwrap_err().sqlcode()
    };
    assert_eq!(out[whole], 6);
    assert_eq!(broken(first, 5), -916_i32, "a text ends outside the lane's texts");
    assert_eq!(broken(first, 3), -916_i32, "the lengths fall short of the whole");
    assert_eq!(broken(whole, 7), -916_i32, "the whole length passes the bytes");
    assert_eq!(broken(octets, 0xFF), -916_i32, "octets that are not UTF-8");
}

/// Verifies: [DESIGN] (RULES.md D1255, D1274) a lane of texts appended to
/// from another at places of it, and a lane put in the order of those
/// places, hold each group's text as the other held it, with its trailing
/// space, and its nulls. A lane that holds no null until one is appended
/// from another tells that group's null, not the empty text in its place.
#[test]
fn lanes_of_texts_are_appended_and_gathered_as_their_groups_were() {
    let mut scratch = Vec::new();
    let made = || {
        let mut lane = Lane::default();
        for g in 0_usize..50 {
            let text = format!("t{g} ");
            lane.push(if g % 5 == 2 { Cell::Null } else { Cell::Text(&text) });
        }
        lane
    };
    let (from, places) = (made(), [49_u32, 2, 0, 7, 31]);
    let mut into = Lane::default();
    into.push(Cell::Text("first"));
    into.extend(&from, &places);
    assert!(matches!(into.data, Data::Texts(_)));
    let gathered = made().gathered(&places);
    for (at, &g) in places.iter().enumerate() {
        let held = from.value(index(g));
        assert_eq!(format!("{:?}", into.value(at.saturating_add(1))), format!("{held:?}"), "group {g} appended at {at}");
        assert_eq!(format!("{:?}", gathered.value(at)), format!("{held:?}"), "group {g} gathered at {at}");
        assert!(into.equal(at.saturating_add(1), Cell::Value(&held), &mut scratch) && gathered.equal(at, Cell::Value(&held), &mut scratch));
    }
    assert_eq!((into.nulls.len(), gathered.nulls.len()), (6, 5));
    assert!(into.is_null(2) && !into.is_null(1), "the null appended is told");
    assert!(gathered.is_null(1) && !gathered.is_null(0));
    assert!(!into.equal(2, Cell::Text(""), &mut scratch), "a null is not the empty text in its place");
}
