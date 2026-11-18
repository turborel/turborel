//! \[DESIGN\] How searches and insertions read the graph. The metric, the
//! codec and a search's filter are plug-in calls, and no lock of the index
//! may be held across one (RULES.md R-ERR-4): a filter evaluates rows of a
//! table, and may reach the index again. So a read of the shared graph
//! (`Shared`) takes the read lock for one hop only, copies the neighbour
//! ids, rows and vectors or codes it needs into buffers, lets the lock go,
//! and only then decodes, measures and filters them.
//!
//! Node ids are stable while the graph grows, as nodes are only appended;
//! maintenance alone renumbers them, swapping in a graph of a new epoch.
//! A shared read that finds the epoch changed stops with `Halt::Moved`,
//! and its caller starts again, a bounded number of times.
//!
//! A graph nothing else sees yet, as one being loaded, replayed or
//! repaired, is read directly (`Owned`), with no lock and no copy.

use std::sync::{Arc, RwLock};

use super::graph::{Graph, Store, slot};
use super::measure::Codec;
use super::scratch::Visited;
use crate::error::{Code, SqlError};
use crate::sync::read;

/// \[DESIGN\] The tries of a search or change that maintenance keeps
/// renumbering the graph under, or another writer keeps changing a list
/// under, before it gives up: maintenance is rare, so a few suffice.
pub const TRIES: u32 = 16;

/// Why a read of the graph stopped.
pub enum Halt {
    /// Maintenance renumbered the graph: start again.
    Moved,
    /// A failure, such as a poisoned lock.
    Failed(SqlError),
}

impl From<SqlError> for Halt {
    fn from(error: SqlError) -> Halt {
        Halt::Failed(error)
    }
}

/// A read of the graph that may have to start again.
pub type Step<T> = Result<T, Halt>;

/// The error of a change or search tried `TRIES` times.
pub fn contended() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] an HNSW index kept changing under a search or change of it")
}

/// What a neighbour gives a hop's callback: its id, its row if it is
/// live, and its vector, decoded.
pub type Each<'a> = dyn FnMut(u32, Option<u64>, &[f32]) + 'a;

/// A graph as searches and insertions read it.
pub trait Source {
    /// Each vector's dimensions.
    fn dims(&self) -> usize;
    /// The entry node and the top layer: `NONE` if the graph is empty.
    ///
    /// # Errors
    ///
    /// `Halt::Moved` if the graph was renumbered; a poisoned lock.
    fn entry(&mut self) -> Step<(u32, u8)>;
    /// Gives `each` every neighbour of node `at.0` on layer `at.1` that `visited` has
    /// not seen, marking it.
    ///
    /// # Errors
    ///
    /// As `entry`.
    fn hop(&mut self, at: (u32, u8), visited: &mut Visited, each: &mut Each<'_>) -> Step<()>;
    /// Node `id`'s vector, decoded, into `out`, and its row if it is live.
    ///
    /// # Errors
    ///
    /// As `entry`.
    fn node(&mut self, id: u32, out: &mut Vec<f32>) -> Step<Option<u64>>;
    /// The vectors of `ids`, decoded, one after another, into `out`.
    ///
    /// # Errors
    ///
    /// As `entry`.
    fn vectors(&mut self, ids: &[u32], out: &mut Vec<f32>) -> Step<()>;
    /// Node `at.0`'s neighbours on layer `at.1`, into `out`, and how many
    /// of the first the heuristic kept, if known.
    ///
    /// # Errors
    ///
    /// As `entry`.
    fn links(&mut self, at: (u32, u8), out: &mut Vec<u32>) -> Step<Option<usize>>;
    /// Node `id`'s row, if it is live.
    ///
    /// # Errors
    ///
    /// As `entry`.
    fn row(&mut self, id: u32) -> Step<Option<u64>>;
    /// The rows of `ids`, into `out`.
    ///
    /// # Errors
    ///
    /// As `entry`.
    fn rows(&mut self, ids: &[u32], out: &mut Vec<u64>) -> Step<()>;
}

/// Node `id`'s row in `g`, if it is live.
fn live_row(g: &Graph, id: u32) -> Option<u64> {
    g.is_live(id).then(|| g.rows.get(slot(id)).copied().unwrap_or(0))
}

/// A graph read directly: one nothing else sees.
pub struct Owned<'g> {
    graph: &'g Graph,
    buf: Vec<f32>,
}

