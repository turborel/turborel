//! \[DESIGN\] An HNSW graph's image in pages, format 1: what a checkpoint
//! keeps of the INMEMORY NEIGHBOR GRAPH so the index is read again, not
//! built again, when the database opens (vector design document §10.1,
//! R14.6). Compact, as it is read whole at open: counts and ids as LEB128
//! varints, each neighbour list sorted and written as gaps, so a list of
//! near ids takes a byte or two an id; vectors as their floats or codes.
//!
//! The stream: whether a codec is kept, and its bytes; the node count,
//! the entry plus one (0 for none), the top layer; then per node its row,
//! count, top layer, vector hash, vector or code, and a list per layer.

use std::sync::Arc;

use super::chain::{Reader, Writer, corrupt, put_floats, put_varint};
use super::graph::{Graph, NONE, Store, slot};
use super::measure::Quantizer;
use super::random::MAX_LEVEL;
use crate::error::Result;

/// Writes `graph`'s image to `w`, its codec's bytes `bytes`, written by
/// the codec before, as no plug-in is called under the graph's lock.
///
/// # Errors
///
/// The buffer pool's.
pub fn write(graph: &Graph, bytes: &[u8], w: &mut Writer<'_>) -> Result<()> {
    let mut out = Vec::new();
    match &graph.store {
        Store::Full(_) => out.push(0),
        Store::Coded { .. } => {
            out.push(1);
            put_varint(&mut out, u64::try_from(bytes.len()).unwrap_or(u64::MAX));
            out.extend_from_slice(bytes);
        }
    }
    put_varint(&mut out, u64::try_from(graph.len()).unwrap_or(u64::MAX));
    put_varint(&mut out, if graph.entry == NONE { 0 } else { u64::from(graph.entry).saturating_add(1) });
    out.push(graph.top);
    w.put(&out)?;
    let mut buf = vec![0.0_f32; graph.dims];
    let mut sorted = Vec::new();
    for id in 0..u32::try_from(graph.len()).unwrap_or(NONE) {
        out.clear();
        node(graph, id, (&mut out, &mut buf, &mut sorted));
        w.put(&out)?;
    }
    Ok(())
}

/// Node `id`'s part of the image, appended to `out`.
fn node(graph: &Graph, id: u32, (out, buf, sorted): (&mut Vec<u8>, &mut [f32], &mut Vec<u32>)) {
    let at = slot(id);
    put_varint(out, graph.rows.get(at).copied().unwrap_or(0));
    put_varint(out, u64::from(graph.counts.get(at).copied().unwrap_or(0)));
    let level = graph.levels.get(at).copied().unwrap_or(0);
    out.push(level);
    out.extend_from_slice(&graph.hashes.get(at).copied().unwrap_or(0).to_le_bytes());
    match graph.code(id) {
        Some(code) => out.extend_from_slice(code),
        None => put_floats(out, graph.vector(id, buf)),
    }
    for l in 0..=level {
        sorted.clear();
        sorted.extend_from_slice(graph.links(id, l));
        sorted.sort_unstable();
        put_varint(out, u64::try_from(sorted.len()).unwrap_or(0));
        let mut before = 0_u32;
        for &n in sorted.iter() {
            put_varint(out, u64::from(n.wrapping_sub(before)));
            before = n;
        }
    }
}

/// What reading an image needs besides its pages.
pub struct Shape<'a> {
    /// Each vector's dimensions.
    pub dims: usize,
    /// M and M0.
    pub m: (usize, usize),
    /// The quantizer that reads a kept codec, if the index has one.
    pub quantizer: Option<&'a Arc<dyn Quantizer>>,
}

/// The graph whose image `r` reads, each node's bytes reserved by
/// `reserve` as it is read.
///
/// # Errors
///
/// The buffer pool's; SQLCODE -916 for a malformed image; `reserve`'s.
pub fn read(r: &mut Reader<'_>, shape: &Shape<'_>, reserve: &mut dyn FnMut(usize) -> Result<()>) -> Result<Graph> {
    let store = match (r.u8()?, shape.quantizer) {
        (0, _) => Store::Full(Vec::new()),
        (1, Some(quantizer)) => {
            let mut bytes = vec![0_u8; usize::try_from(r.varint()?).map_err(|_| corrupt("a codec is too long"))?];
            r.bytes(&mut bytes)?;
            Store::Coded { codec: quantizer.read(&bytes, shape.dims)?, codes: Vec::new() }
        }
        (_, _) => return Err(corrupt("its codec is unknown or its quantizer is missing")),
    };
    let mut graph = Graph::new(shape.dims, shape.m, store);
    let n = u32::try_from(r.varint()?).ok().filter(|n| *n < NONE).ok_or_else(|| corrupt("it holds too many nodes"))?;
    let entry = r.varint()?;
    graph.top = r.u8()?;
    let mut floats = vec![0.0_f32; shape.dims];
    let mut code = graph.code_bytes().map(|b| vec![0_u8; b]).unwrap_or_default();
    let mut list = Vec::new();
    for _ in 0..n {
        read_node(r, &mut graph, (&mut floats, &mut code), (&mut list, n))?;
        reserve(graph.node_bytes(graph.levels.last().copied().unwrap_or(0)))?;
    }
    graph.entry = if entry == 0 { NONE } else { u32::try_from(entry.saturating_sub(1)).unwrap_or(NONE) };
    if (graph.entry == NONE) != (n == 0) || (graph.entry != NONE && graph.entry >= n) || graph.top > MAX_LEVEL {
        return Err(corrupt("its entry or top layer is out of range"));
    }
    Ok(graph)
}

/// Reads one node into `graph`, of `n`, its lists read into `list`.
fn read_node(
    r: &mut Reader<'_>,
    graph: &mut Graph,
    (floats, code): (&mut [f32], &mut [u8]),
    (list, n): (&mut Vec<u32>, u32),
) -> Result<()> {
    let row = r.varint()?;
    let count = u32::try_from(r.varint()?).map_err(|_| corrupt("a count is too large"))?;
    let level = r.u8()?;
    if level > MAX_LEVEL {
        return Err(corrupt("a node's layer is out of range"));
    }
    let hash = r.u64()?;
    let id = if graph.code_bytes().is_some() {
        r.bytes(code)?;
        graph.push_code((row, hash, count), level, code)
    } else {
        r.floats(floats)?;
        graph.push((row, hash, count), level, floats)
    };
    for l in 0..=level {
        let len = usize::try_from(r.varint()?).ok().filter(|len| *len <= graph.most(l)).ok_or_else(|| corrupt("a list is too long"))?;
        list.clear();
        let mut before = 0_u32;
        for _ in 0..len {
            let gap = u32::try_from(r.varint()?).map_err(|_| corrupt("a neighbour is out of range"))?;
            before = before.wrapping_add(gap);
            if before >= n {
                return Err(corrupt("a neighbour names no node"));
            }
            list.push(before);
        }
        graph.set_links(id, l, list);
    }
    Ok(())
}
