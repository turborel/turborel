//! \[DESIGN\] A selection vector (RULES.md D354): the rows of a batch an
//! operator kept, in ascending order, each a `u32` below the batch's row
//! count, passed from one step to the next in place of the rows copied,
//! as Boncz, Zukowski and Nes, "MonetDB/X100" (2005), §4, pass them. One
//! width serves every batch: a join's held build side is one batch of all
//! its rows, past what 16 bits count.

use super::mask::index;

/// \[DESIGN\] The rows of a batch kept, ascending, each below its row count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sel(Vec<u32>);

impl Sel {
    /// Every one of `rows` rows.
    pub fn all(rows: usize) -> Sel {
        Sel((0..u32::try_from(rows).unwrap_or(u32::MAX)).collect())
    }

    /// The rows `rows`, ascending.
    pub fn of(rows: Vec<u32>) -> Sel {
        Sel(rows)
    }

    /// The rows.
    pub fn as_slice(&self) -> &[u32] {
        &self.0
    }

    /// How many rows.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether it keeps no row.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Its vector, for a kernel to write rows into.
    pub fn buffer(&mut self) -> &mut Vec<u32> {
        &mut self.0
    }

    /// Its rows, owned.
    pub fn into_vec(self) -> Vec<u32> {
        self.0
    }

    /// Its rows as places of a vector.
    pub fn places(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().map(|&r| index(r))
    }
}

impl std::ops::Deref for Sel {
    type Target = [u32];

    fn deref(&self) -> &[u32] {
        &self.0
    }
}

impl FromIterator<u32> for Sel {
    fn from_iter<I: IntoIterator<Item = u32>>(rows: I) -> Sel {
        Sel(rows.into_iter().collect())
    }
}

impl FromIterator<usize> for Sel {
    fn from_iter<I: IntoIterator<Item = usize>>(rows: I) -> Sel {
        Sel(rows_of(rows))
    }
}

/// The places `rows`, as rows of a selection: each below a batch's row
/// count, which fits.
pub fn rows_of(rows: impl IntoIterator<Item = usize>) -> Vec<u32> {
    rows.into_iter().map(|r| u32::try_from(r).unwrap_or(u32::MAX)).collect()
}

/// The count of rows `rows` keeps of a batch of `all` rows.
pub fn kept_count(rows: Option<&[u32]>, all: usize) -> usize {
    rows.map_or(all, <[u32]>::len)
}

/// The place of the `n`th row kept, `rows` or all.
pub fn nth(rows: Option<&[u32]>, n: usize) -> usize {
    rows.map_or(n, |rows| rows.get(n).map_or(usize::MAX, |&r| index(r)))
}

/// The places of the rows `rows` keeps of a batch of `all` rows, or of
/// all, in order.
pub fn places_of(rows: Option<Sel>, all: usize) -> Vec<usize> {
    rows.map_or_else(|| (0..all).collect(), |sel| sel.places().collect())
}
