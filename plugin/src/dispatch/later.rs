//! \[EXTENSION\] The calls of the kinds from 10 (RULES.md D220): each a
//! batch, answered by a call of the object's trait for each of it.

use turborel::bytes::{Put, Reader};
use turborel::plugin::payload::{self as p, collation, process, quantizer, ranking, system, types};
use turborel::plugin::values::put_values;
use turborel::plugin::wire::rows;
use turborel::value::Value;
use turborel::value::collation::Pad;

use super::super::registry::{Plugin, find};
use super::super::serve::Cx;
use super::super::{Answer, PluginError};
use super::{f64s_written, read, written};

/// The answer of a kind from 10.
///
/// # Errors
///
/// As `dispatch::answer`.
pub(super) fn answer(plugin: &mut Plugin, cx: &mut Cx<'_>, (kind, op, object): (u8, u8, &str), r: &mut Reader<'_>) -> Answer<Vec<u8>> {
    match kind {
        10 => rank(plugin, object, op, r),
        11 => {
            let t = read(r, quantizer::training)?;
            let trained = find(&mut plugin.quantizers, object)?.train(&t)?;
            let mut out = Vec::new();
            quantizer::put_trained(&mut out, &trained);
            Ok(out)
        }
        12 => {
            let (pad, texts) = read(r, collation::keys)?;
            let c = find(&mut plugin.collations, object)?;
            let keys = texts.iter().map(|t| c.sort_key(t, matches!(pad, Pad::Space))).collect::<Answer<Vec<_>>>()?;
            let mut out = Vec::new();
            collation::put_answered(&mut out, &keys);
            Ok(out)
        }
        13 => system_rows(plugin, object, r),
        14 => {
            let all = read(r, rows)?;
            let f = find(&mut plugin.xquery, object)?;
            let made = all.iter().map(|row| f.call(row)).collect::<Answer<Vec<Value>>>()?;
            written(|o| put_values(o, &made))
        }
        15 => data_type(plugin, object, op, r),
        16 => super::stores::engine(plugin, object, op, r),
        17 => super::stores::index(plugin, cx, object, op, r),
        18 => {
            let (user, password) = read(r, process::verifier)?;
            let v = find(&mut plugin.credentials, object)?.verifier(&user, &password)?;
            let mut out = Vec::new();
            out.put_bytes(&v);
            Ok(out)
        }
        19 => {
            let (events, dropped) = read(r, process::events)?;
            find(&mut plugin.diagnostics, object)?.events(&events, dropped)?;
            Ok(Vec::new())
        }
        _ => Err(PluginError::external(&format!("the plug-in serves no kind {kind}"))),
    }
}

/// A ranking's weights or bounds.
fn rank(plugin: &mut Plugin, object: &str, op: u8, r: &mut Reader<'_>) -> Answer<Vec<u8>> {
    let f = find(&mut plugin.rankings, object)?;
    if op == p::RANKING_BOUNDS {
        let (corpus, terms) = read(r, ranking::bounds)?;
        return Ok(f64s_written(&terms.iter().map(|t| f.bound(&corpus, t)).collect::<Vec<_>>()));
    }
    let (corpus, asked) = read(r, ranking::weights)?;
    Ok(f64s_written(&asked.iter().map(|(t, d)| f.weight(&corpus, t, d)).collect::<Vec<_>>()))
}

/// A page of a system table's rows: all of them asked of the table at
/// the first page, kept until the last is given.
fn system_rows(plugin: &mut Plugin, object: &str, r: &mut Reader<'_>) -> Answer<Vec<u8>> {
    let page = read(r, system::page)?;
    let key = (object.to_owned(), page.user.clone(), page.owner);
    if page.from == 0 || !plugin.held.pages.contains_key(&key) {
        let all = find(&mut plugin.system_tables, object)?.rows(&page.user, page.owner)?;
        plugin.held.pages.insert(key.clone(), all);
    }
    let all = plugin.held.pages.get(&key).map(Vec::as_slice).unwrap_or_default();
    let from = usize::try_from(page.from).unwrap_or(usize::MAX).min(all.len());
    let to = from.saturating_add(usize::try_from(page.max).unwrap_or(usize::MAX)).min(all.len());
    let given = all.get(from..to).unwrap_or_default().to_vec();
    let more = to < all.len();
    if !more {
        plugin.held.pages.remove(&key);
    }
    written(|o| system::put_paged(o, &given, more))
}

/// A type's values made of text or binary forms.
fn data_type(plugin: &mut Plugin, object: &str, op: u8, r: &mut Reader<'_>) -> Answer<Vec<u8>> {
    let t = find(&mut plugin.types, object)?;
    let made = if op == p::TYPE_RECEIVE {
        read(r, types::binaries)?.iter().map(|b| t.receive(b)).collect::<Answer<Vec<_>>>()?
    } else {
        read(r, types::texts)?.iter().map(|s| t.input(s)).collect::<Answer<Vec<_>>>()?
    };
    let mut out = Vec::new();
    types::put_answered(&mut out, &made);
    Ok(out)
}
