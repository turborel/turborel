//! \[DESIGN\] A mark join's right rows, held (RULES.md D642, D603): in
//! memory, each row once, in vector join tables (`JoinTable`, D368) of
//! the hashes of their keys and marks, of the null list's keys, and, made
//! the first time a left row of a null mark value asks, of every row's
//! keys, so a batch's probe hashes its keys a vector at a time; past the
//! work area in tables of its spill area (`Built`, D38), one by keys and
//! marks, one of the null list, and one by keys alone. Of no condition
//! beyond the keys only the first of rows alike is held (`Marker::same`).

use super::super::super::table::{Hashing, JoinTable, Seed, values_hash};
use super::super::super::work::Work;
use super::super::super::{RowIter, broken, column};
use super::super::{Built, Full, Operand, Rescanned, Right, key};
use super::{Marker, or};
use crate::error::Result;
use crate::hash::xxhash::Xxh3Map;
use crate::logical::Truth;
use crate::physical::JoinKey;
use crate::spill::row_bytes;
use crate::storage::Row;
use crate::value::{Bucket, Value};

/// The hashes a left row probes a mark join's rows by: of its keys, none
/// if a key's value matches nothing; and of its keys and marks, none if a
/// mark value is null.
pub(in crate::exec) type Probe = (Option<u64>, Option<u64>);

/// A mark join's right rows.
pub(in crate::exec) enum Side {
    Memory(Box<Memory>),
    Spilled(Box<Spilled>),
}

/// The right rows in memory, with of each the hash of its keys, and
/// whether it is on the null list.
pub(in crate::exec) struct Memory {
    rows: Vec<Row>,
    keys: Vec<u64>,
    null: Vec<bool>,
    /// The rows not on the null list, by the hashes of keys and marks.
    whole: JoinTable,
    /// The null list by the hashes of keys, of a join of marks.
    nulls: Option<JoinTable>,
    /// Every row by the hashes of keys.
    by_keys: Option<JoinTable>,
    seed: Seed,
}

/// The right rows in tables of the spill area.
pub(in crate::exec) struct Spilled {
    whole: Built,
    nulls: Option<Built>,
    all: Option<Built>,
}

/// How a mark join hashes keys: by the database's seed, a text by SipHash
/// keyed by it (RULES.md D369), never as a direct table's numbers.
fn hashing(seed: &Seed) -> Hashing<'_> {
    Hashing { seed, keyed: true, direct: false }
}

/// The hash of the values of `row` at `on`'s columns of `operand`, each
/// matching a null as its key says; none if one matches nothing.
fn hash_of(row: &Row, on: &[&JoinKey], operand: Operand, how: Hashing<'_>) -> Result<Option<u64>> {
    let at = |k: &&JoinKey| match operand {
        Operand::Left => k.left,
        Operand::Right => k.right,
    };
    let values = on.iter().map(|k| column(row, at(k))).collect::<Result<Vec<&Value>>>()?;
    let nulls: Vec<bool> = on.iter().map(|k| k.nulls).collect();
    values_hash(values.into_iter(), &nulls, how)
}

/// The hashes row `row` of `operand` is held or probes by (`Probe`).
fn probe_of(row: &Row, marker: &Marker, operand: Operand, how: Hashing<'_>) -> Result<Probe> {
    let keys: Vec<&JoinKey> = marker.keys.iter().collect();
    let Some(k) = hash_of(row, &keys, operand, how)? else { return Ok((None, None)) };
    if marker.marks.is_empty() {
        return Ok((Some(k), Some(k)));
    }
    let both: Vec<&JoinKey> = marker.keys.iter().chain(&marker.marks).collect();
    Ok((Some(k), hash_of(row, &both, operand, how)?))
}

impl Side {
    /// The right `rows` of `marker`'s join, held in `work`.
    ///
    /// # Errors
    ///
    /// The rows'; the spill area's.
    pub(in crate::exec) fn of(mut rows: RowIter, marker: &Marker, work: &mut Work) -> Result<Side> {
        let seed = work.seed().clone();
        let mut held = Held::default();
        let mut bytes = 0_usize;
        while let Some(r) = rows.next() {
            let row = r?;
            // A null key matches no left row, so the row never counts.
            let (Some(k), whole) = probe_of(&row, marker, Operand::Right, hashing(&seed))? else { continue };
            if held.seen(whole, &row, marker)? {
                continue;
            }
            bytes = bytes.saturating_add(row_bytes(&row));
            held.push(row, (k, whole));
            if bytes > work.limit() {
                let all = held.rows.into_iter().map(Ok).chain(rows);
                return Ok(Side::Spilled(Box::new(Spilled::of(all, marker, work)?)));
            }
        }
        Ok(Side::Memory(Box::new(held.tables(marker, seed))))
    }

