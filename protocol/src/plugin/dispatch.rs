//! \[EXTENSION\] A CALL's payload read, its object's trait called for each
//! row, value or string of its batch, and its RESULT's payload written
//! (RULES.md D167): kinds 1 to 9 here, those from 10 in `later.rs`.

mod later;
mod stores;

use turborel::bytes::{Put, Reader};
use turborel::plugin::bridge::partition::{bound, put_bound, restriction};
use turborel::plugin::payload::{self as p, strings};
use turborel::plugin::values::{put_value, put_values, value, values};
use turborel::plugin::wire::{put_rows, rows};
use turborel::value::Value;

use super::kinds::{Step, Who};
use super::objects::numbered;
use super::registry::{Plugin, find};
use super::serve::Cx;
use super::{Answer, PluginError, coordinates, dense_vector};

/// What a payload's reader fails with.
fn malformed(e: &turborel::error::SqlError) -> PluginError {
    PluginError::external(&format!("the payload is malformed: {}", e.msg))
}

/// `read` of `r`, its failure a malformed payload's.
pub(super) fn read<T>(r: &mut Reader<'_>, read: impl FnOnce(&mut Reader<'_>) -> turborel::error::Result<T>) -> Answer<T> {
    read(r).map_err(|e| malformed(&e))
}

/// `write`'s bytes, its failure an answer the protocol cannot carry.
pub(super) fn written(write: impl FnOnce(&mut Vec<u8>) -> turborel::error::Result<()>) -> Answer<Vec<u8>> {
    let mut out = Vec::new();
    write(&mut out).map_err(|e| PluginError::external(&format!("the answer cannot be written: {}", e.msg)))?;
    Ok(out)
}

/// `xs`, counted, as a RESULT carries numbers.
pub(super) fn f64s_written(xs: &[f64]) -> Vec<u8> {
    let mut out = Vec::new();
    p::put_f64s(&mut out, xs);
    out
}

/// The answer of operation `op` of the object `object` of kind `kind`.
///
/// # Errors
///
/// The object's, or SQLSTATE 38000 of a kind, object or payload unknown.
pub(super) fn answer(plugin: &mut Plugin, cx: &mut Cx<'_>, (kind, op, object): (u8, u8, &str), payload: &[u8]) -> Answer<Vec<u8>> {
    let mut r = p::reader(payload);
    match kind {
        1 => scalar(plugin, cx, object, &mut r),
        2 => aggregate(plugin, object, op, &mut r),
        3 => table(plugin, cx, object, op, &mut r),
        4 => {
            let args = read(&mut r, values)?;
            find(&mut plugin.procedures, object)?.call(cx, &args)?;
            Ok(Vec::new())
        }
        5 => metric(plugin, object, &mut r),
        6 => embed(plugin, object, &mut r),
        7 => authenticate(plugin, object, op, &mut r),
        8 => target(plugin, object, op, &mut r),
        9 => partition(plugin, object, op, &mut r),
        _ => later::answer(plugin, cx, (kind, op, object), &mut r),
    }
}

/// A scalar function, a value of each row.
fn scalar(plugin: &mut Plugin, cx: &mut Cx<'_>, object: &str, r: &mut Reader<'_>) -> Answer<Vec<u8>> {
    let all = read(r, rows)?;
    let f = find(&mut plugin.scalars, object)?;
    let mut out = Vec::with_capacity(all.len());
    for row in &all {
        cx.check()?;
        out.push(f.call(cx, row)?);
    }
    written(|o| put_values(o, &out))
}

/// An aggregate's step, merge or finish.
fn aggregate(plugin: &mut Plugin, object: &str, op: u8, r: &mut Reader<'_>) -> Answer<Vec<u8>> {
    let f = find(&mut plugin.aggregates, object)?;
    let mut state = read(r, |r| r.bytes().map(<[u8]>::to_vec))?;
    match op {
        p::AGGREGATE_STEP => read(r, values)?.iter().try_for_each(|v| f.step(&mut state, v))?,
        p::AGGREGATE_MERGE => f.merge(&mut state, &read(r, |r| r.bytes().map(<[u8]>::to_vec))?)?,
        _ => return finish(&f.finish(&state)?),
    }
    let mut out = Vec::new();
    out.put_bytes(&state);
    Ok(out)
}

