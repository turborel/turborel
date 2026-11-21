//! \[DESIGN\] (RULES.md D490) A groupjoin's groups: its build's rows read
//! into the executor's table (`exec/table.rs`) by their keys, each a group
//! numbered in the order of the build's rows, its grouping columns kept
//! beside it. A row of a null in a key on which a null matches nothing
//! [X3.135-1986 §5.11 GR2] joins no row: of a LEFT join it is a group no
//! row feeds, else it is none. The planner proved no key repeats
//! (`physical/groupjoin/recognized.rs`); a key that does breaks that
//! invariant, SQLCODE -910.

use crate::error::Result;
use crate::exec::batch::{Batches, Kept};
use crate::exec::broken;
use crate::exec::table::{Columns, GroupTable, Key, Seed, hash_rows};
use crate::kernels::Kernels;
use crate::physical::JoinKey;
use crate::spill::row_bytes;
use crate::storage::sel::nth;
use crate::storage::{Batch, Column, Ints, Row, Sel};

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

/// Where each of `columns` is among the vectors of a batch of `layout`.
///
/// # Errors
///
/// SQLCODE -910 if one is not there.
fn placed(columns: impl Iterator<Item = usize>, layout: &[usize]) -> Result<Vec<usize>> {
    columns.map(|c| layout.iter().position(|&at| at == c).ok_or_else(|| broken("a groupjoin's column past its batch"))).collect()
}

impl<S: Columns> Groups<S> {
    /// The groups of the rows of the batches `build`, of `layout`, in
    /// `table`, as `how` says, keyed by hashes of `seed` and `kernels`;
    /// none if they hold more than `limit` bytes, as the groupjoin then
    /// gives way (RULES.md D38).
    ///
    /// # Errors
    ///
    /// As reading the build fails; SQLCODE -910 if a key repeats.
    pub(in crate::exec) fn built(
        mut table: GroupTable<S>,
        (build, layout): (Batches, &[usize]),
        how: Building<'_>,
        (seed, kernels, limit): (&Seed, &dyn Kernels, usize),
    ) -> Result<Option<Groups<S>>> {
        let keys = placed(how.keys.iter().map(|k| k.left), layout)?;
        let kept_at = placed(how.columns.iter().copied(), layout)?;
        let nulls: Vec<bool> = how.keys.iter().map(|k| k.nulls).collect();
        let (mut rows, mut bytes, mut number, mut hashes) = (Vec::new(), 0_usize, 0_u64, Vec::new());
        for batch in build {
            let (batch, given) = batch?;
            let given = given.as_ref().map(Sel::as_slice);
            hash_rows(&batch, (&keys, given), (seed, kernels), &mut hashes);
            for (n, &hash) in hashes.iter().enumerate() {
                let r = nth(given, n);
                let at = number.saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
                let kept: Row = kept_at.iter().map(|&c| batch.cell(c, r).to_value()).collect();
                let group = if !null_key(&batch, (&keys, &nulls), r) {
                    Some(made(&mut table, (&batch, &keys, r), (hash, at))?)
                } else if how.outer {
                    None
                } else {
                    continue;
                };
                bytes = bytes.saturating_add(row_bytes(&kept)).saturating_add(KEPT_BYTES);
                rows.push((group, kept));
            }
            number = number.saturating_add(u64::try_from(hashes.len()).unwrap_or(u64::MAX));
            if table.bytes().saturating_add(bytes) > limit {
                return Ok(None);
            }
        }
        let firsts = vec![u64::MAX; table.len()];
        Ok(Some(Groups { table, rows, firsts }))
    }
}

/// The group made in `table` of row `r` of `batch`, its keys at `keys`
/// hashed `hash`, its number `at`.
///
/// # Errors
///
/// SQLCODE -910 if its key is a group's already.
fn made<S: Columns>(table: &mut GroupTable<S>, (batch, keys, r): (&Batch, &[usize], usize), (hash, at): (u64, u64)) -> Result<u32> {
    let before = table.len();
    let g = table.found_or_made(hash, Key::Row(batch, keys, r), at);
    if table.len() == before {
        return Err(broken("a groupjoin's group operand repeats a key"));
    }
    Ok(g)
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
