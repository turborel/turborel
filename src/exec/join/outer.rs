//! [ISO/IEC 9075:1992 §7.5] The outer join: each row of the left operand
//! with the rows of the right that match it, or, if the left is preserved
//! and none does, with nulls; then, if the right is preserved, its rows
//! that matched none, after nulls. \[DESIGN\] RULES.md D38: with equalities
//! of columns, the right operand is hashed on them while it fits the work
//! area, and both operands are partitioned once it does not
//! (`outer/spilled.rs`); with none, it is read again for each left row, in
//! memory or from a run of the spill area (`rescan.rs`). A right operand
//! held in memory has a flag for each row, whether it matched, counted in
//! the work area with it; one in a run has none, and of a preserved right,
//! the left rows are kept in a run too, so each right row is tried again
//! with every left row once the left is read. Split from exec/join.rs
//! (RULES.md R-SIZE-2).

mod chunked;
mod spilled;

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

use super::super::work::Work;
use super::super::{RowIter, Runner, column, eval_cond, execute, failed};
use super::key;
use super::rescan::{Pass, Rescanned};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::logical::{Cond, OuterJoin, Truth};
use crate::physical::Plan;
use crate::spill::{SpillRun, record_row, row_bytes, row_record};
use crate::storage::Row;
use crate::txn::TxId;
use crate::value::{Bucket, Value, compare};

/// The bytes a right row held for an outer join on keys takes beyond its
/// own: its place in its bucket, and its flag, whether it matched.
pub(super) const PLACE_BYTES: usize = size_of::<usize>() + 1;

/// What an outer join matches on, and keeps.
pub(super) struct Spec {
    pub(super) kind: OuterJoin,
    /// Pairs of a left column and a right column that are equal.
    pub(super) keys: Vec<(usize, usize)>,
    /// The rest of the `<join condition>`, over the pair's columns.
    pub(super) cond: Option<Cond>,
    /// The degrees of the left and right operands, the widths of the nulls.
    pub(super) widths: (usize, usize),
    pub(super) runner: Runner,
}

impl Spec {
    /// `l` concatenated with `r`, if they match: their keys equal
    /// [X3.135-1986 §5.11], and the condition true [§7.5 GR1c].
    pub(super) fn joined(&self, l: &Row, r: &Row) -> Result<Option<Row>> {
        for &(lc, rc) in &self.keys {
            if compare(column(l, lc)?, column(r, rc)?) != Some(Ordering::Equal) {
                return Ok(None);
            }
        }
        let joined: Row = l.iter().chain(r).cloned().collect();
        if let Some(c) = &self.cond
            && eval_cond(c, &joined, &self.runner)? != Truth::True
        {
            return Ok(None);
        }
        Ok(Some(joined))
    }

    /// Left row `l` with nulls for a right row.
    pub(super) fn left_alone(&self, l: &Row) -> Row {
        l.iter().cloned().chain(std::iter::repeat_n(Value::Null, self.widths.1)).collect()
    }

    /// Right row `r` after nulls for a left row.
    pub(super) fn right_alone(&self, r: &Row) -> Row {
        std::iter::repeat_n(Value::Null, self.widths.0).chain(r.iter().cloned()).collect()
    }
}

