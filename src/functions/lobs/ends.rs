//! [ISO/IEC 9075-2:2023 §6.33 GR12, GR13, GR18] The ends a trim removes
//! from a large object, found without holding it whole (RULES.md D79): the
//! characters or octets to remove counted from its start a chunk at a
//! time, and from its end a window at a time, read backwards, so the trim
//! is then a slice.

use crate::error::Result;
use crate::value::lob::{Lob, LobChars, broken};

/// \[DESIGN\] The octets read at once from the end of a large object.
const WINDOW_OCTETS: u64 = 65_536;

/// Whether `b` continues a UTF-8 character, not beginning one.
fn continues(b: u8) -> bool {
    b & 0xC0 == 0x80
}

/// How many characters at the start of `lob`, a character large object,
/// `trimmed` is true of.
///
/// # Errors
///
/// As reading a chunk fails; SQLCODE -910 if its octets are not UTF-8.
pub(crate) fn leading_chars(lob: &Lob, trimmed: &dyn Fn(char) -> bool) -> Result<u64> {
    let mut chars = LobChars::new(lob, 0)?;
    let mut count = 0_u64;
    while let Some(c) = chars.next_char()? {
        if !trimmed(c) {
            break;
        }
        count = count.saturating_add(1);
    }
    Ok(count)
}

/// How many characters at the end of `lob`, a character large object,
/// `trimmed` is true of, counting no more than `most`.
///
/// # Errors
///
/// As `leading_chars`.
pub(crate) fn trailing_chars(lob: &Lob, trimmed: &dyn Fn(char) -> bool, most: u64) -> Result<u64> {
    let mut reader = lob.reader();
    let (mut end, mut count) = (lob.octets(), 0_u64);
    while end > 0 && count < most {
        let start = end.saturating_sub(WINDOW_OCTETS);
        let bytes = reader.read(start, end.saturating_sub(start))?;
        // A window begun within a character leaves its first octets to the
        // window before.
        let skip = if start == 0 { 0 } else { bytes.iter().take_while(|&&b| continues(b)).count() };
        let text = std::str::from_utf8(bytes.get(skip..).unwrap_or_default())
            .map_err(|_| broken("a character large object's octets are not UTF-8"))?;
        for c in text.chars().rev() {
            if count >= most || !trimmed(c) {
                return Ok(count);
            }
            count = count.saturating_add(1);
        }
        let next = start.saturating_add(u64::try_from(skip).unwrap_or(u64::MAX));
        if next >= end {
            return Err(broken("a character large object's window holds no whole character"));
        }
        end = next;
    }
    Ok(count)
}

/// How many octets at the start of `lob` are `octet`.
///
/// # Errors
///
/// As reading a chunk fails.
pub(crate) fn leading_octets(lob: &Lob, octet: u8) -> Result<u64> {
    let mut count = 0_u64;
    lob.reader().each(0, lob.octets(), &mut |piece| {
        let same = piece.iter().take_while(|&&b| b == octet).count();
        count = count.saturating_add(u64::try_from(same).unwrap_or(u64::MAX));
        Ok(same == piece.len())
    })?;
    Ok(count)
}

/// How many octets at the end of `lob` are `octet`, counting no more than
/// `most`.
///
/// # Errors
///
/// As reading a chunk fails.
pub(crate) fn trailing_octets(lob: &Lob, octet: u8, most: u64) -> Result<u64> {
    let mut reader = lob.reader();
    let (mut end, mut count) = (lob.octets(), 0_u64);
    while end > 0 && count < most {
        let start = end.saturating_sub(WINDOW_OCTETS);
        let bytes = reader.read(start, end.saturating_sub(start))?;
        let same = u64::try_from(bytes.iter().rev().take_while(|&&b| b == octet).count()).unwrap_or(u64::MAX);
        count = count.saturating_add(same);
        if same < end.saturating_sub(start) {
            break;
        }
        end = start;
    }
    Ok(count.min(most))
}
