//! \[DESIGN\] A chunk's dictionary written (RULES.md D512): its entries
//! as first seen, or of texts in order as strings compare, so a scan
//! judges a range of them as a range of codes (`judged/sorted.rs`).

use std::collections::HashMap;

use super::super::fields::Cells;
use super::super::packed::pack;
use super::super::{too_large, word};
use super::{MOST_ENTRIES, SORTED};
use crate::error::Result;
use crate::value::compare_strings;

/// Appends `fields` as a dictionary of their values, each one's entry in a
/// code of `bits`, bit-packed; its entries in order as strings compare if
/// `sorted`, they being texts (RULES.md D512), else as first seen.
pub(super) fn write(fields: Cells<'_>, (bits, sorted): (u32, bool), out: &mut Vec<u8>) -> Result<()> {
    let mut places: HashMap<&[u8], usize> = HashMap::new();
    let mut entries: Vec<&[u8]> = Vec::new();
    let codes: Vec<u64> = fields
        .iter()
        .map(|f| {
            f.map_or(0, |f| {
                *places.entry(f).or_insert_with(|| {
                    entries.push(f);
                    entries.len().saturating_sub(1)
                })
            })
        })
        .map(|c| u64::try_from(c).unwrap_or(u64::MAX))
        .collect();
    if entries.len() > MOST_ENTRIES {
        return Err(too_large());
    }
    let sorted = sorted && entries.iter().all(|e| std::str::from_utf8(e).is_ok());
    let codes = if sorted { in_order(&mut entries, codes) } else { codes };
    out.push(u8::try_from(bits).unwrap_or(u8::MAX) | if sorted { SORTED } else { 0 });
    out.extend_from_slice(&word(entries.len())?);
    let mut at = 0_usize;
    out.extend_from_slice(&word(at)?);
    for entry in &entries {
        at = at.checked_add(entry.len()).ok_or_else(too_large)?;
        out.extend_from_slice(&word(at)?);
    }
    for e in &entries {
        out.extend_from_slice(e);
    }
    pack(codes.into_iter(), bits, out);
    Ok(())
}

/// `entries`, texts, put in order as strings compare, padded with spaces
/// [X3.135-1986 §5.11 GR5], their bytes breaking ties; and `codes`, of
/// the entries as they were, as codes of them so ordered.
fn in_order(entries: &mut Vec<&[u8]>, codes: Vec<u64>) -> Vec<u64> {
    let text = |e: &[u8]| std::str::from_utf8(e).unwrap_or_default().to_owned();
    let mut order: Vec<usize> = (0..entries.len()).collect();
    let texts: Vec<String> = entries.iter().map(|e| text(e)).collect();
    order.sort_by(|&a, &b| {
        let (x, y) = (texts.get(a).map_or("", String::as_str), texts.get(b).map_or("", String::as_str));
        compare_strings(x, y).then_with(|| entries.get(a).cmp(&entries.get(b)))
    });
    let mut place = vec![0_u64; entries.len()];
    for (new, &old) in order.iter().enumerate() {
        if let Some(p) = place.get_mut(old) {
            *p = u64::try_from(new).unwrap_or(u64::MAX);
        }
    }
    *entries = order.iter().filter_map(|&old| entries.get(old).copied()).collect();
    // A null's code, 0, moves as entry 0's does; its null bit tells it.
    codes.into_iter().map(|c| usize::try_from(c).ok().and_then(|c| place.get(c)).copied().unwrap_or(0)).collect()
}
