//! \[DESIGN\] Index entries an update keeps (RULES.md D443): of a version
//! superseded, the columns its successor holds unchanged, so an index whose
//! key reads only those shares one entry between the two, which neither the
//! update adds nor collection takes back.

use crate::error::Result;
use crate::txn::Timestamp;
use crate::value::Value;

use super::{Row, RowId};

/// \[DESIGN\] Of a version collection removes, or a checkpoint finds
/// awaiting collection: the commit that superseded it, and the columns, as
/// bits by position below `COLUMN_BITS`, its successor holds unchanged
/// (`TableStorage::keep_entries`). No bit for a column not compared, nor for
/// one past the last bit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Kept {
    /// The commit timestamp of the transaction that superseded it.
    pub at: Timestamp,
    /// The columns kept, a bit each.
    pub unchanged: u64,
}

/// \[DESIGN\] The columns a mask of `Kept::unchanged` has a bit for: those
/// at positions below it. An index reading a column past them shares no
/// entry.
pub const COLUMN_BITS: usize = 64;

/// \[DESIGN\] Where a collection gives each version it removes: its row id,
/// its row and what its successor kept; whether every index entry of it
/// is gone (`TableStorage::gc`).
pub type Removed<'a> = &'a mut dyn Removal;

/// \[DESIGN\] What a collection does with each version it removes: the
/// row of one it `needs` is read and given to `removed`; one it does not
/// need, every entry of it shared with its successor, is not read, its
/// entries counted gone (RULES.md D750).
pub trait Removal {
    /// Whether a version its successor `kept` has entries of its own to
    /// take back, so its row is read. Default: every one has.
    fn needs(&self, _kept: Kept) -> bool {
        true
    }

    /// The columns of a row read for it, the others left null: those its
    /// entries' keys read (RULES.md D751). Default: all.
    fn columns(&self) -> Option<&[usize]> {
        None
    }

    /// Takes back the entries of row `id`, the row of a version removed,
    /// that its successor `kept`: whether every one is gone.
    fn removed(&mut self, id: RowId, row: Row, kept: Kept) -> bool;
}

/// Every version's row is read and given to the function.
impl<F: FnMut(RowId, Row, Kept) -> bool> Removal for F {
    fn removed(&mut self, id: RowId, row: Row, kept: Kept) -> bool {
        self(id, row, kept)
    }
}

/// \[DESIGN\] A removal that reads only the versions `needs` says, of
/// their rows only the `columns` it gives, and gives them to `removed`
/// (`Removal`).
pub struct Needed<'c, N, R> {
    /// As `Removal::needs`.
    pub needs: N,
    /// As `Removal::columns`.
    pub columns: &'c dyn Fn() -> Option<&'c [usize]>,
    /// As `Removal::removed`.
    pub removed: R,
}

impl<N: Fn(Kept) -> bool, R: FnMut(RowId, Row, Kept) -> bool> Removal for Needed<'_, N, R> {
    fn needs(&self, kept: Kept) -> bool {
        (self.needs)(kept)
    }

    fn columns(&self) -> Option<&[usize]> {
        (self.columns)()
    }

    fn removed(&mut self, id: RowId, row: Row, kept: Kept) -> bool {
        (self.removed)(id, row, kept)
    }
}

/// \[DESIGN\] The versions awaiting collection, read as they are pulled,
/// each with what its successor kept (`TableStorage::garbage`).
pub type Garbage = Box<dyn Iterator<Item = Result<(RowId, Row, Kept)>> + Send>;

/// The bit of column `c` in a mask of `Kept::unchanged`, none past the last.
pub fn column_bit(c: usize) -> Option<u64> {
    (c < COLUMN_BITS).then(|| 1u64.checked_shl(u32::try_from(c).unwrap_or(u32::MAX))).flatten()
}

/// The columns of `columns` that `old` and `new`, two versions of a row,
/// hold the same value in, as written out: a value identical in every
/// part, so every key made of it is too (RULES.md D92, D443).
pub fn unchanged(old: &Row, new: &Row, columns: u64) -> u64 {
    let mut kept = 0;
    for at in 0..COLUMN_BITS {
        let Some(bit) = column_bit(at).filter(|bit| columns & bit != 0) else { continue };
        if let (Some(a), Some(b)) = (old.get(at), new.get(at))
            && same(a, b)
        {
            kept |= bit;
        }
    }
    kept
}

/// Whether `a` and `b` are one value as written out: of the common types
/// compared by their parts, of the others by their written form.
pub fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Char(x), Value::Char(y)) => x == y,
        (Value::Exact(x), Value::Exact(y)) => (x.units, x.scale) == (y.units, y.scale),
        (Value::Approx(x), Value::Approx(y)) => x.to_bits() == y.to_bits(),
        (Value::Datetime(x), Value::Datetime(y)) => x == y,
        (Value::Binary(x), Value::Binary(y)) => x == y,
        (x, y) => format!("{x:?}") == format!("{y:?}"),
    }
}

// Unit tests: the comparison by column (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Decimal;

    /// Verifies: [DESIGN] RULES.md D443: a column is kept only when both
    /// versions hold one value as written out; an exact number of another
    /// scale, a float's other zero, or a column not asked about is not.
    #[test]
    fn columns_are_kept_only_when_written_alike() {
        let old = vec![
            Value::Exact(Decimal { units: 10, scale: 1 }),
            Value::Approx(0.0),
            Value::Char("a".into()),
            Value::Null,
            Value::Bool(true),
        ];
        let new = vec![
            Value::Exact(Decimal { units: 100, scale: 2 }),
            Value::Approx(-0.0),
            Value::Char("a".into()),
            Value::Null,
            Value::Bool(true),
        ];
        assert_eq!(unchanged(&old, &new, 0b1_1111), 0b1_1100);
        assert_eq!(unchanged(&old, &new, 0b0_0100), 0b0_0100);
        assert_eq!(unchanged(&old, &old, 0b1_1111), 0b1_1111);
        assert_eq!(unchanged(&old, &new[..2].to_vec(), 0b1_1111), 0);
        assert_eq!(column_bit(63), Some(1_u64 << 63_u32));
        assert_eq!(column_bit(COLUMN_BITS), None);
    }
}