    /// The mark of left row `l`.
    ///
    /// # Errors
    ///
    /// The conditions'; the spill area's.
    pub(in crate::exec) fn mark(&mut self, l: &Row, marker: &Marker) -> Result<Truth> {
        match self {
            Side::Memory(held) => {
                let probe = probe_of(l, marker, Operand::Left, hashing(&held.seed))?;
                held.mark(l, probe, marker)
            }
            Side::Spilled(spilled) => spilled.mark(l, marker),
        }
    }

    /// The mark of left row `l`, whose hashes are `probe`, of rows in
    /// memory; of rows spilled, of `l` alone.
    ///
    /// # Errors
    ///
    /// As `mark`.
    pub(in crate::exec) fn mark_hashed(&mut self, l: &Row, probe: Probe, marker: &Marker) -> Result<Truth> {
        match self {
            Side::Memory(held) => held.mark(l, probe, marker),
            Side::Spilled(spilled) => spilled.mark(l, marker),
        }
    }

    /// How a probe of its rows in memory hashes a batch's keys; none of
    /// rows spilled.
    pub(in crate::exec) fn hashing(&self) -> Option<Hashing<'_>> {
        match self {
            Side::Memory(held) => Some(hashing(&held.seed)),
            Side::Spilled(_) => None,
        }
    }
}

/// The right rows as they are read: of each, the hash of its keys, and of
/// its keys and marks if no mark value is null, else it is on the null
/// list; and the first row of each such hash, for `Marker::same`.
#[derive(Default)]
struct Held {
    rows: Vec<Row>,
    keys: Vec<u64>,
    whole: Vec<u64>,
    null: Vec<bool>,
    firsts: Xxh3Map<u64, usize>,
}

impl Held {
    /// Whether a row of hash `whole` alike to `row` is held: the first
    /// held of its hash, as rows alike are of one hash.
    fn seen(&mut self, whole: Option<u64>, row: &Row, marker: &Marker) -> Result<bool> {
        let Some(h) = whole else { return Ok(false) };
        let Some(&at) = self.firsts.get(&h) else {
            self.firsts.insert(h, self.rows.len());
            return Ok(false);
        };
        let first = self.rows.get(at).ok_or_else(|| broken("a mark join's held row is missing"))?;
        marker.same(first, row)
    }

    /// Holds `row`, of hashes `keys` and `whole`.
    fn push(&mut self, row: Row, (keys, whole): (u64, Option<u64>)) {
        self.rows.push(row);
        self.keys.push(keys);
        self.whole.push(whole.unwrap_or(0));
        self.null.push(whole.is_none());
    }

    /// The rows held, in the tables of `marker`'s join.
    fn tables(self, marker: &Marker, seed: Seed) -> Memory {
        let whole = JoinTable::of(&self.whole, &self.null);
        let nulls = (!marker.marks.is_empty()).then(|| {
            let kept: Vec<bool> = self.null.iter().map(|n| !n).collect();
            JoinTable::of(&self.keys, &kept)
        });
        Memory { rows: self.rows, keys: self.keys, null: self.null, whole, nulls, by_keys: None, seed }
    }
}

impl Memory {
    /// The mark of left row `l`, of hashes `probe`.
    fn mark(&mut self, l: &Row, (keys, whole): Probe, marker: &Marker) -> Result<Truth> {
        let Some(k) = keys else { return Ok(Truth::False) };
        let Some(w) = whole else {
            if self.by_keys.is_none() {
                self.by_keys = Some(JoinTable::of(&self.keys, &vec![false; self.null.len()]));
            }
            let by_keys = self.by_keys.as_ref().map(|t| t.candidates(k));
            return self.any(by_keys.into_iter().flatten(), l, marker);
        };
        let found = self.any(self.whole.candidates(w), l, marker)?;
        if found == Truth::True {
            return Ok(found);
        }
        let nulls = self.nulls.as_ref().map(|t| t.candidates(k));
        Ok(or(found, self.any(nulls.into_iter().flatten(), l, marker)?))
    }

    /// The OR of the truth values the rows at `places` give `l`.
    fn any(&self, places: impl Iterator<Item = usize>, l: &Row, marker: &Marker) -> Result<Truth> {
        let mut out = Truth::False;
        for at in places {
            let r = self.rows.get(at).ok_or_else(|| broken("a mark join's held row is missing"))?;
            out = or(out, marker.truth(l, r)?);
            if out == Truth::True {
                break;
            }
        }
        Ok(out)
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

    /// The mark of left row `l`.
    fn mark(&self, l: &Row, marker: &Marker) -> Result<Truth> {
        let Some(mut b) = key(l, &marker.keys, Operand::Left)? else { return Ok(Truth::False) };
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
