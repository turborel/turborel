//! Unit tests of a cursor's position: FETCH's every orientation over rows
//! some of which are gone (RULES.md R-TEST-1). Split from position.rs
//! (RULES.md R-SIZE-2).

use super::*;
use crate::error::{Code, SqlError};
use crate::value::{Decimal, Value};

/// Places of rows `1..`, `None` where one is gone, `Err` where reading it
/// fails; counting what it read.
struct Listed {
    rows: Vec<std::result::Result<Option<i128>, ()>>,
    read: usize,
}

impl Places for Listed {
    fn at(&mut self, i: usize) -> Result<Slot> {
        self.read = self.read.saturating_add(1);
        match self.rows.get(i) {
            None => Ok(Slot::End),
            Some(Ok(Some(_))) => Ok(Slot::Row),
            Some(Ok(None)) => Ok(Slot::Gone),
            Some(Err(())) => Err(SqlError::new(Code::Data, "unreadable")),
        }
    }
    fn row(&mut self, i: usize) -> Result<Row> {
        let n = self.rows.get(i).copied().and_then(std::result::Result::ok).flatten().unwrap_or(0);
        Ok(vec![Value::Exact(Decimal::new(n, 0))])
    }
    fn count(&mut self) -> Result<usize> {
        Ok(self.rows.len())
    }
}

/// Rows 1, 2, 3 and 4, with a place gone between 2 and 3.
fn places() -> Listed {
    Listed { rows: vec![Ok(Some(1)), Ok(Some(2)), Ok(None), Ok(Some(3)), Ok(Some(4))], read: 0 }
}

/// The row FETCH `to` from `at` gives, as a number, and where it leaves
/// the cursor.
fn moved(at: Position, to: Move) -> (Position, Option<String>) {
    let (left, row) = fetch(&mut places(), at, to).unwrap();
    (left, row.map(|r| r[0].to_string()))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §13.3 GR2, GR3, GR4, GR5a] each orientation
/// reaches its row, counting rows, not the places gone between them:
/// NEXT, PRIOR, FIRST, LAST, ABSOLUTE from either end, RELATIVE either
/// way, and RELATIVE 0 on a row.
#[test]
fn each_orientation_reaches_its_row() {
    let on = |i: usize, n: &str| (Position::On(i), Some(n.to_owned()));
    assert_eq!(moved(Position::Before(0), Move::Next), on(0, "1"));
    assert_eq!(moved(Position::On(1), Move::Next), on(3, "3"), "the place gone passed over");
    assert_eq!(moved(Position::Before(3), Move::Next), on(3, "3"), "before a row, NEXT is that row");
    assert_eq!(moved(Position::On(3), Move::Prior), on(1, "2"));
    assert_eq!(moved(Position::After, Move::Prior), on(4, "4"));
    assert_eq!(moved(Position::Before(3), Move::Prior), on(1, "2"));
    assert_eq!(moved(Position::After, Move::First), on(0, "1"));
    assert_eq!(moved(Position::Before(0), Move::Last), on(4, "4"));
    assert_eq!(moved(Position::On(4), Move::Absolute(3)), on(3, "3"));
    assert_eq!(moved(Position::On(0), Move::Absolute(-3)), on(1, "2"));
    assert_eq!(moved(Position::On(0), Move::Relative(2)), on(3, "3"));
    assert_eq!(moved(Position::On(4), Move::Relative(-3)), on(0, "1"));
    assert_eq!(moved(Position::On(4), Move::Relative(-2)), on(1, "2"));
    assert_eq!(moved(Position::On(3), Move::Relative(0)), on(3, "3"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §13.3 GR5b] with no data, the cursor is
/// left after the last row by NEXT, LAST, and ABSOLUTE or RELATIVE past
/// it; before the first by PRIOR, FIRST, and ABSOLUTE 0 or before it; and
/// where it was by RELATIVE 0 off a row, or on a row gone.
#[test]
fn no_data_leaves_the_cursor_where_the_rules_say() {
    let none = |at: Position| (at, None);
    assert_eq!(moved(Position::On(4), Move::Next), none(Position::After));
    assert_eq!(moved(Position::After, Move::Next), none(Position::After));
    assert_eq!(moved(Position::On(0), Move::Absolute(5)), none(Position::After));
    assert_eq!(moved(Position::On(0), Move::Relative(9)), none(Position::After));
    assert_eq!(moved(Position::After, Move::Relative(1)), none(Position::After));
    assert_eq!(moved(Position::On(0), Move::Prior), none(Position::Before(0)));
    assert_eq!(moved(Position::On(4), Move::Absolute(-5)), none(Position::Before(0)));
    assert_eq!(moved(Position::On(3), Move::Relative(-4)), none(Position::Before(0)));
    assert_eq!(moved(Position::On(3), Move::Absolute(0)), none(Position::Before(0)));
    assert_eq!(moved(Position::Before(3), Move::Relative(0)), none(Position::Before(3)));
    assert_eq!(moved(Position::On(2), Move::Relative(0)), none(Position::On(2)), "a place gone");
    let mut empty = Listed { rows: Vec::new(), read: 0 };
    assert_eq!(fetch(&mut empty, Position::Before(0), Move::First).unwrap().0, Position::Before(0));
    assert_eq!(fetch(&mut empty, Position::Before(0), Move::Last).unwrap().0, Position::After);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §13.3 GR7] a row that cannot be read
/// fails the FETCH, and the cursor stays where it was, for the caller
/// keeps its position; NEXT reads no further than its row.
#[test]
fn a_failed_fetch_moves_nothing() {
    let mut failing = Listed { rows: vec![Ok(Some(1)), Err(()), Ok(Some(3))], read: 0 };
    assert_eq!(fetch(&mut failing, Position::On(0), Move::Next).err().map(|e| e.code), Some(Code::Data));
    let mut counted = places();
    fetch(&mut counted, Position::Before(0), Move::Next).unwrap();
    assert_eq!(counted.read, 1, "NEXT reads one place");
}