/// A FINISH's value, written.
fn finish(v: &Value) -> Answer<Vec<u8>> {
    written(|o| put_value(o, v))
}

/// A u64 read.
pub(super) fn u64_of(r: &mut Reader<'_>) -> turborel::error::Result<u64> {
    r.u64()
}

/// A string read.
fn str_of(r: &mut Reader<'_>) -> turborel::error::Result<String> {
    r.str()
}

/// A table function's reads or rows.
fn table(plugin: &mut Plugin, cx: &mut Cx<'_>, object: &str, op: u8, r: &mut Reader<'_>) -> Answer<Vec<u8>> {
    let f = find(&mut plugin.tables, object)?;
    let args = read(r, values)?;
    if op == p::TABLE_READS {
        let reads = f.reads(&args)?;
        let mut out = Vec::new();
        p::put_strings(&mut out, &reads);
        return Ok(out);
    }
    let read_rows = read(r, |r| (0..r.u32()?).map(|_| rows(r)).collect::<turborel::error::Result<Vec<_>>>())?;
    let made = f.rows(cx, &args, &read_rows)?;
    written(|o| put_rows(o, &made))
}

/// A metric's distances of one vector to each of many.
fn metric(plugin: &mut Plugin, object: &str, r: &mut Reader<'_>) -> Answer<Vec<u8>> {
    let m = find(&mut plugin.metrics, object)?;
    let query = coordinates(&read(r, value)?).unwrap_or_default();
    let candidates = read(r, |r| (0..r.u32()?).map(|_| value(r)).collect::<turborel::error::Result<Vec<_>>>())?;
    let mut out = Vec::with_capacity(candidates.len());
    for c in &candidates {
        let d = m.distance(&query, &coordinates(c).unwrap_or_default())?;
        out.push(d.map_or(Value::Null, Value::Approx));
    }
    written(|o| put_values(o, &out))
}

/// A model's vector of each text.
fn embed(plugin: &mut Plugin, object: &str, r: &mut Reader<'_>) -> Answer<Vec<u8>> {
    let m = find(&mut plugin.embedders, object)?;
    let texts = read(r, strings)?;
    let made = texts.iter().map(|t| m.embed(t).and_then(|xs| Ok(dense_vector(xs)?))).collect::<Answer<Vec<_>>>()?;
    written(|o| put_values(o, &made))
}

/// An authenticator's exchange begun, or its step.
fn authenticate(plugin: &mut Plugin, object: &str, op: u8, r: &mut Reader<'_>) -> Answer<Vec<u8>> {
    let mut out = Vec::new();
    if op == p::AUTH_BEGIN {
        let who = read(r, |r| Ok(Who { user: r.str()?, peer: r.str()?, local: r.u8()? != 0, secure: r.u8()? != 0 }))?;
        let exchange = find(&mut plugin.authenticators, object)?.begin(&who)?;
        let n = plugin.held.number();
        plugin.held.exchanges.insert(n, exchange);
        out.put_u64(n);
        return Ok(out);
    }
    let (n, bytes) = read(r, |r| Ok((r.u64()?, r.bytes()?.to_vec())))?;
    match numbered(&mut plugin.held.exchanges, n, "exchange")?.step(&bytes)? {
        Step::Challenge(b) => {
            out.put_u8(p::AUTH_CHALLENGE);
            out.put_bytes(&b);
        }
        Step::Accept => out.put_u8(p::AUTH_ACCEPT),
        Step::Refuse(why) => {
            out.put_u8(p::AUTH_REFUSE);
            out.put_str(&why);
        }
    }
    Ok(out)
}

