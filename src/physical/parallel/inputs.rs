//! \[DESIGN\] The inputs of a physical plan's node, for a walk that changes
//! them in place: the planner's marking of parts run in parallel
//! (`physical/parallel.rs`).

use super::super::Plan;

impl Plan {
    /// Calls `each` with each input of this node, in order: not the
    /// members of a recursive query, run as a stratum of their own.
    pub(crate) fn each_input(&mut self, each: &mut dyn FnMut(&mut Plan)) {
        match self {
            Plan::SeqScan { .. }
            | Plan::Nearest(_)
            | Plan::IndexScan { .. }
            | Plan::System { .. }
            | Plan::Values { .. }
            | Plan::Recursive { .. }
            | Plan::WorkTable { .. }
            | Plan::Transition { .. } => {}
            Plan::Filter { input, .. }
            | Plan::IndexJoin { left: input, .. }
            | Plan::Aggregate { input, .. }
            | Plan::Project { input, .. }
            | Plan::Distinct { input, .. }
            | Plan::Sort { input, .. }
            | Plan::Limit { input, .. } => each(input),
            Plan::Unnest(u) => each(&mut u.input),
            Plan::Window { window, .. } => each(&mut window.input),
            Plan::Recognize { recognize, .. } => each(&mut recognize.input),
            Plan::Polymorphic(p) => p.inputs.iter_mut().for_each(each),
            Plan::MergeAppend { inputs, .. } => inputs.iter_mut().for_each(each),
            Plan::Parallel(p) => each(&mut p.input),
            Plan::GroupJoin(g) => [&mut g.build, &mut g.probe, &mut g.instead].into_iter().for_each(each),
            Plan::Counted(c) => each(&mut c.input),
            Plan::MergeJoin(m) => {
                each(&mut m.left);
                each(&mut m.right);
            }
            Plan::RangeJoin(r) => {
                each(&mut r.left);
                each(&mut r.right);
            }
            Plan::SemiJoin(s) => {
                each(&mut s.left);
                each(&mut s.right);
            }
            Plan::MarkJoin(m) => {
                each(&mut m.left);
                each(&mut m.right);
            }
            Plan::Product { left, right, .. }
            | Plan::HashJoin { left, right, .. }
            | Plan::OuterJoin { left, right, .. }
            | Plan::SetOp { left, right, .. }
            | Plan::Append { left, right, .. } => {
                each(left);
                each(right);
            }
        }
    }
}