/// [ISO/IEC 9075:1992 §7.5 GR1c, GR2-GR5] The outer join `plan`. The right
/// operand is read first, and the left only if it succeeded. Never inlined
/// into `execute`, whose frame every level of a plan adds to the stack
/// (RULES.md R-CF-6).
#[inline(never)]
pub(in crate::exec) fn outer(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::OuterJoin { kind, left, right, keys, cond, widths, .. } = plan else {
        return failed(SqlError::new(Code::Api, "[DESIGN] an outer join is not one"));
    };
    let spec = Spec { kind, keys, cond, widths, runner: Runner::new(db, tx) };
    let mut work = Work::of(db, tx);
    let rows = execute(*right, db, tx);
    if spec.keys.is_empty() {
        return match Rescanned::of(rows, &mut work) {
            Ok((scanned, count)) => Box::new(OuterRows::new(execute(*left, db, tx), (Side::Scanned(scanned), count), (spec, work))),
            Err(e) => failed(e),
        };
    }
    match held(rows, &spec.keys, work.limit()) {
        Ok(Held::All(table)) => {
            let count = table.rows.len();
            Box::new(OuterRows::new(execute(*left, db, tx), (Side::Hashed(table), count), (spec, work)))
        }
        Ok(Held::Part(rows, rest)) => spilled::joined(execute(*left, db, tx), (rows, rest), &spec, work),
        Err(e) => failed(e),
    }
}

/// The right operand of an outer join on keys, in memory: its rows, and for
/// each bucket of key values the places of the rows with it, in order.
struct Table {
    rows: Arc<Vec<Row>>,
    buckets: HashMap<Vec<Bucket>, Arc<[usize]>>,
}

/// What of an outer join's right operand fits its work area: all of it,
/// hashed; or the rows that came before it was full, and the rest.
enum Held {
    All(Table),
    Part(Vec<Row>, RowIter),
}

/// The right operand `rows`, hashed on its columns of `keys`, if it holds
/// no more than `limit` bytes, with the places and flags of its rows.
fn held(mut rows: RowIter, keys: &[(usize, usize)], limit: usize) -> Result<Held> {
    let (mut kept, mut bytes, mut buckets) = (Vec::new(), 0_usize, HashMap::<Vec<Bucket>, Vec<usize>>::new());
    while let Some(r) = rows.next() {
        let row = r?;
        bytes = bytes.saturating_add(row_bytes(&row)).saturating_add(PLACE_BYTES);
        if let Some(b) = key(&row, keys.iter().map(|k| k.1))? {
            buckets.entry(b).or_default().push(kept.len());
        }
        kept.push(row);
        if bytes > limit {
            return Ok(Held::Part(kept, rows));
        }
    }
    let buckets = buckets.into_iter().map(|(b, places)| (b, Arc::from(places))).collect();
    Ok(Held::All(Table { rows: Arc::new(kept), buckets }))
}

/// The right operand of an outer join held for it.
enum Side {
    Hashed(Table),
    Scanned(Rescanned),
}

/// The rows of a right operand, each with its place in it.
type Numbered = Box<dyn Iterator<Item = Result<(usize, Row)>> + Send>;

impl Side {
    /// The right rows that may match left row `l`, each with its place:
    /// those of its bucket, or every row.
    fn candidates(&mut self, l: &Row, keys: &[(usize, usize)]) -> Result<Numbered> {
        match self {
            Side::Hashed(table) => {
                let found = key(l, keys.iter().map(|k| k.0))?.and_then(|b| table.buckets.get(&b).cloned()).unwrap_or_else(|| Arc::from([]));
                let rows = Arc::clone(&table.rows);
                let places = (0..found.len()).filter_map(move |at| found.get(at).copied());
                Ok(Box::new(places.filter_map(move |n| rows.get(n).cloned().map(|r| Ok((n, r))))))
            }
            Side::Scanned(right) => Ok(numbered(right.pass()?)),
        }
    }

    /// Every right row, with its place.
    fn all(&mut self) -> Result<Numbered> {
        match self {
            Side::Hashed(table) => Ok(numbered(Rescanned::Held(Arc::clone(&table.rows)).pass()?)),
            Side::Scanned(right) => Ok(numbered(right.pass()?)),
        }
    }
}

/// `pass`, each row with its place in it.
fn numbered(pass: Pass) -> Numbered {
    Box::new(pass.enumerate().map(|(n, r)| r.map(|row| (n, row))))
}