impl<'g> Owned<'g> {
    /// A read of `graph`.
    pub fn new(graph: &'g Graph, buf: Vec<f32>) -> Owned<'g> {
        let mut buf = buf;
        buf.resize(graph.dims, 0.0);
        Owned { graph, buf }
    }

    /// Its buffer, for the next read.
    pub fn done(self) -> Vec<f32> {
        self.buf
    }
}

impl Source for Owned<'_> {
    fn dims(&self) -> usize {
        self.graph.dims
    }
    fn entry(&mut self) -> Step<(u32, u8)> {
        Ok((self.graph.entry, self.graph.top))
    }
    #[inline]
    fn hop(&mut self, (id, level): (u32, u8), visited: &mut Visited, each: &mut Each<'_>) -> Step<()> {
        let g = self.graph;
        for &e in g.links(id, level) {
            if visited.first(e) {
                each(e, live_row(g, e), g.vector(e, &mut self.buf));
            }
        }
        Ok(())
    }
    fn node(&mut self, id: u32, out: &mut Vec<f32>) -> Step<Option<u64>> {
        out.clear();
        out.extend_from_slice(self.graph.vector(id, &mut self.buf));
        Ok(live_row(self.graph, id))
    }
    fn vectors(&mut self, ids: &[u32], out: &mut Vec<f32>) -> Step<()> {
        out.clear();
        for &id in ids {
            out.extend_from_slice(self.graph.vector(id, &mut self.buf));
        }
        Ok(())
    }
    fn links(&mut self, (id, level): (u32, u8), out: &mut Vec<u32>) -> Step<Option<usize>> {
        out.clear();
        out.extend_from_slice(self.graph.links(id, level));
        Ok(self.graph.kept(id, level))
    }
    fn row(&mut self, id: u32) -> Step<Option<u64>> {
        Ok(live_row(self.graph, id))
    }
    fn rows(&mut self, ids: &[u32], out: &mut Vec<u64>) -> Step<()> {
        out.clear();
        out.extend(ids.iter().map(|id| self.graph.rows.get(slot(*id)).copied().unwrap_or(0)));
        Ok(())
    }
}

/// A hop's copies, made under the lock and read after it is let go.
#[derive(Default)]
pub struct Hop {
    ids: Vec<u32>,
    rows: Vec<Option<u64>>,
    floats: Vec<f32>,
    codes: Vec<u8>,
    decoded: Vec<f32>,
}

/// The shared graph, read a hop at a time under its read lock.
pub struct Shared<'a> {
    lock: &'a RwLock<Graph>,
    /// The epoch the read began in.
    pub epoch: u64,
    /// The codec of that epoch's graph, if it keeps codes.
    codec: Option<Arc<dyn Codec>>,
    dims: usize,
    hop: Hop,
}

