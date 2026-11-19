//! SORTED and NEAR, the example's index methods: each index a file of the
//! extension's directory, its entries in order of key, written whole and
//! renamed over the last; WRITE marks it not clean, CHECKPOINT clean.

use std::cmp::Ordering;
use std::io::Write;
use std::path::PathBuf;

use turborel::bytes::{Put, Reader};
use turborel::index::Probe;
use turborel::plugin::values::{put_values, values};
use turborel::storage::RowId;
use turborel::value::{Value, compare};
use turborel_plugin::{Answer, Entries, IndexMethod, Near, PluginError, Probed, Search, Write as Written, coordinates};

/// The method, of either name.
pub struct Sorted;

/// An index: whether clean, and its entries, a key and a row each, in
/// order, one for each insert not removed.
#[derive(Default)]
struct Ix {
    clean: bool,
    entries: Vec<(Vec<Value>, RowId)>,
}

/// An I/O failure, as SQLSTATE 58030.
fn io(e: &std::io::Error) -> PluginError {
    PluginError::new("58030", &e.to_string())
}

/// The order of two keys, then rows.
fn order(a: &(Vec<Value>, RowId), b: &(Vec<Value>, RowId)) -> Ordering {
    let keys = a.0.iter().zip(&b.0).map(|(x, y)| compare(x, y).unwrap_or(Ordering::Equal)).find(|o| o.is_ne());
    keys.unwrap_or(Ordering::Equal).then(a.1.cmp(&b.1))
}

/// The index of `locator`, if its file is there.
fn load(locator: &[u8]) -> Answer<Option<Ix>> {
    let bytes = match std::fs::read(PathBuf::from(String::from_utf8_lossy(locator).into_owned())) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io(&e)),
    };
    let mut r = Reader::new(&bytes, "an index of the example");
    let clean = r.u8()? != 0;
    let entries = (0..r.u32()?).map(|_| Ok((values(&mut r)?, r.u64()?))).collect::<turborel::error::Result<_>>()?;
    Ok(Some(Ix { clean, entries }))
}

/// `ix` written over the file of `locator`, durable.
fn save(locator: &[u8], ix: &Ix) -> Answer<()> {
    let mut out = vec![u8::from(ix.clean)];
    out.put_u32(u32::try_from(ix.entries.len()).unwrap_or(u32::MAX));
    for (key, id) in &ix.entries {
        put_values(&mut out, key)?;
        out.put_u64(*id);
    }
    let to = PathBuf::from(String::from_utf8_lossy(locator).into_owned());
    let beside = to.with_extension("new");
    let mut file = std::fs::File::create(&beside).map_err(|e| io(&e))?;
    file.write_all(&out).and_then(|()| file.sync_all()).map_err(|e| io(&e))?;
    std::fs::rename(&beside, &to).map_err(|e| io(&e))
}

/// The index of `locator`, which must be there.
fn index(locator: &[u8]) -> Answer<Ix> {
    load(locator)?.ok_or_else(|| PluginError::external("no such index"))
}

/// The probe `p` is.
fn probe_of(p: &Probed) -> Probe {
    match p {
        Probed::All => Probe::All,
        Probed::Compare(op, value) => Probe::Compare { op: *op, value: value.clone() },
        Probed::Prefix(equal, then) => Probe::Prefix { equal: equal.clone(), then: then.clone() },
        Probed::Range(equal, low, high) => Probe::Range { equal: equal.clone(), low: Box::new(low.clone()), high: Box::new(high.clone()) },
    }
}

/// A cursor: the last entry given.
fn cursor_of(entry: &(Vec<Value>, RowId)) -> Answer<Vec<u8>> {
    let mut out = Vec::new();
    put_values(&mut out, &entry.0)?;
    out.put_u64(entry.1);
    Ok(out)
}

/// The distance of `key` from `near`'s value, by its metric.
fn distance(near: &Near, key: &[Value]) -> f64 {
    let column = usize::try_from(near.column).unwrap_or(0);
    let (a, b) = (key.get(column).and_then(coordinates).unwrap_or_default(), coordinates(&near.value).unwrap_or_default());
    let dot: f64 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
    match near.metric.as_str() {
        "DOT" => -dot,
        "COSINE" => {
            let norms = a.iter().map(|x| x * x).sum::<f64>().sqrt() * b.iter().map(|x| x * x).sum::<f64>().sqrt();
            if norms > 0.0_f64 { 1.0_f64 - dot / norms } else { 1.0_f64 }
        }
        _ => a.iter().zip(&b).map(|(x, y)| (x - y) * (x - y)).sum::<f64>().sqrt(),
    }
}