/// How an outer join knows which right rows matched none: a flag for each,
/// of a right operand held in memory; or, of one in a run, the left rows,
/// kept in a run of the work area if the right is preserved, each right
/// row tried with them again.
enum Matched {
    Flags(Vec<bool>),
    Lefts(Option<Box<dyn SpillRun>>),
}

/// The rows of an outer join, as they are pulled.
struct OuterRows {
    spec: Spec,
    left: RowIter,
    right: Side,
    matched: Matched,
    /// The left row being paired, the right rows it is still to be tried
    /// with, and whether one matched it.
    current: Option<(Row, Numbered, bool)>,
    /// Once the left operand is read, the right rows still to look at, if
    /// the right is preserved.
    tail: Option<Numbered>,
    work: Work,
}

impl OuterRows {
    /// The outer join of `left` with `right`, of `count` rows, as `spec`
    /// says, in `work`.
    fn new(left: RowIter, (right, count): (Side, usize), (spec, work): (Spec, Work)) -> OuterRows {
        let matched = match &right {
            Side::Scanned(Rescanned::Spilled(..)) => Matched::Lefts(None),
            Side::Scanned(Rescanned::Held(_)) | Side::Hashed(_) => Matched::Flags(vec![false; count]),
        };
        OuterRows { spec, left, right, matched, current: None, tail: None, work }
    }

    /// The next row of the left row being paired, if it gives one more;
    /// none, and the pairing over, once it gives no more.
    fn paired(&mut self) -> Result<Option<Row>> {
        let Some((l, candidates, found)) = &mut self.current else { return Ok(None) };
        while let Some((n, r)) = candidates.next().transpose()? {
            if let Some(joined) = self.spec.joined(l, &r)? {
                *found = true;
                if let Matched::Flags(flags) = &mut self.matched
                    && let Some(m) = flags.get_mut(n)
                {
                    *m = true;
                }
                return Ok(Some(joined));
            }
        }
        let alone = (!*found && self.spec.kind.keeps_left()).then(|| self.spec.left_alone(l));
        self.current = None;
        Ok(alone)
    }

    /// Starts pairing left row `l`, kept for the right's rows that match
    /// none if they are found so.
    fn begin(&mut self, l: Row) -> Result<()> {
        if self.spec.kind.keeps_right()
            && let Matched::Lefts(lefts) = &mut self.matched
        {
            let run = match lefts {
                Some(run) => run,
                None => lefts.insert(self.work.run()?),
            };
            run.push(&row_record(&l))?;
        }
        let candidates = self.right.candidates(&l, &self.spec.keys)?;
        self.current = Some((l, candidates, false));
        Ok(())
    }

    /// Whether right row `r`, at place `n`, matched no left row.
    fn unmatched(&mut self, (n, r): (usize, &Row)) -> Result<bool> {
        match &mut self.matched {
            Matched::Flags(flags) => Ok(!flags.get(n).copied().unwrap_or(true)),
            Matched::Lefts(None) => Ok(true),
            Matched::Lefts(Some(lefts)) => {
                for record in lefts.records()? {
                    let l = record_row(&record?, self.work.types())?;
                    if self.spec.joined(&l, r)?.is_some() {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
        }
    }

    /// The next row, if there is one.
    fn step(&mut self) -> Result<Option<Row>> {
        loop {
            if self.current.is_some() {
                if let Some(row) = self.paired()? {
                    return Ok(Some(row));
                }
                continue;
            }
            if let Some(tail) = &mut self.tail {
                let Some((n, r)) = tail.next().transpose()? else { return Ok(None) };
                if self.unmatched((n, &r))? {
                    return Ok(Some(self.spec.right_alone(&r)));
                }
                continue;
            }
            if let Some(l) = self.left.next().transpose()? {
                self.begin(l)?;
            } else {
                if !self.spec.kind.keeps_right() {
                    return Ok(None);
                }
                self.tail = Some(self.right.all()?);
            }
        }
    }
}

impl Iterator for OuterRows {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        self.step().transpose()
    }
}
