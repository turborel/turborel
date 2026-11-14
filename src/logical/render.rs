//! The logical plan as text: expressions, conditions and sort keys, for
//! EXPLAIN and error messages.

use std::fmt::Write;

use super::{Cond, Expr, LogicalPlan, Quantifier, SortKey};
use crate::catalog::MatchType;

/// The display name of column `i`, or `#i` if `names` has none for it.
pub fn column_name(names: &[String], i: usize) -> String {
    names.get(i).cloned().unwrap_or_else(|| format!("#{i}"))
}

/// `e` as text, with columns named from `names`.
pub fn render_expr(e: &Expr, names: &[String]) -> String {
    match e {
        Expr::Column(i) => column_name(names, *i),
        Expr::Const(v) => v.to_literal(),
        Expr::Neg(x) => format!("-{}", render_expr(x, names)),
        Expr::Arith(a, op, b) => format!("({} {} {})", render_expr(a, names), op.symbol(), render_expr(b, names)),
        Expr::Outer(_, name) => name.clone(),
        Expr::Dynamic(_) => "?".to_owned(),
        Expr::Call(f, args) => {
            format!("{}({})", f.0.name(), args.iter().map(|a| render_expr(a, names)).collect::<Vec<_>>().join(", "))
        }
        Expr::Case(whens, otherwise) => {
            let mut text = String::from("CASE");
            for (c, result) in whens {
                // Writing to a String cannot fail.
                let _ = write!(text, " WHEN {} THEN {}", render_cond(c, names), render_expr(result, names));
            }
            let _ = write!(text, " ELSE {} END", render_expr(otherwise, names));
            text
        }
        Expr::Cast(x, t) => format!("CAST({} AS {t})", render_expr(x, names)),
        Expr::SortKey(x, c) => format!("{} COLLATE {}", render_expr(x, names), c.collation.name()),
        Expr::Domain(x, d) => {
            // The CAST to the domain's data type it holds is shown as to the domain.
            let operand = if let Expr::Cast(operand, _) = &**x { operand } else { x };
            format!("CAST({} AS {})", render_expr(operand, names), d.domain)
        }
        Expr::Subquery(_, column) => format!("(<subquery>).{}", column.saturating_add(1)),
        Expr::Truth(c) => format!("({})", render_cond(c, names)),
        Expr::NextValue(seq) => format!("NEXT VALUE FOR {}", seq.name),
    }
}

/// A row of values as text: one alone, or in parentheses.
fn render_row(values: &[Expr], names: &[String]) -> String {
    match values {
        [one] => render_expr(one, names),
        _ => format!("({})", values.iter().map(|e| render_expr(e, names)).collect::<Vec<_>>().join(", ")),
    }
}

/// `c` as text, with columns named from `names`.
pub fn render_cond(c: &Cond, names: &[String]) -> String {
    match c {
        Cond::Const(t) => format!("{t:?}").to_uppercase(),
        Cond::Or(a, b) => format!("({} OR {})", render_cond(a, names), render_cond(b, names)),
        Cond::And(a, b) => format!("{} AND {}", render_cond(a, names), render_cond(b, names)),
        Cond::Not(a) => format!("NOT ({})", render_cond(a, names)),
        Cond::Compare(a, op, b) => format!("{} {} {}", render_expr(a, names), op.symbol(), render_expr(b, names)),
        Cond::Distinct(a, b) => format!("{} IS DISTINCT FROM {}", render_expr(a, names), render_expr(b, names)),
        Cond::IsNull(e, not) => format!("{} IS {}NULL", render_expr(e, names), if *not { "NOT " } else { "" }),
        Cond::Exists(_) => "EXISTS (<subquery>)".to_string(),
        Cond::Value(e) => render_expr(e, names),
        Cond::Quantified(values, op, q, _) => {
            let quantifier = match q {
                Quantifier::All => " ALL",
                Quantifier::Some => " ANY",
                Quantifier::One => "",
            };
            format!("{} {}{quantifier} (<subquery>)", render_row(values, names), op.symbol())
        }
        Cond::Is(a, t, not) => {
            format!("({}) IS {}{}", render_cond(a, names), if *not { "NOT " } else { "" }, format!("{t:?}").to_uppercase())
        }
        Cond::Unique(_, nulls_distinct) => format!("UNIQUE {}(<subquery>)", if *nulls_distinct { "" } else { "NULLS NOT DISTINCT " }),
        Cond::Match { values, unique, match_type, .. } => {
            let kind = match match_type {
                MatchType::Simple => "",
                MatchType::Partial => " PARTIAL",
                MatchType::Full => " FULL",
            };
            format!("{} MATCH{}{kind} (<subquery>)", render_row(values, names), if *unique { " UNIQUE" } else { "" })
        }
        Cond::In(e, set) => {
            let values: Vec<String> = set.values().iter().map(|v| render_expr(&Expr::Const(v.clone()), names)).collect();
            format!("{} IN ({})", render_expr(e, names), values.join(", "))
        }
        Cond::Like(e, pattern, escape) => format!(
            "{} LIKE {}{}",
            render_expr(e, names),
            render_expr(pattern, names),
            escape.as_ref().map(|x| format!(" ESCAPE {}", render_expr(x, names))).unwrap_or_default()
        ),
    }
}