impl<'a> Shared<'a> {
    /// A read of the graph behind `lock`, its copies kept in `hop`.
    ///
    /// # Errors
    ///
    /// A poisoned lock.
    pub fn new(lock: &'a RwLock<Graph>, hop: Hop) -> Result<Shared<'a>, SqlError> {
        let g = read(lock)?;
        let codec = match &g.store {
            Store::Full(_) => None,
            Store::Coded { codec, .. } => Some(Arc::clone(codec)),
        };
        let (epoch, dims) = (g.epoch, g.dims);
        drop(g);
        let mut hop = hop;
        hop.decoded.resize(dims, 0.0);
        Ok(Shared { lock, epoch, codec, dims, hop })
    }

    /// Its copies' buffers, for the next read.
    pub fn done(self) -> Hop {
        self.hop
    }

    /// The codec of the epoch read, if the graph keeps codes.
    pub fn codec(&self) -> Option<&Arc<dyn Codec>> {
        self.codec.as_ref()
    }

    /// Copies node `id`'s vector or code onto the copies, under `g`.
    fn copy(&mut self, g: &Graph, id: u32) {
        match &g.store {
            Store::Full(_) => {
                let mut none = [];
                self.hop.floats.extend_from_slice(g.vector(id, &mut none));
            }
            Store::Coded { .. } => self.hop.codes.extend_from_slice(g.code(id).unwrap_or_default()),
        }
    }

    /// The `at`th vector copied, decoded, the lock let go.
    fn copied(&mut self, at: usize) -> &[f32] {
        let Hop { floats, codes, decoded, .. } = &mut self.hop;
        let dims = self.dims;
        match &self.codec {
            None => floats.get(at.saturating_mul(dims)..at.saturating_add(1).saturating_mul(dims)).unwrap_or_default(),
            Some(codec) => {
                let bytes = codec.code_bytes();
                if let Some(code) = codes.get(at.saturating_mul(bytes)..at.saturating_add(1).saturating_mul(bytes)) {
                    codec.decode(code, decoded);
                }
                decoded
            }
        }
    }

    /// Clears the copies.
    fn clear(&mut self) {
        let Hop { ids, rows, floats, codes, .. } = &mut self.hop;
        ids.clear();
        rows.clear();
        floats.clear();
        codes.clear();
    }

    /// The graph, read, if it is still of the epoch begun in.
    fn graph(&self) -> Step<std::sync::RwLockReadGuard<'a, Graph>> {
        let g = read(self.lock)?;
        if g.epoch != self.epoch {
            return Err(Halt::Moved);
        }
        Ok(g)
    }
}

impl Source for Shared<'_> {
    fn dims(&self) -> usize {
        self.dims
    }
    fn entry(&mut self) -> Step<(u32, u8)> {
        let g = self.graph()?;
        Ok((g.entry, g.top))
    }
    fn hop(&mut self, (id, level): (u32, u8), visited: &mut Visited, each: &mut Each<'_>) -> Step<()> {
        self.clear();
        {
            let g = self.graph()?;
            for &e in g.links(id, level) {
                if visited.first(e) {
                    self.hop.ids.push(e);
                    self.hop.rows.push(live_row(&g, e));
                    self.copy(&g, e);
                }
            }
        }
        for at in 0..self.hop.ids.len() {
            let (Some(e), Some(row)) = (self.hop.ids.get(at).copied(), self.hop.rows.get(at).copied()) else { break };
            each(e, row, self.copied(at));
        }
        Ok(())
    }
    fn node(&mut self, id: u32, out: &mut Vec<f32>) -> Step<Option<u64>> {
        self.clear();
        let row = {
            let g = self.graph()?;
            self.copy(&g, id);
            live_row(&g, id)
        };
        out.clear();
        out.extend_from_slice(self.copied(0));
        Ok(row)
    }
    fn vectors(&mut self, ids: &[u32], out: &mut Vec<f32>) -> Step<()> {
        self.clear();
        {
            let g = self.graph()?;
            for id in ids {
                self.copy(&g, *id);
            }
        }
        out.clear();
        for at in 0..ids.len() {
            out.extend_from_slice(self.copied(at));
        }
        Ok(())
    }
    fn links(&mut self, (id, level): (u32, u8), out: &mut Vec<u32>) -> Step<Option<usize>> {
        let g = self.graph()?;
        out.clear();
        out.extend_from_slice(g.links(id, level));
        Ok(g.kept(id, level))
    }
    fn row(&mut self, id: u32) -> Step<Option<u64>> {
        let g = self.graph()?;
        Ok(live_row(&g, id))
    }
    fn rows(&mut self, ids: &[u32], out: &mut Vec<u64>) -> Step<()> {
        let g = self.graph()?;
        out.clear();
        out.extend(ids.iter().map(|id| g.rows.get(slot(*id)).copied().unwrap_or(0)));
        Ok(())
    }
}
