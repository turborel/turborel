//! \[DESIGN\] (RULES.md D1325) A morsel's runs of one key handed on an
//! entry each. A morsel that hands its rows on ungrouped makes every row
//! a group of its own (RULES.md D1260, D1313). Where rows of one key come
//! one after another, a grouping looks up the first row of each such run
//! alone (RULES.md D1323), so the rows that repeat the key before them
//! cost a table nothing, and handing each of them on as an entry would
//! give the partitions several times the entries a table gives them. So
//! a run is handed on as one entry: its key and hash its first row's,
//! its first row's number that row's among all the morsel's rows, and
//! its states those of all its rows, each row taken by the entry of its
//! own run and by no other. Two runs of one key are two entries, which
//! the partition merges as it merges two rows of one key.
//!
//! The batches are held with the first row of each run as the rows kept
//! of them, so the keys, the hashes and the places are written of those
//! rows as they are of any rows kept (`apart.rs`). What is kept here is
//! what that leaves out: of each batch that told a run, every row kept
//! of it and where each run ends among them. A batch no row of which
//! repeats the one before it is held as it came, an entry a row.
//!
//! So the rows a batch is held with mean what they mean of rows handed
//! on one by one, the rows that are entries, and nothing that reads them
//! so is changed. The states alone read every row, here: a column that
//! a run's rows set is made whole (`states/typed/apart/runs.rs`), and
//! any other is grown once and fed every row of every batch with the
//! place of its run's entry as its group, as a table's states are fed.

use super::spans;
use crate::error::Result;
use crate::exec::Runner;
use crate::exec::batch::{Fed, Taken, eliminated};
use crate::exec::table::{GroupTable, Pending, narrow};
use crate::storage::sel::{kept_count, nth};
use crate::storage::{Batch, Row, Sel};

/// A morsel's batches as it holds them, each with the rows kept of it.
type Held = [(Batch, Option<Sel>)];

/// A place among a morsel's rows, as a row's number is counted.
fn wide(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// The runs of one batch: every row kept of it, where the batch is held
/// with each run's first row alone, and where each run ends among those
/// rows, the last their count.
#[derive(Debug)]
struct Told {
    rows: Option<Sel>,
    ends: Vec<usize>,
}

impl Told {
    /// Where run `j` begins among the rows kept.
    fn begin(&self, j: usize) -> usize {
        j.checked_sub(1).map_or(0, |before| self.ends.get(before).copied().unwrap_or(0))
    }

    /// From which to which of the rows kept run `j` lies.
    fn span(&self, j: usize) -> (usize, usize) {
        (self.begin(j), self.ends.get(j).copied().unwrap_or(0))
    }

    /// How many rows were kept.
    fn count(&self) -> usize {
        self.ends.last().copied().unwrap_or(0)
    }
}

/// \[DESIGN\] (RULES.md D1325) The runs of a morsel's batches handed on
/// an entry each: of each batch, its runs if it told any longer than a
/// row, and the rows kept of all the batches.
#[derive(Debug, Default)]
pub(in crate::exec) struct Runs {
    told: Vec<Option<Told>>,
    rows: usize,
    any: bool,
}

impl Runs {
    /// The batches `kept`, a morsel's, held an entry a run, and their
    /// runs. `ends_of` tells where each run of the rows kept of a batch
    /// ends among them, the last their count, or none. A batch it tells
    /// runs of, fewer than its rows, is held with the first row of each
    /// run as the rows kept of it. Any other is held as it came.
    pub(in crate::exec) fn held(
        kept: Vec<(Batch, Option<Sel>)>,
        mut ends_of: impl FnMut(&Batch, Option<&[u32]>) -> Option<Vec<usize>>,
    ) -> (Vec<(Batch, Option<Sel>)>, Runs) {
        let (mut held, mut runs) = (Vec::with_capacity(kept.len()), Runs::default());
        for (batch, rows) in kept {
            let given = rows.as_ref().map(Sel::as_slice);
            let count = kept_count(given, batch.rows);
            runs.rows = runs.rows.saturating_add(count);
            // Runs that do not end with the batch's last row tell nothing
            // of the rows after them, so such a batch is held as it came.
            let whole = |ends: &Vec<usize>| ends.len() < count && ends.last() == Some(&count);
            let Some(ends) = ends_of(&batch, given).filter(whole) else {
                runs.told.push(None);
                held.push((batch, rows));
                continue;
            };
            let begins = std::iter::once(0).chain(ends.iter().copied()).take(ends.len());
            let heads: Sel = begins.map(|n| narrow(nth(given, n))).collect();
            runs.any = true;
            runs.told.push(Some(Told { rows, ends }));
            held.push((batch, Some(heads)));
        }
        (held, runs)
    }

    /// Whether any batch is held an entry a run of several rows.
    pub(in crate::exec::table) fn any(&self) -> bool {
        self.any
    }

    /// The rows kept of all the batches, of which the entries are fewer
    /// where any run is told.
    pub(in crate::exec::table) fn rows(&self) -> usize {
        self.rows
    }

    /// The runs of batch `b`, if it is held an entry a run.
    fn told(&self, b: usize) -> Option<&Told> {
        self.told.get(b).and_then(Option::as_ref)
    }

    /// Every row kept of batch `b`, held with the rows `heads`: its own
    /// rows where it is held an entry a run, else `heads`, an entry a
    /// row. None is every row of the batch.
    pub(in crate::exec) fn rows_of<'a>(&'a self, b: usize, heads: Option<&'a Sel>) -> Option<&'a [u32]> {
        match self.told(b) {
            Some(told) => told.rows.as_ref().map(Sel::as_slice),
            None => heads.map(Sel::as_slice),
        }
    }

    /// From which to which of the rows kept of batch `b` each of its
    /// `entries` entries lies, in their order: a run's rows, or one row.
    pub(in crate::exec::table) fn spans(&self, b: usize, entries: usize) -> impl Iterator<Item = (usize, usize)> + '_ {
        let told = self.told(b);
        (0..entries).map(move |j| told.map_or((j, j.saturating_add(1)), |told| told.span(j)))
    }

    /// The number of each entry's first row, the entries of `held` in
    /// their order: `first` and the rows kept before that row in the
    /// morsel, the rows of the runs before it among them.
    pub(in crate::exec::table) fn numbers<'a>(&'a self, held: &'a Held, first: u64) -> impl Iterator<Item = u64> + 'a {
        let mut base = first;
        held.iter().enumerate().flat_map(move |(b, (batch, heads))| {
            let (from, entries) = (base, heads.as_ref().map_or(batch.rows, Sel::len));
            base = base.saturating_add(wide(self.told(b).map_or(entries, Told::count)));
            self.spans(b, entries).map(move |(begin, _)| from.saturating_add(wide(begin)))
        })
    }

    /// Notes in `pending` every row kept of batch `b`, each to be fed to
    /// the entry of its run, the entries of the batch at `places` in
    /// their order, and nothing else: whether the batch is held an entry
    /// a run. Of one held an entry a row nothing is noted.
    pub(super) fn take(&self, b: usize, pending: &mut Pending, places: &[u32]) -> bool {
        let Some(told) = self.told(b) else { return false };
        let count = told.count();
        pending.given.clear();
        pending.read.clear();
        pending.groups.clear();
        pending.given.extend((0..count).map(narrow));
        match &told.rows {
            Some(rows) => pending.read.extend_from_slice(rows),
            None => pending.read.extend((0..count).map(narrow)),
        }
        for (j, &place) in places.iter().enumerate() {
            let (from, to) = told.span(j);
            pending.groups.extend(std::iter::repeat_n(place, to.saturating_sub(from)));
        }
        true
    }
}

