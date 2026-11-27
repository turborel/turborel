//! \[DESIGN\] (RULES.md D1226) A column of states written to a spill and
//! read back as it was: its kind, then each group's count, 128-bit total
//! or best value, or the two parts of its approximate sum, by their bits,
//! and the scale of its integers once one came. Its function and result's
//! type are the set function's, given again as it is read back.

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::functions::Wide;
use crate::logical::AggCall;

use super::{Approx, Exact, Typed};

/// The tags of a column's kinds, as written.
const COUNT: u8 = 0;
const EXACT: u8 = 1;
const APPROX: u8 = 2;
const BEST: u8 = 3;

/// Reads `len` counts.
fn counts(reader: &mut Reader<'_>, len: usize) -> Result<Vec<i64>> {
    (0..len).map(|_| reader.u64().map(u64::cast_signed)).collect()
}

impl Typed {
    /// Appends the states of the groups at `places`.
    pub(in crate::exec::table) fn put_groups(&self, places: &[u32], out: &mut Vec<u8>) {
        let groups = || places.iter().map(|&i| crate::storage::mask::index(i));
        let put_counts = |counts: &[i64], to: &mut Vec<u8>| {
            groups().for_each(|g| to.put_u64(counts.get(g).copied().unwrap_or(0).cast_unsigned()));
        };
        match self {
            Typed::Count(counts) => {
                out.put_u8(COUNT);
                put_counts(counts, out);
            }
            Typed::Exact(exact) => {
                out.put_u8(EXACT);
                out.put_u8(u8::from(exact.scale.is_some()));
                out.put_u32(exact.scale.unwrap_or(0));
                groups().for_each(|g| out.put_i128(exact.units.get(g).copied().unwrap_or(0)));
                put_counts(&exact.counts, out);
            }
            Typed::Approx(approx) => {
                out.put_u8(APPROX);
                for g in groups() {
                    let (big, small) = approx.sums.get(g).copied().unwrap_or(crate::functions::NO_FLOATS).parts();
                    out.put_u64(big.to_bits());
                    out.put_u64(small.to_bits());
                }
                put_counts(&approx.counts, out);
            }
            Typed::Best(best) => {
                out.put_u8(BEST);
                best.put_groups(places, out);
            }
        }
    }

    /// Reads the states of `len` groups of `agg` `put_groups` wrote.
    ///
    /// # Errors
    ///
    /// SQLCODE -901 if the bytes are not such states, or not of `agg`.
    pub(in crate::exec::table) fn read(reader: &mut Reader<'_>, len: usize, agg: &AggCall) -> Result<Typed> {
        // A part's column is its function's (RULES.md D1186).
        let own = || Typed::of(agg.func.0.columnar().map(|c| c.0), &agg.result).or_else(|| Typed::best(agg));
        let template = super::super::parts::column_of(agg).or_else(own);
        match (reader.u8()?, template) {
            (COUNT, Some(Typed::Count(_))) => Ok(Typed::Count(counts(reader, len)?)),
            (EXACT, Some(Typed::Exact(Exact { kind, result, .. }))) => {
                let scaled = reader.u8()? != 0;
                let scale = reader.u32()?;
                let units = (0..len).map(|_| reader.i128()).collect::<Result<Vec<_>>>()?;
                let scale = scaled.then_some(scale);
                Ok(Typed::Exact(Exact { kind, units, counts: counts(reader, len)?, scale, result }))
            }
            (APPROX, Some(Typed::Approx(Approx { mean, result, .. }))) => {
                let mut sums = Vec::with_capacity(len);
                for _ in 0..len {
                    let big = f64::from_bits(reader.u64()?);
                    sums.push(Wide::of_parts((big, f64::from_bits(reader.u64()?))));
                }
                Ok(Typed::Approx(Approx { mean, sums, counts: counts(reader, len)?, result }))
            }
            (BEST, Some(Typed::Best(best))) => Ok(Typed::Best(best.read(reader, len)?)),
            (tag, _) => Err(reader.corrupt(&format!("no column of states of this set function has tag {tag}"))),
        }
    }
}

// Unit tests: each column of states read back as written (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests;
