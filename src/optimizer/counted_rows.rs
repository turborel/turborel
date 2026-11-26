//! \[DESIGN\] COUNT of a value never null counts rows (RULES.md D1172).
//! [X3.135-1986 §5.8 GR1, GR4] COUNT of a value expression counts the
//! rows whose value is not null, once nulls are taken out, and COUNT(*)
//! counts the rows. Of a value that is never null the two are one count,
//! and no null taken out raises no warning [GR1]. So a COUNT, not
//! DISTINCT, of a column known to hold no null (`unnesting::never_null`),
//! or of a literal that is not the null value, becomes COUNT(*), its
//! filter clause, type and label kept. Its rows are then counted with no
//! value read: of a bare table, from what the storage counts (RULES.md
//! D351, D1180); of batches, by their rows (`exec/batch.rs`). The column,
//! read by nothing else, is not read at all.

use super::unnesting::never_null;
use super::{Rule, RuleContext};
use crate::lexer::kw;
use crate::logical::{AggCall, Expr, LogicalPlan};

/// The rule's name, as EXPLAIN shows it and its setting names it.
pub const COUNT_ROWS_OF_VALUES: &str = "count_rows_of_values";

/// COUNT of a value never null counts rows, as the module says.
pub struct CountRowsOfValues;

impl Rule for CountRowsOfValues {
    fn name(&self) -> &'static str {
        COUNT_ROWS_OF_VALUES
    }
    fn apply(&self, plan: &LogicalPlan, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Aggregate { groups, collations, aggs, seeds, input } = plan else { return None };
        let catalog = &*ctx.db.catalog;
        let filled = |a: &AggCall| match &a.arg {
            Some(Expr::Column(c)) => never_null(input, *c, catalog, 0),
            Some(Expr::Const(v)) => !v.is_null(),
            Some(_) | None => false,
        };
        let counted = |a: &AggCall| a.func.0.name() == kw::COUNT && !a.distinct && a.distribution.is_none() && filled(a);
        if !aggs.iter().any(counted) {
            return None;
        }
        let aggs = aggs.iter().map(|a| AggCall { arg: if counted(a) { None } else { a.arg.clone() }, ..a.clone() }).collect();
        Some(LogicalPlan::Aggregate {
            groups: groups.clone(),
            collations: collations.clone(),
            aggs,
            seeds: seeds.clone(),
            input: input.clone(),
        })
    }
}
