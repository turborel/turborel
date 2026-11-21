//! \[DESIGN\] A mark join's right rows, held (RULES.md D642): in memory, by
//! the buckets of their keys and marks, or of their keys alone when a mark
//! value is null, each row once; past the work area in tables of its spill
//! area (`Built`, D38), one by keys and marks, one of the null list, and
//! one by keys alone. Of no condition beyond the keys only the first of
//! rows alike is held (`Marker::same`).

use super::super::super::work::Work;
use super::super::super::{RowIter, broken};
use super::super::{Built, Full, Operand, Rescanned, Right, key};
use super::{Marker, or};
use crate::error::Result;
use crate::hash::xxhash::Xxh3Map;
use crate::logical::Truth;
use crate::physical::JoinKey;
use crate::spill::row_bytes;
use crate::storage::Row;
use crate::value::Bucket;

/// The places of the held rows of each bucket.
type Places = Xxh3Map<Vec<Bucket>, Vec<usize>>;

/// A mark join's right rows.
pub(in crate::exec) enum Side {
    Memory(Box<Memory>),
    Spilled(Box<Spilled>),
}

/// The right rows in memory: by keys and marks, the null list by keys,
/// and, made the first time a left row of a null mark value asks, every
/// row by keys.
pub(in crate::exec) struct Memory {
    rows: Vec<Row>,
    whole: Places,
    nulls: Places,
    by_keys: Option<Places>,
}

/// The right rows in tables of the spill area.
pub(in crate::exec) struct Spilled {
    whole: Built,
    nulls: Option<Built>,
    all: Option<Built>,
}

/// Where a right row goes: its bucket, and whether on the null list.
fn placed(row: &Row, marker: &Marker) -> Result<Option<(Vec<Bucket>, bool)>> {
    let Some(mut b) = key(row, &marker.keys, Operand::Right)? else { return Ok(None) };
    if marker.marks.is_empty() {
        return Ok(Some((b, false)));
    }
    Ok(Some(match key(row, &marker.marks, Operand::Right)? {
        Some(m) => {
            b.extend(m);
            (b, false)
        }
        None => (b, true),
    }))
}

impl Side {
    /// The right `rows` of `marker`'s join, held in `work`.
    ///
    /// # Errors
    ///
    /// The rows'; the spill area's.
    pub(in crate::exec) fn of(mut rows: RowIter, marker: &Marker, work: &mut Work) -> Result<Side> {
        let mut held = Memory { rows: Vec::new(), whole: Places::default(), nulls: Places::default(), by_keys: None };
        let mut bytes = 0_usize;
        while let Some(r) = rows.next() {
            let row = r?;
            // A null key matches no left row, so the row never counts.
            let Some((b, null)) = placed(&row, marker)? else { continue };
            let places = if null { held.nulls.entry(b).or_default() } else { held.whole.entry(b).or_default() };
            if !null && first_held(places, &held.rows, (&row, marker))? {
                continue;
            }
            places.push(held.rows.len());
            bytes = bytes.saturating_add(row_bytes(&row));
            held.rows.push(row);
            if bytes > work.limit() {
                let all = held.rows.into_iter().map(Ok).chain(rows);
                return Ok(Side::Spilled(Box::new(Spilled::of(all, marker, work)?)));
            }
        }
        Ok(Side::Memory(Box::new(held)))
    }

    /// The mark of left row `l`.
    ///
    /// # Errors
    ///
    /// The conditions'; the spill area's.
    pub(in crate::exec) fn mark(&mut self, l: &Row, marker: &Marker) -> Result<Truth> {
        let Some(b) = key(l, &marker.keys, Operand::Left)? else { return Ok(Truth::False) };
        match self {
            Side::Memory(held) => held.mark(l, b, marker),
            Side::Spilled(spilled) => spilled.mark(l, b, marker),
        }
    }
}

/// Whether `places`, of rows held in `rows`, holds one alike to `row`.
fn first_held(places: &[usize], rows: &[Row], (row, marker): (&Row, &Marker)) -> Result<bool> {
    for &at in places {
        let held = rows.get(at).ok_or_else(|| broken("a mark join's held row is missing"))?;
        if marker.same(held, row)? {
            return Ok(true);
        }
    }
    Ok(false)
}

