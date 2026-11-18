//! \[DESIGN\] The merged build, for builds past memory [DiskANN, NeurIPS
//! 2019 §3.1, §4.3]: k-means on a sample, each point sent to its ℓ
//! nearest centres, a Vamana graph built in memory for each cluster's
//! points, and the graphs merged "by taking a simple union of edges".
//!
//! The nodes are already in their pages, so they are the spill. A
//! cluster's vectors are read from them, and its edges written back to
//! them. k grows past `Tuning::clusters` until a cluster's vectors fit the
//! build memory. Each cluster's graph has degree ⌈R / ℓ⌉, so a node's
//! union of ℓ lists about fills its R slots, as the paper's merged index
//! of R = 64 shards (average degree 92.1) stands beside its one-shot index
//! of R = 128 [§4.3]; a union past R is pruned again by RobustPrune.

use crate::error::Result;

use super::DiskAnn;
use super::finite;
use super::kmeans::{kmeans, nearest_centers};
use super::node::Node;
use super::prune::Vectors;
use super::vamana::{self, Build, centroid};

/// Bytes a clustered point costs beside its vector: id, edges, state.
const POINT_OVERHEAD: usize = 64;
/// The most clusters a build makes.
const MOST_CLUSTERS: usize = 4096;

impl DiskAnn {
    /// Edges for nodes `0..n`, written in their pages; the medoid.
    pub(super) fn merged(&self, sample: &[f32], n: u32) -> Result<u32> {
        let tuning = self.tuning()?;
        let (dims, degree) = (self.inner.shape.dims, self.inner.shape.degree);
        let copies = usize::try_from(tuning.copies).unwrap_or(1).max(1);
        let point =
            dims.saturating_mul(size_of::<f32>()).saturating_add(degree.saturating_mul(size_of::<u32>())).saturating_add(POINT_OVERHEAD);
        let points = usize::try_from(n).unwrap_or(usize::MAX).saturating_mul(copies);
        let needed = points.saturating_mul(point).checked_div(tuning.build_memory.max(1)).unwrap_or(0).saturating_add(1);
        let k = usize::try_from(tuning.clusters).unwrap_or(1).max(needed).min(MOST_CLUSTERS);
        let metric = &*self.inner.plugins.metric;
        let centers = kmeans(sample, dims, k, metric, self.inner.seed)?;
        let (members, medoid) = self.assign(&centers, copies, n, &centroid(sample, dims))?;
        let build = Build {
            list: usize::try_from(tuning.build_list).unwrap_or(usize::MAX),
            alpha: tuning.alpha,
            passes: tuning.passes,
            degree: degree.div_ceil(copies).max(1),
            seed: self.inner.seed,
        };
        for (cluster, ids) in members.iter().enumerate() {
            let seed = build.seed ^ u64::try_from(cluster).unwrap_or(0);
            self.build_cluster(ids, Build { seed, ..build })?;
        }
        self.inner.medoid.store(medoid, std::sync::atomic::Ordering::Release);
        self.stitch(n, medoid)?;
        Ok(medoid)
    }

    /// Each node's ℓ nearest clusters, as member lists; and the node
    /// nearest the sample's centroid, the medoid.
    fn assign(&self, centers: &[f32], copies: usize, n: u32, middle: &[f32]) -> Result<(Vec<Vec<u32>>, u32)> {
        let dims = self.inner.shape.dims;
        let metric = &*self.inner.plugins.metric;
        let mut members = vec![Vec::new(); centers.len().checked_div(dims).unwrap_or(0)];
        let mut best = (f32::INFINITY, 0_u32);
        let mut node = Node::default();
        for id in 0..n {
            self.read_node(id, &mut node)?;
            for c in nearest_centers(&node.vector, centers, dims, copies, metric)? {
                if let Some(list) = members.get_mut(c) {
                    list.push(id);
                }
            }
            let d = finite(metric.distance(middle, &node.vector), "metric")?;
            if d < best.0 {
                best = (d, id);
            }
        }
        Ok((members, best.1))
    }

