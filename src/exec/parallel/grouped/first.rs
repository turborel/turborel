//! \[DESIGN\] (RULES.md D1215) The groups asked of a grouping run in
//! parallel, fixed before its morsels are grouped. A FETCH FIRST with no
//! ORDER BY over a grouping takes any of its groups [ISO/IEC 9075-2:2023
//! §7.17 GR5a], each whole [§7.13 GR2b]. A morsel cannot keep the first
//! keys it meets as a grouping alone does (`batch/groups.rs`), since
//! another morsel would keep others and a group would lack rows. So the
//! statement's thread groups the first morsel by its keys alone, and the
//! first groups it makes, as many as are asked for, are the groups kept.
//!
//! They are told to the grouping's input as a hash join tells its built
//! keys (`table/runtime.rs`, RULES.md D369, D1063): of each grouping
//! column a runtime filter of the kept groups' values of it, a null among
//! them matching a null. The scans skip what the filters' ranges exclude,
//! and each morsel drops the rows a filter excludes before the grouping
//! reads them. A filter drops no row whose value it was made of, so every
//! row of a kept group reaches the grouping, and each group it makes is
//! whole. A row whose every column passes its filter, though no kept
//! group has that key, makes a whole group too, one more than asked for.
//!
//! A group is numbered by its first row, morsel then place
//! (`grouped.rs`), and the groups are given in that order. The kept
//! groups' first rows are in the first morsel, before the first row of
//! any other group there, as they were the first made, and before every
//! row of a later morsel. So they are given first, and are the groups the
//! grouping gives first when none is fixed.
//!
//! Nothing is fixed, and every group is made as before, when the first
//! morsel makes fewer groups than are asked for, when the input is not
//! scans under filters and projections, whose pipeline costs nothing to
//! make twice, or when EXECUTOR.RUNTIME_FILTERS is off. The first morsel
//! is read twice.

use std::sync::Arc;

use super::{Limit, Pipeline};
use crate::db::State;
use crate::error::Result;
use crate::exec::batch::{Read, Told, morsel_groups, read_for, wanted};
use crate::exec::group::unpadded_keys;
use crate::exec::parallel::Held;
use crate::exec::table::{Hashing, KeyHashes, RuntimeFilter};
use crate::exec::{Db, Runner};
use crate::physical::Plan;
use crate::storage::Batch;
use crate::txn::TxId;

/// The runtime filters of the groups the grouping `plan` keeps, of its
/// input's columns, if it is asked for some of its groups and fixes them,
/// as the module says; else none.
///
/// # Errors
///
/// As reading the first morsel fails.
pub(super) fn told(plan: &Plan, (db, tx): (&Db, TxId)) -> Result<Vec<Told>> {
    let Plan::Aggregate { groups, collations, first: Some(first), input, .. } = plan else { return Ok(Vec::new()) };
    let Some((input, unpadded)) = scanned(input).filter(|_| wanted(db)).zip(unpadded_keys(collations)) else { return Ok(Vec::new()) };
    let Held::Kept(pipeline) = Pipeline::of(&input, (db, tx), &Limit::of(db))? else { return Ok(Vec::new()) };
    let Some(read) = pipeline.batched.as_ref().and_then(|by| read_for(groups, &[], &by.layout)) else { return Ok(Vec::new()) };
    if pipeline.count() == 0 {
        return Ok(Vec::new());
    }
    let seed = db.functions.seed().unpadded(&unpadded);
    let (read, runner) = (Read { unpadded, ..read }, Runner::new(db, tx));
    let mut table = morsel_groups(&read, &[], pipeline.morsel_kept(0, (db, tx))?, (&runner, 0, &seed), (*first, None))?;
    if table.len() < *first {
        return Ok(Vec::new());
    }
    table.truncate(*first);
    let (keys, _) = table.into_batch()?;
    filters(&keys, groups, db)
}

/// `plan` with no count of an analyzed plan in it (RULES.md D378), which
/// the morsel read twice would add twice, if it is scans in no order
/// under filters and projections; else none.
fn scanned(plan: &Plan) -> Option<Plan> {
    if let Plan::Counted(c) = plan {
        return scanned(&c.input);
    }
    if matches!(plan, Plan::SeqScan { .. }) {
        return Some(plan.clone());
    }
    let mut own = plan.clone();
    let (Plan::Filter { input, .. } | Plan::Project { input, .. }) = &mut own else { return None };
    **input = scanned(input)?;
    Some(own)
}

/// Of each grouping column, of `groups`, the runtime filter of the
/// values `keys` holds of it, the kept groups' a row each, told of that
/// column of the grouping's input and judged of each morsel whole.
fn filters(keys: &Batch, groups: &[usize], db: &State) -> Result<Vec<Told>> {
    let seed = db.functions.seed().clone();
    let how = Hashing { seed: &seed, keyed: false, direct: false, kernels: None };
    let mut told = Vec::with_capacity(groups.len());
    for (at, &column) in groups.iter().enumerate() {
        let nulls = (0..keys.rows).any(|r| keys.cell(at, r).is_null());
        let mut hashes = KeyHashes::default();
        hashes.of_batch((keys, None), (&[at], &[nulls]), how)?;
        if let Some(filter) = RuntimeFilter::of_built(keys, (at, nulls), &hashes, how) {
            told.push(Told::of(Arc::new(filter), column).given_above());
        }
    }
    Ok(told)
}