impl GroupTable {
    /// \[DESIGN\] (RULES.md D1325) As `feed_placed`, of batches `held`
    /// an entry a run as `runs` says: gives each entry its run's states,
    /// a set function at a time. A column of states that a run's rows
    /// set is made whole, each entry's written once (`States::set_runs`).
    /// Any other is grown to the entries' count and fed every row kept
    /// of every batch, a row of a run with its run's entry's place as
    /// its group, of `lanes`, a batch's each, evaluated of every row.
    ///
    /// # Errors
    ///
    /// As a set function, or an expression it takes, fails.
    pub(super) fn feed_runs(
        &mut self,
        (held, runs): (&Held, &Runs),
        (taken, lanes): (&[Taken], &[Vec<Option<Fed<'_>>>]),
        (scratch, runner): (&mut Row, &Runner),
    ) -> Result<()> {
        let Self { placed, states, aggs, pending, .. } = self;
        let Some(placed) = placed.as_ref() else { return Ok(()) };
        let spans = spans(held);
        for (s, ((column, agg), take)) in states.iter_mut().zip(aggs.iter()).zip(taken).enumerate() {
            if let Some(nulls) = column.set_runs(take, (held, runs), &placed.places) {
                eliminated(nulls, runner);
                continue;
            }
            column.grow_by(placed.places.len());
            for (b, (((batch, heads), of_batch), &(from, to))) in held.iter().zip(lanes).zip(&spans).enumerate() {
                // A batch held an entry a row is taken as rows handed on
                // one by one are.
                if !runs.take(b, pending, placed.places.get(from..to).unwrap_or(&[])) {
                    placed.take(pending, (from, to), heads.as_ref().map(Sel::as_slice));
                }
                if let Some(lane) = of_batch.get(s) {
                    column.feed((&*pending, agg), (batch, take, lane), (&mut *scratch, runner))?;
                }
            }
        }
        placed.take(pending, (0, 0), None);
        Ok(())
    }
}
