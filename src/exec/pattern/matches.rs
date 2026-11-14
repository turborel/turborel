//! [ISO/IEC 9075-2:2023 §7.7 GR5 to GR11] The retained matches of one row
//! pattern partition and the rows they give, found as the rows are asked
//! for. A match is sought from the partition's first row; once one is
//! found, it is numbered [§6.31], its rows given, and the next sought from
//! the row its `<row pattern skip to>` names [GR8b]; else from the next
//! row. Only a preferred match from each row can be retained, so seeking
//! from the row skipped to finds the next retained one [GR6, GR8a]. A row
//! no match maps is a row whose place is past every retained match's
//! rows, seen when no match is found from it [GR11b iii].

use super::measures::Measures;
use super::scene::{Reading, Scene};
use crate::error::Result;
use crate::exec::{HeldIter, HeldRows, broken};
use crate::logical::{Recognize, RowsPerMatch, Skip};
use crate::pattern::{Mapped, Program};
use crate::storage::Row;
use crate::value::Value;

/// [GR8b iii] The rules SKIP TO breaks: of no row mapped to its variable,
/// and of its row the match's first.
const SKIP_RULES: (&str, &str) = ("[ISO/IEC 9075-2:2023 §7.7 GR8b iii 1]", "[ISO/IEC 9075-2:2023 §7.7 GR8b iii 2 A, 3 A]");

/// A row pattern partition, as its matches are found.
pub(super) struct Part {
    /// Its rows, in order.
    pub(super) rows: HeldRows<Row>,
    /// The place of the row the next match is sought from.
    at: usize,
    /// How many matches were found so far.
    number: u64,
    /// The place past the last row of every match found so far, each row
    /// before it matched [GR11b ii].
    covered: usize,
    /// The rows the match found last gives, not yet given.
    given: Option<HeldIter<Row>>,
}

/// A retained match, as its rows are made.
struct Current {
    start: usize,
    mapped: Vec<Mapped>,
    number: u64,
    /// How many of its rows were made or passed over; of one row per match
    /// or an empty match, whether its row was made.
    given: usize,
}

impl Current {
    /// [GR11] The place of the next row it gives, of `rows` per match, and
    /// how many of its rows its measures read as running there; none past
    /// its last.
    fn next_place(&mut self, rows: RowsPerMatch) -> Option<(usize, usize)> {
        let (len, first) = (self.mapped.len(), self.given == 0);
        match rows {
            // [GR11a] one row, of the match's first row, its measures
            // evaluated in its last row.
            RowsPerMatch::One => first.then_some((self.start, len)),
            RowsPerMatch::AllOmitEmpty => self.listed(),
            RowsPerMatch::AllShowEmpty | RowsPerMatch::AllWithUnmatched => {
                // [GR11b ii 1] an empty match's row, of the row it starts at.
                if len == 0 { first.then_some((self.start, 0)) } else { self.listed() }
            }
        }
    }

    /// [GR11b ii 2] The place of the next of its rows not between {- and
    /// -}, those passed over, and how many of its rows are up to it.
    fn listed(&mut self) -> Option<(usize, usize)> {
        let skipped = self.mapped.iter().skip(self.given).take_while(|m| m.excluded).count();
        self.given = self.given.saturating_add(skipped);
        (self.given < self.mapped.len()).then(|| (self.start.saturating_add(self.given), self.given.saturating_add(1)))
    }
}

impl Part {
    /// The partition of the rows `rows`, no match sought yet.
    pub(super) fn new(rows: HeldRows<Row>) -> Part {
        Part { rows, at: 0, number: 0, covered: 0, given: None }
    }

