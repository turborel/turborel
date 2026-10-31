use super::*;
use crate::spill::testing::{Held, areas};
use crate::value::types::TypeRegistry;

/// A number, of 8 bytes in memory and as a record.
#[derive(Debug, Clone, PartialEq)]
struct N(u64);

impl ListItem for N {
    fn bytes(&self) -> usize {
        8
    }
}

fn encode(n: &N) -> Vec<u8> {
    n.0.to_le_bytes().to_vec()
}

fn decode(record: &[u8]) -> Result<N> {
    <[u8; 8]>::try_from(record).map(|b| N(u64::from_le_bytes(b))).map_err(|_| SqlError::new(Code::Corrupt, "[DESIGN] not a number"))
}

/// A list holding two numbers before it writes them, of runs `broken` or
/// not, holding 0 to `n` with a mark before each third.
fn numbers(n: u64, broken: bool) -> SpillList<N> {
    let space = ListSpace {
        limit: 16,
        batch: 4096,
        areas: Some(areas(Some(Arc::new(Held { broken })), usize::MAX)),
        types: TypeRegistry::default(),
    };
    let mut list = SpillList::new(space);
    for i in 0..n {
        if i.is_multiple_of(3) {
            assert_eq!(list.mark(), usize::try_from(i).unwrap());
        }
        list.push(N(i), &mut encode).unwrap();
    }
    list
}

/// Every item of `list`, its segments oldest or newest first.
fn all(list: SpillList<N>, newest: bool) -> Vec<u64> {
    list.into_segments(newest, &decode).flat_map(Result::unwrap).map(|n| n.0).collect()
}

/// Verifies: [DESIGN] RULES.md D38, `SpillList`: past its bytes, its items
/// are written, the segments held to a run; read back oldest or newest segment
/// first, each in order, they are as pushed; visited, the list is kept.
#[test]
fn items_are_read_back_as_pushed() {
    let mut list = numbers(10, false);
    assert_eq!(list.len(), 10);
    assert!(list.held() <= 16, "{}", list.held());
    let mut seen = Vec::new();
    list.visit(&decode, &mut |n| {
        seen.push(n.0);
        Ok(())
    })
    .unwrap();
    assert_eq!(seen, (0..10).collect::<Vec<_>>());
    let mut newest = Vec::new();
    list.visit_newest(&decode, &mut |n| {
        newest.push(n.0);
        Ok(())
    })
    .unwrap();
    assert_eq!(newest, (0..10).rev().collect::<Vec<_>>());
    let parts: Vec<u64> = (0..=list.segments()).flat_map(|at| list.segment(at, &decode).unwrap()).map(|n| n.0).collect();
    assert_eq!(parts, (0..10).collect::<Vec<_>>(), "each segment, and none past the last");
    assert_eq!(all(numbers(10, false), false), (0..10).collect::<Vec<_>>());
    let backwards = all(list, true);
    assert_eq!(backwards.first(), Some(&9));
    assert_eq!(backwards.len(), 10);
}

/// Verifies: [DESIGN] RULES.md D38, `SpillList::take_from`: from a mark,
/// the items on are taken whole, no run read, so even runs that cannot be
/// read are parted; from inside a run, the run is read back, and if it
/// cannot be, nothing is taken; the items before are kept, and appended
/// lists keep their items and order.
#[test]
fn items_are_taken_from_a_place() {
    let mut lost = numbers(10, true);
    let tail = lost.take_from(6, &decode).unwrap();
    assert_eq!((lost.len(), tail.len()), (6, 4));
    assert_eq!(lost.take_from(4, &decode).err().map(|e| e.code), Some(Code::Io));
    assert_eq!(lost.len(), 6, "none taken");
    let mut kept = numbers(10, false);
    let rest = kept.take_from(4, &decode).unwrap();
    assert_eq!(all(rest, false), (4..10).collect::<Vec<_>>());
    assert_eq!(kept.len(), 4);
    kept.append(numbers(2, false));
    assert_eq!(all(kept, false), [0, 1, 2, 3, 0, 1]);
    let mut empty = SpillList::<N>::default();
    assert!(empty.is_empty() && empty.take_from(0, &decode).unwrap().is_empty());
}

/// Verifies: [DESIGN] RULES.md D38, `SpillList::locate` and `take_oldest`:
/// an item is found held, or in its run by segment and place; past the
/// last, none. Taken oldest first, the segments give every item in order,
/// the list emptied; a run that cannot be read back is taken, its error in
/// its place.
#[test]
fn items_are_located_and_taken_oldest_first() {
    let mut list = numbers(10, false);
    assert!(matches!(list.locate(4), Some(Located::Spilled { at: 1, .. })));
    let end = list.len() - 1;
    match list.locate(end) {
        Some(Located::Held(n)) => assert_eq!(n.0, 9),
        Some(Located::Spilled { segment, at }) => assert_eq!(list.segment(segment, &decode).unwrap()[at].0, 9),
        None => panic!("item {end} not found"),
    }
    assert!(list.locate(10).is_none());
    let mut taken = Vec::new();
    while let Some(items) = list.take_oldest(&decode) {
        taken.extend(items.unwrap().into_iter().map(|n| n.0));
    }
    assert_eq!(taken, (0..10).collect::<Vec<_>>());
    assert!(list.is_empty() && list.held() == 0);
    let mut unread = numbers(10, true);
    assert_eq!(unread.take_oldest(&decode).and_then(Result::err).map(|e| e.code), Some(Code::Io));
    assert_eq!(unread.len(), 7, "the segment taken");
}

/// Verifies: [DESIGN] RULES.md D38, `SpillList::push`: segments held one
/// after another, a mark's of an item each, are written together as one
/// run, not a run, and a page, each; an item among them is found in it,
/// and a place among them is taken by reading it back. Regression test
/// (R-TEST-7): a transaction of many single-row statements wrote each
/// statement's changes to a run of its own, a page each as it was read, so
/// it used up the memory budget of a database in memory. A list whose area
/// is refused (SQLCODE -918) keeps every item held, in order.
#[test]
fn small_segments_are_written_together() {
    let space = |areas| ListSpace { limit: 16, batch: 4096, areas: Some(areas), types: TypeRegistry::default() };
    let mut list = SpillList::new(space(areas(Some(Arc::new(Held { broken: false })), usize::MAX)));
    let mut refused = SpillList::new(space(areas(None, 0)));
    let mut codes = Vec::new();
    for i in 0..5_u64 {
        list.mark();
        list.push(N(i), &mut encode).unwrap();
        refused.mark();
        codes.push(refused.push(N(i), &mut encode).err().map(|e| e.code));
    }
    assert_eq!(list.segments(), 3, "items 0 to 2 in one run, 3 and 4 held apart");
    assert!(matches!(list.locate(2), Some(Located::Spilled { segment: 0, at: 2 })));
    let tail = list.take_from(1, &decode).unwrap();
    assert_eq!(all(tail, false), [1, 2, 3, 4]);
    assert_eq!(all(list, false), [0]);
    assert_eq!(codes, [None, None, Some(Code::Resources), Some(Code::Resources), Some(Code::Resources)]);
    assert_eq!((refused.len(), refused.held()), (5, 40));
    assert_eq!(all(refused, false), [0, 1, 2, 3, 4]);
}