/// Sort keys as text, with columns named from `names`.
pub fn render_keys(keys: &[SortKey], names: &[String]) -> String {
    keys.iter()
        .map(|k| format!("{}{}{}", column_name(names, k.column), if k.descending { " DESC" } else { "" }, k.order().null_suffix()))
        .collect::<Vec<_>>()
        .join(", ")
}

impl LogicalPlan {
    /// The plan as indented text, one node per line.
    pub fn explain(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out
    }

    fn write(&self, out: &mut String, depth: usize) {
        let pad = "  ".repeat(depth);
        let inner = |p: &LogicalPlan| p.names();
        // Writing to a String cannot fail.
        let _ = match self {
            LogicalPlan::Scan { table, .. } => writeln!(out, "{pad}Scan {table}"),
            LogicalPlan::Filter { cond, input } => writeln!(out, "{pad}Filter {}", render_cond(cond, &inner(input))),
            LogicalPlan::Product { .. } => writeln!(out, "{pad}Product"),
            LogicalPlan::OuterJoin { kind, cond, .. } => {
                writeln!(out, "{pad}{kind:?} outer join on {}", render_cond(cond, &self.names()))
            }
            LogicalPlan::Aggregate { groups, aggs, input, .. } => {
                let n = inner(input);
                let g: Vec<String> = groups.iter().map(|&i| column_name(&n, i)).collect();
                let a: Vec<&str> = aggs.iter().map(|a| a.label.as_str()).collect();
                writeln!(out, "{pad}Aggregate group by [{}] computing [{}]", g.join(", "), a.join(", "))
            }
            LogicalPlan::Project { exprs, input } => {
                let n = inner(input);
                writeln!(out, "{pad}Project {}", exprs.iter().map(|e| render_expr(e, &n)).collect::<Vec<_>>().join(", "))
            }
            LogicalPlan::Distinct { .. } => writeln!(out, "{pad}Distinct"),
            LogicalPlan::SetOp { op, all, .. } => writeln!(out, "{pad}{op:?}{}", if *all { " all" } else { "" }),
            LogicalPlan::System { read, .. } => writeln!(out, "{pad}System {}", read.name()),
            LogicalPlan::Values { rows, .. } => writeln!(
                out,
                "{pad}Values {}",
                rows.iter()
                    .map(|r| format!("({})", r.iter().map(|e| render_expr(e, &[])).collect::<Vec<_>>().join(", ")))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            LogicalPlan::Sort { keys, input } => writeln!(out, "{pad}Sort {}", render_keys(keys, &inner(input))),
            LogicalPlan::Recursive { member, fixpoint, .. } => writeln!(out, "{pad}Recursive member {member} {}", fixpoint.word()),
            LogicalPlan::WorkTable { member, .. } => writeln!(out, "{pad}Work table of member {member}"),
            LogicalPlan::Unnest(u) => {
                let n = inner(&u.input);
                let c: Vec<String> = u.collections.iter().map(|e| render_expr(e, &n)).collect();
                let left = if u.outer.is_some() { "Left unnest" } else { "Unnest" };
                writeln!(out, "{pad}{left} {}{}", c.join(", "), if u.ordinality { " with ordinality" } else { "" })
            }
            LogicalPlan::Window(w) => writeln!(out, "{pad}{}", w.render(&inner(&w.input))),
        };
        for input in self.inputs() {
            input.write(out, depth.saturating_add(1));
        }
        // Its subqueries, each run for a row with its outer references'
        // values.
        for s in self.subqueries() {
            let _ = writeln!(out, "{pad}  Subquery");
            s.plan.write(out, depth.saturating_add(2));
        }
    }
}