    /// Builds one cluster's graph in memory and unites its edges with
    /// those its points already have.
    fn build_cluster(&self, ids: &[u32], build: Build) -> Result<()> {
        let dims = self.inner.shape.dims;
        let reservation = self.inner.budget.reserve(
            ids.len().saturating_mul(dims.saturating_mul(size_of::<f32>()).saturating_add(build.degree.saturating_mul(size_of::<u32>()))),
            "a cluster of a DiskANN merged build",
        )?;
        let mut data = Vec::with_capacity(ids.len().saturating_mul(dims));
        let mut node = Node::default();
        for &id in ids {
            self.read_node(id, &mut node)?;
            data.extend_from_slice(&node.vector);
        }
        let (graph, _) = vamana::build(Vectors { data: &data, dims }, build, &*self.inner.plugins.metric)?;
        for (local, &id) in ids.iter().enumerate() {
            let edges: Vec<u32> = graph
                .neighbours(u32::try_from(local).unwrap_or(u32::MAX))
                .iter()
                .filter_map(|&l| ids.get(usize::try_from(l).ok()?).copied())
                .collect();
            self.unite(id, &edges)?;
        }
        drop(reservation);
        Ok(())
    }

    /// Links each part of the merged graph the medoid cannot reach, as an
    /// insert links a new node: a search for one of its nodes, the
    /// visited set pruned into its edges and back edges to it, which
    /// makes its whole part reachable. This is beyond the paper, whose
    /// overlapping clusters connect real data [§3.1]. Clusters far apart
    /// may not overlap at all, and an unreachable node is never found.
    fn stitch(&self, n: u32, medoid: u32) -> Result<()> {
        let mut reached = vec![false; usize::try_from(n).unwrap_or(0)];
        self.reach(medoid, &mut reached)?;
        let tuning = self.tuning()?;
        let list = usize::try_from(tuning.build_list).unwrap_or(usize::MAX);
        let width = usize::try_from(tuning.beam_width).unwrap_or(1).max(1);
        let mut node = Node::default();
        for id in 0..n {
            if reached.get(usize::try_from(id).unwrap_or(usize::MAX)).copied().unwrap_or(true) {
                continue;
            }
            self.read_node(id, &mut node)?;
            let expanded = self.walk(&node.vector, (list, width), &mut super::SearchStats::default())?;
            let ids: Vec<u32> = expanded.iter().map(|e| e.id).chain(node.neighbours.iter().copied()).collect();
            let kept = self.reprune(&node.vector, id, &ids, None)?;
            self.unite(id, &kept)?;
            for &j in kept.iter().filter(|&&j| reached.get(usize::try_from(j).unwrap_or(usize::MAX)).copied().unwrap_or(false)) {
                self.link(j, id, &node.vector)?;
            }
            self.reach(id, &mut reached)?;
        }
        Ok(())
    }

    /// Marks every node reachable from `from` in `reached`.
    pub(super) fn reach(&self, from: u32, reached: &mut [bool]) -> Result<()> {
        let mut stack = vec![from];
        let mut node = Node::default();
        while let Some(id) = stack.pop() {
            let Some(mark) = reached.get_mut(usize::try_from(id).unwrap_or(usize::MAX)) else { continue };
            if *mark {
                continue;
            }
            *mark = true;
            self.read_node(id, &mut node)?;
            stack.extend(node.neighbours.iter().copied());
        }
        Ok(())
    }

    /// Node `id`'s edges united with `edges`, pruned again past R.
    fn unite(&self, id: u32, edges: &[u32]) -> Result<()> {
        let mut node = Node::default();
        self.read_node(id, &mut node)?;
        let mut union = node.neighbours.clone();
        union.extend(edges.iter().copied().filter(|e| !node.neighbours.contains(e) && *e != id));
        if union.len() > self.inner.shape.degree {
            union = self.reprune(&node.vector, id, &union, None)?;
        }
        node.neighbours = union;
        self.put(id, &node)
    }
}
