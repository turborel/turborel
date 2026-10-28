//! A cursor's position among its rows [ISO/IEC 9075:1992 §4.21], and where
//! a FETCH moves it [§13.3 GR2–GR5]. Split from db/cursor.rs (RULES.md
//! R-SIZE-2).
//!
//! A cursor's rows are places in its order, `0..`: a read-only cursor's
//! each a row read, an updatable one's each a row it may cover, which is
//! gone if, as the cursor reaches it, it was deleted or no longer
//! satisfies the cursor's condition (`CursorRows::Updatable`). A FETCH
//! counts rows only, passing over places that are gone.

use crate::error::Result;
use crate::storage::Row;

/// \[DESIGN\] Where an open cursor is [ISO/IEC 9075:1992 §4.21]: before the
/// row at a place, on one, or after the last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Position {
    /// Before the row at this place, or the next one there is: `Before(0)`
    /// is before the first row, where OPEN puts it.
    Before(usize),
    /// On the row at this place, the current row.
    On(usize),
    /// After the last row.
    After,
}

/// What is at a place among a cursor's rows.
pub(super) enum Slot {
    /// One of its rows.
    Row,
    /// A row it no longer covers.
    Gone,
    /// No place: past the last.
    End,
}

/// The places of a cursor's rows.
pub(super) trait Places {
    /// What is at place `i`.
    ///
    /// # Errors
    ///
    /// Reading or deriving the row there failed.
    fn at(&mut self, i: usize) -> Result<Slot>;
    /// The values of the row at place `i`, one `at` found.
    ///
    /// # Errors
    ///
    /// As `at`'s.
    fn row(&mut self, i: usize) -> Result<Row>;
    /// The number of places: every row, read.
    ///
    /// # Errors
    ///
    /// Reading a row failed.
    fn count(&mut self) -> Result<usize>;
}

/// \[DESIGN\] Where a FETCH moves its cursor [ISO/IEC 9075:1992 §13.3]:
/// its `<fetch orientation>`, with J's value for ABSOLUTE or RELATIVE.
#[derive(Debug, Clone, Copy)]
pub(super) enum Move {
    /// NEXT, and no orientation \[SR1\].
    Next,
    /// PRIOR.
    Prior,
    /// FIRST.
    First,
    /// LAST.
    Last,
    /// ABSOLUTE J.
    Absolute(i128),
    /// RELATIVE J.
    Relative(i128),
}

/// [ISO/IEC 9075:1992 §13.3 GR2–GR5] Moves a cursor at `at` among `places`
/// as `to` says: where it is then, and the row it is on, or none for no
/// data. An error leaves the cursor where it was \[GR7\]: the caller keeps
/// `at`.
///
/// # Errors
///
/// Reading or deriving a row failed.
pub(super) fn fetch(places: &mut dyn Places, at: Position, to: Move) -> Result<(Position, Option<Row>)> {
    let found = match to {
        Move::Next => relative(places, at, 1)?,
        Move::Prior => relative(places, at, -1)?,
        // [GR5b iii] FIRST with no data is before the first row.
        Move::First => absolute(places, 1)?.or_else(|| Found::Missing(Position::Before(0))),
        // [GR5b ii] LAST with no data is after the last row.
        Move::Last => absolute(places, -1)?.or_else(|| Found::Missing(Position::After)),
        Move::Absolute(j) => absolute(places, j)?,
        Move::Relative(j) => relative(places, at, j)?,
    };
    match found {
        Found::Row(i) => Ok((Position::On(i), Some(places.row(i)?))),
        Found::Missing(left) => Ok((left, None)),
    }
}

/// Where a move ended: on the row at a place, or with no data, and where
/// the cursor is left [GR5b].
enum Found {
    Row(usize),
    Missing(Position),
}

impl Found {
    /// As it is, or `other` with no data.
    fn or_else(self, other: impl FnOnce() -> Found) -> Found {
        match self {
            Found::Row(i) => Found::Row(i),
            Found::Missing(_) => other(),
        }
    }
}

/// [GR3a, GR4] ABSOLUTE J (FIRST is 1, LAST -1): the J-th row from the
/// first, or, J negative, the -J-th from the last. Past the last is after
/// it \[GR5b ii\]; before the first, or J zero, before it \[GR5b iii\].
fn absolute(places: &mut dyn Places, j: i128) -> Result<Found> {
    if j > 0 {
        return forward(places, 0, j.unsigned_abs(), Position::After);
    }
    if j < 0 {
        let end = places.count()?;
        return backward(places, end, j.unsigned_abs());
    }
    Ok(Found::Missing(Position::Before(0)))
}

/// [GR3b–GR3d, GR4] RELATIVE J from `at` (NEXT is 1, PRIOR -1): the J-th
/// row after the cursor's position, or, J negative, the -J-th before it,
/// or, J zero, the row it is on again, if it still is one.
fn relative(places: &mut dyn Places, at: Position, j: i128) -> Result<Found> {
    if j > 0 {
        let from = match at {
            Position::Before(i) => i,
            Position::On(i) => i.saturating_add(1),
            Position::After => return Ok(Found::Missing(Position::After)),
        };
        return forward(places, from, j.unsigned_abs(), Position::After);
    }
    if j < 0 {
        let end = match at {
            Position::Before(i) | Position::On(i) => i,
            Position::After => places.count()?,
        };
        return backward(places, end, j.unsigned_abs());
    }
    // [GR3d, GR5b i] on a row still, that row; else no data, the cursor
    // where it was.
    match at {
        Position::On(i) => match places.at(i)? {
            Slot::Row => Ok(Found::Row(i)),
            Slot::Gone | Slot::End => Ok(Found::Missing(at)),
        },
        Position::Before(_) | Position::After => Ok(Found::Missing(at)),
    }
}

/// The `k`-th row at or after place `from`; none, the cursor left at
/// `past`.
fn forward(places: &mut dyn Places, from: usize, k: u128, past: Position) -> Result<Found> {
    let mut seen = 0_u128;
    let mut i = from;
    loop {
        match places.at(i)? {
            Slot::End => return Ok(Found::Missing(past)),
            Slot::Gone => {}
            Slot::Row => {
                seen = seen.saturating_add(1);
                if seen == k {
                    return Ok(Found::Row(i));
                }
            }
        }
        i = i.saturating_add(1);
    }
}

/// The `k`-th row before place `end`, counting back; none, the cursor left
/// before the first row \[GR5b iii\].
fn backward(places: &mut dyn Places, end: usize, k: u128) -> Result<Found> {
    let mut seen = 0_u128;
    for i in (0..end).rev() {
        if let Slot::Row = places.at(i)? {
            seen = seen.saturating_add(1);
            if seen == k {
                return Ok(Found::Row(i));
            }
        }
    }
    Ok(Found::Missing(Position::Before(0)))
}

// Unit tests: FETCH's every orientation over rows some of which are gone
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
