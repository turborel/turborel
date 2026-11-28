//! \[DESIGN\] (RULES.md D1313) Key lanes written once, each row's key at
//! the place it is given. A morsel that hands its rows on ungrouped knows
//! where each row lies among its partition's before any key is written
//! (`made/apart.rs`), so a lane is made at its size and each value put
//! where it stays, with no lane appended to a row at a time and none
//! gathered after. A lane of integers of one scale is written straight
//! from the batches' vectors. Any other is appended a value at a time in
//! the order of the places, as a table's lane takes its values, so it is
//! the lane those rows appended and gathered give, of every type.

use super::{Data, Keys, Lane};
use crate::storage::mask::index;
use crate::storage::sel::nth;
use crate::storage::{Batch, Column, Ints, Sel};

/// A morsel's batches as it holds them, each with the rows kept of it.
type Held = [(Batch, Option<Sel>)];

/// The integers of the column at `c` of `batch`, if it holds them so.
fn ints_of(batch: &Batch, c: usize) -> Option<&Ints> {
    if let Some(Column::Ints(ints)) = batch.columns.get(c) { Some(ints) } else { None }
}

/// The scale of the column at `c` of every batch of `held`, if each
/// holds it as integers of one scale.
fn one_scale(held: &Held, c: usize) -> Option<u32> {
    let scale = held.first().and_then(|(batch, _)| ints_of(batch, c)).map(|ints| ints.scale)?;
    held.iter().all(|(batch, _)| ints_of(batch, c).is_some_and(|ints| ints.scale == scale)).then_some(scale)
}

impl Lane {
    /// The lane of the integers of scale `scale` at `c` of the rows kept
    /// of `held`, each at its place of `places`: a null's unit 0, as a
    /// lane appended to holds it, no type yet of a lane of nulls alone,
    /// as one appended to has none, and told to hold a null only if one
    /// came (RULES.md D1274).
    fn placed_ints(held: &Held, (c, scale): (usize, u32), places: &[u32]) -> Lane {
        let (mut units, mut nulls) = (vec![0_i64; places.len()], vec![false; places.len()]);
        let (mut any, mut nulled) = (false, false);
        let mut place = places.iter();
        for (batch, rows) in held {
            let Some(ints) = ints_of(batch, c) else { continue };
            let rows = rows.as_ref().map(Sel::as_slice);
            for n in 0..rows.map_or(batch.rows, <[u32]>::len) {
                let (r, Some(&p)) = (nth(rows, n), place.next()) else { break };
                let null = ints.is_null(r);
                (any, nulled) = (any || !null, nulled || null);
                if let Some(at) = nulls.get_mut(index(p)) {
                    *at = null;
                }
                // A null's unit is left 0, as what its vector holds there
                // has no meaning (RULES.md D1289).
                if let Some(unit) = units.get_mut(index(p)).filter(|_| !null) {
                    *unit = ints.units.get(r).copied().unwrap_or(0);
                }
            }
        }
        let data = if any { Data::Ints { units, scale } } else { Data::Unset };
        Lane { data, nulls, nulled, room: places.len(), ..Lane::default() }
    }

    /// The lane of the values at `c` of the rows `order` names of `held`,
    /// batch and row, appended in that order, of any type.
    fn placed_cells(held: &Held, c: usize, order: &[(u32, u32)], unpadded: bool) -> Lane {
        let mut lane = Lane { unpadded, nulls: Vec::with_capacity(order.len()), room: order.len(), ..Lane::default() };
        for &(b, r) in order {
            if let Some((batch, _)) = held.get(index(b)) {
                lane.push(batch.cell(c, index(r)));
            }
        }
        lane
    }
}

impl Keys {
    /// \[DESIGN\] (RULES.md D1313) The keys of the rows kept of `held`,
    /// of the columns at `at`, the key of the `n`th row as they come at
    /// place `places[n]`, each lane compared as `like`'s is: the keys
    /// those rows appended in the order they come and then put in the
    /// order of their places hold. `sources` names the row at each
    /// place, its batch and its row in it.
    pub(in crate::exec::table) fn placed(held: &Held, at: &[usize], (places, sources): (&[u32], &[(u32, u32)]), like: &Keys) -> Keys {
        let lanes = at.iter().zip(&like.lanes).map(|(&c, like)| {
            if let Some(scale) = one_scale(held, c) {
                return Lane { unpadded: like.unpadded, ..Lane::placed_ints(held, (c, scale), places) };
            }
            Lane::placed_cells(held, c, sources, like.unpadded)
        });
        Keys { lanes: lanes.collect() }
    }
}