impl IndexMethod for Sorted {
    fn create(&mut self, _types: &[String]) -> Answer<Vec<u8>> {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let locator = format!("ix-{}-{stamp}.index", std::process::id()).into_bytes();
        save(&locator, &Ix { clean: true, entries: Vec::new() })?;
        Ok(locator)
    }
    fn open(&mut self, locator: &[u8]) -> Answer<(bool, bool)> {
        Ok(load(locator)?.map_or((false, false), |ix| (true, ix.clean)))
    }
    fn write(&mut self, locator: &[u8], writes: &[Written]) -> Answer<()> {
        let mut ix = index(locator)?;
        ix.clean = false;
        for w in writes {
            let entry = (w.key.clone(), w.id);
            let at = ix.entries.binary_search_by(|e| order(e, &entry));
            match (w.insert, at) {
                (true, Ok(at) | Err(at)) => ix.entries.insert(at, entry),
                (false, Ok(at)) => drop(ix.entries.remove(at)),
                (false, Err(_)) => {}
            }
        }
        save(locator, &ix)
    }
    fn search(&mut self, locator: &[u8], search: &Search) -> Answer<(Entries, Vec<u8>)> {
        let ix = index(locator)?;
        let probe = probe_of(&search.probe);
        let (descending, max) = (search.descending, search.max);
        let after = if search.cursor.is_empty() {
            None
        } else {
            let mut r = Reader::new(&search.cursor, "a cursor");
            Some((values(&mut r)?, r.u64()?))
        };
        let mut all: Vec<&(Vec<Value>, RowId)> = ix.entries.iter().filter(|e| probe.answers(&e.0)).collect();
        if descending {
            all.reverse();
        }
        let wanted = if descending { Ordering::Less } else { Ordering::Greater };
        let mut rest = all.into_iter().filter(|e| after.as_ref().is_none_or(|a| order(e, a) == wanted));
        let page: Vec<&(Vec<Value>, RowId)> = rest.by_ref().take(usize::try_from(max).unwrap_or(usize::MAX)).collect();
        let next = match (rest.next(), page.last()) {
            (Some(_), Some(last)) => cursor_of(last)?,
            (None, _) | (_, None) => Vec::new(),
        };
        Ok((page.into_iter().map(|(key, id)| (*id, key.clone())).collect(), next))
    }
    fn rows(&mut self, locator: &[u8], key: &[Value]) -> Answer<Vec<RowId>> {
        let same = |k: &[Value]| {
            k.len() == key.len() && k.iter().zip(key).all(|(a, b)| (a.is_null() && b.is_null()) || compare(a, b) == Some(Ordering::Equal))
        };
        let mut ids: Vec<RowId> = index(locator)?.entries.iter().filter(|(k, _)| same(k)).map(|(_, id)| *id).collect();
        ids.dedup();
        Ok(ids)
    }
    fn nearest(&mut self, locator: &[u8], near: &Near) -> Answer<Vec<(f64, RowId, Vec<Value>)>> {
        let mut all: Vec<(f64, RowId, Vec<Value>)> =
            index(locator)?.entries.into_iter().map(|(key, id)| (distance(near, &key), id, key)).collect();
        all.sort_by(|a, b| a.0.total_cmp(&b.0));
        if near.descending {
            all.reverse();
        }
        all.truncate(usize::try_from(near.first).unwrap_or(usize::MAX));
        Ok(all)
    }
    fn checkpoint(&mut self, locator: &[u8]) -> Answer<()> {
        let mut ix = index(locator)?;
        ix.clean = true;
        save(locator, &ix)
    }
    fn drop_index(&mut self, locator: &[u8]) -> Answer<()> {
        std::fs::remove_file(PathBuf::from(String::from_utf8_lossy(locator).into_owned())).map_err(|e| io(&e))
    }
}
