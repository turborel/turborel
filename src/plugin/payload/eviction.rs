//! \[DESIGN\] What a BUFFER EVICTION's calls carry (RULES.md D283, D284):
//! the pool's report, and the policy pushed in answer.
//!
//! ```text
//! kind                 op          CALL payload                               RESULT payload
//! 38 buffer eviction   1 advise    report                                     changed u8 | policy if changed
//! report: interval u64 | frames u64 | evicted count u32 | (file u32 | page u64)...
//!         | sampled count u32 | (file u32 | page u64 | uses u32 | age u64 | weight u32)...
//! policy: history u8 | recency u32 | frequency u32 | once u32 | sample u32
//!         | ranges count u32 | (file u32 | first u64 | last u64 | weight u32)...
//! ```

use crate::bytes::{Put, Reader};
use crate::disk::buffer::{Policy, RANGES, Range, Report, Sampled};
use crate::error::Result;

use super::super::errors::violation;

/// Operation 1 of a buffer eviction: the report, answered by a policy.
pub const EVICTION_ADVISE: u8 = 1;

/// A count, as protocol 1 writes it.
fn put_count(out: &mut Vec<u8>, n: usize) {
    out.put_u32(u32::try_from(n).unwrap_or(u32::MAX));
}

/// Writes `report`.
pub fn put_report(out: &mut Vec<u8>, report: &Report) {
    out.put_u64(report.interval);
    out.put_u64(report.frames);
    put_count(out, report.evicted.len());
    for (file, page) in &report.evicted {
        out.put_u32(*file);
        out.put_u64(*page);
    }
    put_count(out, report.sampled.len());
    for s in &report.sampled {
        out.put_u32(s.file);
        out.put_u64(s.page);
        out.put_u32(s.uses);
        out.put_u64(s.age);
        out.put_u32(s.weight);
    }
}

/// Reads a report.
///
/// # Errors
///
/// If `r` does not hold one.
pub fn report(r: &mut Reader<'_>) -> Result<Report> {
    let (interval, frames) = (r.u64()?, r.u64()?);
    let evicted = (0..r.u32()?).map(|_| Ok((r.u32()?, r.u64()?))).collect::<Result<Vec<_>>>()?;
    let sampled = (0..r.u32()?)
        .map(|_| Ok(Sampled { file: r.u32()?, page: r.u64()?, uses: r.u32()?, age: r.u64()?, weight: r.u32()? }))
        .collect::<Result<Vec<_>>>()?;
    Ok(Report { interval, frames, evicted, sampled })
}

/// Writes the answer of an advise: `policy`, or none for no change.
pub fn put_answer(out: &mut Vec<u8>, policy: Option<&Policy>) {
    out.put_u8(u8::from(policy.is_some()));
    let Some(p) = policy else { return };
    out.put_u8(p.history);
    for n in [p.recency, p.frequency, p.once, p.sample] {
        out.put_u32(n);
    }
    put_count(out, p.ranges.len());
    for range in &p.ranges {
        out.put_u32(range.file);
        out.put_u64(range.first);
        out.put_u64(range.last);
        out.put_u32(range.weight);
    }
}

/// Reads the answer of an advise: the policy pushed, none for no change.
///
/// # Errors
///
/// SQLSTATE 38T03 if its HISTORY is neither 1 nor 2, it has more than
/// `RANGES` ranges, or a range ends before it begins; `Reader`'s if `r`
/// holds no answer.
pub fn answer(r: &mut Reader<'_>) -> Result<Option<Policy>> {
    if r.u8()? == 0 {
        return Ok(None);
    }
    let history = r.u8()?;
    let (recency, frequency, once, sample) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?);
    let count = usize::try_from(r.u32()?).unwrap_or(usize::MAX);
    if !(1..=2).contains(&history) || count > RANGES {
        return Err(violation(&format!("a policy's HISTORY is 1 or 2, and it has at most {RANGES} ranges")));
    }
    let mut ranges = Vec::with_capacity(count);
    for _ in 0..count {
        let range = Range { file: r.u32()?, first: r.u64()?, last: r.u64()?, weight: r.u32()? };
        if range.last < range.first {
            return Err(violation("a policy's range ends before it begins"));
        }
        ranges.push(range);
    }
    Ok(Some(Policy { history, recency, frequency, once, sample, ranges }))
}
