//! The mutations of one statement's statistics (RULES.md R-TEST-9,
//! D1089): each step of a statement's planning reads its tables'
//! statistics as the statement first asked them.

use super::super::{Mutation, Package};

/// Mechanisms whose removal must fail the requirements of one snapshot
/// of statistics a statement.
pub const PLANNING_STATISTICS: [Mutation; 5] = [
    Mutation {
        name: "seen-asks-anew",
        package: Package::Core,
        breaks: "a statement's statistics are those it first asked, kept for every later asking (D1089)",
        file: "src/physical/cardinality/seen.rs",
        original: "        let known = self.stats.borrow().get(table).cloned();",
        replacement: "        let known: Option<Option<Arc<dyn Statistics>>> = None;",
        integration: &[],
        unit: &["physical::cardinality::seen::tests::a_statement_s_statistics_are_those_it_first_asked"],
    },
    Mutation {
        name: "exchange-asks-anew",
        package: Package::Core,
        breaks: "a grouping's exchange counts its columns by the statistics its statement saw (D1089)",
        file: "src/physical/parallel/exchange.rs",
        original: "        return db.1.statistics(db.0, table).and_then(|s| s.distinct(c)).is_some();",
        replacement: "        return db.0.statistics(table).and_then(|s| s.distinct(c)).is_some();",
        integration: &[],
        unit: &["physical::parallel::tests::a_grouping_s_exchange_counts_by_the_statistics_its_statement_saw"],
    },
    Mutation {
        name: "transfer-asks-anew",
        package: Package::Core,
        breaks: "a step of predicate transfer counts its values by the statistics its statement saw (D1089)",
        file: "src/physical/transfer/payoff.rs",
        original: "            return db.1.statistics(db.0, table)?.distinct(column);",
        replacement: "            return db.0.statistics(table)?.distinct(column);",
        integration: &[],
        unit: &["physical::transfer::payoff::tests::a_step_counts_by_the_statistics_its_statement_saw"],
    },
    Mutation {
        name: "pregrouped-asks-anew",
        package: Package::Core,
        breaks: "whether grouping first pays is judged by the statistics its statement saw (D1089)",
        file: "src/optimizer/pregrouped.rs",
        original: "    let rows = |plan: &LogicalPlan| estimated_rows(ctx.db, ctx.seen, plan);",
        replacement: "    let rows = |plan: &LogicalPlan| estimated_rows(ctx.db, &crate::physical::Seen::default(), plan);",
        integration: &[],
        unit: &["optimizer::pregrouped::tests::grouping_first_pays_by_the_statistics_its_statement_saw"],
    },
    Mutation {
        name: "learnt-asks-anew",
        package: Package::Core,
        breaks: "a plan learned is measured from its tables as its planning saw them (D1089)",
        file: "src/db/prepared/plans/grown.rs",
        original: "        let (mut found, looked) = (Vec::new(), Looked { st, seen: Some(seen) });",
        replacement: "        let (mut found, looked) = (Vec::new(), Looked { st, seen: Some(seen).filter(|_| false) });",
        integration: &[],
        unit: &["db::prepared::plans::grown::tests::a_plan_learned_is_measured_from_what_its_planning_saw"],
    },
];