/// A backup target's operation.
fn target(plugin: &mut Plugin, object: &str, op: u8, r: &mut Reader<'_>) -> Answer<Vec<u8>> {
    let mut out = Vec::new();
    if op == p::TARGET_OPEN {
        let store = find(&mut plugin.targets, object)?.open(&read(r, str_of)?)?;
        let n = plugin.held.number();
        plugin.held.stores.insert(n, store);
        out.put_u64(n);
        return Ok(out);
    }
    let store = read(r, u64_of)?;
    let held = &mut plugin.held;
    match op {
        p::TARGET_BEGIN => {
            let upload = numbered(&mut held.stores, store, "target")?.begin(&read(r, str_of)?)?;
            let n = held.number();
            held.uploads.insert(n, upload);
            out.put_u64(n);
        }
        p::TARGET_ABANDON => drop(held.uploads.remove(&read(r, u64_of)?)),
        p::TARGET_WRITE => {
            let (n, bytes) = read(r, |r| Ok((r.u64()?, r.bytes()?.to_vec())))?;
            numbered(&mut held.uploads, n, "upload")?.write(&bytes)?;
        }
        p::TARGET_FINISH => {
            let n = read(r, u64_of)?;
            held.uploads.remove(&n).ok_or_else(|| PluginError::external(&format!("no upload has number {n}")))?.finish()?;
        }
        _ => return stored(numbered(&mut held.stores, store, "target")?.as_mut(), op, r),
    }
    Ok(out)
}

/// A backup target's read, list or removal.
fn stored(store: &mut dyn super::kinds::TargetStore, op: u8, r: &mut Reader<'_>) -> Answer<Vec<u8>> {
    let name = read(r, str_of)?;
    let mut out = Vec::new();
    match op {
        p::TARGET_READ => {
            let (offset, max) = read(r, |r| Ok((r.u64()?, r.u32()?)))?;
            out.put_bytes(&store.read(&name, offset, max)?);
        }
        p::TARGET_LIST => p::put_strings(&mut out, &store.list(&name)?),
        p::TARGET_REMOVE => store.remove(&name)?,
        _ => return Err(PluginError::external(&format!("a target has no operation {op}"))),
    }
    Ok(out)
}

/// A partitioning strategy's question.
fn partition(plugin: &mut Plugin, object: &str, op: u8, r: &mut Reader<'_>) -> Answer<Vec<u8>> {
    let s = find(&mut plugin.strategies, object)?;
    let truth = |b: bool| Ok(vec![u8::from(b)]);
    match op {
        p::PARTITION_CHECK_KEY => s.check_key(&read(r, strings)?).map(|()| Vec::new()),
        p::PARTITION_BOUND => {
            let types = read(r, strings)?;
            let made = s.bound(&types, read(r, bound)?)?;
            written(|o| put_bound(o, &made))
        }
        p::PARTITION_OVERLAPS => truth(s.overlaps(&read(r, bound)?, &read(r, bound)?)?),
        p::PARTITION_CONTAINS => truth(s.contains(&read(r, bound)?, &read(r, values)?)?),
        p::PARTITION_ROUTE => {
            let bounds = read(r, |r| (0..r.u32()?).map(|_| bound(r)).collect::<turborel::error::Result<Vec<_>>>())?;
            let key = read(r, values)?;
            let mut out = Vec::new();
            match s.route(&bounds, &key)?.and_then(|at| u32::try_from(at).ok()) {
                Some(at) => {
                    out.put_u8(1);
                    out.put_u32(at);
                }
                None => out.put_u8(0),
            }
            Ok(out)
        }
        _ => {
            let b = read(r, bound)?;
            let restrictions = read(r, |r| (0..r.u32()?).map(|_| restriction(r)).collect::<turborel::error::Result<Vec<_>>>())?;
            truth(s.admits(&b, &restrictions)?)
        }
    }
}
