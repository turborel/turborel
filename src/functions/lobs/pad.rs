//! [ISO/IEC 9075-2:2023 §6.33 SR16c ii, GR14] LPAD and RPAD of a
//! character large object: a large object as long as 4G characters at
//! most (IL046). The source is cut by a slice, nothing read, or joined to
//! its padding without a copy; the padding is written a chunk at a time
//! to the temporary store (RULES.md D79).

use super::like::held_text;
use crate::error::Result;
use crate::value::Value;
use crate::value::lob::{Lob, LobBuilder, LobKind, MAX_LOB_LENGTH, TempLobs};

/// \[DESIGN\] About how many octets of the padding are pushed to the
/// builder at once.
const BLOCK_OCTETS: usize = 4096;

/// [§6.33 SR16c ii] LR, the most characters a pad of a character large
/// object gives (IL046).
pub const PADDED_LOB_LENGTH: u64 = MAX_LOB_LENGTH;

/// The padding character value `pv`: a character string's, or a large
/// object's held whole up to a chunk; none if null.
///
/// # Errors
///
/// As `held_text`; SQLCODE -910 if it is no character string.
pub(crate) fn padding(pv: &Value) -> Result<Option<String>> {
    if let Value::Lob(l) = pv {
        return held_text(l, "a padding character value").map(Some);
    }
    super::super::strings::text(pv).map(|t| t.map(str::to_owned))
}

/// [GR14e to GR14o] `ps` padded on the left if `left`, else on the right,
/// with `pv` repeated and cut, to `tpl` characters, at least 1, or cut to
/// them; at most LR.
///
/// # Errors
///
/// As reading or writing a chunk fails; SQLCODE -910 if the padding is
/// longer than a chunk and no temporary store is given.
pub(crate) fn padded(ps: &Lob, tpl: u64, pv: &str, left: bool, temp: Option<&dyn TempLobs>) -> Result<Lob> {
    // [GR14f, GR14g, GR14h]
    let (mrl, lps) = (tpl.min(PADDED_LOB_LENGTH), ps.chars());
    // [GR14i, GR14j] PS, or its first MRL characters.
    let Some(mpl) = mrl.checked_sub(lps).filter(|&n| n > 0) else { return ps.slice_chars(0, mrl) };
    // Of an empty PV each repetition adds nothing [GR14m].
    if pv.is_empty() {
        return Ok(ps.clone());
    }
    let pad = repeated(pv, mpl, temp)?;
    // [GR14o]
    Ok(if left { pad.concat(ps) } else { ps.concat(&pad) })
}

/// [GR14k to GR14n] `pv`, not empty, repeated, then cut, to `count`
/// characters.
fn repeated(pv: &str, count: u64, temp: Option<&dyn TempLobs>) -> Result<Lob> {
    let lpv = u64::try_from(pv.chars().count()).unwrap_or(u64::MAX).max(1);
    let times = BLOCK_OCTETS.checked_div(pv.len()).unwrap_or(1).max(1);
    let block = pv.repeat(times);
    let block_chars = lpv.saturating_mul(u64::try_from(times).unwrap_or(u64::MAX));
    let mut built = LobBuilder::new(LobKind::Character, temp);
    let mut left = count;
    while left >= block_chars {
        built.push_str(&block)?;
        left = left.saturating_sub(block_chars);
    }
    let rest: String = pv.chars().cycle().take(usize::try_from(left).unwrap_or(usize::MAX)).collect();
    built.push_str(&rest)?;
    built.finish()
}
