//! PROBE ONLY, never merged: of each chunk of a fixed-width column a scan opens, what the writer's sizer finds of its
//! values in each encoding that can hold them (bytes, frame bits, runs, entries), tallied by column and by the
//! parameters a read cost depends on, so the choice by any cost table is made of the tally afterwards. Printed when
//! the server stops.
#![allow(clippy::all, clippy::pedantic, clippy::restriction, missing_docs)]

use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;

use super::Chunk;
use super::encoded::{Candidates, Sizer};
use super::group::{Pages, Seal, Zone};
use crate::kernels::Portable;
use crate::storage::Mask;
use crate::value::DataType;

/// Column, variant (0 the fields as stored, 1 fields of 16 bytes narrowed to 8), width, kind stored, kind by bytes,
/// frame bits, runs bucket (8 x log2 of the mean run), code bits, entries bucket (16 x entries a row), decimals' bits,
/// whether one value throughout. 255 where the encoding cannot hold the values.
type Key = (u32, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8);
const SUMS: usize = 12;

#[derive(Default)]
struct State {
    seen: HashSet<(u32, u32, u64, u64, u32)>,
    tally: BTreeMap<Key, [u64; SUMS]>,
    types: BTreeMap<u32, String>,
    /// By column and reason: chunks and rows not sized.
    skipped: BTreeMap<(u32, &'static str), [u64; 2]>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn small(n: usize) -> u8 {
    u8::try_from(n).unwrap_or(254).min(254)
}

fn key_of(column: u32, variant: u8, width: usize, stored: u8, c: &Candidates) -> Key {
    let runs_bucket = c.runs.map_or(255, |(runs, _, _)| {
        let mean = c.count as f64 / runs.max(1) as f64;
        small((8.0 * mean.log2()).floor() as usize)
    });
    let fill = c.dictionary.map_or(255, |(entries, _, _)| small(entries * 16 / c.count.max(1)));
    (
        column,
        variant,
        small(width),
        stored,
        c.kind,
        c.frame.map_or(255, |(bits, _)| small(bits as usize)),
        runs_bucket,
        c.dictionary.map_or(255, |(_, bits, _)| small(bits as usize)),
        fill,
        c.alp.map_or(255, |(bits, _)| small(bits as usize)),
        u8::from(c.constant.is_some()),
    )
}

fn sums_of(c: &Candidates, nulls: usize) -> [u64; SUMS] {
    let n = |v: usize| v as u64;
    [
        1,
        n(c.count),
        n(nulls),
        n(c.bytes),
        n(c.fixed.unwrap_or(0)),
        n(c.constant.unwrap_or(0)),
        n(c.frame.map_or(0, |f| f.1)),
        n(c.runs.map_or(0, |r| r.2)),
        n(c.runs.map_or(0, |r| r.0)),
        n(c.dictionary.map_or(0, |d| d.2)),
        n(c.dictionary.map_or(0, |d| d.0)),
        n(c.alp.map_or(0, |a| a.1)),
    ]
}

fn sized(width: usize, floats: bool, words: &[i64], nulls: &Mask) -> Candidates {
    let mut bytes = Vec::with_capacity(words.len() * width);
    for &w in words {
        if width == 16 {
            bytes.extend_from_slice(&i128::from(w).to_le_bytes());
        } else {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
    }
    let mut sizer = Sizer::new(Some(width), (true, floats), None);
    for n in 0..words.len() {
        sizer.add(if nulls.get(n) { None } else { bytes.get(n * width..(n + 1) * width) });
    }
    sizer.candidates()
}

/// Notes the chunk `zone` of column `column` of type `t`, opened of `pages`, its entries as integers `words` if it is a
/// dictionary, its first byte `stored`.
pub fn note<S: Seal>(pages: &Pages<'_, S>, (column, zone, t): (usize, &Zone, &DataType), chunk: &Chunk<'_>, (words, stored): (Option<&[i64]>, u8)) {
    let Some(width) = t.kind().record_width(t).filter(|&w| w == 8 || w == 16) else { return };
    if super::padding(t).is_some() {
        return;
    }
    let column = column as u32;
    let place = (column, pages.file.0, pages.base, zone.run.page.0, zone.run.at);
    {
        let mut state = STATE.lock().unwrap();
        let state = state.get_or_insert_with(State::default);
        if !state.seen.insert(place) {
            return;
        }
        state.types.entry(column).or_insert_with(|| format!("{t:?}").replace(' ', ""));
    }
    let count = chunk.len();
    let (mut units, mut nulls) = (Vec::with_capacity(count), Mask::none(0));
    let read = chunk.ints_with((0, count), (&mut units, &mut nulls), (words, &Portable));
    let floats = matches!(t, DataType::Approximate { .. });
    let made: Vec<(u8, Candidates)> = match read {
        None => Vec::new(),
        Some(()) if width == 16 => vec![(0, sized(16, floats, &units, &nulls)), (1, sized(8, floats, &units, &nulls))],
        Some(()) => vec![(0, sized(8, floats, &units, &nulls))],
    };
    let mut state = STATE.lock().unwrap();
    let state = state.get_or_insert_with(State::default);
    if made.is_empty() {
        let skipped = state.skipped.entry((column, "not-integers")).or_default();
        skipped[0] += 1;
        skipped[1] += count as u64;
        return;
    }
    for (variant, c) in made {
        let sums = sums_of(&c, nulls.count());
        let at = state.tally.entry(key_of(column, variant, if variant == 1 { 8 } else { width }, stored, &c)).or_default();
        for (a, s) in at.iter_mut().zip(sums) {
            *a += s;
        }
    }
}

/// Prints the tally.
pub fn dump() {
    let Ok(state) = STATE.lock() else { return };
    let Some(state) = &*state else { return };
    for (column, t) in &state.types {
        println!("RESIZECOL col={column} type={t}");
    }
    for ((column, why), n) in &state.skipped {
        println!("RESIZESKIP col={column} why={why} chunks={} rows={}", n[0], n[1]);
    }
    for (k, v) in &state.tally {
        println!(
            "RESIZE col={} var={} w={} stored={} byb={} fb={} rq={} db={} dq={} ab={} one={} : {}",
            k.0,
            k.1,
            k.2,
            k.3,
            k.4,
            k.5,
            k.6,
            k.7,
            k.8,
            k.9,
            k.10,
            v.iter().map(u64::to_string).collect::<Vec<_>>().join(" ")
        );
    }
    println!("RESIZEDONE chunks={}", state.seen.len());
}
