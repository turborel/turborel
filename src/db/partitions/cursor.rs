//! \[EXTENSION\] An updatable cursor over a partitioned table (RULES.md
//! D156). Its rows are its partitions', so it reads each partition by a
//! plan of its own, as a typed table's cursor reads each proper subtable
//! (`binder::family_in`), and a positioned UPDATE or DELETE writes the row
//! in the partition the cursor is on (`Session::positioned_member`); an
//! UPDATE that takes the row's key out of the partition's bounds moves it
//! (`move_row`).

use super::super::State;
use crate::catalog::QualifiedName;
use crate::logical::LogicalPlan;

/// Of the plan of an updatable cursor reading a partitioned table, that
/// table and then each partition holding its rows, in the order of the
/// tree; none of a plan that reads none, or another table too.
pub(in crate::db) fn partition_members(st: &State, plan: &LogicalPlan) -> Option<Vec<QualifiedName>> {
    let read = plan.subquery_tables();
    let own: Vec<QualifiedName> = plan.tables().into_iter().filter(|t| !read.contains(t)).collect();
    let [table] = own.as_slice() else { return None };
    let p = st.catalog.partitioning();
    p.is_partitioned(table).then(|| std::iter::once(table.clone()).chain(p.leaves(table)).collect())
}

/// `plan` reading the partition `leaf` of the partitioned table `table`
/// in its place, of the same columns.
pub(in crate::db) fn leaf_plan(plan: LogicalPlan, table: &QualifiedName, leaf: &QualifiedName) -> LogicalPlan {
    if let LogicalPlan::Scan { table: read, names, needed, located } = &plan
        && read == table
    {
        return LogicalPlan::Scan { table: leaf.clone(), names: names.clone(), needed: needed.clone(), located: *located };
    }
    plan.map_inputs(|input| leaf_plan(input, table, leaf))
}