impl Memory {
    /// The mark of left row `l`, of keys `b`.
    fn mark(&mut self, l: &Row, mut b: Vec<Bucket>, marker: &Marker) -> Result<Truth> {
        if marker.marks.is_empty() {
            return self.any(self.whole.get(&b), l, marker);
        }
        let Some(m) = key(l, &marker.marks, Operand::Left)? else {
            if self.by_keys.is_none() {
                self.by_keys = Some(self.keyed(&marker.keys)?);
            }
            return self.any(self.by_keys.as_ref().and_then(|k| k.get(&b)), l, marker);
        };
        let nulls = self.nulls.get(&b);
        b.extend(m);
        let found = self.any(self.whole.get(&b), l, marker)?;
        if found == Truth::True {
            return Ok(found);
        }
        Ok(or(found, self.any(nulls, l, marker)?))
    }

    /// The OR of the truth values the rows at `places` give `l`.
    fn any(&self, places: Option<&Vec<usize>>, l: &Row, marker: &Marker) -> Result<Truth> {
        let mut out = Truth::False;
        for &at in places.into_iter().flatten() {
            let r = self.rows.get(at).ok_or_else(|| broken("a mark join's held row is missing"))?;
            out = or(out, marker.truth(l, r)?);
            if out == Truth::True {
                break;
            }
        }
        Ok(out)
    }

    /// The places of its rows by their `keys` alone.
    fn keyed(&self, keys: &[JoinKey]) -> Result<Places> {
        let mut by: Places = Places::default();
        for (at, row) in self.rows.iter().enumerate() {
            if let Some(b) = key(row, keys, Operand::Right)? {
                by.entry(b).or_default().push(at);
            }
        }
        Ok(by)
    }
}

impl Spilled {
    /// The right `rows`, in tables of `work`'s spill area: first in a run,
    /// read once for each table.
    fn of(rows: impl Iterator<Item = Result<Row>>, marker: &Marker, work: &mut Work) -> Result<Spilled> {
        let (mut run, _) = Rescanned::of(rows, work)?;
        if marker.marks.is_empty() {
            let whole = built(run.pass()?, marker.keys.clone(), work)?;
            return Ok(Spilled { whole, nulls: None, all: None });
        }
        let both: Vec<JoinKey> = marker.keys.iter().chain(&marker.marks).copied().collect();
        // Of a null mark value `key` gives none, so a table by keys and
        // marks holds no row of the null list.
        let whole = built(run.pass()?, both, work)?;
        let mut nulls = Vec::new();
        for r in run.pass()? {
            let row = r?;
            if key(&row, &marker.marks, Operand::Right)?.is_none() {
                nulls.push(Ok(row));
            }
        }
        let nulls = built(nulls.into_iter(), marker.keys.clone(), work)?;
        let all = built(run.pass()?, marker.keys.clone(), work)?;
        Ok(Spilled { whole, nulls: Some(nulls), all: Some(all) })
    }

    /// The mark of left row `l`, of keys `b`.
    fn mark(&self, l: &Row, mut b: Vec<Bucket>, marker: &Marker) -> Result<Truth> {
        let (Some(nulls), Some(all)) = (&self.nulls, &self.all) else { return any(&self.whole, &b, (l, marker)) };
        let Some(m) = key(l, &marker.marks, Operand::Left)? else { return any(all, &b, (l, marker)) };
        let of_nulls = any(nulls, &b, (l, marker))?;
        b.extend(m);
        Ok(or(any(&self.whole, &b, (l, marker))?, of_nulls))
    }
}

/// `rows` built by `keys` in `work`, past it in its spill area.
fn built(rows: impl Iterator<Item = Result<Row>>, keys: Vec<JoinKey>, work: &mut Work) -> Result<Built> {
    match Built::of(rows, keys, (work, Full::Table))? {
        Right::Held(built) => Ok(built),
        Right::Partitioned(_) => Err(broken("a mark join's right rows were partitioned")),
    }
}

/// The OR of the truth values the rows of bucket `b` of `built` give `l`.
fn any(built: &Built, b: &[Bucket], (l, marker): (&Row, &Marker)) -> Result<Truth> {
    let mut out = Truth::False;
    for r in built.bucket_rows(b)? {
        out = or(out, marker.truth(l, &*r?)?);
        if out == Truth::True {
            break;
        }
    }
    Ok(out)
}
