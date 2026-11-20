//! \[DESIGN\] Factoring disjunctions (RULES.md D35).

use super::{Rule, RuleContext};
use crate::logical::{Cond, LogicalPlan, and_all, cond_columns, map_cond_columns, split_and};
use crate::value::CompOp;

/// See [`super::CONSTANT_FOLDING`].
pub const FACTOR_DISJUNCTIONS: &str = "factor_disjunctions";

/// Where a filter's conjunct is a disjunction whose every operand holds
/// the same conjunct, that conjunct is taken out of it, as AND
/// distributes over OR and absorbs it in three-valued logic as in two
/// [X3.135-1986 §5.18 GR2]: `(a AND x) OR (a AND y)` is `a AND (x OR y)`,
/// and `a OR (a AND x)` is `a`. A join's equality repeated in each
/// operand so becomes one the planner joins by. And of each table of a
/// product of which each operand of a disjunction holds a conjunct, the
/// disjunction of those conjuncts is added, implied by the whole, so it
/// can be pushed to the table and filter it first. The filter keeps the
/// same rows; neither applies to one that can raise an error, as each
/// changes which rows a condition is evaluated on (RULES.md D15).
pub struct FactorDisjunctions;

impl Rule for FactorDisjunctions {
    fn name(&self) -> &'static str {
        FACTOR_DISJUNCTIONS
    }
    fn apply(&self, plan: &LogicalPlan, _ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Filter { cond, input } = plan else { return None };
        if cond.can_raise() {
            return None;
        }
        let mut conjuncts = Vec::new();
        split_and(cond.clone(), &mut conjuncts);
        let mut changed = false;
        let mut out = Vec::new();
        for c in conjuncts {
            match factored(&c) {
                Some(parts) => {
                    changed = true;
                    out.extend(parts);
                }
                None => out.push(c),
            }
        }
        let mut leaves = Vec::new();
        leaves_of(input, 0, &mut leaves);
        let mut known: Vec<String> = out.iter().map(key).collect();
        let derived: Vec<Cond> = out.iter().flat_map(|c| implied(c, &leaves)).collect();
        for d in derived {
            if !known.contains(&key(&d)) {
                known.push(key(&d));
                out.push(d);
                changed = true;
            }
        }
        if !changed {
            return None;
        }
        and_all(out).map(|factored| LogicalPlan::Filter { cond: factored, input: input.clone() })
    }
}

/// A condition's form, by which two are the same: of an equality, its
/// operands in either order, as `a = b` is `b = a` [X3.135-1986 §5.11
/// GR4].
fn key(c: &Cond) -> String {
    if let Cond::Compare(a, CompOp::Eq, b) = c {
        let (x, y) = (format!("{a:?}"), format!("{b:?}"));
        return if x <= y { format!("{x} = {y}") } else { format!("{y} = {x}") };
    }
    format!("{c:?}")
}

/// The operands of the disjunction `c`, nested ORs flattened.
fn disjuncts(c: &Cond, out: &mut Vec<Cond>) {
    if let Cond::Or(a, b) = c {
        disjuncts(a, out);
        disjuncts(b, out);
    } else {
        out.push(c.clone());
    }
}

/// The conjuncts of each operand of the disjunction `c`, if it has two or
/// more.
fn operands(c: &Cond) -> Option<Vec<Vec<Cond>>> {
    let mut ors = Vec::new();
    disjuncts(c, &mut ors);
    if ors.len() < 2 {
        return None;
    }
    Some(
        ors.into_iter()
            .map(|d| {
                let mut parts = Vec::new();
                split_and(d, &mut parts);
                parts
            })
            .collect(),
    )
}

/// The disjunction of the conjunctions `parts`.
fn or_of(parts: Vec<Vec<Cond>>) -> Option<Cond> {
    parts.into_iter().filter_map(and_all).reduce(|a, b| Cond::Or(Box::new(a), Box::new(b)))
}

/// The conjuncts the disjunction `c` is, its operands' common conjuncts
/// taken out; none if they have none.
fn factored(c: &Cond) -> Option<Vec<Cond>> {
    let ors = operands(c)?;
    let (first, rest) = ors.split_first()?;
    let mut common: Vec<Cond> = Vec::new();
    for part in first {
        let k = key(part);
        if rest.iter().all(|o| o.iter().any(|p| key(p) == k)) && !common.iter().any(|p| key(p) == k) {
            common.push(part.clone());
        }
    }
    if common.is_empty() {
        return None;
    }
    let shared: Vec<String> = common.iter().map(key).collect();
    let residuals: Vec<Vec<Cond>> = ors.into_iter().map(|o| o.into_iter().filter(|p| !shared.contains(&key(p))).collect()).collect();
    if residuals.iter().any(Vec::is_empty) {
        return Some(common);
    }
    common.extend(or_of(residuals));
    Some(common)
}

/// An operand of a product: its first column, its width, and the forms
/// of the conjuncts filtering it, by its own columns; none if it is not a
/// table's rows, filtered or not, as a disjunction is derived only for
/// those.
struct Leaf {
    start: usize,
    width: usize,
    filtered: Option<Vec<String>>,
}

/// The operands of the products of `plan`, its columns from `start`.
fn leaves_of(plan: &LogicalPlan, start: usize, out: &mut Vec<Leaf>) {
    if let LogicalPlan::Product { left, right } = plan {
        leaves_of(left, start, out);
        leaves_of(right, start.saturating_add(left.width()), out);
        return;
    }
    if let LogicalPlan::Filter { input, .. } = plan
        && matches!(**input, LogicalPlan::Product { .. })
    {
        return leaves_of(input, start, out);
    }
    out.push(Leaf { start, width: plan.width(), filtered: table_filters(plan) });
}

/// The forms of the conjuncts filtering a table's rows `plan`; none if it
/// is not a table's rows, filtered or not.
fn table_filters(plan: &LogicalPlan) -> Option<Vec<String>> {
    if matches!(plan, LogicalPlan::Scan { .. }) {
        return Some(Vec::new());
    }
    let LogicalPlan::Filter { cond, input } = plan else { return None };
    let mut keys = table_filters(input)?;
    let mut parts = Vec::new();
    split_and(cond.clone(), &mut parts);
    keys.extend(parts.iter().map(key));
    Some(keys)
}

/// Of each table of `leaves` of which each operand of the disjunction `c`
/// holds a conjunct, but not each wholly, the disjunction of those
/// conjuncts, unless the table is filtered by it already.
fn implied(c: &Cond, leaves: &[Leaf]) -> Vec<Cond> {
    let Some(ors) = operands(c) else { return Vec::new() };
    let mut out = Vec::new();
    for leaf in leaves {
        let Some(filtered) = &leaf.filtered else { continue };
        let end = leaf.start.saturating_add(leaf.width);
        let within = |p: &Cond| {
            let mut cols = Vec::new();
            cond_columns(p, &mut cols);
            !cols.is_empty() && cols.iter().all(|&i| (leaf.start..end).contains(&i))
        };
        let parts: Vec<Vec<Cond>> = ors.iter().map(|o| o.iter().filter(|p| within(p)).cloned().collect()).collect();
        let whole = ors.iter().all(|o| o.iter().all(within));
        if whole || parts.iter().any(Vec::is_empty) {
            continue;
        }
        let Some(derived) = or_of(parts) else { continue };
        let own = map_cond_columns(derived.clone(), &|i| i.saturating_sub(leaf.start));
        if !filtered.contains(&key(&own)) {
            out.push(derived);
        }
    }
    out
}
