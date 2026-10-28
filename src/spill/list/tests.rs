use super::*;
use crate::spill::testing::{Held, areas};

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
    let space = ListSpace { limit: 16, batch: 4096, areas: Some(areas(Some(Arc::new(Held { broken })), usize::MAX)) };
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
/// are written, a segment to a run; read back oldest or newest segment
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