    /// The next row the partition gives, of `r` with `program`; none past
    /// its last.
    ///
    /// # Errors
    ///
    /// The definitions' and measures' evaluations'; the search's, past its
    /// steps; SQLSTATE 2202K or 2202L of SKIP TO [GR8b iii].
    pub(super) fn next(&mut self, r: &Recognize<()>, program: &Program, reading: &Reading<'_>) -> Result<Option<Row>> {
        loop {
            let made = self.given.as_mut().and_then(Iterator::next);
            if let Some(row) = made {
                return row.map(Some);
            }
            self.given = None;
            let k = self.at;
            if k >= self.rows.len() {
                return Ok(None);
            }
            match self.seek(program, reading, k)? {
                None => {
                    self.at = k.saturating_add(1);
                    // [GR11b iii] a row no match maps, of WITH UNMATCHED ROWS.
                    if r.rows == RowsPerMatch::AllWithUnmatched && k >= self.covered {
                        let mut row = self.row(k)?;
                        row.extend(r.measures.expr.iter().map(|_| Value::Null));
                        return Ok(Some(row));
                    }
                }
                Some(mapped) => {
                    self.number = self.number.saturating_add(1);
                    self.at = next_start(reading, r.matching.skip, k, &mapped)?;
                    self.covered = self.covered.max(k.saturating_add(mapped.len().max(1)));
                    let current = Current { start: k, mapped, number: self.number, given: 0 };
                    self.given = Some(self.made(r, reading, current)?.into_iter());
                }
            }
        }
    }

    /// The `place`th row.
    fn row(&mut self, place: usize) -> Result<Row> {
        self.rows.get(place)?.ok_or_else(|| broken("a row pattern's row past its partition's last"))
    }

    /// [ISO/IEC 9075-2:2023 §9.41] The preferred match from the `k`th row,
    /// if any, each definition judged of a potential match as `reading`
    /// reads it, the number it would be given.
    fn seek(&mut self, program: &Program, reading: &Reading<'_>, k: usize) -> Result<Option<Vec<Mapped>>> {
        let (len, number) = (self.rows.len(), Some(self.number.saturating_add(1)));
        let rows = &mut self.rows;
        program.find((0, k, len), &mut |variable, path| {
            let define = reading.matching.defines.get(variable).ok_or_else(|| broken("a primary row pattern variable of no definition"))?;
            let mut scene = Scene { rows: &mut *rows, start: k, mapped: path, number };
            reading.holds(&mut scene, define.as_ref())
        })
    }

    /// [GR11] The rows the match `c` gives, held in the space a set
    /// function's values are, its measures of each evaluated as it reads
    /// them.
    fn made(&mut self, r: &Recognize<()>, reading: &Reading<'_>, mut c: Current) -> Result<HeldRows<Row>> {
        let mut made = HeldRows::new(reading.space.clone());
        let mut measures = Measures::new(reading, &c.mapped, r.measures.values.len());
        while let Some((place, upto)) = c.next_place(r.rows) {
            let mut row = self.row(place)?;
            let mut scene = Scene { rows: &mut self.rows, start: c.start, mapped: &c.mapped, number: Some(c.number) };
            let values = match r.rows {
                RowsPerMatch::One => reading.measured(&mut scene, &r.measures, upto)?,
                RowsPerMatch::AllShowEmpty | RowsPerMatch::AllOmitEmpty | RowsPerMatch::AllWithUnmatched => {
                    measures.row(reading, &mut scene, &r.measures, upto)?
                }
            };
            row.extend(values);
            made.push(row)?;
            c.given = c.given.saturating_add(1);
        }
        Ok(made)
    }
}

/// [ISO/IEC 9075-2:2023 §7.7 GR8b] The place of the row the match after
/// that from the `k`th row, `mapped`, is sought from, of `skip`: past the
/// row an empty match starts at, and of SKIP TO NEXT ROW; past its last
/// row of SKIP PAST LAST ROW; the row SKIP TO goes to.
///
/// # Errors
///
/// SQLSTATE 2202K or 2202L of SKIP TO [GR8b iii].
fn next_start(reading: &Reading<'_>, skip: Skip, k: usize, mapped: &[Mapped]) -> Result<usize> {
    if mapped.is_empty() {
        return Ok(k.saturating_add(1));
    }
    let j = match skip {
        Skip::NextRow => 1,
        Skip::PastLastRow => mapped.len(),
        Skip::First(v) => reading.skipped_to(mapped, (v, false), SKIP_RULES)?,
        Skip::Last(v) => reading.skipped_to(mapped, (v, true), SKIP_RULES)?,
    };
    Ok(k.saturating_add(j))
}
