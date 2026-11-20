//! \[DESIGN\] (RULES.md D490) A groupjoin's groups: its build's rows read
//! into the executor's table (`exec/table.rs`) by their keys, each a group
//! numbered in the order of the build's rows, its grouping columns kept
//! beside it. A row of a null in a key on which a null matches nothing
//! [X3.135-1986 §5.11 GR2] joins no row: of a LEFT join it is a group no
//! row feeds, else it is none. The planner proved no key repeats
//! (`physical/groupjoin/recognized.rs`); a key that does breaks that
//! invariant, SQLCODE -910.

use crate::error::Result;
use crate::exec::batch::Kept;
use crate::exec::table::{Columns, GroupTable, Key, Seed, hash_rows, hash_values};
use crate::exec::{RowIter, broken};
use crate::kernels::Kernels;
use crate::physical::JoinKey;
use crate::spill::row_bytes;
use crate::storage::sel::nth;
use crate::storage::{Batch, Column, Ints, Row, Sel};
use crate::value::Value;

/// \[DESIGN\] The bytes a build row kept takes beside its values: its
/// vector's slot and its group's number.
const KEPT_BYTES: usize = 32;

/// A groupjoin's groups, of a table whose groups keep `S` beside their
/// keys: each build row kept, in order, with its group, none of a key that
/// joins nothing; and each group's first probe row's number, `u64::MAX`
/// of one none joined.
pub(in crate::exec) struct Groups<S: Columns> {
    pub(in crate::exec) table: GroupTable<S>,
    pub(in crate::exec) rows: Vec<(Option<u32>, Row)>,
    pub(in crate::exec) firsts: Vec<u64>,
}

/// What a groupjoin's build is read by: its keys, the columns of its rows
/// kept, and whether it is of a LEFT join.
#[derive(Clone, Copy)]
pub(in crate::exec) struct Building<'a> {
    pub(in crate::exec) keys: &'a [JoinKey],
    pub(in crate::exec) columns: &'a [usize],
    pub(in crate::exec) outer: bool,
}

/// The value of column `c` of `row`.
fn column_of(row: &Row, c: usize) -> Result<Value> {
    row.get(c).cloned().ok_or_else(|| broken("a groupjoin's column past its row"))
}

impl<S: Columns> Groups<S> {
    /// The groups of the rows of `build` in `table`, as `how` says, keyed
    /// by hashes of `seed`; none if they hold more than `limit` bytes, as
    /// the groupjoin then gives way (RULES.md D38).
    ///
    /// # Errors
    ///
    /// As reading the build fails; SQLCODE -910 if a key repeats.
    pub(in crate::exec) fn built(
        mut table: GroupTable<S>,
        build: RowIter,
        how: Building<'_>,
        (seed, limit): (&Seed, usize),
    ) -> Result<Option<Groups<S>>> {
        let (mut rows, mut bytes) = (Vec::new(), 0_usize);
        for (n, row) in (0_u64..).zip(build) {
            let row = row?;
            let values = how.keys.iter().map(|k| column_of(&row, k.left)).collect::<Result<Row>>()?;
            let kept = how.columns.iter().map(|&c| column_of(&row, c)).collect::<Result<Row>>()?;
            let joins = how.keys.iter().zip(&values).all(|(k, v)| k.nulls || !v.is_null());
            let group = if joins {
                let hash = hash_values(seed, &values);
                if table.find(hash, Key::Values(&values)).is_some() {
                    return Err(broken("a groupjoin's group operand repeats a key"));
                }
                Some(table.make(hash, Key::Values(&values), n))
            } else if how.outer {
                None
            } else {
                continue;
            };
            bytes = bytes.saturating_add(row_bytes(&kept)).saturating_add(KEPT_BYTES);
            rows.push((group, kept));
            if table.bytes().saturating_add(bytes) > limit {
                return Ok(None);
            }
        }
        let firsts = vec![u64::MAX; table.len()];
        Ok(Some(Groups { table, rows, firsts }))
    }
}

/// Whether row `r` of `batch` has a null in a key, its column at
/// `places`, on which a null matches nothing, as `nulls` says.
pub(in crate::exec) fn null_key(batch: &Batch, (places, nulls): (&[usize], &[bool]), r: usize) -> bool {
    places.iter().zip(nulls).any(|(&at, &matches)| !matches && batch.cell(at, r).is_null())
}

/// \[DESIGN\] (RULES.md D490) What tags each probe row of a groupjoin run
/// in parallel with its group: the build's keys alone, found by the
/// threads at once; where each key is in a probe's batch; and of each
/// key, whether a null matches a null.
pub(in crate::exec) struct Tag {
    pub(in crate::exec) table: GroupTable<()>,
    pub(in crate::exec) places: Vec<usize>,
    pub(in crate::exec) nulls: Vec<bool>,
}

impl Tag {
    /// `kept` with a vector after its own of each row's group, of the
    /// rows given whose keys find one, which alone it gives; keys hashed
    /// by `seed` and `kernels`.
    pub(in crate::exec) fn tagged(&self, (mut batch, rows): Kept, (seed, kernels): (&Seed, &dyn Kernels)) -> Kept {
        let given = rows.as_ref().map(Sel::as_slice);
        let mut hashes = Vec::new();
        hash_rows(&batch, (&self.places, given), (seed, kernels), &mut hashes);
        let (mut units, mut kept, mut scratch) = (vec![0_i64; batch.rows], Vec::new(), Vec::new());
        for (n, &hash) in hashes.iter().enumerate() {
            let r = nth(given, n);
            if null_key(&batch, (&self.places, &self.nulls), r) {
                continue;
            }
            let Some(g) = self.table.found(hash, Key::Row(&batch, &self.places, r), &mut scratch) else { continue };
            if let (Some(unit), Ok(at)) = (units.get_mut(r), u32::try_from(r)) {
                *unit = i64::from(g);
                kept.push(at);
            }
        }
        batch.columns.push(Column::Ints(Ints::new(units, 0, None)));
        (batch, Some(Sel::of(kept)))
    }
}
